//! The stop-verdict budget journeys: the native Stop hook,
//! `onepipeline stop-guard --format claude-code --unpublished`, timed over both
//! unchanged host-shaped workloads.
//!
//! One journey builds each workload once from an empty root — 20 identities, 371
//! retained sessions, 41 launcher-labelled and 2,308 streams, then ten times that —
//! with `onevcs-testing`'s real-Git fixture, which writes through onevcs's own
//! production types. Beside it sit runs the measured session owns, copied from a
//! recorded settled run, so the guard's `unwatched` half reads real run roots. Every
//! scenario's verdict is established and checked before any timing counts; the
//! timed calls are the current binary with Claude Code's stdin and nothing attached;
//! git is counted in **separate** calls through a counting shim on `PATH`. Only once
//! every assertion has passed is the record written under
//! `target/budget-records/`, which the read-only budget commands validate and
//! report.
//!
//! Unix only: the fixture's live sessions are held by this process, and the
//! counting shim is a shell script.

#![cfg(unix)]

// llmlint: ignore-file[e2e_not_mocked] nothing is substituted: the guard is the compiled
// release binary, the host is real git and onevcs's own persistence, and the run roots are
// copies of a run a real release recorded. The one program on `PATH` that is not the real
// one is the git-counting shim, which forwards every call to the real git and is used only
// in the separate counting calls, never in a timed one.

mod telemetry;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use onevcs_testing::recovery::{build, Class, Fixture, Scale};
use serde_json::{json, Value};
use telemetry::{digest, Build, Record, Sample, Scenario, Workload};

/// The recorded settled run the owned run roots are copied from.
const RECORDED_RUN: &str = "onemessagebus-repair-2";

/// The host the recorded launch record names.
const RECORDING_HOST: &str = "U-17UN402ICR95C";

/// A pid no platform issues, for the copied runs' drivers.
const NO_PROCESS: u32 = 2_147_483_647;

/// Run roots the measured session owns, and run roots others do.
const OWN_RUNS: usize = 8;
const OTHER_RUNS: usize = 32;

/// Timed calls per mode.
const CALLS: usize = 10;

/// The command that regenerates the records.
const JOURNEY_COMMAND: &str = "just stop-verdict-journeys";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load1() -> f64 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|text| text.split_whitespace().next().and_then(|n| n.parse().ok()))
        .unwrap_or(0.0)
}

fn ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis())
        .expect("elapsed fits")
        .max(1)
}

/// The binary measured: the release build the producing recipe names, else this
/// build's own (a record of which no budget accepts).
fn binary() -> (PathBuf, &'static str) {
    match std::env::var_os("ONEPIPELINE_BUDGET_BINARY") {
        Some(path) => (PathBuf::from(path), "release"),
        None => (PathBuf::from(env!("CARGO_BIN_EXE_onepipeline")), "debug"),
    }
}

/// One workload's host: the onevcs fixture, the runs root, the state root, and
/// the two acknowledgement directories its scenarios read.
struct Host {
    fixture: Fixture,
    runs: PathBuf,
    state: PathBuf,
    owed: PathBuf,
    none: PathBuf,
    shim: PathBuf,
    counted: PathBuf,
}

/// The `PATH` every call runs with.
fn path() -> std::ffi::OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

