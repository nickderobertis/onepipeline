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

#[test]
fn refresh_rereads_truncation_and_same_length_prefix_edits_including_before_growth() {
    for change in ["truncate", "edit", "replace", "edit-then-append"] {
        let mut run = Run::new(change, super::tests::plan());
        run.settle("done");
        let mut view = run.open();
        let bytes = std::fs::read_to_string(run.paths.journal()).expect("journal");
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
    for expire in [true, false] {
        let mut plan = super::tests::plan();
        plan.tasks[0].deps = vec!["run:upstream#build".into()];
        let run = Run::new(
            if expire {
                "stale-expired"
            } else {
                "stale-refreshed"
            },
            plan,
        );
        let upstream = RunPaths::under(&run.root, "upstream");
        upstream.create().expect("upstream directory");
        ledger::write_json(&upstream.launch(), &super::tests::launch(0)).expect("upstream launch");
        let mut writer = Journal::open(&upstream);
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
        let mut view = run.open();
        assert_eq!(
            view.state.cross_dag["run:upstream#build"],
            NodeStatus::Blocked,
            "this journey must exercise open reusing an older cached answer"
        );
        if expire {
            // Exercise the real cache's lifetime, not a simulated clock or answer.
            std::thread::sleep(
                2 * crate::engine::UPSTREAM_EVERY + std::time::Duration::from_millis(20),
            );
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
