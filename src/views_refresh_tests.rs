//! Refresh journeys through the real RunView, journal writer and fold.
use super::*;
use crate::journal::{self, Journal};
use serde_json::{json, Value};

struct Run {
    root: PathBuf,
    paths: RunPaths,
    writer: Journal,
}

impl Run {
    fn new(name: &str, plan: crate::plan::Plan) -> Self {
        let root = std::env::var_os("ONEPIPELINE_NODE_SCRATCH_DIR")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join(format!("refresh-{name}-{}", sys::pid()));
        std::fs::create_dir_all(&root).expect("scratch directory");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("run directory");
        let launch = super::tests::launch(0);
        ledger::write_json(&paths.launch(), &launch).expect("launch");
        let mut writer = Journal::open(&paths);
        writer
            .emit(
                PipelineKind::RunStarted,
                journal::labels("demo", None),
                journal::payload(&[("plan", json!(plan))]),
            )
            .expect("started");
        Self {
            root,
            paths,
            writer,
        }
    }

    fn settle(&mut self, status: &str) {
        self.writer
            .emit(
                PipelineKind::NodeSettled,
                journal::labels("demo", Some("build")),
                journal::payload(&[("status", json!(status)), ("landing", json!("unlanded"))]),
            )
            .expect("settlement");
    }

    fn open(&self) -> RunView {
        RunView::open(&self.paths).expect("view opens")
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove scratch");
    }
}

fn document(value: &(impl serde::Serialize + ?Sized)) -> Value {
    serde_json::to_value(value).expect("serializes")
}

fn same_as_open(view: &RunView) {
    let fresh = RunView::open(&view.paths).expect("fresh open");
    assert_eq!(document(&view.events), document(&fresh.events), "events");
    assert_eq!(document(&view.state), document(&fresh.state), "state");
    assert_eq!(
        view.state.cross_dag, fresh.state.cross_dag,
        "cross-DAG resolution is not serialized in checkpoint state"
    );
    assert_eq!(document(&view.launch), document(&fresh.launch), "launch");
    assert_eq!(reading(view), reading(&fresh), "run reading");
}

#[test]
fn refresh_reads_parses_and_folds_only_the_appended_bytes_and_records() {
    let mut run = Run::new("suffix", super::tests::plan());
    for _ in 0..30 {
        run.settle("failed");
    }
    let mut view = run.open();
    let before = document(&view.events);
    let first = view.events.len();
    let offset = std::fs::metadata(run.paths.journal())
        .expect("journal")
        .len();
    run.settle("done");
    run.settle("failed");
    let end = std::fs::metadata(run.paths.journal())
        .expect("journal")
        .len();
    crate::readprobe::start();
    assert_eq!(
        view.refresh().expect("refresh"),
        Refreshed::Appended { first }
    );
    let observed = crate::readprobe::finish();
    assert_eq!(document(&view.events[..first]), before);
    let suffix = &view.events[first..];
    assert_eq!(suffix.len(), 2);
    assert_eq!(suffix[0].payload["status"], "done");
    assert_eq!(suffix[1].payload["status"], "failed");
    assert_eq!(
        observed.folded,
        suffix
            .iter()
            .map(|e| (e.stream.clone(), e.seq))
            .collect::<Vec<_>>()
    );
    assert_eq!(observed.parsed.len(), 2);
    assert!(observed
        .parsed
        .iter()
        .all(|(path, at)| *path == run.paths.journal() && *at >= offset));
    let reads: Vec<_> = observed
        .reads
        .iter()
        .filter(|(path, _, _)| *path == run.paths.journal())
        .collect();
    assert_eq!(reads, vec![&(run.paths.journal(), offset, end - offset)]);
    same_as_open(&view);
    crate::readprobe::start();
    assert_eq!(view.refresh().expect("unchanged"), Refreshed::Unchanged);
    let observed = crate::readprobe::finish();
    assert!(observed.parsed.is_empty());
    assert!(observed.folded.is_empty());
    assert!(observed
        .reads
        .iter()
        .all(|(path, _, _)| *path != run.paths.journal()));
    same_as_open(&view);
}

#[test]
fn refresh_rereads_out_of_order_records_and_then_resumes_appending() {
    let mut run = Run::new("order", super::tests::plan());
    run.settle("failed");
    let mut view = run.open();
    let mut earlier = view.events.last().expect("event").clone();
    earlier.stream = "earlier-host".into();
    earlier.seq = 0;
    earlier.ts = "1999-01-01T00:00:00.000Z".into();
    earlier.payload.insert("status".into(), json!("done"));
    run.writer.relay(&earlier).expect("relayed");
    assert_eq!(view.refresh().expect("reread"), Refreshed::Reread);
    same_as_open(&view);
    let first = view.events.len();
    run.settle("done");
    assert_eq!(
        view.refresh().expect("append"),
        Refreshed::Appended { first }
    );
    same_as_open(&view);
}

