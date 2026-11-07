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
            "kind": KINDS[rng.below(6) as usize],
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
    ] {
        let run = Run::new(
            &format!("select-{seed}"),
            &generated(seed, streams, records),
        );
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
    let body = generated(21, 6, 300);
    let last = written(&json!({
        "v": 2, "ts": stamp(9_000_000), "stream": "stream-1", "seq": 100_000,
        "source": "pipeline", "kind": "node-started", "labels": {"node": "ship"},
    }));
    let whole = last.clone();
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
        ("not a record", format!("{body}this is not a record\n")),
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
    for (what, journal) in cases {
        let run = Run::new(&format!("torn-{}", what.replace(' ', "-")), &journal);
        every_selection_agrees(&run, what);
    }
}

/// A kept line whose opening and end are in the form and whose middle is not is found
/// when it is parsed, and the read is then the complete reader's. (Where a selection
/// does not keep such a line nothing parses it, as `read_selected`'s module note states.)
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
    for selection in [
        Selection::FirstScreen { node: None },
        Selection::FirstScreen { node: Some("ship") },
        Selection::Share(Some("ship")),
    ] {
        agrees_with_the_complete_read(&run, &selection, "a corrupt kept line");
    }
}
