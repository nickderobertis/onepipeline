//! The status-read corpus: one long run, a settled upstream its consumer names, one small
//! settled run, one run of a long dispatch's references, and a root of 850 runs.
//!
//! Every figure shaping it is an **aggregate** of this host's largest run, read for its
//! record mix and for what fills its checkpoint, and no record, text, identifier or path of
//! it is here: of that run's 6.96 MB checkpoint, 3.55 MB was the 116 nodes live edits added,
//! 1.55 MB the plan and graph of its tasks, 0.62 MB the records the fold keeps whole, and
//! 0.49 MB its oneharness sessions.

// llmlint: ignore-file[contracts_have_one_source_or_a_drift_gate] this file writes run
// directories from JSON on purpose, as `tests/journal_workload/generate.rs` does: a long run a
// newer build reads was written by an older one, so deriving these from today's types would
// measure a run no host has. Its drift gate is the journey beside it, which asserts the build
// reads this run as 19 nodes settled with a checkpoint at the size `report.mjs` requires.

use std::io::Write;
use std::path::{Path, PathBuf};

use oneharness_core::domain::events::ActionEvent;
use oneharness_core::domain::history::HistoryLabels;
use oneharness_core::io::history::HistoryWriter;
use serde_json::{json, Map, Value};

use crate::workload::{prose, stamp, Rng};

pub const LARGE: &str = "large-run";
pub const SMALL: &str = "small-settled";
pub const UPSTREAM: &str = "upstream-run";
pub const DISPATCH: &str = "long-dispatch";
pub const DISPATCH_NODE: &str = "t01-synthesized-workload-node";
pub const RUNS: usize = 850;
pub const LARGE_RECORDS: u64 = 124_000;
pub const LARGE_NODES: usize = 19;
pub const LARGE_STREAMS: u64 = 2_100;
pub const LARGE_BYTES: u64 = 102_000_000;
pub const UPSTREAM_BYTES: u64 = 30_000_000;
pub const DISPATCH_RECORDS: u64 = 6_000;
pub const REFERENCES: usize = 5_000;
pub const SESSION_FILES: usize = 3;
pub const SESSION_BYTES: usize = 1_800_000;

/// The first record of every generated run, in the writers' own past.
const STARTED_MS: i64 = 1_790_000_000_000;

/// What the long dispatch was generated with.
pub struct Generated {
    pub session_bytes: u64,
    pub references: usize,
}

/// One envelope in the form the writers leave it.
fn line(
    ts: i64,
    stream: &str,
    seq: u64,
    source: &str,
    kind: &str,
    labels: &Value,
    payload: &Value,
) -> String {
    let v = if source == "pipeline" { 2 } else { 1 };
    let record = json!({
        "v": v, "ts": stamp(ts), "stream": stream, "seq": seq, "source": source, "kind": kind,
        "labels": labels, "payload": payload, "artifacts": [],
    });
    let mut text = serde_json::to_string(&record).expect("a record serializes");
    text.push('\n');
    text
}

fn node_id(index: usize) -> String {
    format!("t{:02}-synthesized-workload-node", index + 1)
}

fn task_text(rng: &mut Rng, id: &str, bytes: usize) -> String {
    format!(
        "## What\nDo {id}.\n{}\n\n## Why\nSo the run can settle.\n",
        prose(rng, bytes)
    )
}

fn write_run_files(root: &Path, run: &str, journal: &[u8]) -> PathBuf {
    let dir = root.join(run);
    std::fs::create_dir_all(&dir).expect("a run directory");
    let launch = json!({
        "run_id": run,
        "project": format!("plans:{run}"),
        "dir": root.with_file_name("project"),
        "graph": "graphs/dag-scope.yaml",
        "graph_run": format!("dag-scope-{STARTED_MS}-4194000"),
        "observer_runs": [],
        "node_graph": "graphs/node-scope.yaml",
        "launcher": "e2e",
        "session": "status-read",
        "pid": 2_147_483_647_u32,
        "host": "status-read-host",
        "started": "linux-proc-stat:1",
        "started_at": stamp(STARTED_MS),
        "heartbeat_interval": 1_800,
        "writeback_item_budget": 12,
        "adoptions": 0,
    });
    std::fs::write(
        dir.join("launch.json"),
        serde_json::to_string_pretty(&launch).expect("a launch record serializes"),
    )
    .expect("a launch record");
    std::fs::write(dir.join("events.jsonl"), journal).expect("a journal");
    dir
}

