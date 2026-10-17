//! The checkpoint `onepipeline host` extends over an ended run's final records,
//! read by the last release before this build began extending it.
//!
//! A reader that has proved a run ended covers its trailing records in
//! `checkpoint.json`. Review intake advanced the fold schema to 12, and review
//! replies to 13, so the preceding release must discard that checkpoint and recover
//! the same answers from the journal, writing its own schema-11 checkpoint. This drives that
//! release's `status`, `host` and `results` over the extended checkpoint.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value;

use crate::harness::{agent, plan_of, World};

/// The last release of `onepipeline` before `host` extended an ended run's
/// checkpoint.
const BEFORE: &str = "0.65.0";

/// That release's `onepipeline`, resolved once through `uv tool run` and refused
/// unless it reports [`BEFORE`].
fn before() -> &'static Path {
    static PROGRAM: OnceLock<PathBuf> = OnceLock::new();
    PROGRAM.get_or_init(|| {
        let from = format!("onepipeline-cli=={BEFORE}");
        let resolved = std::process::Command::new("uv")
            .args([
                "tool",
                "run",
                "--from",
                &from,
                "sh",
                "-c",
                "command -v onepipeline",
            ])
            .output()
            .unwrap_or_else(|failure| {
                panic!(
                    "this journey runs the {from} wheel through `uv tool run`, and uv could not \
                     be started ({failure}); install uv (https://docs.astral.sh/uv/) — CI sets \
                     it up for the `release-compat` job that runs this binary"
                )
            });
        assert!(
            resolved.status.success(),
            "uv could not provide {from}: {}",
            String::from_utf8_lossy(&resolved.stderr)
        );
        let program = PathBuf::from(String::from_utf8_lossy(&resolved.stdout).trim());
        let reported = std::process::Command::new(&program)
            .arg("--version")
            .output()
            .expect("the release's binary runs");
        assert_eq!(
            String::from_utf8_lossy(&reported.stdout).trim(),
            format!("onepipeline {BEFORE}"),
            "{} is not the {BEFORE} release; clear uv's cached copy with `uv cache clean \
             onepipeline-cli`",
            program.display()
        );
        program
    })
}

fn document(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("the document")).expect("JSON")
}

/// Compare the recorded run's answers without live filesystem measurements,
/// the elapsed duration the two releases compute at different instants, or the
/// scratch notice (a header and its indented directory lines) that only a
/// release journalling scratch records can print.
fn stable_answer(answer: &str) -> String {
    let mut in_scratch_notice = false;
    answer
        .lines()
        .filter(|line| {
            in_scratch_notice = line.starts_with("  scratch kept: ")
                || (in_scratch_notice && line.starts_with("    "));
            !in_scratch_notice && !line.starts_with("  free space: ")
        })
        .map(|line| {
            if line.contains(": worker ended when the run was stopped, ") {
                line.rsplit_once(", ").expect("the stop's duration").0
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A run is stopped with its dispatch's registry entry left stale, so this
/// build's `host` proves it ended and extends its checkpoint over the stop; then
/// the preceding release reads the run with `status`, `host` and `results`.
/// Each exits 0, status and results agree with this build, and the release
/// recovers its own checkpoint without changing the journal.
#[test]
fn a_checkpoint_the_host_view_extended_is_refolded_by_the_preceding_release() {
    let world = World::new("extended-checkpoint");
    world.script("build.wait", "hold");
    let run = "extended";
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch to record where it is running", |world| {
        !world.dispatch_records(run).is_empty()
    });
    world.run(&["stop", run]).exited(0);

    let checkpoint = world.run_file(run, "checkpoint.json");
    let journal_len = || {
        std::fs::metadata(world.run_file(run, "events.jsonl"))
            .expect("a journal")
            .len()
    };
    let covered = |document: &Value| document["coverage"]["bytes"].as_u64();
    assert!(
        covered(&document(&checkpoint)) < Some(journal_len()),
        "the driver's own checkpoint already covered the stop, so this journey proves nothing"
    );
    world
        .run(&["host"])
        .exited(0)
        .out_has("1 stale registry entry ignored");
    let extended = document(&checkpoint);
    assert_eq!(
        covered(&extended),
        Some(journal_len()),
        "this build's view did not extend the ended run's checkpoint"
    );
    assert_eq!(extended["schema_version"], 13);

    let status = world.run(&["status", run]).exited(0).stdout.clone();
    let results = world.run(&["results", run]).exited(0).stdout.clone();
    let journal = world.run_file(run, "events.jsonl");
    let recorded = std::fs::read(&journal).expect("the journal");
    for args in [&["status", run][..], &["host"], &["results", run]] {
        let read = world
            .cmd_on(before(), args)
            .output()
            .expect("the release runs");
        assert_eq!(
            read.status.code(),
            Some(0),
            "{BEFORE}'s `{}` exited {:?}: {}",
            args.join(" "),
            read.status.code(),
            String::from_utf8_lossy(&read.stderr)
        );
        if let Some(expected) = match args[0] {
            "status" => Some(&status),
            "results" => Some(&results),
            _ => None,
        } {
            assert_eq!(
                stable_answer(&String::from_utf8_lossy(&read.stdout)),
                stable_answer(expected)
            );
        }
        assert_eq!(document(&checkpoint)["schema_version"], 11);
        assert_eq!(
            std::fs::read(&journal).expect("the journal"),
            recorded,
            "{BEFORE}'s `{}` changed the journal while refolding",
            args.join(" ")
        );
    }
}
