//! The status, results and transcript read budgets: three commands, timed as subprocesses of
//! the release-built binary over [`generate`]'s corpus, with what each spent recorded beside it.
//!
//! Its own binary and its own Nx project (`tests/status_read/project.json`,
//! `onepipeline-status-read`, a budget tier), run only by that project's `test` target. The
//! recording is `target/budget-records/status-read.json`; `report.mjs` reads it and nothing
//! re-measures. This asserts what each command said and holds no time bound: the budgets do.

#![cfg(target_os = "linux")]

mod generate;
#[path = "../journal_workload/generate.rs"]
#[allow(dead_code)]
mod workload;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// How long the stand-in provider CLI takes: what one sweep of an authenticated
/// `claude` cost on this host, so a read that starts it cannot pass.
const STAND_IN_SECONDS: u64 = 20;

/// What guards the journey against a hang, and is no bound on any budget.
const HANG: Duration = Duration::from_secs(600);

/// The release-built binary: `STATUS_READ_BINARY`, which the project's `test` target builds.
fn binary() -> PathBuf {
    let binary = std::env::var_os("STATUS_READ_BINARY")
        .map(PathBuf::from)
        .expect("STATUS_READ_BINARY names the release-built binary; run `just status-read-budget`");
    assert!(binary.is_file(), "{} is not a file", binary.display());
    binary
}

struct World {
    dir: PathBuf,
    runs: PathBuf,
    starts: PathBuf,
    phases: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = std::env::var_os("STATUS_READ_WORK")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join(format!("onepipeline-status-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let stand_ins = dir.join("bin");
        std::fs::create_dir_all(&stand_ins).expect("a directory");
        std::fs::create_dir_all(dir.join("home")).expect("a home");
        let starts = dir.join("claude-starts");
        let script = stand_ins.join("claude");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho started >> '{}'\nsleep {STAND_IN_SECONDS}\n",
                starts.display()
            ),
        )
        .expect("a stand-in");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        Self {
            runs: dir.join("runs"),
            phases: dir.join("phases"),
            starts,
            dir,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(binary());
        command
            .args(args)
            .current_dir(&self.dir)
            .env_clear()
            .env("ONEPIPELINE_RUNS_DIR", &self.runs)
            .env("ONEPIPELINE_HOST_PHASES", &self.phases)
            .env("HOME", self.dir.join("home"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.dir.join("bin").display()),
            );
        command
    }

    fn starts(&self) -> usize {
        std::fs::read_to_string(&self.starts).map_or(0, |text| text.lines().count())
    }

