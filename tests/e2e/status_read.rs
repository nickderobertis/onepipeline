//! What `onepipeline status` says of a run, driven through the binary.
//!
//! The provider sweep it no longer pays for by default, against a stand-in `claude`; and
//! every other line, against the text release `v0.101.5`'s `--no-providers` printed for the
//! same run, held in `tests/golden/status-read-v0.101.5.txt`. To regenerate that from the
//! release, point `STATUS_READ_GOLDEN_RELEASE` at its binary and run this module.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::World;

/// A run named with no host's own detail in it.
const RUN: &str = "golden";

/// The host every command here runs as, so the fixture's driver is one this host can prove
/// gone rather than one elsewhere it can say nothing of.
const HOST: &str = "fixture-host";

fn record(ts: &str, seq: u64, kind: &str, node: Option<&str>, payload: Value) -> String {
    let mut labels = json!({"run_id": RUN});
    if let Some(node) = node {
        labels["node"] = json!(node);
    }
    let line = json!({
        "v": 2, "ts": ts, "stream": "fixture-host-1", "seq": seq, "source": "pipeline",
        "kind": kind, "labels": labels, "payload": payload, "artifacts": [],
    });
    format!("{line}\n")
}

fn task(id: &str, deps: &[&str]) -> Value {
    json!({"id": id, "task": "## What\nDo it.", "persona": "engineer", "deps": deps})
}

/// A run nothing drives any more, in the writers' own form, at fixed stamps: nodes landed,
/// unlanded and failed, one skipped behind the failure and one not yet settled.
fn fixture_run(root: &Path) -> PathBuf {
    let dir = root.join(RUN);
    std::fs::create_dir_all(&dir).expect("a run directory");
    let plan = json!({
        "schema_version": onepipeline::plan::PLAN_SCHEMA_VERSION,
        "goal": {"text": "Deliver the fixture"},
        "name": RUN,
        "concurrency": 4,
        "tasks": [
            task("landed", &[]),
            task("unlanded", &[]),
            task("failed", &[]),
            task("behind-the-failure", &["failed"]),
            task("not-settled", &["landed"]),
        ],
    });
    let mut journal = record(
        "2026-01-01T00:00:00.000Z",
        0,
        "run-started",
        None,
        json!({"plan": plan, "graph": "graphs/dag-scope.yaml", "dir": "/nonexistent"}),
    );
    journal += &record(
        "2026-01-01T00:01:00.000Z",
        1,
        "node-settled",
        Some("landed"),
        json!({"status": "done", "outcome": "complete", "landing": "landed"}),
    );
    journal += &record(
        "2026-01-01T00:02:00.000Z",
        2,
        "node-settled",
        Some("unlanded"),
        json!({"status": "done", "outcome": "complete-but-unlanded", "landing": "unlanded"}),
    );
    journal += &record(
        "2026-01-01T00:03:00.000Z",
        3,
        "node-settled",
        Some("failed"),
        json!({"status": "failed", "outcome": "dispatch-died"}),
    );
    std::fs::write(dir.join("events.jsonl"), journal).expect("a journal");
    let launch = json!({
        "run_id": RUN,
        "project": "plans:golden",
        "dir": "/nonexistent",
        "graph": "graphs/dag-scope.yaml",
        "graph_run": "dag-scope-1-1",
        "observer_runs": [],
        "node_graph": "graphs/node-scope.yaml",
        "launcher": "e2e",
        "session": "status-read",
        "pid": 2_147_483_647_u32,
        "host": HOST,
        "started": "linux-proc-stat:1",
        "started_at": "2026-01-01T00:00:00.000Z",
        "heartbeat_interval": 1_800,
        "writeback_item_budget": 12,
        "adoptions": 0,
    });
    std::fs::write(
        dir.join("launch.json"),
        serde_json::to_string_pretty(&launch).expect("a launch record"),
    )
    .expect("a launch record");
    dir
}

/// The readings a view says of the moment and the host rather than of the run: the free
/// space and the runs root, and how long the unread surfaces have been waiting.
fn stable(text: &str, world: &World) -> String {
    let root = world.runs.display().to_string();
    text.lines()
        .map(|line| {
            if line.starts_with("  free space: ") {
                return "  free space: <read off this host>\n".to_owned();
            }
            let mut line = line.replace(&root, "<runs>");
            if let Some(at) = line.find(", unread for ") {
                line.replace_range(at.., ", unread for <age>");
            }
            format!("{line}\n")
        })
        .collect()
}

fn golden_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/status-read-v0.101.5.txt")
}

fn appended(dir: &Path, line: &str) {
    use std::io::Write;
    let mut journal = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join("events.jsonl"))
        .expect("the journal");
    journal.write_all(line.as_bytes()).expect("appended");
}