/// Wait until a write would stamp a later file time than `journal` carries. The
/// kernel's file times advance by the tick, so a same-length edit landing inside
/// the tick of the last append is one no reader of the metadata can see, and that
/// timing is not the edit this journey is about.
fn past_the_file_time_of(journal: &std::path::Path) {
    let modified = |path: &std::path::Path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .expect("a file time")
    };
    let probe = journal.with_extension("tick");
    loop {
        std::fs::write(&probe, "").expect("probe");
        if modified(&probe) > modified(journal) {
            std::fs::remove_file(&probe).expect("remove probe");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn refresh_rereads_truncation_and_same_length_prefix_edits_including_before_growth() {
    for change in ["truncate", "edit", "replace", "edit-then-append"] {
        let mut run = Run::new(change, super::tests::plan());
        run.settle("done");
        let mut view = run.open();
        let bytes = std::fs::read_to_string(run.paths.journal()).expect("journal");
        past_the_file_time_of(&run.paths.journal());
        match change {
            "truncate" => std::fs::write(
                run.paths.journal(),
                bytes.lines().next().expect("first").to_owned() + "\n",
            )
            .expect("truncate"),
            _ => {
                let changed = bytes.replace("\"done\"", "\"fail\"");
                assert_ne!(changed, bytes);
                assert_eq!(changed.len(), bytes.len());
                if change == "replace" {
                    let replacement = run.paths.dir.join("replacement");
                    std::fs::write(&replacement, changed).expect("replacement");
                    std::fs::rename(replacement, run.paths.journal()).expect("replace");
                } else {
                    std::fs::write(run.paths.journal(), changed).expect("edit");
                }
                if change == "edit-then-append" {
                    run.settle("failed");
                }
            }
        }
        assert_eq!(
            view.refresh().expect("reread"),
            Refreshed::Reread,
            "{change}"
        );
        same_as_open(&view);
    }
}

#[test]
fn refresh_rereads_label_overlays_with_and_without_growth() {
    for append in [false, true] {
        let mut run = Run::new(
            if append { "labels-append" } else { "labels" },
            super::tests::plan(),
        );
        let mut view = run.open();
        crate::labels::replace(
            &run.paths,
            crate::verbs::LabelRequest {
                labels: &crate::labels::parse(&["team=engine".into()]).expect("valid labels"),
                unlabel: &[],
                group: crate::verbs::LabelGroup::Keep,
                force: true,
                launcher: "test",
                session: "session-a",
            },
        )
        .expect("labels replaced");
        if append {
            run.settle("done");
        }
        assert_eq!(view.refresh().expect("reread"), Refreshed::Reread);
        assert_eq!(view.launch.labels["team"], "engine");
        same_as_open(&view);
    }
}

#[test]
fn refresh_rereads_upstream_resolution_with_and_without_growth() {
    for append in [false, true] {
        let mut plan = super::tests::plan();
        plan.tasks[0].deps = vec!["run:upstream#build".into()];
        let mut run = Run::new(
            if append {
                "upstream-append"
            } else {
                "upstream"
            },
            plan,
        );
        let upstream = RunPaths::under(&run.root, "upstream");
        upstream.create().expect("upstream");
        ledger::write_json(&upstream.launch(), &super::tests::launch(0)).expect("launch");
        let mut writer = Journal::open(&upstream);
        let mut view = run.open();
        assert_eq!(
            view.state.cross_dag["run:upstream#build"],
            NodeStatus::Blocked
        );
        crate::readprobe::start();
        assert_eq!(
            view.refresh().expect("unchanged upstream"),
            Refreshed::Unchanged
        );
        let observed = crate::readprobe::finish();
        assert!(observed.parsed.is_empty());
        assert!(observed.folded.is_empty());
        assert!(observed
            .reads
            .iter()
            .all(|(path, _, _)| path.file_name().is_none_or(|name| name != "events.jsonl")));

        writer
            .emit(
                PipelineKind::NodeSettled,
                journal::labels("upstream", Some("build")),
                journal::payload(&[("status", json!("done"))]),
            )
            .expect("settle upstream");
        if append {
            run.settle("done");
        }
        assert_eq!(view.refresh().expect("reread"), Refreshed::Reread);
        assert_eq!(view.state.cross_dag["run:upstream#build"], NodeStatus::Done);
        same_as_open(&view);
    }
}

#[test]
fn refresh_rereads_landing_information_with_and_without_growth() {
    for append in [false, true] {
        let mut run = Run::new(
            if append { "landing-append" } else { "landing" },
            super::tests::plan(),
        );
        run.settle("done");
        let mut view = run.open();
        assert_eq!(view.state.landings.get("build"), Some(&Landing::Unlanded));
        let result = json!({"schema_version": 6, "run_id": "demo", "state": "complete", "ok": true,
            "nodes": [{"id": "build", "status": "done", "landing": "landed"}]});
        ledger::write_json(&run.paths.result(), &result).expect("landing report");
        if append {
            run.settle("done");
        }
        assert_eq!(view.refresh().expect("reread"), Refreshed::Reread);
        assert_eq!(view.state.landings.get("build"), Some(&Landing::Landed));
        same_as_open(&view);
        let first = view.events.len();
        run.settle("done");
        assert_eq!(
            view.refresh().expect("compatible landing overlay"),
            Refreshed::Appended { first }
        );
        same_as_open(&view);
        std::fs::remove_file(run.paths.result()).expect("remove report");
        assert_eq!(view.refresh().expect("reread removal"), Refreshed::Reread);
        assert_eq!(view.state.landings.get("build"), Some(&Landing::Unlanded));
        same_as_open(&view);
    }
}

#[test]
fn refresh_recovers_from_missing_or_unreadable_append_receipts() {
    for contents in [None, Some("not json"), Some("unsupported")] {
        let mut run = Run::new(
            &format!("receipt-{}", contents.unwrap_or("missing").len()),
            super::tests::plan(),
        );
        let mut view = run.open();
        let receipt = run.paths.journal().with_extension("append.json");
        if contents == Some("unsupported") {
            let mut document: Value = ledger::read_json(&receipt).expect("valid receipt");
            document["schema_version"] = json!(999);
            ledger::write_json(&receipt, &document).expect("unsupported but well-formed receipt");
        } else if let Some(contents) = contents {
            std::fs::write(&receipt, contents).expect("corrupt receipt");
        } else {
            std::fs::remove_file(&receipt).expect("remove receipt");
        }
        run.settle("failed");
        assert_eq!(view.refresh().expect("reread"), Refreshed::Reread);
        same_as_open(&view);
        let first = view.events.len();
        run.settle("done");
        assert_eq!(
            view.refresh().expect("append"),
            Refreshed::Appended { first }
        );
        same_as_open(&view);
    }
}

#[test]
fn refresh_merges_only_the_new_streams_and_refuses_lower_sequence_appends() {
    let mut run = Run::new("merge-suffix", super::tests::plan());
    let mut view = run.open();
    let first = view.events.len();
    let mut later = view.events[0].clone();
    later.kind = PipelineKind::NodeSettled.into();
    later.labels.node = Some("build".into());
    later.stream = "z-stream".into();
    later.seq = 3;
    later.ts = "2099-01-01T00:00:00.000Z".into();
    later.payload = journal::payload(&[("status", json!("done"))]);
    run.writer.relay(&later).expect("later appended first");
    let mut earlier = later.clone();
    earlier.stream = "a-stream".into();
    earlier.payload.insert("status".into(), json!("failed"));
    run.writer.relay(&earlier).expect("earlier appended second");
    assert_eq!(
        view.refresh().expect("append"),
        Refreshed::Appended { first }
    );
    assert_eq!(view.events[first].stream, "a-stream");
    assert_eq!(view.events[first + 1].stream, "z-stream");
    same_as_open(&view);
    later.seq = 2;
    run.writer.relay(&later).expect("lower sequence appended");
    assert_eq!(view.refresh().expect("reread"), Refreshed::Reread);
    same_as_open(&view);
}

#[test]
fn refresh_refuses_invalid_external_labels_and_recovers_without_losing_the_suffix() {
    let mut run = Run::new("invalid-labels", super::tests::plan());
    let mut view = run.open();
    let first = view.events.len();
    let before = document(&view.state);
    ledger::write_json(
        &run.paths.dir.join("labels.json"),
        &json!({"schema_version": 999}),
    )
    .expect("invalid overlay");
    run.settle("done");
    assert!(view.refresh().is_err());
    assert_eq!(view.events.len(), first);
    assert_eq!(document(&view.state), before);
    std::fs::remove_file(run.paths.dir.join("labels.json")).expect("remove overlay");
    assert_eq!(
        view.refresh().expect("append after recovery"),
        Refreshed::Appended { first }
    );
    same_as_open(&view);
}

/// Trace the actual kernel reads, including reads inside dependencies. The child
/// enters the same public refresh interface after marking the writer's work done.
#[cfg(target_os = "linux")]
#[test]
fn refresh_syscalls_read_only_the_appended_journal_range() {
    const CHILD: &str = "ONEPIPELINE_REFRESH_IO_CHILD";
    const TEST: &str =
        "views::refresh_tests::refresh_syscalls_read_only_the_appended_journal_range";
    if let Some(root) = std::env::var_os(CHILD) {
        let mut run = Run::new("syscall-child", super::tests::plan());
        for _ in 0..10 {
            run.settle("failed");
        }
        let mut view = run.open();
        let offset = std::fs::metadata(run.paths.journal())
            .expect("journal")
            .len();
        run.settle("done");
        let end = std::fs::metadata(run.paths.journal())
            .expect("journal")
            .len();
        let root = PathBuf::from(root);
        ledger::write_json(
            &root.join("range.json"),
            &json!({"path": run.paths.journal(), "from": offset, "bytes": end - offset}),
        )
        .expect("range evidence");
        std::fs::write(root.join("refresh-io-begin"), b"").expect("begin marker");
        assert_eq!(
            view.refresh().expect("refresh"),
            Refreshed::Appended { first: 11 }
        );
        std::fs::write(root.join("refresh-io-end"), b"").expect("end marker");
        return;
    }
    let evidence = Run::new("syscall-parent", super::tests::plan());
    let trace = evidence.root.join("syscalls.log");
    let output = std::process::Command::new("strace")
        .args([
            "-f",
            "-yy",
            "-s",
            "512",
            "-e",
            "trace=read,pread64,lseek,openat,close",
            "-o",
        ])
        .arg(&trace)
        .arg(std::env::current_exe().expect("test binary"))
        .args(["--exact", TEST, "--nocapture"])
        .env(CHILD, &evidence.root)
        .output()
        .expect("strace runs");
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let range: Value =
        ledger::read_json(&evidence.root.join("range.json")).expect("range evidence");
    let journal = format!("<{}>", range["path"].as_str().expect("journal path"));
    let from = range["from"].as_u64().expect("offset");
    let expected = range["bytes"].as_u64().expect("byte count");
    let trace = std::fs::read_to_string(trace).expect("trace");
    let mut in_refresh = false;
    let mut ended = false;
    let mut read_bytes = 0;
    let mut seeks = Vec::new();
    for line in trace.lines() {
        if line.contains("openat(") && line.contains("refresh-io-begin") {
            in_refresh = true;
        }
        if line.contains("openat(") && line.contains("refresh-io-end") {
            ended = true;
            break;
        }
        if !in_refresh || !line.contains(&journal) {
            continue;
        }
        if line.contains("lseek(") {
            assert!(
                line.contains(&format!(", {from}, SEEK_SET)")),
                "prefix seek: {line}"
            );
            seeks.push(line);
        }
        if line.contains("read(") || line.contains("pread64(") {
            assert!(
                !line.contains("pread64("),
                "unexpected positioned read: {line}"
            );
            let bytes = line
                .rsplit_once(" = ")
                .expect("read result")
                .1
                .split_whitespace()
                .next()
                .expect("read bytes")
                .parse::<u64>()
                .expect("successful read");
            read_bytes += bytes;
        }
    }
    assert!(in_refresh && ended, "refresh markers absent: {trace}");
    assert_eq!(seeks.len(), 1, "unexpected journal seeks: {trace}");
    assert_eq!(
        read_bytes, expected,
        "journal I/O exceeded appended range: {trace}"
    );
}

thread_local! {
    // Schedule a real external file mutation at the read boundary. No read,
    // parse, fold or result is substituted by this interleaving control.
    static BEFORE_APPENDED_READ: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn before_appended_read() {
    if let Some(change) = BEFORE_APPENDED_READ.with_borrow_mut(Option::take) {
        change();
    }
}

#[test]
fn refresh_recovers_when_receipt_publication_fails_after_a_successful_append() {
    const CHILD: &str = "ONEPIPELINE_REFRESH_RECEIPT_FAILURE_CHILD";
    const TEST: &str = "views::refresh_tests::refresh_recovers_when_receipt_publication_fails_after_a_successful_append";
    if let Some(root) = std::env::var_os(CHILD) {
        let paths = RunPaths::under(Path::new(&root), "demo");
        Journal::open(&paths)
            .emit(
                PipelineKind::NodeSettled,
                journal::labels("demo", Some("build")),
                journal::payload(&[("status", json!("failed"))]),
            )
            .expect("append succeeds despite receipt failure");
        return;
    }
    let mut run = Run::new("receipt-publication", super::tests::plan());
    let mut view = run.open();
    let first = view.events.len();
    let receipt = run.paths.journal().with_extension("append.json");
    std::fs::remove_file(&receipt).expect("remove receipt");
    std::fs::create_dir(&receipt).expect("a directory obstructs publication");
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", TEST, "--nocapture"])
        .env(CHILD, &run.root)
        .output()
        .expect("writer child runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("onepipeline: append receipt:"), "{stderr}");
    assert!(stderr.contains("events.append.json"), "{stderr}");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("onepipeline: append receipt:"),
        "diagnostic belongs on stderr"
    );
    assert_eq!(view.refresh().expect("safe reread"), Refreshed::Reread);
    assert_eq!(view.events.len(), first + 1, "the journal append succeeded");
    same_as_open(&view);
    std::fs::remove_dir(&receipt).expect("remove obstruction");
    run.settle("done");
    assert_eq!(view.refresh().expect("new epoch"), Refreshed::Reread);
    same_as_open(&view);
    let first = view.events.len();
    run.settle("failed");
    assert_eq!(
        view.refresh().expect("recovered append"),
        Refreshed::Appended { first }
    );
    same_as_open(&view);
}

#[test]
fn refresh_recovers_from_incomplete_failed_and_changing_range_reads() {
    use std::io::{Seek, SeekFrom, Write};
    for change in ["incomplete", "shortened", "grew"] {
        let mut run = Run::new(change, super::tests::plan());
        let mut view = run.open();
        let from = std::fs::metadata(run.paths.journal())
            .expect("journal")
            .len();
        run.settle("failed");
        let paths = run.paths.clone();
        BEFORE_APPENDED_READ.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || match change {
                "incomplete" => {
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .open(paths.journal())
                        .expect("journal");
                    file.seek(SeekFrom::End(-1)).expect("terminator");
                    file.write_all(b" ").expect("torn suffix");
                }
                "shortened" => std::fs::OpenOptions::new()
                    .write(true)
                    .open(paths.journal())
                    .expect("journal")
                    .set_len(from)
                    .expect("shortened range"),
                "grew" => Journal::open(&paths)
                    .emit(
                        PipelineKind::NodeSettled,
                        journal::labels("demo", Some("build")),
                        journal::payload(&[("status", json!("done"))]),
                    )
                    .expect("concurrent append"),
                _ => unreachable!(),
            }));
        });
        assert_eq!(
            view.refresh().expect("reread"),
            Refreshed::Reread,
            "{change}"
        );
        same_as_open(&view);
        run.writer = Journal::open(&run.paths);
        run.settle("done");
        view.refresh().expect("first recovery read");
        same_as_open(&view);
        let first = view.events.len();
        run.settle("failed");
        assert_eq!(
            view.refresh().expect("append after recovery"),
            Refreshed::Appended { first }
        );
        same_as_open(&view);
    }
}