fn plan_doc(run: &str, tasks: Vec<Value>) -> Value {
    json!({
        "schema_version": onepipeline::plan::PLAN_SCHEMA_VERSION,
        "goal": {"text": format!("Deliver {run}")},
        "name": run,
        "concurrency": 4,
        "tasks": tasks,
    })
}

/// A settled run of 8 records.
fn small_run(run: &str) -> Vec<u8> {
    let stream = "host-100";
    let labels = json!({"run_id": run});
    let node = json!({"run_id": run, "node": "only"});
    let plan = plan_doc(
        run,
        vec![json!({"id": "only", "task": "## What\nDo it.", "persona": "engineer"})],
    );
    let records: [(&str, &Value, Value); 8] = [
        (
            "run-started",
            &labels,
            json!({"plan": plan, "graph": "graphs/dag-scope.yaml", "dir": "/nonexistent"}),
        ),
        ("node-ready", &node, json!({})),
        (
            "node-dispatched",
            &node,
            json!({"persona": "engineer", "attempt": 1}),
        ),
        ("quiet-worker", &node, json!({"seconds": 30})),
        ("quiet-worker", &node, json!({"seconds": 60})),
        (
            "node-settled",
            &node,
            json!({"status": "done", "outcome": "complete", "landing": "landed"}),
        ),
        ("quiet-worker", &labels, json!({"seconds": 90})),
        ("run-finished", &labels, json!({"outcome": "complete"})),
    ];
    records
        .iter()
        .enumerate()
        .map(|(seq, (kind, labels, payload))| {
            line(
                STARTED_MS + seq as i64 * 1000,
                stream,
                seq as u64,
                "pipeline",
                kind,
                labels,
                payload,
            )
        })
        .collect::<String>()
        .into_bytes()
}

/// `size` bytes of heartbeats ending in the settlement a consumer's edge reads.
fn upstream_run(file: &mut impl Write, run: &str, node: &str, size: u64) {
    let labels = json!({"run_id": run, "node": node, "member": "worker", "persona": "engineer"});
    let plan = plan_doc(
        run,
        vec![json!({"id": node, "task": "## What\nDo it.", "persona": "engineer"})],
    );
    let started = line(
        STARTED_MS,
        "host-200",
        0,
        "pipeline",
        "run-started",
        &json!({"run_id": run}),
        &json!({"plan": plan, "graph": "graphs/dag-scope.yaml", "dir": "/nonexistent"}),
    );
    file.write_all(started.as_bytes()).expect("a record");
    let mut written = started.len() as u64;
    let mut seq = 0;
    while written < size {
        let record = line(
            STARTED_MS + seq as i64 * 15,
            "graph-200",
            seq,
            "agentgraph",
            "member-heartbeat",
            &labels,
            &json!({}),
        );
        file.write_all(record.as_bytes()).expect("a record");
        written += record.len() as u64;
        seq += 1;
    }
    let settled = line(
        STARTED_MS + seq as i64 * 15,
        "host-200",
        1,
        "pipeline",
        "node-settled",
        &json!({"run_id": run, "node": node}),
        &json!({"status": "done", "outcome": "complete", "landing": "landed"}),
    );
    file.write_all(settled.as_bytes()).expect("a record");
}