    /// One read, timed, with the phases it recorded.
    fn read(&self, args: &[&str]) -> Read {
        let _ = std::fs::remove_file(&self.phases);
        let mut child = self
            .command(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the binary starts");
        let began = Instant::now();
        // Drained on threads as `wait_with_output` would, with a guard against a hang
        // that reads only the wall clock.
        let id = child.id();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard = {
            let stop = std::sync::Arc::clone(&stop);
            std::thread::spawn(move || {
                let began = Instant::now();
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if began.elapsed() > HANG {
                        let _ = Command::new("kill").arg(id.to_string()).status();
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
        };
        let _ = &mut child;
        let output = child.wait_with_output().expect("the binary ran");
        let seconds = began.elapsed().as_secs_f64();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        guard.join().expect("the guard");
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Read {
            seconds,
            stdout: String::from_utf8(output.stdout).expect("text"),
            phases: std::fs::read_to_string(&self.phases)
                .ok()
                .and_then(|text| {
                    text.lines()
                        .next_back()
                        .and_then(|line| serde_json::from_str(line).ok())
                })
                .unwrap_or(Value::Null),
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Read {
    seconds: f64,
    stdout: String,
    phases: Value,
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |about| about.len())
}

fn load() -> String {
    std::fs::read_to_string("/proc/loadavg").map_or_else(
        |_| "unknown".to_owned(),
        |text| {
            text.split_whitespace()
                .next()
                .unwrap_or("unknown")
                .to_owned()
        },
    )
}

/// What a whole journal holds, read from its bytes: records and streams.
fn journal_shape(path: &Path) -> (u64, usize) {
    let text = std::fs::read_to_string(path).expect("a journal");
    let mut streams = std::collections::HashSet::new();
    let mut records = 0;
    for line in text.lines() {
        let record: Value = serde_json::from_str(line).expect("a record");
        streams.insert(record["stream"].as_str().expect("a stream").to_owned());
        records += 1;
    }
    (records, streams.len())
}

#[test]
fn status_results_and_transcript_read_a_long_run_and_record_what_each_spent() {
    let world = World::new();
    let store = world.dir.join("store");
    let generated = generate::corpus(&world.runs, &store);

    let large_dir = world.runs.join(generate::LARGE);
    let (records, streams) = journal_shape(&large_dir.join("events.jsonl"));

    // A real reader's checkpoint over the generated run, which the budget's read resumes.
    let primed = world.read(&["status", generate::LARGE, "--json"]);
    let reading: Value = serde_json::from_str(primed.stdout.trim()).expect("a reading");
    let nodes = reading["nodes"].as_array().expect("nodes");
    assert_eq!(nodes.len(), generate::LARGE_NODES, "{}", primed.stdout);
    assert!(
        nodes.iter().all(|node| node["status"] == "done"),
        "{}",
        primed.stdout
    );
    let checkpoint = large_dir.join("checkpoint.json");

    let checkpoint_fields: Vec<(String, usize)> = {
        let document: Value =
            serde_json::from_slice(&std::fs::read(&checkpoint).expect("a checkpoint"))
                .expect("a document");
        let mut fields: Vec<(String, usize)> = document["state"]
            .as_object()
            .expect("the fold")
            .iter()
            .map(|(field, value)| (field.clone(), value.to_string().len()))
            .collect();
        fields.sort_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));
        fields.truncate(6);
        fields
    };

    let large = world.read(&["status", generate::LARGE]);
    assert!(
        large.stdout.contains("19/19 done"),
        "the run's summary: {}",
        large.stdout
    );
    assert!(!large.stdout.contains("providers:"), "{}", large.stdout);
    let small = world.read(&["status", generate::SMALL]);
    assert!(
        small.stdout.contains("1/1 done"),
        "the run's summary: {}",
        small.stdout
    );
    assert!(!small.stdout.contains("providers:"), "{}", small.stdout);
    let default_starts = world.starts();

    let results = world.read(&["results", generate::LARGE]);
    assert!(
        results.stdout.contains(generate::LARGE),
        "{}",
        results.stdout
    );

    let transcript = world.read(&["transcript", generate::DISPATCH, generate::DISPATCH_NODE]);
    assert!(
        transcript.stdout.lines().count() >= generated.references,
        "every reference is a line of the transcript: {} lines",
        transcript.stdout.lines().count()
    );
    let unresolved = transcript
        .stdout
        .lines()
        .filter(|line| {
            line.ends_with(": missing-entry")
                || line.ends_with(": missing-file")
                || line.ends_with(": unreadable")
        })
        .count();

    let slowest = large.seconds.max(small.seconds);
    let record = json!({
        "host_load": load(),
        "status": {
            "seconds": slowest,
            "large": {"seconds": large.seconds, "phases": large.phases},
            "small": {"seconds": small.seconds, "phases": small.phases},
            "checkpoint_bytes": file_size(&checkpoint),
            "checkpoint_largest_fields": checkpoint_fields,
            "journal_bytes": file_size(&large_dir.join("events.jsonl")),
            "upstream_journal_bytes": file_size(&world.runs.join(generate::UPSTREAM).join("events.jsonl")),
            "records": records,
            "streams": streams,
            "run_directories": std::fs::read_dir(&world.runs).expect("the root").count(),
            "stand_in_starts": default_starts,
        },
        "results": {
            "seconds": results.seconds,
            "phases": results.phases,
            "records": records,
            "streams": streams,
        },
        "transcript": {
            "seconds": transcript.seconds,
            "references": generated.references,
            "unresolved": unresolved,
            "session_bytes": generated.session_bytes,
            "session_files": generate::SESSION_FILES,
        },
    });
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/budget-records/status-read.json");
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("the records directory");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&record).expect("a record"),
    )
    .expect("written");
    println!("{record}");
}
