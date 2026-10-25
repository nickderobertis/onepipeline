//! Release-only large scratch journeys. No per-change test target selects this binary.

#[path = "../e2e/harness.rs"]
mod harness;

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Instant;

use harness::{agent, plan_of, World};
use serde_json::{json, Value};

const ENTRIES: usize = 840_000;

/// Three dispatch directories at the largest measured host shape. Empty sparse
/// files put pressure on traversal and unlinking without spending disk capacity.
fn generate(world: &World, run: &str) -> usize {
    let root = world.run_file(run, "scratch");
    let mut dirs: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    dirs.sort();
    assert_eq!(dirs.len(), 3);
    let mut existing = 0;
    for dir in &dirs {
        existing += 1 + fs::read_dir(dir).unwrap().count();
    }
    let to_create = ENTRIES - existing;
    // Each worker writes only one dispatch directory; generation finishes before
    // any ending is released, and the generated shape is unchanged.
    std::thread::scope(|scope| {
        for (dispatch, dir) in dirs.iter().enumerate() {
            let directory_count = dirs.len();
            scope.spawn(move || {
                for index in (dispatch..to_create).step_by(directory_count) {
                    fs::File::create(dir.join(format!("sparse-{index}")))
                        .unwrap()
                        .set_len(1 << 20)
                        .unwrap();
                }
            });
        }
    });

    ENTRIES
}