/// `status` says of a run with landed, unlanded and failed nodes and unread surfaces what
/// `v0.101.5`'s `--no-providers` said, line for line, and reads every later change off the
/// disk: nothing a reader relies on moved or went stale.
#[test]
fn status_says_of_each_node_what_it_said_and_reads_each_change_off_the_disk() {
    let world = World::new("status-read-golden").with_env("HOSTNAME", HOST);
    let dir = fixture_run(&world.runs);
    world
        .run(&[
            "surface",
            RUN,
            "--kind",
            "finding",
            "--message",
            "read this first",
        ])
        .exited(0);
    world
        .run(&["surface", RUN, "--kind", "finding", "--message", "and this"])
        .exited(0);

    // Regenerating: the release's own text, over this same fixture before anything of
    // this build has read it.
    if let Some(release) = std::env::var_os("STATUS_READ_GOLDEN_RELEASE") {
        let output = world
            .cmd_on(Path::new(&release), &["status", RUN, "--no-providers"])
            .output()
            .expect("the release runs");
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).expect("text");
        std::fs::write(golden_path(), stable(&text, &world)).expect("the golden");
    }

    let first = world.run(&["status", RUN]);
    first.exited(0);
    let text = stable(&first.stdout, &world);
    let golden = std::fs::read_to_string(golden_path()).expect("the golden");
    assert_eq!(text, golden, "the default text moved from v0.101.5's");
    for said in [
        "2/5 done, 1 not landed, 1 never attempted",
        "failed: the dispatch died rather than failing its task",
        "not-settled: ready",
        "1 node(s) have not landed: unlanded",
        "2 planner update(s) waiting",
        "  free space: ",
    ] {
        first.out_has(said);
    }
    assert!(
        dir.join("checkpoint.json").is_file(),
        "the read leaves a checkpoint the next records land past"
    );
    let flagged = world.run(&["status", RUN, "--no-providers"]);
    flagged.exited(0);
    assert_eq!(stable(&flagged.stdout, &world), text);

    // A surface read, then the other: the line follows, then goes.
    world.run(&["next", RUN, "--all"]).exited(0);
    world
        .run(&["status", RUN])
        .exited(0)
        .out_has("1 planner update(s) waiting");
    world.run(&["next", RUN, "--all"]).exited(0);
    world
        .run(&["status", RUN])
        .exited(0)
        .out_lacks("planner update(s) waiting");

    // A landing recorded, past the checkpoint.
    appended(
        &dir,
        &record(
            "2026-01-01T00:04:00.000Z",
            4,
            "node-landed",
            Some("unlanded"),
            json!({
                "node": "unlanded",
                "change_url": "https://example.invalid/pull/1",
                "commit": "0123456789abcdef",
                "detected_by": "readjust",
                "trigger": "readjust",
            }),
        ),
    );
    let landed = world.run(&["status", RUN]);
    landed.exited(0).out_lacks("not landed");
    landed.out_has("2/5 done");

    // A node settled, past the checkpoint.
    appended(
        &dir,
        &record(
            "2026-01-01T00:05:00.000Z",
            5,
            "node-settled",
            Some("not-settled"),
            json!({"status": "done", "outcome": "complete", "landing": "landed"}),
        ),
    );
    let settled = world.run(&["status", RUN]);
    settled.exited(0).out_has("3/5 done");

    // A failure settled afresh, past the checkpoint, lifts the failure it replaced.
    appended(
        &dir,
        &record(
            "2026-01-01T00:06:00.000Z",
            6,
            "node-settled",
            Some("failed"),
            json!({"status": "done", "outcome": "complete", "landing": "landed"}),
        ),
    );
    let lifted = world.run(&["status", RUN]);
    lifted
        .exited(0)
        .out_lacks("the dispatch died")
        .out_has("4/5 done");
    assert_ne!(
        stable(&lifted.stdout, &world),
        text,
        "every change above reached the text"
    );
}

/// The three texts of `status` over a stand-in `claude`: the default and `--no-providers`
/// leave it never started and print no report, `--providers` starts it and prints the
/// report where it has always been printed, and no other line moves between them.
#[cfg(unix)]
#[test]
fn the_default_read_never_starts_the_provider_cli_and_providers_does() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new("status-read-providers").with_env("HOSTNAME", HOST);
    fixture_run(&world.runs);
    let work = world.runs.with_file_name("status-read-providers");
    let bin = work.join("bin");
    std::fs::create_dir_all(&bin).expect("a directory");
    std::fs::create_dir_all(work.join("home")).expect("a home");
    let starts = work.join("starts");
    let claude = bin.join("claude");
    std::fs::write(
        &claude,
        format!("#!/bin/sh\necho started >> '{}'\n", starts.display()),
    )
    .expect("a stand-in");
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).expect("executable");
    let started = || std::fs::read_to_string(&starts).map_or(0, |text| text.lines().count());
    let status = |flags: &[&str]| {
        let mut args = vec!["status", RUN];
        args.extend_from_slice(flags);
        let output = world
            .cmd(&args)
            .env_remove("ONEPIPELINE_ONEAGENTGRAPH_BIN")
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HOME", work.join("home"))
            .output()
            .expect("the binary runs");
        (
            output.status.code(),
            String::from_utf8(output.stdout).expect("text"),
            String::from_utf8(output.stderr).expect("text"),
        )
    };

    let (code, default, stderr) = status(&[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!default.contains("providers:"), "{default}");
    assert_eq!(started(), 0, "the default read started the stand-in");
    let (code, omitted, stderr) = status(&["--no-providers"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!omitted.contains("providers:"), "{omitted}");
    assert_eq!(started(), 0, "--no-providers started the stand-in");

    let (code, reported, stderr) = status(&["--providers"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(reported.contains("  providers: "), "{reported}");
    assert!(started() > 0, "--providers never started the stand-in");

    // The report is the view's last block here, and the only thing between the texts.
    let above_the_report: String = reported
        .lines()
        .take_while(|line| !line.starts_with("  providers: "))
        .map(|line| format!("{line}\n"))
        .collect();
    let same = stable(&default, &world);
    assert_eq!(stable(&omitted, &world), same);
    assert_eq!(stable(&above_the_report, &world), same);

    let (code, _, refusal) = status(&["--providers", "--no-providers"]);
    assert_eq!(code, Some(2), "{refusal}");
    assert!(
        refusal.contains("--providers") && refusal.contains("--no-providers"),
        "the refusal names both flags: {refusal}"
    );
}