impl Host {
    /// The guard command, Claude Code's rendering, over `acknowledgements`.
    fn guard(&self, acknowledgements: &Path, path: &std::ffi::OsStr) -> Command {
        let mut command = Command::new(binary().0);
        command
            .args([
                "stop-guard",
                "--format",
                "claude-code",
                "--unpublished",
                "--unpublished-acknowledgements",
            ])
            .arg(acknowledgements)
            .env_clear()
            .env("PATH", path)
            .env("HOME", &self.fixture.root)
            .env("ONEVCS_HOME", &self.fixture.home)
            .env("ONEPIPELINE_RUNS_DIR", &self.runs)
            .env("XDG_STATE_HOME", &self.state)
            .env("HOSTNAME", RECORDING_HOST)
            .env(
                "ONEPIPELINE_ONEAGENTGRAPH_BIN",
                "/nonexistent/oneagentgraph",
            )
            .current_dir(&self.fixture.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// One call: the guard fed the measured session's Stop payload, and its stdout.
    fn call(&self, acknowledgements: &Path, path: &std::ffi::OsStr) -> (Duration, String) {
        let payload = json!({
            "session_id": self.fixture.launcher,
            "stop_hook_active": false,
            "hook_event_name": "Stop",
            "transcript_path": "/dev/null",
            "cwd": self.fixture.root,
        })
        .to_string();
        let started = Instant::now();
        let mut child = self
            .guard(acknowledgements, path)
            .spawn()
            .expect("the guard starts");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(payload.as_bytes())
            .expect("the payload is written");
        let output = child.wait_with_output().expect("the guard runs");
        let took = started.elapsed();
        assert_eq!(
            output.status.code(),
            Some(0),
            "the guard failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        (took, String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// The listing the verdict is checked against, outside every timing.
    fn listing(&self, acknowledgements: &Path) -> (i32, Value) {
        let output = Command::new(binary().0)
            .args(["unpublished", "--format", "json", "--session"])
            .arg(&self.fixture.launcher)
            .arg("--acknowledgements")
            .arg(acknowledgements)
            .env_clear()
            .env("PATH", path())
            .env("HOME", &self.fixture.root)
            .env("ONEVCS_HOME", &self.fixture.home)
            .current_dir(&self.fixture.root)
            .output()
            .expect("the listing runs");
        let document = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "no listing ({error}): {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().expect("an exit code"), document)
    }

    fn clear_caches(&self) {
        let cache = self.fixture.home.join("cache");
        if cache.exists() {
            std::fs::remove_dir_all(&cache).expect("the caches clear");
        }
    }

    /// Git executions one separate call made, through the counting shim.
    fn counted_call(&self, acknowledgements: &Path, cold: bool) -> (u64, String) {
        if cold {
            self.clear_caches();
        }
        let _ = std::fs::remove_file(&self.counted);
        let mut shimmed = std::ffi::OsString::from(&self.shim);
        shimmed.push(":");
        shimmed.push(path());
        let (_, stdout) = self.call(acknowledgements, &shimmed);
        let count = std::fs::read_to_string(&self.counted)
            .map(|text| text.lines().count() as u64)
            .unwrap_or(0);
        (count, stdout)
    }
}

/// The copied settled runs: [`OWN_RUNS`] the measured session owns, the rest
/// another's — each a copy of the recorded run with its driver proved gone.
fn assemble_runs(runs: &Path, session: &str) {
    let recorded = root().join("tests/recorded/run-root").join(RECORDED_RUN);
    for nth in 0..OWN_RUNS + OTHER_RUNS {
        let run = format!("settled-{nth:03}");
        let into = runs.join(&run);
        std::fs::create_dir_all(into.join("dispatches")).expect("a run root");
        for entry in std::fs::read_dir(&recorded).expect("the recorded run") {
            let entry = entry.expect("a recorded file");
            if entry.file_name() == "README.md" || entry.path().is_dir() {
                continue;
            }
            std::fs::copy(entry.path(), into.join(entry.file_name())).expect("a copy");
        }
        let launch = into.join("launch.json");
        let mut record: Value =
            serde_json::from_slice(&std::fs::read(&launch).expect("the launch record"))
                .expect("a launch record");
        record["run_id"] = Value::String(run.clone());
        record["pid"] = json!(NO_PROCESS);
        record["session"] = Value::String(if nth < OWN_RUNS {
            session.to_owned()
        } else {
            "another-manager".to_owned()
        });
        std::fs::write(&launch, record.to_string()).expect("the launch record");
    }
}

/// The counting shim: appends a line per call, then runs the real git.
fn install_shim(directory: &Path, counted: &Path) {
    let real = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .expect("sh runs")
            .stdout,
    )
    .expect("a path")
    .trim()
    .to_owned();
    assert!(!real.is_empty(), "no git on PATH");
    std::fs::create_dir_all(directory).expect("a shim directory");
    let shim = directory.join("git");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\necho x >> '{}'\nexec '{real}' \"$@\"\n",
            counted.display()
        ),
    )
    .expect("the shim");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("executable");
}

fn class_name(class: Class) -> &'static str {
    match class {
        Class::Landed => "landed",
        Class::Retirable => "retirable",
        Class::Superseded => "superseded",
        Class::Live => "live",
        Class::No => "no",
        Class::Unknown => "unknown",
        Class::InPart => "in-part",
    }
}