/// `World::run` eagerly captures a diagnostic dump, including file contents.
/// Large sparse trees use the same real command and environment without that dump.
fn run(world: &World, args: &[&str], input: Option<&str>, expected: i32) -> Output {
    let mut command = world.cmd(args);
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(expected),
        "{args:?}: stdout {}, stderr {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn wait_record(world: &World, name: &str, kind: &str) {
    let began = Instant::now();
    while world.events_of(name, kind).is_empty() {
        assert!(
            began.elapsed().as_secs() < 120,
            "{name}: no {kind} record arrived"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn start(world: &World, name: &str) {
    for node in ["one", "two", "three"] {
        world.script(&format!("{node}.wait"), "hold");
        world.script(&format!("{node}.turn-open"), "");
        world.script(&format!("{node}.fail"), "1");
    }
    let mut plan = plan_of(
        name,
        vec![agent("one", &[]), agent("two", &[]), agent("three", &[])],
    );
    plan["concurrency"] = json!(3);
    let plan = world.plan(name, &plan);
    run(world, &["start", &plan, "--detach"], None, 0);
    world.until("three scratch-writing dispatches", |world| {
        world.events_of(name, "turn-started").len() >= 3
    });
}

fn fail(world: &World, run: &str) {
    for node in ["one", "two", "three"] {
        world.release(&format!("{node}.go"));
    }
    wait_record(world, run, "scratch-retained");
    let status = self::run(world, &["status", run], None, 0);
    assert!(String::from_utf8_lossy(&status.stdout).contains("ENDED failed"));
}

fn measurement(root: &Path, run: &str, trigger: &str, entries: usize) -> Value {
    let recording: Value =
        serde_json::from_slice(&fs::read(root.join(format!("{run}-{trigger}.json"))).unwrap())
            .unwrap();
    assert_eq!(recording["entries"], entries);
    assert_eq!(recording["dirs"], 3);
    recording
}

fn recorder() -> (std::path::PathBuf, std::path::PathBuf) {
    let record_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/budget-records");
    fs::create_dir_all(&record_dir).unwrap();
    let telemetry = record_dir.join(format!(
        "scratch-telemetry-{}",
        std::env::var("NEXTEST_RUN_ID").unwrap()
    ));
    fs::create_dir_all(&telemetry).unwrap();
    (record_dir, telemetry)
}

/// Both independent journeys contribute to one recording, under a real file
/// lease. A new nextest invocation starts fresh; neither can reuse an old figure.
fn record(record_dir: &Path, values: Value) {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(record_dir.join("scratch-workload.json"))
        .unwrap();
    fs4::fs_std::FileExt::lock_exclusive(&file).unwrap();
    let invocation = std::env::var("NEXTEST_RUN_ID").unwrap();
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    let mut document: Value = if text.is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text).unwrap()
    };
    if document["invocation"] != invocation {
        document = json!({"invocation": invocation});
    }
    for (key, value) in values.as_object().unwrap() {
        document[key] = value.clone();
    }
    file.seek(SeekFrom::Start(0)).unwrap();
    file.set_len(0).unwrap();
    file.write_all(&serde_json::to_vec_pretty(&document).unwrap())
        .unwrap();
}

#[test]
fn large_scratch_failed_and_success_endings_record_the_same_generated_tree() {
    let (record_dir, telemetry) = recorder();
    let world = World::new("scratch-workload-endings").with_env(
        "ONEPIPELINE_SCRATCH_TELEMETRY_DIR",
        telemetry.to_str().unwrap(),
    );
    start(&world, "ending");
    let entries = generate(&world, "ending");
    fail(&world, "ending");
    let failed = measurement(&telemetry, "ending", "retained", entries);
    // Human replacements reopen the failed run without generating another scratch
    // tree; attestations and adoption then carry that same tree to success.
    let commands: Vec<_> = ["one", "two", "three"].into_iter().map(|node| json!({"op": "retry", "id": node, "node": harness::human(&format!("{node}-retry"), &[])})).collect();
    run(
        &world,
        &["reply", "ending"],
        Some(&json!({"version":3, "commands":commands}).to_string()),
        0,
    );
    run(&world, &["adopt", "ending"], None, 0);
    for node in ["one-retry", "two-retry", "three-retry"] {
        run(&world, &["attest", "ending", node], None, 0);
    }
    run(&world, &["adopt", "ending"], None, 0);
    assert!(!world.run_file("ending", "scratch").exists());
    let success = measurement(&telemetry, "ending", "success", entries);
    record(&record_dir, json!({"failed":failed, "success":success}));
}

#[test]
fn large_scratch_discard_records_the_generated_tree_and_invocation_time() {
    let (record_dir, telemetry) = recorder();
    let world = World::new("scratch-workload-discard").with_env(
        "ONEPIPELINE_SCRATCH_TELEMETRY_DIR",
        telemetry.to_str().unwrap(),
    );
    start(&world, "discard");
    let entries = generate(&world, "discard");
    fail(&world, "discard");
    let began = Instant::now();
    run(&world, &["scratch", "discard", "discard"], None, 0);
    let wall_seconds = began.elapsed().as_secs_f64();
    let mut discard = measurement(&telemetry, "discard", "discard", entries);
    let state: Value =
        serde_json::from_slice(&fs::read(telemetry.join("discard-discard-state.json")).unwrap())
            .unwrap();
    discard["state_seconds"] = state["seconds"].clone();
    discard["wall_seconds"] = json!(wall_seconds);
    record(&record_dir, json!({"discard":discard}));
}

/// Fixed synthetic timings prove analysis selection, never a measured budget.
#[test]
fn analysis_reports_the_slower_ending_and_the_discard_invocation_duration() {
    let world = World::new("scratch-analysis-selection");
    let cwd = world.root.join("tests/scratch_workload");
    fs::create_dir_all(&cwd).unwrap();
    let records = world.root.join("target/budget-records");
    fs::create_dir_all(&records).unwrap();
    for (failed, success) in [(7, 13), (19, 11)] {
        fs::write(records.join("scratch-workload.json"), serde_json::to_vec(&json!({
            "failed":{"seconds":failed,"entries":840000,"dirs":3,"bytes":4096},
            "success":{"seconds":success,"entries":840000,"dirs":3,"bytes":4096},
            "discard":{"seconds":17,"wall_seconds":23,"state_seconds":2,"entries":840000,"dirs":3,"bytes":4096}
        })).unwrap()).unwrap();
        for selector in ["ending", "discard"] {
            let report = world.root.join(format!("{selector}-report.json"));
            let output = std::process::Command::new("node")
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/scratch_workload/scratch-budget.mjs"),
                )
                .arg(selector)
                .current_dir(&cwd)
                .env("ONEBUDGETSPEC_RESULT", &report)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let result: Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
            assert_eq!(
                result["value"],
                if selector == "ending" {
                    failed.max(success)
                } else {
                    23
                }
            );
            let detail = result["detail"].as_str().unwrap();
            assert!(detail.contains("840000 entries, 3 directories, 4096 bytes"));
            assert!(detail.contains(if selector == "ending" {
                "success ending"
            } else {
                "invocation to exit 23.000 seconds"
            }));
        }
    }
}