#[test]
fn refresh_rereads_when_edits_introduce_or_remove_an_upstream() {
    let mut run = Run::new("edited-upstream", super::tests::plan());
    let mut view = run.open();
    for operation in [
        crate::edits::Operation::EdgeAdded {
            from: "run:upstream#build".into(),
            to: "build".into(),
            target: None,
        },
        crate::edits::Operation::EdgeRemoved {
            from: "run:upstream#build".into(),
            to: "build".into(),
        },
    ] {
        run.writer
            .emit(
                PipelineKind::EditCommitted,
                journal::labels("demo", None),
                journal::payload(&[("operations", json!([operation]))]),
            )
            .expect("committed graph edit");
        assert_eq!(
            view.refresh().expect("reread edited dependencies"),
            Refreshed::Reread
        );
        same_as_open(&view);
    }
    assert!(view.state.cross_dag.is_empty());
    let first = view.events.len();
    run.settle("done");
    assert_eq!(
        view.refresh().expect("append after graph edits"),
        Refreshed::Appended { first }
    );
    same_as_open(&view);
}

#[test]
fn refresh_replaces_the_view_when_its_public_paths_select_another_run() {
    let run = Run::new("old-directory", super::tests::plan());
    let mut other = Run::new("new-directory", super::tests::plan());
    other.settle("done");
    let mut view = run.open();
    view.paths = other.paths.clone();
    assert_eq!(view.refresh().expect("new directory"), Refreshed::Reread);
    assert_eq!(view.events.len(), 2);
    same_as_open(&view);
    let first = view.events.len();
    other.settle("failed");
    assert_eq!(
        view.refresh().expect("append on new run"),
        Refreshed::Appended { first }
    );
    same_as_open(&view);
}