/// The long run: 19 nodes settled, the last naming [`UPSTREAM`] across runs, over 2,100
/// streams, in this host's largest run's record mix and with a checkpoint its size.
fn large_run(file: &mut impl Write) {
    let run = LARGE;
    let pipeline_labels = json!({"run_id": run});
    let mut rng = Rng::new(20_261_106);
    let mut pipeline_seq = 0_u64;
    let mut written = 0_u64;
    let mut records = 0_u64;
    let mut seqs = vec![0_u64; LARGE_STREAMS as usize];
    let emit = |file: &mut dyn Write, text: String, written: &mut u64, records: &mut u64| {
        file.write_all(text.as_bytes()).expect("a record");
        *written += text.len() as u64;
        *records += 1;
    };

    // The plan: the task bodies that fill the checkpoint's plan and graph twice over.
    let mut tasks = Vec::new();
    for index in 0..LARGE_NODES {
        let id = node_id(index);
        let task = task_text(&mut rng, &id, 35_000);
        let mut node = json!({"id": id, "task": task, "persona": "engineer"});
        if index == LARGE_NODES - 1 {
            node["deps"] = json!([format!("run:{UPSTREAM}#t01-upstream-node")]);
        }
        tasks.push(node);
    }
    let started = line(
        STARTED_MS,
        "host-100",
        pipeline_seq,
        "pipeline",
        "run-started",
        &pipeline_labels,
        &json!({"plan": plan_doc(run, tasks), "graph": "graphs/dag-scope.yaml", "dir": "/nonexistent"}),
    );
    pipeline_seq += 1;
    emit(file, started, &mut written, &mut records);

    let span_ms = 120_000 * 1000_i64;
    let at = |record: u64| STARTED_MS + 1000 + (record as i64) * span_ms / LARGE_RECORDS as i64;

    // Live edits: 116 nodes added and dropped, which is what `nodes_added` keeps, and the
    // records the fold keeps whole.
    let mut edit = 0;
    let mut kept = 0;
    let mut sessions = 0;
    let mut written_of_checkpoint = 0_u64;
    // What the checkpoint's own records take, which the rest of the mix is sized around.
    let checkpoint_records = 116 * 2 * 36_500 + 384 * 1_700;
    while records < LARGE_RECORDS - LARGE_NODES as u64 {
        let record = records;
        let ts = at(record);
        let stream_index =
            (record * LARGE_STREAMS / LARGE_RECORDS + rng.below(6)).min(LARGE_STREAMS - 2);
        let stream = format!("graph-{:04}-agentgraph-worker-stream", stream_index);
        let node = node_id(stream_index as usize % LARGE_NODES);
        let labels = json!({
            "run_id": run, "node": node, "member": "worker", "persona": "engineer",
            "onepipeline.run_id": run, "onepipeline.node": node, "onepipeline.persona": "engineer",
        });
        let seq = &mut seqs[stream_index as usize];
        // Evenly through the run, the checkpoint's own bytes.
        let text = if record % 900 == 500 && edit < 116 {
            let id = format!("added-{edit:03}");
            let added =
                json!({"id": id, "task": task_text(&mut rng, &id, 36_000), "persona": "engineer"});
            edit += 1;
            let payload = json!({
                "author": "planner",
                "command": {"op": "add", "id": id, "node": added},
                "operations": [{"kind": "node-added", "node": added}],
                "operation_kinds": ["node-added"],
            });
            let seq = pipeline_seq;
            pipeline_seq += 1;
            let added_line = line(
                ts,
                "host-100",
                seq,
                "pipeline",
                "edit-committed",
                &pipeline_labels,
                &payload,
            );
            let dropped = json!({
                "author": "planner",
                "command": {"op": "drop", "id": id, "dependents": "detach"},
                "operations": [{"kind": "node-dropped", "node": id, "dependents": "detach"}],
                "operation_kinds": ["node-dropped"],
            });
            let seq = pipeline_seq;
            pipeline_seq += 1;
            records += 1;
            let both = added_line
                + &line(
                    ts,
                    "host-100",
                    seq,
                    "pipeline",
                    "edit-committed",
                    &pipeline_labels,
                    &dropped,
                );
            written_of_checkpoint += both.len() as u64;
            both
        } else if record % 300 == 150 && kept < 384 {
            kept += 1;
            let payload = json!({"producer": "github.com/owner/engine", "target": "crate", "version": format!("1.0.{kept}"), "note": prose(&mut rng, 1_500)});
            let seq = pipeline_seq;
            pipeline_seq += 1;
            let kept_line = line(
                ts,
                "host-100",
                seq,
                "pipeline",
                "release-arrived",
                &pipeline_labels,
                &payload,
            );
            written_of_checkpoint += kept_line.len() as u64;
            kept_line
        } else if record % 110 == 55 && sessions < 1_100 {
            sessions += 1;
            let payload = json!({
                "role": "agent", "turn": sessions, "identity": "claude-code",
                "session_id": format!("0190a000-0000-7000-8000-{sessions:012}"),
                "history_id": format!("0190a000-0000-7000-9000-{sessions:012}"),
                "history_dir": "/nonexistent/oneharness/history",
                "history_project": "generated-project-slug",
                "history_session": format!("generated-session-{sessions:06}"),
            });
            let this = *seq;
            *seq += 1;
            line(
                ts,
                &stream,
                this,
                "agentgraph",
                "oneharness-session",
                &labels,
                &payload,
            )
        } else {
            // This host's mix of the rest: 40% heartbeats, 39% activity, and the remainder
            // about 830 bytes each, with the activity sized to reach the journal's bytes.
            let this = *seq;
            *seq += 1;
            let draw = rng.below(100);
            if draw < 42 {
                line(
                    ts,
                    &stream,
                    this,
                    "agentgraph",
                    "member-heartbeat",
                    &labels,
                    &json!({}),
                )
            } else if draw < 82 {
                let left = LARGE_RECORDS.saturating_sub(records).max(1);
                let spare = (LARGE_BYTES - checkpoint_records)
                    .saturating_sub(written.saturating_sub(written_of_checkpoint))
                    / left;
                let output = prose(
                    &mut rng,
                    ((spare as usize).saturating_sub(275) * 5 / 2)
                        .saturating_sub(470)
                        .clamp(100, 6_000),
                );
                line(
                    ts,
                    &stream,
                    this,
                    "agentgraph",
                    "turn-activity",
                    &labels,
                    &json!({"kind": "tool_result", "name": null, "detail": "ran it", "output": output, "index": this}),
                )
            } else {
                line(
                    ts,
                    &stream,
                    this,
                    "vcs",
                    "change-check",
                    &labels,
                    &json!({"name": "test-linux", "required": true, "status": "completed", "from_status": null, "conclusion": null, "state": "success"}),
                )
            }
        };
        emit(file, text, &mut written, &mut records);
    }

    // Each node settled, after what its record mix says it did.
    let end = at(LARGE_RECORDS) + 1000;
    for index in 0..LARGE_NODES {
        let node = json!({"run_id": run, "node": node_id(index)});
        let seq = pipeline_seq;
        pipeline_seq += 1;
        let settled = line(
            end + index as i64,
            "host-100",
            seq,
            "pipeline",
            "node-settled",
            &node,
            &json!({"status": "done", "outcome": "complete", "landing": "landed"}),
        );
        file.write_all(settled.as_bytes()).expect("a record");
    }
}

