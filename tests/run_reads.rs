//! The published reads of a long run, against what a complete read of the same journal
//! holds: [`journal::merge_order`]'s rule record by record, and
//! [`journal::read_selected`]'s kept records and places.
//!
//! The complete read is the real one, [`RunView::open`], over a real run directory.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use onepipeline::event::Envelope;
use onepipeline::journal::{self, Selection};
use onepipeline::views::{RunPaths, RunView};
use serde_json::{json, Value};

/// SplitMix64, so every generated journal is the same on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A real run directory holding `lines` as its journal, removed when dropped.
struct Run {
    root: PathBuf,
    paths: RunPaths,
}

impl Run {
    fn new(name: &str, journal: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-run-reads-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let paths = RunPaths::under(&root, "generated");
        std::fs::create_dir_all(&paths.dir).expect("a run directory");
        let launch = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/recorded/run-root/onemessagebus-repair-2/launch.json");
        std::fs::copy(launch, paths.dir.join("launch.json")).expect("a launch record");
        std::fs::write(paths.dir.join("events.jsonl"), journal).expect("a journal");
        Self { root, paths }
    }

    fn journal(&self) -> PathBuf {
        self.paths.dir.join("events.jsonl")
    }

    /// The merged store as the complete reader holds it, repeats included.
    fn complete(&self) -> Vec<Envelope> {
        RunView::open(&self.paths).expect("the run opens").events
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stamp(tick: u64) -> String {
    format!(
        "2026-11-06T10:{:02}:{:02}.{:03}Z",
        tick / 60_000,
        (tick / 1000) % 60,
        tick % 1000
    )
}

/// One record as a writer leaves it: serialized from the envelope type.
fn written(record: &Value) -> String {
    let envelope: Envelope = serde_json::from_value(record.clone()).expect("a record reads");
    serde_json::to_string(&envelope).expect("a record writes")
}

/// The same record's fields in the opposite order, which no writer here leaves.
fn reversed(line: &str) -> String {
    let value: serde_json::Map<String, Value> = serde_json::from_str(line).expect("an object");
    let fields: Vec<String> = value
        .iter()
        .rev()
        .map(|(key, value)| format!("{}:{value}", serde_json::to_string(key).expect("a key")))
        .collect();
    format!("{{{}}}", fields.join(","))
}

const NODES: [&str; 5] = ["build", "ship", "a\"quoted", "verify", "tail"];
const KINDS: [&str; 6] = [
    "member-heartbeat",
    "turn-activity",
    "member-heartbeat",
    "turn-activity",
    "turn-message",
    "node-started",
];

/// A journal of many streams: timestamps from a small pool, so streams tie; one stream
/// whose clock steps back; one stream with a single early record and none after; a few
/// repeated `(stream, seq)`; records with evidence, with a phase and with labels of
/// every form; and every tenth line in another field order.
fn generated(seed: u64, streams: u64, records: u64) -> String {
    generated_of(seed, streams, records, &KINDS)
}

/// [`generated`], its records' kinds drawn from `kinds`.
fn generated_of(seed: u64, streams: u64, records: u64, kinds: &[&str]) -> String {
    let mut rng = Rng(seed);
    let mut next_seq: HashMap<u64, u64> = HashMap::new();
    let mut lines = Vec::new();
    lines.push(written(&json!({
        "v": 2, "ts": stamp(1), "stream": "single-stream", "seq": 7,
        "source": "pipeline", "kind": "member-heartbeat", "labels": {"run_id": "generated"},
    })));
    for record in 0..records {
        let stream = rng.below(streams);
        let seq = next_seq.entry(stream).or_insert(0);
        let this_seq = *seq;
        *seq += 1;
        // A pool of stamps per step: most records tie with another stream's.
        let mut tick = (record / 16) * 16;
        if stream == 3 && record % 7 == 0 {
            // A stream whose clock stepped back under it.
            tick = tick.saturating_sub(4000);
        }
        let node = rng.below(8);
        let mut labels = json!({"run_id": "generated"});
        if node < 5 {
            labels["node"] = json!(NODES[node as usize]);
        }
        if rng.below(4) == 0 {
            labels["round"] = json!(rng.below(5));
        }
        if rng.below(9) == 0 {
            labels["extra"] = json!({"nested": [1, 2]});
        }
        let (source, phase) = match rng.below(3) {
            0 => ("agentgraph", None),
            1 => ("vcs", Some("integrate")),
            _ => ("pipeline", None),
        };
        let mut value = json!({
            "v": if source == "pipeline" { 2 } else { 1 },
            "ts": stamp(tick),
            "stream": format!("stream-{stream}"),
            "seq": this_seq,
            "source": source,
            "kind": kinds[rng.below(kinds.len() as u64) as usize],
            "labels": labels,
            "payload": {"text": "{\"v\":1} is only text here", "n": record, "artifacts": []},
        });
        if let Some(phase) = phase {
            value["phase"] = json!(phase);
        }
        if rng.below(11) == 0 {
            value["artifacts"] = json!([{"id": format!("a-{record}"), "kind": "log", "bytes": 3}]);
        }
        let line = written(&value);
        lines.push(if record % 10 == 3 {
            reversed(&line)
        } else {
            line
        });
        if rng.below(97) == 0 {
            // A producer in error: the same `(stream, seq)`, different content.
            let repeat = lines.last().expect("just pushed").clone();
            lines.push(repeat.replace("is only text", "is a repeat of text"));
        }
    }
    lines.join("\n") + "\n"
}

/// `merge_order`'s rule, as a scan rather than the heap: the earliest head by
/// `(ts, stream)`, each stream in its own `seq`, stably.
fn by_the_rule(events: &[Envelope]) -> Vec<Envelope> {
    let mut queues: BTreeMap<&str, Vec<&Envelope>> = BTreeMap::new();
    for event in events {
        queues.entry(event.stream.as_str()).or_default().push(event);
    }
    for queue in queues.values_mut() {
        queue.sort_by_key(|event| event.seq);
        queue.reverse();
    }
    let mut merged = Vec::new();
    while let Some(stream) = queues
        .iter()
        .filter_map(|(stream, queue)| queue.last().map(|head| (&head.ts, *stream)))
        .min()
        .map(|(_, stream)| stream)
    {
        let queue = queues.get_mut(stream).expect("the stream chosen");
        merged.push((*queue.pop().expect("its head")).clone());
    }
    merged
}

#[test]
fn the_merged_order_follows_the_rule_record_by_record() {
    for (seed, streams) in [(1, 1), (2, 7), (3, 300), (4, 2100)] {
        let run = Run::new(&format!("merge-{seed}"), &generated(seed, streams, 4000));
        // In the order the file holds them, through the complete reader's own parse.
        let mut events: Vec<Envelope> = std::fs::read_to_string(run.journal())
            .expect("the journal")
            .lines()
            .map(|line| serde_json::from_str(line).expect("a record"))
            .collect();
        let expected = by_the_rule(&events);
        journal::merge_order(&mut events);
        assert_eq!(events.len(), expected.len());
        for (at, (got, want)) in events.iter().zip(&expected).enumerate() {
            assert_eq!(
                (&got.ts, &got.stream, got.seq, &got.payload),
                (&want.ts, &want.stream, want.seq, &want.payload),
                "record {at} of the seed {seed} journal"
            );
        }
        // And the complete reader reads the same order.
        assert_eq!(run.complete(), expected);
    }
}

const SKIPPED: [&str; 2] = ["member-heartbeat", "turn-activity"];

/// The places a complete merge gives each record, `None` for a repeated `(stream, seq)`.
fn store_places(complete: &[Envelope]) -> (Vec<Option<usize>>, usize) {
    let mut seen = HashSet::new();
    let mut places = Vec::new();
    let mut length = 0;
    for event in complete {
        if seen.insert((event.stream.clone(), event.seq)) {
            places.push(Some(length));
            length += 1;
        } else {
            places.push(None);
        }
    }
    (places, length)
}

/// The kept indices of `complete` each selection's rule names, written against the
/// complete read rather than against the skim.
fn named_by_the_rule(complete: &[Envelope], selection: &Selection<'_>) -> Vec<usize> {
    let (places, _) = store_places(complete);
    let mut kept = HashSet::new();
    if complete.is_empty() {
        return Vec::new();
    }
    kept.insert(0);
    kept.insert(complete.len() - 1);
    match selection {
        Selection::Share(owner) => {
            for (at, event) in complete.iter().enumerate() {
                if event.labels.node.as_deref() == *owner {
                    kept.insert(at);
                }
            }
        }
        Selection::FirstScreen { node } => {
            let mut first_of: HashMap<&str, usize> = HashMap::new();
            let mut last_of: HashMap<&str, usize> = HashMap::new();
            let mut previous: HashMap<&str, usize> = HashMap::new();
            for (at, event) in complete.iter().enumerate() {
                first_of.entry(&event.stream).or_insert(at);
                // A stream's last in its own order, which the clock does not decide.
                let held = last_of.entry(&event.stream).or_insert(at);
                if complete[*held].seq <= event.seq {
                    *held = at;
                }
                if !SKIPPED.contains(&event.kind.0.as_str()) {
                    kept.insert(at);
                }
                if node.is_some() && event.labels.node.as_deref() == *node {
                    kept.insert(at);
                }
                if let (Some(owner), Some(_)) = (event.labels.node.as_deref(), places[at]) {
                    if !event.artifacts.is_empty() {
                        kept.extend(previous.get(owner).copied());
                    }
                    previous.insert(owner, at);
                }
            }
            kept.extend(first_of.into_values());
            kept.extend(last_of.into_values());
        }
    }
    let mut kept: Vec<usize> = kept.into_iter().collect();
    kept.sort_unstable();
    kept
}

/// One selection over `run`, held to the complete read's records and places.
fn agrees_with_the_complete_read(run: &Run, selection: &Selection<'_>, what: &str) {
    let complete = run.complete();
    let (places, store_len) = store_places(&complete);
    let selected = journal::read_selected(&run.journal(), selection);
    let kept = named_by_the_rule(&complete, selection);
    let expected: Vec<&Envelope> = kept.iter().map(|at| &complete[*at]).collect();
    assert_eq!(
        selected.events.iter().collect::<Vec<_>>(),
        expected,
        "{what}: {selection:?} kept other records than its rule names"
    );
    assert_eq!(
        selected.places,
        kept.iter().map(|at| places[*at]).collect::<Vec<_>>(),
        "{what}: {selection:?} places"
    );
    assert_eq!(selected.store_len, store_len, "{what}: store_len");
    assert_eq!(
        selected.last_seq,
        complete.iter().map(|event| event.seq).max().unwrap_or(0),
        "{what}: last_seq"
    );
    let mut lengths: HashMap<&str, usize> = HashMap::new();
    let mut shares: HashMap<String, Vec<usize>> = HashMap::new();
    let chosen: HashSet<usize> = kept.iter().copied().collect();
    for (at, event) in complete.iter().enumerate() {
        let (Some(node), Some(_)) = (event.labels.node.as_deref(), places[at]) else {
            continue;
        };
        let length = lengths.entry(node).or_insert(0);
        if chosen.contains(&at) {
            shares.entry(node.to_owned()).or_default().push(*length);
        }
        *length += 1;
    }
    assert_eq!(selected.shares, shares, "{what}: {selection:?} shares");
}

fn every_selection_agrees(run: &Run, what: &str) {
    for selection in [
        Selection::FirstScreen { node: None },
        Selection::FirstScreen { node: Some("ship") },
        Selection::FirstScreen {
            node: Some("a\"quoted"),
        },
        Selection::FirstScreen {
            node: Some("no-such-node"),
        },
        Selection::Share(Some("build")),
        Selection::Share(Some("a\"quoted")),
        Selection::Share(None),
    ] {
        agrees_with_the_complete_read(run, &selection, what);
    }
}

#[test]
fn each_selection_keeps_what_its_rule_names_at_the_places_a_complete_merge_gives_them() {
    for (seed, streams, records) in [
        (11, 1, 300),
        (12, 9, 2000),
        (13, 400, 6000),
        (14, 2100, 9000),
        // Past the size a read takes in parts across threads.
        (15, 2100, 40_000),
    ] {
        let run = Run::new(
            &format!("select-{seed}"),
            &generated(seed, streams, records),
        );
        if records == 40_000 {
            let bytes = std::fs::metadata(run.journal()).expect("a journal").len();
            assert!(bytes > 8 << 20, "{bytes} bytes is read on one thread");
        }
        every_selection_agrees(&run, &format!("seed {seed}"));
    }
}

#[test]
fn a_journal_that_is_not_there_or_holds_nothing_reads_as_holding_nothing() {
    let run = Run::new("empty", "");
    every_selection_agrees(&run, "an empty journal");
    let selected = journal::read_selected(
        &run.root.join("no-such-file"),
        &Selection::FirstScreen { node: None },
    );
    assert_eq!(selected, journal::SelectedRecords::default());
}

/// What a live writer or a died one leaves: each is what the complete reader makes of
/// it, in either field order, so places and `store_len` stay equal.
#[test]
fn torn_glued_and_unreadable_lines_read_as_the_complete_reader_reads_them() {
    for (what, journal) in torn_cases(&generated(21, 6, 300)) {
        let run = Run::new(&format!("torn-{}", what.replace(' ', "-")), &journal);
        every_selection_agrees(&run, what);
    }
}

/// `body` with what a live writer or a died one leaves after it, each named.
fn torn_cases(body: &str) -> Vec<(&'static str, String)> {
    let last = written(&json!({
        "v": 2, "ts": stamp(9_000_000), "stream": "stream-1", "seq": 100_000,
        "source": "pipeline", "kind": "node-started", "labels": {"node": "ship"},
    }));
    let whole = last.clone();
    let heartbeat = written(&json!({
        "v": 1, "ts": stamp(1), "stream": "stream-1", "seq": 50_000,
        "source": "agentgraph", "kind": "member-heartbeat", "labels": {"node": "ship"},
        "payload": {"text": "beat"},
    }));
    let later = written(&json!({
        "v": 2, "ts": stamp(9_100_000), "stream": "stream-1", "seq": 100_001,
        "source": "pipeline", "kind": "node-started", "labels": {"node": "ship"},
    }));
    let cases: Vec<(&str, String)> = vec![
        // A whole record no newline has finished is no record yet, in either order.
        ("finished bytes, no terminator", format!("{body}{whole}")),
        (
            "reversed, no terminator",
            format!("{body}{}", reversed(&whole)),
        ),
        // Half a record, with and without a terminator after it.
        ("torn", format!("{body}{}", &whole[..whole.len() / 2])),
        (
            "torn reversed",
            format!("{body}{}", &reversed(&whole)[..40]),
        ),
        (
            "torn and terminated",
            format!("{body}{}\n", &whole[..whole.len() - 8]),
        ),
        // Past `labels`, so its opening alone looks whole.
        (
            "cut in its payload and terminated",
            format!(
                "{body}{}\n",
                &whole[..whole.find("\"payload\"").unwrap() + 11]
            ),
        ),
        // A fragment, and then a whole record appended after it.
        (
            "glued",
            format!("{body}{}{whole}\n", &whole[..whole.len() / 2]),
        ),
        (
            "glued past its labels",
            format!(
                "{body}{}{whole}\n",
                &whole[..whole.find("\"payload\"").unwrap() + 11]
            ),
        ),
        // A fragment of another record, a heartbeat early in the stream, glued to a whole
        // record a first screen keeps, with a later record after them: only the whole one
        // is there, wherever the fragment's opening would place it.
        (
            "glued to another record's fragment",
            format!(
                "{body}{}{whole}\n{}\n",
                &heartbeat[..heartbeat.find("\"payload\"").unwrap() + 11],
                later,
            ),
        ),
        ("not a record", format!("{body}this is not a record\n")),
        // A `ts` in another form than the writers stamp, which is merged on its text.
        (
            "a timestamp in another form",
            format!(
                "{body}{}\n",
                whole.replace(&stamp(9_000_000), "2026-11-06T10:00:00Z")
            ),
        ),
        (
            "a future schema",
            format!("{body}{{\"v\":99,\"from\":\"the future\"}}\n"),
        ),
        ("blank", format!("{body}\n\n{whole}\n")),
        (
            "an unknown source word",
            format!("{body}{}\n", whole.replace("\"pipeline\"", "\"elsewhere\"")),
        ),
        ("carriage return", format!("{body}{whole}\r\n")),
    ];
    cases
}

/// Every selection's kept records are exactly the complete read's records its rule names,
/// whatever `read_selected` merged beside them: what places and `store_len` hold where a
/// discarded line is one the complete read refuses is not.
fn every_selection_keeps_the_complete_records(run: &Run, what: &str) {
    let complete = run.complete();
    for selection in [
        Selection::FirstScreen { node: None },
        Selection::FirstScreen { node: Some("ship") },
        Selection::Share(Some("ship")),
        Selection::Share(Some("build")),
        Selection::Share(None),
    ] {
        let selected = journal::read_selected(&run.journal(), &selection);
        let expected: Vec<&Envelope> = named_by_the_rule(&complete, &selection)
            .iter()
            .map(|at| &complete[*at])
            .collect();
        assert_eq!(
            selected.events.iter().collect::<Vec<_>>(),
            expected,
            "{what}: {selection:?} kept other records than the complete read holds"
        );
    }
}

/// A heartbeat or activity record no selection keeps, between valid records, whose opening
/// and end are the writers' and whose middle a complete reader refuses. Its middle is never
/// parsed, by the manager's ruling that a first screen not pay for every byte of a journal,
/// so it is merged by its opening; where a selection keeps it, as the end of a stream of its
/// own, it is found when it is parsed and the whole read is the complete reader's. Either
/// way every record returned is the one the complete read holds, whichever way its middle
/// is wrong.
#[test]
fn a_discarded_line_malformed_in_its_middle_leaves_every_kept_record_the_complete_reads() {
    let valid = |seq: u64, tick: u64, kind: &str| {
        written(&json!({
            "v": 1, "ts": stamp(tick), "stream": "stream-a", "seq": seq,
            "source": "agentgraph", "kind": kind, "labels": {"node": "ship"},
            "payload": {"text": "intact"},
        }))
    };
    let shaped = |kind: &str, stream: &str, seq: u64| {
        written(&json!({
            "v": 1, "ts": stamp(1_500), "stream": stream, "seq": seq,
            "source": "agentgraph", "kind": kind, "labels": {"node": "ship"},
            "payload": {"text": "intact"},
        }))
    };
    let nested = "[".repeat(200) + &"]".repeat(200);
    let middles: [(&str, &str, &str); 8] = [
        ("bare text", r#""intact""#, "intact"),
        ("a lone surrogate", r#""intact""#, r#""\ud800""#),
        ("a number out of range", r#""intact""#, "1e999"),
        ("nesting past the parser's depth", r#""intact""#, &nested),
        ("a control character", r#""intact""#, "\"a\tb\""),
        (
            "an unknown top-level field",
            r#","artifacts":[]"#,
            r#","stray":1,"artifacts":[]"#,
        ),
        (
            "an artifact missing its fields",
            r#""artifacts":[]"#,
            r#""artifacts":[{"id":"a"}]"#,
        ),
        (
            "a version past its type",
            r#"{"v":1,"#,
            r#"{"v":99999999999,"#,
        ),
    ];
    for kind in ["member-heartbeat", "turn-activity"] {
        for (what, from, to) in middles {
            // In the middle of its own stream, where a first screen discards it; and as
            // the end of a stream of its own, its `seq` far above the journal's, where a
            // first screen keeps it and a selection of one owner discards it.
            for (place, stream, seq) in
                [("mid-stream", "stream-a", 1), ("high", "stream-b", 900_000)]
            {
                let malformed = shaped(kind, stream, seq).replace(from, to);
                assert!(malformed.contains(to), "{what}: the fixture broke nothing");
                let journal = format!(
                    "{}\n{malformed}\n{}\n{}\n",
                    valid(0, 1_000, "node-dispatched"),
                    valid(2, 2_000, "member-heartbeat"),
                    valid(3, 3_000, "node-dispatched"),
                );
                let name = format!("discarded-{kind}-{place}-{}", what.replace(' ', "-"));
                let run = Run::new(&name, &journal);
                assert_eq!(
                    run.complete().len(),
                    3,
                    "{name}: the complete read refuses it"
                );
                every_selection_keeps_the_complete_records(&run, &name);
            }
        }

        // Bytes that are not text in its middle.
        let mut journal = valid(0, 1_000, "node-dispatched").into_bytes();
        journal.push(b'\n');
        let mut malformed = shaped(kind, "stream-a", 1)
            .replace("intact", "in@tact")
            .into_bytes();
        let mark = malformed
            .iter()
            .position(|byte| *byte == b'@')
            .expect("the mark");
        malformed[mark] = 0xFF;
        journal.extend_from_slice(&malformed);
        journal.push(b'\n');
        for (seq, tick) in [(2, 2_000), (3, 3_000)] {
            journal.extend_from_slice(valid(seq, tick, "node-dispatched").as_bytes());
            journal.push(b'\n');
        }
        let run = Run::new(&format!("discarded-{kind}-not-text"), "");
        std::fs::write(run.journal(), journal).expect("a journal");
        assert_eq!(run.complete().len(), 3, "the complete read refuses it");
        every_selection_keeps_the_complete_records(&run, "bytes that are not text");
    }
}

/// A kept line whose opening and end are in the form and whose middle is not is found
/// when it is read, and the read is then the complete reader's; a selection that does not
/// keep it still returns only the complete read's records.
#[test]
fn a_kept_line_corrupt_only_in_its_middle_leaves_the_read_equal_to_the_complete_one() {
    let body = generated(31, 6, 200);
    let good = written(&json!({
        "v": 2, "ts": stamp(8_000_000), "stream": "stream-1", "seq": 100_000,
        "source": "pipeline", "kind": "node-started", "labels": {"node": "ship"},
        "payload": {"text": "intact"},
    }));
    let corrupt = good.replace("\"intact\"", "intact");
    let after = good
        .replace("100000", "100001")
        .replace("08:00:00", "08:00:01");
    let run = Run::new("corrupt-middle", &format!("{body}{corrupt}\n{after}\n"));
    // Where a selection keeps it, places and `store_len` are the complete read's too.
    for selection in [
        Selection::FirstScreen { node: None },
        Selection::FirstScreen { node: Some("ship") },
        Selection::Share(Some("ship")),
    ] {
        agrees_with_the_complete_read(&run, &selection, "a corrupt kept line");
    }
    every_selection_keeps_the_complete_records(&run, "a corrupt line kept by some");
}

/// Every kind a long run's journal holds: the five a heads read folds by their heads, and
/// kinds whose payloads it parses, a pipeline settlement's among them.
const EVERY_KIND: [&str; 9] = [
    "member-heartbeat",
    "turn-activity",
    "turn-message",
    "turn-started",
    "turn-completed",
    "change-check",
    "member-started",
    "oneharness-session",
    "node-started",
];

/// A pipeline record of the driver's stream, at `tick`.
fn driven(seq: u64, tick: u64, kind: &str, node: Option<&str>, payload: Value) -> String {
    let mut labels = json!({"run_id": "generated"});
    if let Some(node) = node {
        labels["node"] = json!(node);
    }
    written(&json!({
        "v": 2, "ts": stamp(tick), "stream": "driver-1", "seq": seq,
        "source": "pipeline", "kind": kind, "labels": labels, "payload": payload,
    }))
}

/// A run's whole journal: its plan, every node's dispatch and how each settled, a
/// failed node's dependent left skipped, one awaiting a release and one still running,
/// beside [`generated_of`]'s records of every kind, which name those nodes.
fn run_journal(seed: u64, streams: u64, records: u64) -> String {
    let task = |id: &str, deps: &[&str]| json!({"id": id, "persona": "engineer", "task": "## What\ndo it", "deps": deps});
    let plan = json!({
        "schema_version": 3, "name": "generated", "concurrency": 4,
        "tasks": [task("build", &[]), task("ship", &[]), task("after-ship", &["ship"]),
                  task("verify", &[]), task("tail", &[])],
    });
    let late = records * 1_000;
    let mut driver = vec![driven(0, 0, "run-started", None, json!({"plan": plan}))];
    for node in ["build", "ship", "verify", "tail"] {
        let seq = driver.len() as u64;
        driver.push(driven(
            seq,
            0,
            "node-dispatched",
            Some(node),
            json!({"attempt": 1}),
        ));
    }
    for (node, payload) in [
        (
            "build",
            json!({"status": "done", "outcome": "complete", "landing": "landed"}),
        ),
        (
            "ship",
            json!({"status": "failed", "outcome": "judge-rejected", "detail": "the judge said \"no\""}),
        ),
        (
            "verify",
            json!({"status": "complete-but-held",
                   "awaiting": [{"producer": "github.com/owner/engine", "target": "crate"}]}),
        ),
    ] {
        let seq = driver.len() as u64;
        driver.push(driven(seq, late, "node-settled", Some(node), payload));
    }
    driver.join("\n") + "\n" + &generated_of(seed, streams, records, &EVERY_KIND)
}

/// A real run directory holding `journal`, with a result beside it.
fn resulted(name: &str, journal: &str) -> Run {
    let run = Run::new(name, journal);
    std::fs::write(
        run.paths.dir.join("result.json"),
        json!({"outcome": "unfinished"}).to_string(),
    )
    .expect("a result");
    run
}

/// What a heads read's view must report as [`RunView::open`] does: every node's
/// status, reason, awaits and result, through the run's published reading; the
/// summary; each node's event tally; and the rest of the fold but what a running
/// node is drawn as doing, which only the payloads it leaves unparsed say.
fn same_state(view: &RunView, fresh: &RunView, what: &str) {
    assert_eq!(
        view.state.statuses(),
        fresh.state.statuses(),
        "{what}: statuses"
    );
    let reading = |view: &RunView| {
        serde_json::to_value(onepipeline::views::reading(view)).expect("a reading")
    };
    assert_eq!(
        reading(view),
        reading(fresh),
        "{what}: reasons, awaits and results"
    );
    assert_eq!(view.summary(), fresh.summary(), "{what}: summary");
    let tallies = |view: &RunView| -> BTreeMap<String, Option<(u64, u64)>> {
        view.state
            .activity
            .iter()
            .map(|(node, activity)| {
                let progress = activity
                    .progress
                    .map(|progress| (progress.events(), progress.last_at()));
                (node.clone(), progress)
            })
            .collect()
    };
    assert_eq!(tallies(view), tallies(fresh), "{what}: event tallies");
    let without_doing = |view: &RunView| {
        let mut state = serde_json::to_value(&view.state).expect("a state");
        for activity in state["activity"]
            .as_object_mut()
            .into_iter()
            .flat_map(|nodes| nodes.values_mut())
        {
            activity
                .as_object_mut()
                .expect("an activity")
                .remove("doing");
        }
        state
    };
    assert_eq!(
        without_doing(view),
        without_doing(fresh),
        "{what}: the fold"
    );
}

/// `RunView::heads` and `fetch` over `run`, held to the complete read of its journal.
fn heads_agree(run: &Run, what: &str) {
    let fresh = RunView::open(&run.paths).expect("the run opens");
    let complete = &fresh.events;
    let (places, store_len) = store_places(complete);
    let read = RunView::heads(&run.paths).expect("the heads read");
    assert!(
        read.view.events.is_empty(),
        "{what}: a heads view holds no record"
    );
    assert_eq!(
        read.heads.len(),
        complete.len(),
        "{what}: one head per record"
    );
    for (at, (head, record)) in read.heads.iter().zip(complete).enumerate() {
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
                places[at],
            ),
            "{what}: the head of record {at}"
        );
    }
    assert_eq!(read.store_len, store_len, "{what}: store_len");
    assert_eq!(
        read.last_seq,
        complete.iter().map(|event| event.seq).max().unwrap_or(0),
        "{what}: last_seq"
    );
    same_state(&read.view, &fresh, what);

    let handles: Vec<journal::RecordHandle> =
        read.heads.iter().map(|head| head.handle.clone()).collect();
    assert_eq!(
        &journal::fetch(&run.journal(), &handles).expect("every record"),
        complete,
        "{what}: every record fetched"
    );
    assert_eq!(
        journal::fetch(&run.journal(), &[]).expect("none"),
        Vec::<Envelope>::new()
    );
    // A subset in another order, with a record named twice.
    let mut rng = Rng(complete.len() as u64);
    let mut chosen: Vec<usize> = (0..complete.len()).filter(|_| rng.below(5) == 0).collect();
    chosen.reverse();
    if let Some(first) = chosen.first().copied() {
        chosen.push(first);
    }
    for swap in 1..chosen.len() {
        let with = rng.below(swap as u64 + 1) as usize;
        chosen.swap(swap, with);
    }
    let named: Vec<journal::RecordHandle> = chosen.iter().map(|at| handles[*at].clone()).collect();
    assert_eq!(
        journal::fetch(&run.journal(), &named).expect("the chosen records"),
        chosen
            .iter()
            .map(|at| complete[*at].clone())
            .collect::<Vec<_>>(),
        "{what}: the chosen records, in the order named"
    );
}

#[test]
fn every_head_and_every_fetched_record_is_the_complete_reads_and_the_state_is_opens() {
    for (seed, streams, records) in [(41, 1, 300), (42, 9, 1500), (43, 600, 4000)] {
        let run = resulted(
            &format!("heads-{seed}"),
            &run_journal(seed, streams, records),
        );
        let statuses: BTreeMap<String, &str> = RunView::open(&run.paths)
            .expect("the run opens")
            .state
            .statuses()
            .into_iter()
            .map(|(node, status)| (node, status.as_str()))
            .collect();
        assert_eq!(
            statuses,
            BTreeMap::from([
                ("after-ship".to_owned(), "skipped"),
                ("build".to_owned(), "done"),
                ("ship".to_owned(), "failed"),
                ("tail".to_owned(), "running"),
                ("verify".to_owned(), "complete-but-held"),
            ]),
            "the fixture's nodes stand every way a first screen draws"
        );
        heads_agree(&run, &format!("seed {seed}"));
    }
    // A run recorded live, its journal as its writers left it.
    let recorded = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/recorded/run-root/onemessagebus-repair-2/events.jsonl");
    let run = resulted(
        "heads-recorded",
        &std::fs::read_to_string(recorded).expect("a recorded journal"),
    );
    heads_agree(&run, "a recorded run");
}

/// A journal large enough that the heads read cuts its bytes into a run for each of its
/// workers (past 1 MiB) and builds its heads a run of 2,048 records at a time on them,
/// so every stream is numbered in several runs, in a different order in each, and its
/// numbers are joined across them: the read must still head every record as the
/// complete read gives it, in the merged order, and fold the state `open` folds.
#[test]
fn a_journal_cut_into_runs_across_the_workers_is_headed_as_the_complete_reader_reads_it() {
    // Under 6,000 records, so the settlements' late stamp keeps a two-digit minute.
    let journal = run_journal(44, 2_000, 5_900);
    assert!(
        journal.len() > 1 << 20,
        "the journal is cut into runs only past 1 MiB, and this one is {} bytes",
        journal.len()
    );
    assert!(
        journal.lines().count() > 2 * 2_048,
        "heads are built in runs of 2,048 records, and this journal holds fewer than two"
    );
    // Every `ts` in the writers' form, so the merge orders the streams by the numbers
    // the runs gave them rather than by their text.
    for line in journal.lines() {
        let record: Value = serde_json::from_str(line).expect("a record");
        let ts = record["ts"].as_str().expect("a ts");
        assert_eq!(ts.len(), "2026-11-06T10:00:00.000Z".len(), "{ts}");
    }
    let run = resulted("heads-cut-into-runs", &journal);
    heads_agree(&run, "a journal cut into runs");
}

#[test]
fn torn_glued_and_unreadable_lines_are_headed_as_the_complete_reader_reads_them() {
    for (what, journal) in torn_cases(&run_journal(51, 6, 300)) {
        let run = resulted(&format!("heads-torn-{}", what.replace(' ', "-")), &journal);
        heads_agree(&run, what);
    }
    let run = Run::new("heads-empty", "");
    heads_agree(&run, "an empty journal");
    let gone = RunPaths::under(&run.root, "no-such-run");
    assert!(matches!(
        RunView::heads(&gone),
        Err(onepipeline::Error::NoSuchRun { .. })
    ));
}

/// Records appended after the heads read are folded by its view's refresh, which then
/// reports what a fresh open does.
#[test]
fn a_heads_views_refresh_equals_a_fresh_open() {
    let run = resulted("heads-refresh", &run_journal(61, 12, 600));
    let mut read = RunView::heads(&run.paths).expect("the heads read");
    let appended = [
        driven(
            40,
            900_000_000,
            "node-settled",
            Some("tail"),
            json!({"status": "done", "outcome": "complete", "landing": "landed"}),
        ),
        written(&json!({
            "v": 1, "ts": stamp(900_000_001), "stream": "stream-late", "seq": 0,
            "source": "agentgraph", "kind": "turn-activity", "labels": {"node": "tail"},
            "payload": {"name": "Bash", "detail": "just check"},
        })),
    ];
    let mut journal = std::fs::OpenOptions::new()
        .append(true)
        .open(run.journal())
        .expect("the journal");
    std::io::Write::write_all(&mut journal, (appended.join("\n") + "\n").as_bytes())
        .expect("appended");
    drop(journal);
    read.view.refresh().expect("refreshed");
    let fresh = RunView::open(&run.paths).expect("the run opens");
    assert_eq!(read.view.state.statuses()["tail"].as_str(), "done");
    same_state(&read.view, &fresh, "refreshed");
}

/// A handle into a journal rewritten after the heads read, so another record's line sits
/// where the handle's did, answers an error and never that other record.
#[test]
fn a_handle_into_a_rewritten_journal_answers_an_error_and_never_another_record() {
    let run = resulted("heads-rewritten", &run_journal(71, 4, 200));
    let read = RunView::heads(&run.paths).expect("the heads read");
    let text = std::fs::read_to_string(run.journal()).expect("the journal");
    let lines: Vec<&str> = text.lines().collect();
    // The same lines, each pair swapped where both are of one length, so another
    // record's line sits at each of those handles' offsets; and every line cut short.
    let mut swapped: Vec<&str> = lines.clone();
    let mut moved = 0;
    for pair in (0..swapped.len() - 1).step_by(2) {
        if swapped[pair].len() == swapped[pair + 1].len() && swapped[pair] != swapped[pair + 1] {
            swapped.swap(pair, pair + 1);
            moved += 1;
        }
    }
    assert!(moved > 0, "the fixture swaps no line");
    std::fs::write(run.journal(), swapped.join("\n") + "\n").expect("rewritten");
    let mut refused = 0;
    for head in &read.heads {
        match journal::fetch(&run.journal(), std::slice::from_ref(&head.handle)) {
            Ok(records) => {
                assert_eq!(
                    (&records[0].stream, records[0].seq),
                    (&head.stream, head.seq),
                    "a fetch answered another record"
                );
            }
            Err(error) => {
                assert!(
                    matches!(error, onepipeline::Error::RecordMoved { .. }),
                    "{error}"
                );
                assert!(error.to_string().contains("rewritten"), "{error}");
                refused += 1;
            }
        }
    }
    assert_eq!(refused, moved * 2, "every moved line is refused");
    // A journal shorter than a handle's line, and one that is gone.
    std::fs::write(run.journal(), &lines[0][..10]).expect("truncated");
    let last = &read.heads.last().expect("a head").handle;
    assert!(matches!(
        journal::fetch(&run.journal(), std::slice::from_ref(last)),
        Err(onepipeline::Error::RecordMoved { .. })
    ));
    std::fs::remove_file(run.journal()).expect("removed");
    assert!(matches!(
        journal::fetch(&run.journal(), std::slice::from_ref(last)),
        Err(onepipeline::Error::Ledger { .. })
    ));
}

/// The manager's ruling for a line of a kind the fold reads by its head, whose opening
/// and end are the writers' and whose middle a complete reader refuses: it is headed by
/// its opening, folded as that head, and its fetch answers an error.
#[test]
fn a_headed_line_malformed_in_its_middle_is_headed_by_its_opening_and_never_fetched() {
    let valid = run_journal(81, 3, 60);
    let malformed = written(&json!({
        "v": 1, "ts": stamp(1), "stream": "stream-broken", "seq": 0,
        "source": "agentgraph", "kind": "turn-activity", "labels": {"node": "build"},
        "payload": {"text": "intact"},
    }))
    .replace("\"intact\"", "intact");
    let run = resulted("heads-malformed", &format!("{valid}{malformed}\n"));
    let fresh = RunView::open(&run.paths).expect("the run opens");
    let read = RunView::heads(&run.paths).expect("the heads read");
    assert_eq!(
        read.heads.len(),
        fresh.events.len() + 1,
        "headed by its opening"
    );
    let head = read
        .heads
        .iter()
        .find(|head| head.stream == "stream-broken")
        .expect("its head");
    assert_eq!(
        (head.seq, head.kind.0.as_str(), head.labels.node.as_deref()),
        (0, "turn-activity", Some("build"))
    );
    assert!(matches!(
        journal::fetch(&run.journal(), std::slice::from_ref(&head.handle)),
        Err(onepipeline::Error::RecordMoved { .. })
    ));
    // Every other line, the writers' own, is headed and fetched as the complete read
    // holds it, in its order; only the places from the malformed line's on move by it.
    let others: Vec<&journal::RecordHead> = read
        .heads
        .iter()
        .filter(|head| head.stream != "stream-broken")
        .collect();
    assert_eq!(others.len(), fresh.events.len());
    let (places, _) = store_places(&fresh.events);
    let broken_place = head.place.expect("a place of its own");
    for (at, (other, record)) in others.iter().zip(&fresh.events).enumerate() {
        assert_eq!(
            (
                &other.ts,
                &other.stream,
                other.seq,
                &other.kind,
                &other.labels,
                other.evidence
            ),
            (
                &record.ts,
                &record.stream,
                record.seq,
                &record.kind,
                &record.labels,
                !record.artifacts.is_empty()
            ),
            "the head of record {at}"
        );
        assert_eq!(
            other.place,
            places[at].map(|place| place + usize::from(place >= broken_place)),
            "the place of record {at}"
        );
    }
    let handles: Vec<journal::RecordHandle> =
        others.iter().map(|other| other.handle.clone()).collect();
    assert_eq!(
        journal::fetch(&run.journal(), &handles).expect("every other record"),
        fresh.events
    );
    // Folded as its head: every value but the node's tally is the complete read's, and
    // the tally counts it.
    assert_eq!(read.view.state.statuses(), fresh.state.statuses());
    assert_eq!(read.view.summary(), fresh.summary());
    let events = |view: &RunView| {
        view.state.activity["build"]
            .progress
            .map(|progress| progress.events())
    };
    assert_eq!(events(&read.view), events(&fresh).map(|events| events + 1));
}

/// A line of a kind the fold parses whole, in the writers' form but malformed in its
/// middle, is dropped when its whole parse refuses it, before the merge, as the complete
/// read drops it: every head, place, fetched record and the state are the complete
/// read's, and the malformed line has no head. That no headed record is parsed whole for
/// it is held by the engine's own `journal_heads` test, whose counter only tests compile.
#[test]
fn a_folded_line_malformed_in_its_middle_leaves_the_heads_read_the_complete_ones() {
    let body = run_journal(91, 6, 300);
    let good = written(&json!({
        "v": 1, "ts": stamp(150_000), "stream": "stream-checks", "seq": 0,
        "source": "agentgraph", "kind": "change-check", "labels": {"node": "build"},
        "payload": {"text": "intact"},
    }));
    let corrupt = good.replace("\"intact\"", "intact");
    let after = good
        .replace("\"seq\":0", "\"seq\":1")
        .replace(&stamp(150_000), &stamp(150_001));
    let run = resulted(
        "heads-folded-corrupt",
        &format!("{body}{corrupt}\n{after}\n"),
    );
    let complete = run.complete();
    assert!(
        complete
            .iter()
            .any(|record| record.stream == "stream-checks"),
        "the valid record beside it is held"
    );
    assert!(
        !complete
            .iter()
            .any(|record| record.stream == "stream-checks" && record.seq == 0),
        "the complete read refuses the corrupt line"
    );
    heads_agree(&run, "a corrupt line the fold parses");
}