#[test]
fn refresh_decodes_malformed_glued_and_unsupported_suffix_records_as_open_does() {
    let mut run = Run::new("damaged-suffix", super::tests::plan());
    let mut view = run.open();
    let first = view.events.len();
    ledger::append_line(&run.paths.journal(), "not json").expect("old malformed record");
    assert_eq!(
        view.refresh().expect("account for unreadable suffix"),
        Refreshed::Appended { first }
    );
    assert_eq!(view.events.len(), first);
    same_as_open(&view);
    let mut event = view.events[0].clone();
    event.kind = PipelineKind::NodeSettled.into();
    event.labels.node = Some("build".into());
    event.seq += 1;
    event.payload = journal::payload(&[("status", json!("done"))]);
    let glued = format!(
        "fragment{}",
        serde_json::to_string(&event).expect("envelope")
    );
    ledger::append_line(&run.paths.journal(), &glued).expect("old glued record");
    assert_eq!(
        view.refresh().expect("recover glued envelope"),
        Refreshed::Appended { first }
    );
    assert_eq!(view.events.len(), first + 1);
    same_as_open(&view);
    let first = view.events.len();
    event.v += 1;
    event.seq += 1;
    run.writer.relay(&event).expect("newer producer's envelope");
    assert_eq!(
        view.refresh().expect("unsupported suffix"),
        Refreshed::Appended { first }
    );
    assert!(
        !view.state.strict,
        "unknown pipeline schema cannot prove a strict fold"
    );
    same_as_open(&view);
}