/// A store of three session files, written by oneharness's own writer, and the journal of
/// a run whose one node holds 5,000 references into them.
fn dispatch_run(root: &Path, store: &Path) -> (Vec<u8>, Generated) {
    let mut rng = Rng::new(7);
    let project = root.with_file_name("project");
    std::fs::create_dir_all(&project).expect("a project directory");
    let identity = "claude-code".parse().expect("a harness identity");
    let per_file = REFERENCES / SESSION_FILES;
    let mut references: Vec<(Value, u64, String)> = Vec::new();
    let mut files = Vec::new();
    // About as large as the budget's files: a line's envelope is about 400 bytes.
    let output_bytes = SESSION_BYTES / per_file - 330;
    for file in 0..SESSION_FILES {
        let writer = HistoryWriter::open(
            store,
            &project,
            &format!("worker-{file}"),
            HistoryLabels::default(),
        )
        .expect("a history session opens");
        let run_id = writer.begin_harness_run(&identity, "## What\nDo the long dispatch.");
        let begun =
            serde_json::to_value(writer.run_begun(&identity, run_id)).expect("a begun candidate");
        let count = if file == SESSION_FILES - 1 {
            REFERENCES - per_file * (SESSION_FILES - 1)
        } else {
            per_file
        };
        for index in 0..count {
            let call = format!("call-{file}-{index}");
            writer
                .append_event(
                    run_id,
                    "claude-code",
                    ActionEvent {
                        kind: "tool_result".into(),
                        name: Some("Bash".into()),
                        input: None,
                        output: Some(prose(&mut rng, output_bytes)),
                        index,
                        tool_call_id: Some(call.clone()),
                        started_at: None,
                        finished_at: None,
                        duration_ms: None,
                        status: None,
                        timing_source: None,
                    },
                )
                .expect("an event appends");
            let reference = json!({
                "history_dir": begun["history_dir"],
                "history_project": begun["history_project"],
                "history_session": begun["history_session"],
                "history_id": run_id.to_string(),
                "index": index,
                "tool_call_id": call,
            });
            references.push((reference, index as u64, call));
        }
        files.push(
            Path::new(begun["history_dir"].as_str().expect("a store"))
                .join(begun["history_project"].as_str().expect("a project"))
                .join(format!(
                    "{}.jsonl",
                    begun["history_session"].as_str().expect("a session")
                )),
        );
    }
    let session_bytes = files
        .iter()
        .map(|file| std::fs::metadata(file).expect("a session file").len())
        .sum();

    let run = DISPATCH;
    let labels = json!({"run_id": run});
    let node =
        json!({"run_id": run, "node": DISPATCH_NODE, "member": "worker", "persona": "engineer"});
    let plan = plan_doc(
        run,
        vec![json!({"id": DISPATCH_NODE, "task": "## What\nDo it.", "persona": "engineer"})],
    );
    let mut journal = line(
        STARTED_MS,
        "host-300",
        0,
        "pipeline",
        "run-started",
        &labels,
        &json!({"plan": plan, "graph": "graphs/dag-scope.yaml", "dir": "/nonexistent"}),
    );
    journal += &line(
        STARTED_MS + 1,
        "host-300",
        1,
        "pipeline",
        "node-dispatched",
        &json!({"run_id": run, "node": DISPATCH_NODE}),
        &json!({"persona": "engineer", "attempt": 1}),
    );
    let references_count = references.len();
    let mut seq = 0;
    for (at, (reference, index, _)) in references.into_iter().enumerate() {
        let mut payload = Map::new();
        payload.insert("transcript".into(), reference);
        payload.insert("index".into(), json!(index));
        journal += &line(
            STARTED_MS + 10 + at as i64,
            "graph-300",
            seq,
            "agentgraph",
            "turn-activity",
            &node,
            &Value::Object(payload),
        );
        seq += 1;
    }
    let beats = DISPATCH_RECORDS - references_count as u64 - 3;
    for beat in 0..beats {
        journal += &line(
            STARTED_MS + 6_000 + beat as i64,
            "graph-300",
            seq,
            "agentgraph",
            "member-heartbeat",
            &node,
            &json!({}),
        );
        seq += 1;
    }
    journal += &line(
        STARTED_MS + 10_000,
        "host-300",
        2,
        "pipeline",
        "node-settled",
        &json!({"run_id": run, "node": DISPATCH_NODE}),
        &json!({"status": "done", "outcome": "complete", "landing": "landed"}),
    );
    (
        journal.into_bytes(),
        Generated {
            session_bytes,
            references: references_count,
        },
    )
}

/// Write the whole corpus under `root`.
pub fn corpus(root: &Path, store: &Path) -> Generated {
    std::fs::create_dir_all(root).expect("a runs root");
    for index in 0..RUNS - 4 {
        let run = format!("history-{index:03}");
        write_run_files(root, &run, &small_run(&run));
    }
    write_run_files(root, SMALL, &small_run(SMALL));

    let open = |run: &str| {
        let dir = write_run_files(root, run, b"");
        std::io::BufWriter::new(std::fs::File::create(dir.join("events.jsonl")).expect("a journal"))
    };
    let mut upstream = open(UPSTREAM);
    upstream_run(&mut upstream, UPSTREAM, "t01-upstream-node", UPSTREAM_BYTES);
    upstream.flush().expect("flushed");
    let mut large = open(LARGE);
    large_run(&mut large);
    large.flush().expect("flushed");

    let (journal, generated) = dispatch_run(root, store);
    write_run_files(root, DISPATCH, &journal);
    generated
}