/// Whether a class is one the counting rule counts for an unacknowledged row.
fn counted_class(class: Class) -> bool {
    matches!(
        class,
        Class::No | Class::Unknown | Class::InPart | Class::Superseded
    )
}

/// Check the session listing row by row against what the fixture's real evidence
/// must establish, and answer the class counts.
fn validate(host: &Host) -> BTreeMap<String, usize> {
    let (code, document) = host.listing(&host.owed);
    assert_eq!(code, 7, "the owed scenario answers 7: {document:#}");
    assert_eq!(document["verdict"], "owed");
    assert_eq!(document["unresolved"], json!([]), "{document:#}");
    let rows = document["rows"].as_array().expect("rows");
    let mut classes = BTreeMap::new();
    let mut listed = 0;
    for expected in &host.fixture.expected {
        *classes
            .entry(class_name(expected.class).to_owned())
            .or_insert(0) += 1;
        let row = rows
            .iter()
            .find(|row| row["identity"] == expected.identity && row["branch"] == *expected.branch);
        let Some(row) = row else {
            assert!(
                matches!(expected.class, Class::Landed | Class::Retirable),
                "{} {} ({}) is missing from the listing",
                expected.identity,
                &*expected.branch,
                class_name(expected.class)
            );
            continue;
        };
        listed += 1;
        assert_eq!(row["tip"], expected.tip.as_str());
        assert_eq!(row["manager_session"], host.fixture.launcher.as_str());
        assert_eq!(row["in_flight"], expected.class == Class::Live);
        assert_eq!(row["counted"], counted_class(expected.class), "{row:#}");
        let state = match expected.class {
            Class::Superseded | Class::Live | Class::No => "no",
            Class::Unknown => "unknown",
            Class::InPart => "in-part",
            Class::Landed | Class::Retirable => unreachable!("withheld"),
        };
        assert_eq!(row["landed"]["state"], state, "{row:#}");
    }
    assert_eq!(listed, rows.len(), "every listed row is an expected one");
    assert_eq!(
        classes.len(),
        7,
        "every recovery class is present: {classes:?}"
    );
    let (code, document) = host.listing(&host.none);
    assert_eq!(code, 0, "the none scenario answers 0: {document:#}");
    assert_eq!(document["verdict"], "none");
    assert_eq!(document["unresolved"], json!([]), "{document:#}");
    classes
}

/// Write an acknowledgement for every counted branch, at its tip.
fn acknowledge_all(host: &Host) {
    let entries: Vec<Value> = host
        .fixture
        .expected
        .iter()
        .filter(|expected| counted_class(expected.class))
        .map(|expected| {
            json!({
                "branch": &*expected.branch,
                "identity": expected.identity,
                "tip": expected.tip.as_str(),
                "reason": "kept for the stop-verdict budget",
                "at": "2026-10-07T00:00:00Z",
            })
        })
        .collect();
    std::fs::create_dir_all(&host.none).expect("a directory");
    std::fs::write(
        host.none
            .join(format!("{}.json", digest(host.fixture.launcher.as_bytes()))),
        json!({"version": 1, "acknowledged": entries}).to_string(),
    )
    .expect("the acknowledgements");
}