/// The kernel accepts part of a record, refuses the rest, and the real writer
/// rolls it back. Run the resource ceiling in a child so other tests are untouched.
#[cfg(unix)]
#[test]
fn refresh_rereads_after_a_failed_append_rolls_back_and_then_recovers() {
    use std::os::unix::process::CommandExt;
    const CHILD: &str = "ONEPIPELINE_REFRESH_SHORT_WRITE_CHILD";
    const TEST: &str =
        "views::refresh_tests::refresh_rereads_after_a_failed_append_rolls_back_and_then_recovers";
    if let Some(root) = std::env::var_os(CHILD) {
        let paths = RunPaths::under(Path::new(&root), "demo");
        let error = Journal::open(&paths)
            .emit(
                PipelineKind::NodeSettled,
                journal::labels("demo", Some("build")),
                journal::payload(&[
                    ("status", json!("done")),
                    ("outcome", json!("x".repeat(3000))),
                ]),
            )
            .expect_err("the kernel's file-size ceiling refuses the append");
        match error {
            crate::Error::Ledger { path, source } => {
                assert_eq!(path, paths.journal());
                assert_eq!(source.raw_os_error(), Some(libc::EFBIG));
            }
            error => panic!("unexpected refusal: {error}"),
        }
        return;
    }
    let mut run = Run::new("short-write", super::tests::plan());
    let mut view = run.open();
    let before = std::fs::read(run.paths.journal()).expect("journal");
    let ceiling = before.len() as u64 + 64;
    let mut command = std::process::Command::new(std::env::current_exe().expect("test binary"));
    command
        .args(["--exact", TEST, "--nocapture"])
        .env(CHILD, &run.root)
        .env(
            "LLVM_PROFILE_FILE",
            run.root.join("under-ceiling-%p.profraw"),
        );
    // SAFETY: only async-signal-safe syscalls run between fork and exec;
    // the resource limit and signal disposition belong to this child alone.
    unsafe {
        command.pre_exec(move || {
            let ceiling = libc::rlimit {
                rlim_cur: ceiling,
                rlim_max: ceiling,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &ceiling) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::signal(libc::SIGXFSZ, libc::SIG_IGN) == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().expect("limited child runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(run.paths.journal()).expect("journal after refusal"),
        before,
        "the failed append rolled back to the original boundary"
    );
    assert_eq!(
        view.refresh().expect("reread rolled-back journal"),
        Refreshed::Reread
    );
    same_as_open(&view);
    run.settle("done");
    assert_eq!(view.refresh().expect("new epoch"), Refreshed::Reread);
    same_as_open(&view);
    let first = view.events.len();
    run.settle("failed");
    assert_eq!(
        view.refresh().expect("recovered suffix processing"),
        Refreshed::Appended { first }
    );
    same_as_open(&view);
}

#[test]
fn refresh_rereads_when_the_upstream_answer_open_reused_expires_or_changes() {
    let window = 2 * crate::engine::UPSTREAM_EVERY;
    for expire in [true, false] {
        // The second open reuses the primed answer only inside the cache's real
        // window. A loaded runner can spend that whole window between the two
        // opens, which is no evidence about reuse, so only an attempt that
        // provably outlived it is set up again; one inside it must reuse.
        let (run, mut view) = (0..5)
            .find_map(|attempt| {
                let mut plan = super::tests::plan();
                plan.tasks[0].deps = vec!["run:upstream#build".into()];
                let kind = if expire { "expired" } else { "refreshed" };
                let run = Run::new(&format!("stale-{kind}-{attempt}"), plan);
                let upstream = RunPaths::under(&run.root, "upstream");
                upstream.create().expect("upstream directory");
                ledger::write_json(&upstream.launch(), &super::tests::launch(0))
                    .expect("upstream launch");
                let mut writer = Journal::open(&upstream);
                let started = std::time::Instant::now();
                let primed = run.open();
                assert_eq!(
                    primed.state.cross_dag["run:upstream#build"],
                    NodeStatus::Blocked
                );
                writer
                    .emit(
                        PipelineKind::NodeSettled,
                        journal::labels("upstream", Some("build")),
                        journal::payload(&[("status", json!("done"))]),
                    )
                    .expect("upstream settles before retained open");
                let view = run.open();
                let reused = view.state.cross_dag["run:upstream#build"] == NodeStatus::Blocked;
                assert!(
                    reused || started.elapsed() >= window,
                    "this journey must exercise open reusing an older cached answer"
                );
                reused.then_some((run, view))
            })
            .expect("one attempt opens inside the cache window");
        if expire {
            // Exercise the real cache's lifetime, not a simulated clock or answer.
            std::thread::sleep(window + std::time::Duration::from_millis(20));
        } else {
            crate::crossdag::resolve_for_refresh(&run.root, &view.state.graph);
        }
        assert_eq!(
            view.refresh()
                .expect("cache no longer matches retained answer"),
            Refreshed::Reread
        );
        assert_eq!(view.state.cross_dag["run:upstream#build"], NodeStatus::Done);
        same_as_open(&view);
        crate::readprobe::start();
        assert_eq!(view.refresh().expect("now unchanged"), Refreshed::Unchanged);
        let observed = crate::readprobe::finish();
        assert!(observed.parsed.is_empty());
        assert!(observed.folded.is_empty());
        assert!(observed
            .reads
            .iter()
            .all(|(path, _, _)| path.file_name().is_none_or(|name| name != "events.jsonl")));
    }
}

#[test]
fn refresh_does_not_reread_unchanged_upstreams_when_the_shared_cache_expires() {
    let mut plan = super::tests::plan();
    plan.tasks[0].deps = vec!["run:upstream#build".into()];
    let run = Run::new("unchanged-expired", plan);
    let upstream = RunPaths::under(&run.root, "upstream");
    upstream.create().expect("upstream directory");
    ledger::write_json(&upstream.launch(), &super::tests::launch(0)).expect("launch");
    Journal::open(&upstream)
        .emit(
            PipelineKind::NodeSettled,
            journal::labels("upstream", Some("build")),
            journal::payload(&[("status", json!("done"))]),
        )
        .expect("settlement");
    let initial = run.open();
    crate::crossdag::resolve_for_refresh(&run.root, &initial.state.graph);
    let mut view = run.open();
    assert_eq!(view.state.cross_dag["run:upstream#build"], NodeStatus::Done);
    std::thread::sleep(2 * crate::engine::UPSTREAM_EVERY + std::time::Duration::from_millis(20));
    crate::readprobe::start();
    assert_eq!(
        view.refresh().expect("unchanged after cache expiration"),
        Refreshed::Unchanged
    );
    let observed = crate::readprobe::finish();
    assert!(observed.parsed.is_empty());
    assert!(observed.folded.is_empty());
    assert!(observed
        .reads
        .iter()
        .all(|(path, _, _)| path.file_name().is_none_or(|name| name != "events.jsonl")));
    same_as_open(&view);
}

#[test]
fn refresh_preserves_the_view_when_launch_reads_fail_and_recovers_after_restoration() {
    for corrupt in [false, true] {
        let mut run = Run::new(
            if corrupt {
                "corrupt-launch"
            } else {
                "missing-launch"
            },
            super::tests::plan(),
        );
        let mut view = run.open();
        let launch = std::fs::read(run.paths.launch()).expect("original launch");
        let first = view.events.len();
        let events = document(&view.events);
        let state = document(&view.state);
        let attribution = document(&view.launch);
        if corrupt {
            std::fs::write(run.paths.launch(), b"not json").expect("corrupt launch");
        } else {
            std::fs::remove_file(run.paths.launch()).expect("remove launch");
        }
        run.settle("done");
        assert!(
            view.refresh().is_err(),
            "an unreadable launch must be reported"
        );
        assert_eq!(document(&view.events), events);
        assert_eq!(document(&view.state), state);
        assert_eq!(document(&view.launch), attribution);
        std::fs::write(run.paths.launch(), launch).expect("restore launch");
        assert_eq!(
            view.refresh().expect("recovery reads retained suffix"),
            Refreshed::Appended { first }
        );
        same_as_open(&view);
    }
}

#[test]
fn refresh_rereads_launch_attribution_changes_with_and_without_growth() {
    for append in [false, true] {
        let mut run = Run::new(
            if append {
                "launch-append"
            } else {
                "launch-attribution"
            },
            super::tests::plan(),
        );
        let mut view = run.open();
        let mut launch: LaunchRecord = ledger::read_json(&run.paths.launch()).expect("launch");
        launch.session = "adopting-session".into();
        launch.adoptions += 1;
        ledger::write_json(&run.paths.launch(), &launch).expect("new driver attribution");
        if append {
            run.settle("done");
        }
        assert_eq!(
            view.refresh().expect("reread attribution"),
            Refreshed::Reread
        );
        assert_eq!(view.launch.session, "adopting-session");
        assert_eq!(view.launch.adoptions, 1);
        same_as_open(&view);
    }
}

#[test]
fn refresh_rereads_dependency_edits_within_the_same_upstream_journal() {
    let mut plan = super::tests::plan();
    plan.tasks[0].deps = vec!["run:upstream#build".into()];
    let mut run = Run::new("same-upstream-edits", plan);
    let upstream = RunPaths::under(&run.root, "upstream");
    upstream.create().expect("upstream directory");
    ledger::write_json(&upstream.launch(), &super::tests::launch(0)).expect("launch");
    let mut writer = Journal::open(&upstream);
    for (node, status) in [("build", "done"), ("ship", "failed")] {
        writer
            .emit(
                PipelineKind::NodeSettled,
                journal::labels("upstream", Some(node)),
                journal::payload(&[("status", json!(status))]),
            )
            .expect("upstream settlement");
    }
    let initial = run.open();
    crate::crossdag::resolve_for_refresh(&run.root, &initial.state.graph);
    let mut view = run.open();
    assert_eq!(view.state.cross_dag["run:upstream#build"], NodeStatus::Done);
    let operations = [
        crate::edits::Operation::EdgeAdded {
            from: "run:upstream#ship".into(),
            to: "build".into(),
            target: None,
        },
        crate::edits::Operation::EdgeRemoved {
            from: "run:upstream#build".into(),
            to: "build".into(),
        },
        crate::edits::Operation::Reparent {
            node: "build".into(),
            from: vec!["run:upstream#ship".into()],
            to: vec!["run:upstream#release".into()],
        },
    ];
    for operation in operations {
        run.writer
            .emit(
                PipelineKind::EditCommitted,
                journal::labels("demo", None),
                journal::payload(&[("operations", json!([operation]))]),
            )
            .expect("committed edit within one upstream run");
        assert_eq!(
            view.refresh().expect("reread changed node references"),
            Refreshed::Reread
        );
        same_as_open(&view);
    }
    assert_eq!(
        view.state.cross_dag,
        BTreeMap::from([("run:upstream#release".into(), NodeStatus::Blocked)])
    );
    let first = view.events.len();
    run.settle("done");
    assert_eq!(
        view.refresh().expect("append after dependency edits"),
        Refreshed::Appended { first }
    );
    same_as_open(&view);
}

/// The answers a first screen is drawn from, each of which a selective view must give
/// exactly as [`RunView::open`] does.
fn same_answers(view: &RunView, fresh: &RunView) {
    assert_eq!(view.state.statuses(), fresh.state.statuses(), "statuses");
    assert_eq!(
        reading(view),
        reading(fresh),
        "reasons, awaits and outcomes"
    );
    assert_eq!(view.summary(), fresh.summary(), "summary");
    let last_seq = |view: &RunView| view.events.iter().map(|event| event.seq).max();
    assert_eq!(last_seq(view), last_seq(fresh), "last_seq");
    assert_eq!(view.refresh.result, fresh.refresh.result, "result");
    assert_eq!(
        view.state.last_write_at, fresh.state.last_write_at,
        "the run's span"
    );
    assert_eq!(
        document(&view.state.landings),
        document(&fresh.state.landings),
        "landings"
    );
}

fn over_first_screen(paths: &RunPaths) -> RunView {
    let selected = crate::journal::read_selected(
        &paths.journal(),
        &crate::journal::Selection::FirstScreen { node: None },
    );
    RunView::over_selected(paths, selected.events).expect("a selective view opens")
}

/// A node of a selective view's plan, depending on `deps`.
fn node(id: &str, deps: &[&str]) -> crate::plan::Node {
    crate::plan::Node {
        id: id.into(),
        persona: Some("engineer".into()),
        task: Some("## What\ndo it".into()),
        deps: deps.iter().map(|dep| (*dep).to_string()).collect(),
        ..crate::plan::Node::default()
    }
}

/// A relayed record of another stream, stamped `at` milliseconds into a day long past
/// or, at `0`, now.
fn relayed(stream: &str, seq: u64, kind: &str, node: Option<&str>, at: u64) -> Envelope {
    serde_json::from_value(json!({
        "v": 1,
        "ts": if at == 0 { sys::now_rfc3339() } else { format!("2020-01-01T00:{:02}:{:02}.{:03}Z", at / 60_000, (at / 1000) % 60, at % 1000) },
        "stream": stream,
        "seq": seq,
        "source": "agentgraph",
        "kind": kind,
        "labels": {"run_id": "demo", "node": node},
        "payload": {"member": "worker", "persona": "engineer"},
    }))
    .expect("a relayed record")
}

fn settled_with(run: &mut Run, id: &str, fields: &[(&str, Value)]) {
    run.writer
        .emit(
            PipelineKind::NodeSettled,
            journal::labels("demo", Some(id)),
            journal::payload(fields),
        )
        .expect("a settlement");
}

/// Settled, unsettled and gated nodes, over a journal whose first and last records,
/// and each stream's, are heartbeat and activity records.
#[test]
fn a_view_over_first_screen_records_answers_as_a_complete_open_does_and_refresh_keeps_it_so() {
    let mut plan = super::tests::plan();
    plan.tasks = vec![
        node("landed", &[]),
        node("unlanded", &[]),
        node("failed", &[]),
        node("after-failed", &["failed"]),
        node("awaiting", &[]),
        node("gated", &["run:never-launched#build"]),
        node("running", &[]),
    ];
    let mut run = Run::new("selected", plan);
    // In front of the run's first record of its own and past its last, on streams of
    // their own: the span rests on what a first screen is told to keep only as ends.
    run.writer
        .relay(&relayed("sibling-a", 0, "member-heartbeat", None, 1))
        .expect("relay");
    for seq in 1..40 {
        let kind = if seq % 2 == 0 {
            "member-heartbeat"
        } else {
            "turn-activity"
        };
        run.writer
            .relay(&relayed("sibling-a", seq, kind, Some("running"), seq * 900))
            .expect("relay");
        run.writer
            .relay(&relayed("sibling-b", seq, kind, Some("landed"), seq * 1000))
            .expect("relay");
    }
    settled_with(
        &mut run,
        "landed",
        &[
            ("status", json!("done")),
            ("outcome", json!("complete")),
            ("landing", json!("landed")),
        ],
    );
    settled_with(
        &mut run,
        "unlanded",
        &[
            ("status", json!("done")),
            ("outcome", json!("complete-but-unlanded")),
            ("landing", json!("unlanded")),
        ],
    );
    settled_with(
        &mut run,
        "failed",
        &[
            ("status", json!("failed")),
            ("outcome", json!("judge-rejected")),
            ("detail", json!("the judge said no")),
        ],
    );
    settled_with(
        &mut run,
        "awaiting",
        &[
            ("status", json!("complete-but-held")),
            (
                "awaiting",
                json!([{"producer": "github.com/owner/engine", "target": "crate"}]),
            ),
        ],
    );
    run.writer
        .emit(
            PipelineKind::NodeDispatched,
            journal::labels("demo", Some("running")),
            journal::payload(&[]),
        )
        .expect("started");
    run.writer
        .relay(&relayed(
            "sibling-c",
            3,
            "member-heartbeat",
            Some("running"),
            0,
        ))
        .expect("relay");
    ledger::write_json(&run.paths.result(), &json!({"outcome": "unfinished"})).expect("a result");

    let fresh = run.open();
    let nodes = reading(&fresh);
    assert_eq!(
        document(&nodes)["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .filter(|node| node.get("awaiting").is_some())
            .count(),
        1,
        "the fixture holds an awaiting node: {nodes:?}"
    );
    let statuses = fresh.state.statuses();
    assert_eq!(statuses["failed"], NodeStatus::Failed);
    assert_eq!(statuses["after-failed"], NodeStatus::Skipped);
    assert_eq!(statuses["gated"], NodeStatus::Blocked);

    let selected = crate::journal::read_selected(
        &run.paths.journal(),
        &crate::journal::Selection::FirstScreen { node: None },
    );
    assert!(
        selected.events.len() < fresh.events.len(),
        "a first screen keeps fewer records than the store holds"
    );
    let mut view = RunView::over_selected(&run.paths, selected.events).expect("opens");
    same_answers(&view, &fresh);

    // Nothing appended: nothing to read.
    assert_eq!(view.refresh().expect("refresh"), Refreshed::Unchanged);

    // Appended past the read: folded, and only that, and equal to a fresh open.
    let before = view.events.len();
    std::thread::sleep(std::time::Duration::from_millis(5));
    settled_with(
        &mut run,
        "gated",
        &[
            ("status", json!("done")),
            ("outcome", json!("complete")),
            ("landing", json!("landed")),
        ],
    );
    run.writer
        .relay(&relayed(
            "sibling-d",
            0,
            "member-heartbeat",
            Some("running"),
            0,
        ))
        .expect("relay");
    crate::readprobe::start();
    assert_eq!(
        view.refresh().expect("refresh"),
        Refreshed::Appended { first: before }
    );
    let observed = crate::readprobe::finish();
    assert_eq!(observed.folded.len(), 2, "folded only the appended records");
    same_answers(&view, &run.open());
}

/// Records that arrive between the read that chose a view's records and the view are
/// neither lost nor counted twice, and nor is a record a writer has yet to finish.
#[test]
fn what_arrives_between_the_selective_read_and_the_view_is_folded_by_the_first_refresh() {
    let mut plan = super::tests::plan();
    plan.tasks = vec![node("first", &[]), node("second", &[])];
    let mut run = Run::new("selected-race", plan);
    settled_with(
        &mut run,
        "first",
        &[
            ("status", json!("done")),
            ("outcome", json!("complete")),
            ("landing", json!("landed")),
        ],
    );
    let selected = crate::journal::read_selected(
        &run.paths.journal(),
        &crate::journal::Selection::FirstScreen { node: None },
    );
    settled_with(
        &mut run,
        "second",
        &[
            ("status", json!("failed")),
            ("outcome", json!("judge-rejected")),
        ],
    );
    let mut view = RunView::over_selected(&run.paths, selected.events).expect("opens");
    assert_eq!(view.state.statuses()["second"], NodeStatus::Ready);
    assert!(matches!(
        view.refresh().expect("refresh"),
        Refreshed::Appended { .. }
    ));
    assert_eq!(view.state.statuses()["second"], NodeStatus::Failed);
    same_answers(&view, &run.open());
    assert_eq!(view.refresh().expect("refresh"), Refreshed::Unchanged);
}

/// The premise `resolve_open` rests on: a consumer that has settled no longer reads its
/// edge, so its status stands whatever its upstream later does, while one that has not
/// still reads it and follows it, and both read as a full fold does.
#[test]
fn a_settled_consumer_stands_whatever_its_upstream_does_and_an_unsettled_one_follows_it() {
    let mut plan = super::tests::plan();
    plan.tasks = vec![
        node("settled-consumer", &["run:settled-upstream#build"]),
        node("failed-consumer", &["run:settled-upstream#build"]),
        node("waiting-consumer", &["run:open-upstream#build"]),
        // A recorded gate is re-derived against the graph, so it has not settled.
        node("gated-consumer", &["run:gating-upstream#build"]),
        // One edge, one consumer settled and one not: the edge is still waited on.
        node("settled-beside", &["run:shared-upstream#build"]),
        node("waiting-beside", &["run:shared-upstream#build"]),
    ];
    let mut run = Run::new("selected-edges", plan);
    let upstream = |name: &str| {
        let paths = RunPaths::under(&run.root, name);
        paths.create().expect("an upstream");
        ledger::write_json(&paths.launch(), &super::tests::launch(0)).expect("launch");
        paths
    };
    let settled_upstream = upstream("settled-upstream");
    let open_upstream = upstream("open-upstream");
    let gating_upstream = upstream("gating-upstream");
    let mut settled_writer = Journal::open(&settled_upstream);
    let mut open_writer = Journal::open(&open_upstream);
    let mut gating_writer = Journal::open(&gating_upstream);
    let done = |writer: &mut Journal, run: &str, status: &str| {
        writer
            .emit(
                PipelineKind::NodeSettled,
                journal::labels(run, Some("build")),
                journal::payload(&[("status", json!(status))]),
            )
            .expect("an upstream settles");
    };
    done(&mut settled_writer, "settled-upstream", "done");
    settled_with(
        &mut run,
        "settled-consumer",
        &[
            ("status", json!("done")),
            ("outcome", json!("complete")),
            ("landing", json!("landed")),
        ],
    );
    settled_with(
        &mut run,
        "failed-consumer",
        &[
            ("status", json!("failed")),
            ("outcome", json!("judge-rejected")),
        ],
    );
    settled_with(&mut run, "gated-consumer", &[("status", json!("blocked"))]);
    settled_with(
        &mut run,
        "settled-beside",
        &[
            ("status", json!("done")),
            ("outcome", json!("complete")),
            ("landing", json!("landed")),
        ],
    );

    let current = RunView::open_current(&run.paths).expect("opens");
    assert!(
        !current
            .state
            .cross_dag
            .contains_key("run:settled-upstream#build"),
        "an edge only settled consumers name is not read: {:?}",
        current.state.cross_dag
    );
    assert!(current
        .state
        .cross_dag
        .contains_key("run:open-upstream#build"));
    assert!(
        current
            .state
            .cross_dag
            .contains_key("run:shared-upstream#build"),
        "an edge one unsettled consumer names is read"
    );
    assert!(
        current
            .state
            .cross_dag
            .contains_key("run:gating-upstream#build"),
        "a recorded gate is no settlement"
    );
    let mut expected = current.state.statuses();
    assert_eq!(expected["settled-consumer"], NodeStatus::Done);
    assert_eq!(expected["failed-consumer"], NodeStatus::Failed);
    assert_eq!(expected["waiting-consumer"], NodeStatus::Blocked);
    assert_eq!(expected["gated-consumer"], NodeStatus::Blocked);
    assert_eq!(
        expected,
        run.open().state.statuses(),
        "as a full fold reports"
    );
    same_answers(&over_first_screen(&run.paths), &run.open());

    // The upstream moves, then goes wrong, then goes altogether: nothing a settled
    // consumer reports moves.
    done(&mut settled_writer, "settled-upstream", "failed");
    crate::crossdag::resolve_for_refresh(&run.root, &run.open().state.graph);
    assert_eq!(
        RunView::open_current(&run.paths)
            .expect("opens")
            .state
            .statuses(),
        expected
    );
    assert_eq!(run.open().state.statuses(), expected);
    drop(settled_writer);
    std::fs::remove_dir_all(&settled_upstream.dir).expect("the upstream goes");
    crate::crossdag::resolve_for_refresh(&run.root, &run.open().state.graph);
    assert_eq!(
        RunView::open_current(&run.paths)
            .expect("opens")
            .state
            .statuses(),
        expected
    );
    assert_eq!(over_first_screen(&run.paths).state.statuses(), expected);

    // An unsettled consumer's upstream settles: it is read and followed.
    let mut held = over_first_screen(&run.paths);
    assert_eq!(held.refresh().expect("refresh"), Refreshed::Unchanged);
    done(&mut open_writer, "open-upstream", "done");
    done(&mut gating_writer, "gating-upstream", "done");
    crate::crossdag::resolve_for_refresh(&run.root, &run.open().state.graph);
    expected.insert("waiting-consumer".into(), NodeStatus::Ready);
    expected.insert("gated-consumer".into(), NodeStatus::Ready);
    let current = RunView::open_current(&run.paths).expect("opens");
    assert_eq!(current.state.statuses(), expected);
    assert_eq!(run.open().state.statuses(), expected);
    assert_eq!(over_first_screen(&run.paths).state.statuses(), expected);
    assert_eq!(held.refresh().expect("refresh"), Refreshed::Reread);
    assert_eq!(held.state.statuses(), expected);
}

/// A retained selective view meets an edit that opens an edge it never resolved: a new
/// consumer of another run, and a settled consumer asked to run again. Neither edge has
/// an answer in the view, so it reads the run again, and reports as a fresh open does.
#[test]
fn a_selective_view_reads_again_when_an_edit_opens_an_edge_it_never_resolved() {
    let mut plan = super::tests::plan();
    plan.tasks = vec![node("settled-consumer", &["run:first-upstream#build"])];
    let mut run = Run::new("selected-opened", plan);
    settled_with(
        &mut run,
        "settled-consumer",
        &[
            ("status", json!("done")),
            ("outcome", json!("complete")),
            ("landing", json!("landed")),
        ],
    );
    let mut view = over_first_screen(&run.paths);
    assert!(view.state.cross_dag.is_empty(), "no consumer waits yet");
    assert_eq!(view.refresh().expect("refresh"), Refreshed::Unchanged);

    let edit = |operation: Value| {
        let kind = operation["kind"].clone();
        journal::payload(&[
            ("author", json!("planner")),
            ("operations", json!([operation])),
            ("operation_kinds", json!([kind])),
        ])
    };
    let added = node("new-consumer", &["run:second-upstream#build"]);
    run.writer
        .emit(
            PipelineKind::EditCommitted,
            journal::labels("demo", None),
            edit(json!({"kind": "node-added", "node": added})),
        )
        .expect("an edit");
    assert_eq!(view.refresh().expect("refresh"), Refreshed::Reread);
    assert_eq!(view.state.statuses()["new-consumer"], NodeStatus::Blocked);
    same_answers(&view, &run.open());

    let mut view = over_first_screen(&run.paths);
    assert!(!view
        .state
        .cross_dag
        .contains_key("run:first-upstream#build"));
    run.writer
        .emit(
            PipelineKind::EditCommitted,
            journal::labels("demo", None),
            edit(json!({"kind": "node-requeued", "node": "settled-consumer"})),
        )
        .expect("an edit");
    assert_eq!(view.refresh().expect("refresh"), Refreshed::Reread);
    assert!(view
        .state
        .cross_dag
        .contains_key("run:first-upstream#build"));
    assert_eq!(
        view.state.statuses()["settled-consumer"],
        NodeStatus::Blocked
    );
    same_answers(&view, &run.open());
}