/// Check one call's stdout is the scenario's verdict.
fn check(scenario: &str, stdout: &str, host: &Host) {
    match scenario {
        "none" => assert_eq!(stdout, "", "none answers silence"),
        "owed" => {
            let decision: Value = serde_json::from_str(stdout.trim()).expect("a decision");
            assert_eq!(decision["decision"], "block");
            let reason = decision["reason"].as_str().expect("a reason");
            for expected in host
                .fixture
                .expected
                .iter()
                .filter(|e| counted_class(e.class))
            {
                assert!(
                    reason.contains(&format!("{} [", &*expected.branch)),
                    "{} is not named: {reason}",
                    &*expected.branch
                );
            }
            assert!(
                !reason.contains("settled-"),
                "an own run is reported: {reason}"
            );
        }
        "unreadable" => {
            let decision: Value = serde_json::from_str(stdout.trim()).expect("a decision");
            assert_eq!(decision["decision"], "block");
            let reason = decision["reason"].as_str().expect("a reason");
            assert!(reason.contains("is unanswered"), "{reason}");
            assert!(reason.contains("the recovery read failed"), "{reason}");
        }
        other => panic!("no scenario {other}"),
    }
}

/// Time `CALLS` calls, every verdict checked before its timing is kept.
fn timed(host: &Host, scenario: &str, acknowledgements: &Path, cold: bool) -> (Sample, String) {
    let load = load1();
    let mut calls = Vec::new();
    let mut verdict = None;
    if !cold {
        let (_, stdout) = host.call(acknowledgements, &path());
        check(scenario, &stdout, host);
        verdict = Some(stdout);
    }
    for _ in 0..CALLS {
        if cold {
            host.clear_caches();
        }
        let (took, stdout) = host.call(acknowledgements, &path());
        check(scenario, &stdout, host);
        if let Some(verdict) = &verdict {
            assert_eq!(&stdout, verdict, "an unchanged state answered differently");
        }
        verdict = Some(stdout);
        calls.push(u64::try_from(took.as_micros()).expect("fits"));
    }
    let mut sorted = calls.clone();
    sorted.sort_unstable();
    (
        Sample {
            median_us: (sorted[CALLS / 2 - 1] + sorted[CALLS / 2]) / 2,
            max_us: sorted[CALLS - 1],
            calls_us: calls,
            load1: load,
        },
        verdict.expect("a verdict"),
    )
}

/// Generated load workers, started for a load run alone and ended with it.
struct Load(Vec<std::process::Child>);

impl Load {
    fn start(workers: u32) -> Self {
        let children = (0..workers)
            .map(|_| {
                Command::new("sh")
                    .args([
                        "-c",
                        "while :; do head -c 4000000 /dev/urandom | gzip -c > /dev/null; done",
                    ])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("a load worker starts")
            })
            .collect();
        Self(children)
    }
}

impl Drop for Load {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// One workload end to end: built from empty, validated, timed, counted, removed.
fn workload(scale: Scale, scratch: &Path, preparation_ms: &mut u64) -> Workload {
    let root = scratch.join(format!("scale-{}", scale.number()));
    let _ = std::fs::remove_dir_all(&root);
    let preparation = Instant::now();
    let fixture = build(&root.join("host"), scale).expect("the host-shaped fixture builds");
    let runs = root.join("runs");
    assemble_runs(&runs, &fixture.launcher);
    let host = Host {
        runs,
        state: root.join("state"),
        owed: root.join("acknowledged-none"),
        none: root.join("acknowledged-every-counted"),
        shim: root.join("shim"),
        counted: root.join("git-calls"),
        fixture,
    };
    std::fs::create_dir_all(&host.owed).expect("an empty acknowledgements directory");
    acknowledge_all(&host);
    install_shim(&host.shim, &host.counted);
    // A listing leaves each copied run's summary current, as on a real host.
    let refreshed = Command::new(binary().0)
        .arg("runs")
        .env("ONEPIPELINE_RUNS_DIR", &host.runs)
        .env("HOSTNAME", RECORDING_HOST)
        .env(
            "ONEPIPELINE_ONEAGENTGRAPH_BIN",
            "/nonexistent/oneagentgraph",
        )
        .env("XDG_STATE_HOME", &host.state)
        .output()
        .expect("runs");
    assert!(refreshed.status.success(), "{refreshed:?}");
    *preparation_ms += ms(preparation);

    let counts = scale.counts();
    for (subdirectory, expected) in [("sessions", counts[1]), ("streams", counts[3])] {
        assert_eq!(
            std::fs::read_dir(host.fixture.home.join(subdirectory))
                .expect("a directory")
                .count(),
            expected,
            "{subdirectory}"
        );
    }
    let class_counts = validate(&host);

    let mut scenarios = Vec::new();
    let registry = host.fixture.home.join("registry.json");
    for scenario in telemetry::SCENARIOS {
        let acknowledgements = if scenario == "none" {
            host.none.clone()
        } else {
            host.owed.clone()
        };
        let readable = std::fs::read(&registry).expect("the registry");
        if scenario == "unreadable" {
            std::fs::write(&registry, "not a registry").expect("unreadable");
            let (code, document) = host.listing(&acknowledgements);
            assert_eq!(code, 1, "{document:#}");
            assert_eq!(document["verdict"], "unanswered");
        }
        let (warm, verdict) = timed(&host, scenario, &acknowledgements, false);
        let cold = (scale == Scale::One).then(|| {
            let (sample, cold_verdict) = timed(&host, scenario, &acknowledgements, true);
            assert_eq!(cold_verdict, verdict, "cold and warm verdicts differ");
            sample
        });
        let cold_git = (scale == Scale::One).then(|| {
            let (count, stdout) = host.counted_call(&acknowledgements, true);
            assert_eq!(stdout, verdict, "the counted cold call's verdict");
            count
        });
        // A prime, then the counted warm call.
        host.counted_call(&acknowledgements, false);
        let (warm_git, stdout) = host.counted_call(&acknowledgements, false);
        assert_eq!(stdout, verdict, "the counted warm call's verdict");
        if scenario != "unreadable" {
            assert!(
                warm_git > 0 || cold_git.unwrap_or(1) > 0,
                "no git was counted"
            );
        }
        std::fs::write(&registry, readable).expect("the registry is restored");
        eprintln!(
            "stop-verdict scale {} {scenario}: warm median {}us max {}us at load1 {}{}; git warm \
             {warm_git}{}",
            scale.number(),
            warm.median_us,
            warm.max_us,
            warm.load1,
            cold.as_ref().map_or(String::new(), |cold| format!(
                ", cold median {}us max {}us at load1 {}",
                cold.median_us, cold.max_us, cold.load1
            )),
            cold_git.map_or(String::new(), |git| format!(" cold {git}")),
        );
        scenarios.push(Scenario {
            scenario: scenario.to_owned(),
            verdict_sha256: digest(verdict.as_bytes()),
            warm,
            cold,
            warm_git,
            cold_git,
        });
    }
    drop(host);
    std::fs::remove_dir_all(&root).expect("the workload is removed");
    Workload {
        scale: scale.number(),
        shape: counts,
        class_counts,
        own_runs: OWN_RUNS,
        scenarios,
    }
}

#[test]
fn full_workload_stop_verdict() {
    let directory = root().join("target/budget-records");
    std::fs::create_dir_all(&directory).expect("the records directory");
    let load_workers: u32 = std::env::var("ONEPIPELINE_BUDGET_LOAD_WORKERS")
        .ok()
        .map(|n| n.parse().expect("a worker count"))
        .unwrap_or(0);
    let run_id = std::env::var("ONEPIPELINE_BUDGET_INVOCATION").unwrap_or_else(|_| {
        digest(format!("{} {:?}", std::process::id(), std::time::SystemTime::now()).as_bytes())
    });
    let manifest = directory.join(telemetry::INVOCATION);
    let record_path = directory.join(if load_workers == 0 {
        telemetry::RECORD
    } else {
        telemetry::LOAD_RECORD
    });
    if load_workers == 0 {
        // Old evidence goes before anything is measured, so a journey that fails
        // leaves nothing a budget could read as current.
        std::fs::write(
            &manifest,
            json!({"state": "started", "run_id": run_id}).to_string(),
        )
        .expect("the manifest");
        let _ = std::fs::remove_file(&record_path);
    }
    let (program, profile) = binary();
    let source_sha256 = telemetry::source_fingerprint(&root());
    let binary_sha256 = digest(&std::fs::read(&program).expect("the measured binary"));
    let _load = (load_workers > 0).then(|| {
        let load = Load::start(load_workers);
        std::thread::sleep(Duration::from_secs(30));
        load
    });
    let scratch = std::env::var_os("ONEPIPELINE_NODE_SCRATCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("stop-verdict-{}", std::process::id()));
    let started = Instant::now();
    let initial_load = load1();
    let mut preparation_ms = 0;
    let workloads = vec![
        workload(Scale::One, &scratch, &mut preparation_ms),
        workload(Scale::Ten, &scratch, &mut preparation_ms),
    ];
    let _ = std::fs::remove_dir_all(&scratch);
    assert_eq!(
        telemetry::source_fingerprint(&root()),
        source_sha256,
        "the build inputs moved during the journey"
    );
    let mut record = Record {
        version: telemetry::VERSION,
        build: Build {
            run_id: run_id.clone(),
            binary: telemetry::BINARY_COPY.into(),
            binary_sha256: binary_sha256.clone(),
            profile: profile.into(),
            source_sha256: source_sha256.clone(),
        },
        load_workers,
        workloads,
        preparation_ms,
        total_ms: ms(started),
        load1: initial_load,
    };
    eprintln!(
        "stop-verdict preparation {}ms; total {}ms; load1 {}",
        record.preparation_ms, record.total_ms, record.load1
    );
    if load_workers > 0 {
        std::fs::write(
            &record_path,
            serde_json::to_vec_pretty(&record).expect("a record"),
        )
        .expect("the load record");
        return;
    }
    std::fs::copy(&program, directory.join(telemetry::BINARY_COPY))
        .expect("the producing binary is kept beside its telemetry");
    std::fs::write(
        &record_path,
        serde_json::to_vec_pretty(&record).expect("a record"),
    )
    .expect("the record");
    std::fs::write(
        &manifest,
        json!({
            "state": "complete",
            "run_id": run_id,
            "binary": telemetry::BINARY_COPY,
            "binary_sha256": binary_sha256,
            "source_sha256": source_sha256,
        })
        .to_string(),
    )
    .expect("the completed manifest");
    // The journey-to-reader path: every registered budget command reads what this
    // journey just wrote, through the checker's own SDK.
    if profile == "release" {
        for args in [
            vec![],
            vec!["--cold"],
            vec!["--scale", "10"],
            vec!["--journey-time"],
        ] {
            let result = directory.join("stop-guard-unpublished-sdk-result.json");
            let status = Command::new("node")
                .current_dir(root())
                .arg("scripts/stop-guard-unpublished-budget.mjs")
                .args(&args)
                .env("ONEBUDGETSPEC_RESULT", &result)
                .status()
                .expect("the read-only budget reader runs");
            assert!(
                status.success(),
                "the reader refused {args:?}; run {JOURNEY_COMMAND}"
            );
            assert!(
                std::fs::metadata(&result).expect("a result").len() > 0,
                "the SDK reported nothing"
            );
            std::fs::remove_file(result).expect("cleanup");
        }
    }
    record.total_ms = ms(started);
    std::fs::write(
        &record_path,
        serde_json::to_vec_pretty(&record).expect("a record"),
    )
    .expect("elapsed time includes the readers and cleanup");
    eprintln!("stop-verdict validated total {}ms", record.total_ms);
}
