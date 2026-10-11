//! `budgets.yaml` and the command behind its one budget, held to the checker that reads them:
//! `onebudgetspec-core`, the library the `onebudgetspec` binary is built from, at the release
//! `justfile` installs that binary at.
//!
//! The command runs a whole e2e journey, which `just budgets` measures; what is held here is
//! everything around that journey — the file loading as the budget it states, the result the
//! journey writes being one the checker parses as a value, and the command refusing to report a
//! measurement it did not take.
//!
//! `host-view-time` is held here the same way: its entry, its command — which runs no
//! journey at all, only reads the record `scripts/host-view-measure.mjs` leaves — reporting
//! that record and refusing one it cannot vouch for, and the measurement refusing to record a
//! read that failed.

#[cfg(unix)]
#[path = "e2e/budget_result.rs"]
mod budget_result;

use std::path::PathBuf;

use onebudgetspec_core::{Direction, Measure};
#[cfg(unix)]
use onebudgetspec_core::{Selection, Verdict};

const BUDGET: &str = "linear-requests-per-writeback-settlement";

const JOURNEY: &str =
    "linear_writeback::the_costliest_linear_settlement_after_the_first_is_reported_as_measured";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[cfg(unix)]
fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("onepipeline-budgets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The file loads under the pinned checker, and registers the budget as it is meant: the
/// requests a Linear settlement write-back sends, reported by its command, at most two.
#[test]
fn the_budgets_file_registers_the_linear_writeback_budget_for_the_pinned_checker() {
    let loaded = onebudgetspec_core::load(&[root().join("budgets.yaml")], false)
        .unwrap_or_else(|error| panic!("budgets.yaml does not load: {error}"));
    let budget = loaded
        .files()
        .iter()
        .flat_map(|file| &file.contents.budgets)
        .find(|budget| budget.id == BUDGET)
        .unwrap_or_else(|| panic!("budgets.yaml registers no {BUDGET}"));
    assert_eq!(budget.measure, Measure::Reported);
    assert_eq!(budget.unit, "requests");
    assert_eq!(budget.direction, Direction::Max);
    assert!(
        (budget.threshold - 2.0).abs() < f64::EPSILON,
        "{}",
        budget.threshold
    );
    assert_eq!(
        budget.command,
        ["bash", "scripts/linear-writeback-budget.sh"],
        "the budget's command is not the committed script"
    );
    let description = budget.description.as_deref().unwrap_or_default();
    assert!(
        description.contains("production Linear key") && description.contains("no check may reach"),
        "{description}"
    );
}

/// The checker library linked here is the release `just budgets` installs, so what this file
/// proves of it is true of the binary that measures.
#[test]
fn the_linked_checker_is_the_release_the_justfile_installs() {
    let justfile = std::fs::read_to_string(root().join("justfile")).expect("the justfile reads");
    let installed = justfile
        .lines()
        .find_map(|line| line.strip_prefix("onebudgetspec-version := "))
        .map(|version| version.trim().trim_matches('"').to_owned())
        .expect("the justfile names the onebudgetspec release it installs");
    let lock = std::fs::read_to_string(root().join("Cargo.lock")).expect("the lock reads");
    let linked = lock
        .split("[[package]]")
        .find(|package| package.contains("name = \"onebudgetspec-core\""))
        .and_then(|package| {
            package
                .lines()
                .find_map(|line| line.strip_prefix("version = "))
                .map(|version| version.trim_matches('"').to_owned())
        })
        .expect("the lock resolves onebudgetspec-core");
    assert_eq!(installed, linked);
}

/// What the journey writes is a value to the checker: measured through the library's own
/// `check`, over a budget whose command writes exactly the result the journey builds. That
/// command is a POSIX shell line, as the budget's own is, so it runs where a shell does.
#[cfg(unix)]
#[test]
fn the_result_the_journey_writes_is_a_value_the_checker_measures() {
    let dir = scratch("result");
    let written = budget_result::reported(3, "three requests for the second attempt").to_string();
    std::fs::write(dir.join("result.json"), &written).expect("the result is staged");
    std::fs::write(
        dir.join("budgets.yaml"),
        "schema_version: 1\nbudgets:\n  - id: staged\n    measure: reported\n    \
         command: [\"sh\", \"-c\", \"cat result.json > \\\"$ONEBUDGETSPEC_RESULT\\\"\"]\n    \
         unit: requests\n    direction: max\n    threshold: 2\n",
    )
    .expect("a budgets file");
    let loaded = onebudgetspec_core::load(&[dir.join("budgets.yaml")], false)
        .unwrap_or_else(|error| panic!("the staged budgets file does not load: {error}"));
    let report = loaded
        .select(&Selection::default())
        .expect("every budget is selected")
        .check();
    let [result] = report.results.as_slice() else {
        panic!("one budget, one result: {:?}", report.results);
    };
    assert_eq!(result.error, None, "the checker could not read {written}");
    assert_eq!(result.actual, Some(3.0));
    assert_eq!(result.verdict, Verdict::Over);
    assert_eq!(
        result.detail.as_deref(),
        Some("three requests for the second attempt")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The command, run with `just` standing in for the journey it hands to: `behaviour` is the
/// stand-in's shell body, and `result` the file the command is told to write to, if any.
#[cfg(unix)]
fn command(name: &str, behaviour: &str, result: Option<&std::path::Path>) -> std::process::Output {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch(name);
    let just = dir.join("just");
    std::fs::write(&just, format!("#!/bin/sh\n{behaviour}\n")).expect("a stand-in just");
    std::fs::set_permissions(&just, std::fs::Permissions::from_mode(0o755))
        .expect("the stand-in is executable");
    let path = std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("a PATH");
    let mut run = std::process::Command::new("bash");
    run.arg(root().join("scripts/linear-writeback-budget.sh"))
        .current_dir(root())
        .env("PATH", path)
        .env_remove("ONEBUDGETSPEC_RESULT");
    if let Some(result) = result {
        run.env("ONEBUDGETSPEC_RESULT", result);
    }
    run.output().expect("the budget command runs")
}

/// Run outside the checker, the command refuses before it runs anything; run by it, a journey
/// that fails, or passes and writes nothing, fails the command rather than reporting a value it
/// never measured, and one that writes its result passes.
#[cfg(unix)]
#[test]
fn the_budget_command_reports_only_a_measurement_its_journey_took() {
    let stderr =
        |output: &std::process::Output| String::from_utf8_lossy(&output.stderr).into_owned();

    let unset = command("unset", "echo ran >&2; exit 0", None);
    assert_eq!(unset.status.code(), Some(2), "{}", stderr(&unset));
    assert!(
        stderr(&unset).contains("ONEBUDGETSPEC_RESULT is not set"),
        "{}",
        stderr(&unset)
    );
    assert!(
        !stderr(&unset).contains("ran"),
        "it ran the journey with nowhere to write"
    );

    let dir = scratch("results");
    let failing = command("failing", "exit 4", Some(&dir.join("failing.json")));
    assert_eq!(failing.status.code(), Some(4), "{}", stderr(&failing));

    let silent = dir.join("silent.json");
    std::fs::write(&silent, "").expect("a fresh, empty result file");
    let silent = command("silent", "exit 0", Some(&silent));
    assert_eq!(silent.status.code(), Some(1), "{}", stderr(&silent));
    assert!(
        stderr(&silent).contains("wrote no result"),
        "{}",
        stderr(&silent)
    );

    // The stand-in writes a result only when it was asked for exactly the journey that measures
    // this budget, through the recipe that runs e2e journeys, with the result file reaching it.
    let measured = dir.join("measured.json");
    std::fs::write(&measured, "").expect("a fresh, empty result file");
    let wrote = command(
        "wrote",
        &format!(
            "[ \"$1\" = test-e2e ] && [ \"$2\" = 'test(={JOURNEY})' ] && [ $# -eq 2 ] \\\n  \
             && printf '{{\"value\": 2}}' > \"$ONEBUDGETSPEC_RESULT\""
        ),
        Some(&measured),
    );
    assert_eq!(wrote.status.code(), Some(0), "{}", stderr(&wrote));
    assert_eq!(
        std::fs::read_to_string(&measured).expect("the result reads"),
        r#"{"value": 2}"#
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The journey the command asks for is one the e2e binary has: a filter naming a test that is
/// not there would select nothing.
#[test]
fn the_budget_command_names_a_journey_the_e2e_binary_holds() {
    let journeys = std::fs::read_to_string(root().join("tests/e2e/linear_writeback.rs"))
        .expect("the journeys read");
    let (module, name) = JOURNEY
        .split_once("::")
        .expect("a module-qualified journey");
    assert_eq!(module, "linear_writeback");
    assert!(
        journeys.contains(&format!("#[test]\nfn {name}()")),
        "tests/e2e/linear_writeback.rs holds no journey {name}"
    );
    let script = std::fs::read_to_string(root().join("scripts/linear-writeback-budget.sh"))
        .expect("the command reads");
    assert!(
        script.contains(&format!("journey='{JOURNEY}'")),
        "the command runs another journey than {JOURNEY}"
    );
}

/// The view budget, as `budgets.yaml` registers it: one `onepipeline host` read, reported by
/// its command off the record the host-view journeys write, at most 100 milliseconds.
#[test]
fn the_budgets_file_registers_the_host_view_budget() {
    let loaded = onebudgetspec_core::load(&[root().join("budgets.yaml")], false)
        .unwrap_or_else(|error| panic!("budgets.yaml does not load: {error}"));
    let budget = loaded
        .files()
        .iter()
        .flat_map(|file| &file.contents.budgets)
        .find(|budget| budget.id == "host-view-time")
        .expect("budgets.yaml registers host-view-time");
    assert_eq!(budget.measure, Measure::Reported);
    assert_eq!(budget.unit, "milliseconds");
    assert_eq!(budget.direction, Direction::Max);
    assert!(
        (budget.threshold - 100.0).abs() < f64::EPSILON,
        "{}",
        budget.threshold
    );
    assert_eq!(
        budget.command,
        ["node", "scripts/host-view-budget.mjs"],
        "the budget's command is not the committed reporter"
    );
    // It times the release binary CI's `install` job builds, so the gate's `just budgets`,
    // which builds none, leaves it to `just host-view-budget` there.
    assert_eq!(budget.labels, ["release-binary"]);
    let justfile = std::fs::read_to_string(root().join("justfile")).expect("the justfile reads");
    assert!(justfile.contains("check budgets.yaml --exclude-label release-binary"));
    assert!(justfile.contains("onebudgetspec check budgets.yaml --id host-view-time"));
    let ci = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).expect("ci.yml");
    assert!(ci.contains("just host-view-budget \"$(command -v onepipeline)\""));
}

/// The reporter, run from `cwd` with the result going to `result` when one is named.
fn host_view_report(
    cwd: &std::path::Path,
    result: Option<&std::path::Path>,
) -> std::process::Output {
    let mut command = std::process::Command::new("node");
    command
        .arg(root().join("scripts/host-view-budget.mjs"))
        .current_dir(cwd)
        .env_remove("ONEBUDGETSPEC_RESULT");
    if let Some(result) = result {
        command.env("ONEBUDGETSPEC_RESULT", result);
    }
    command.output().expect("node runs the committed reporter")
}

/// A record shaped as `scripts/host-view-measure.mjs` writes one.
fn host_view_record() -> serde_json::Value {
    let breakdown = |live_fold_ms: f64| {
        serde_json::json!({
            "event_registry_ms": 2.0, "list_ms": 0.4, "launch_and_summary_ms": 1.8,
            "registry_ms": 0.2, "pid_proofs_ms": 0.3, "decide_ended_ms": 0.2,
            "live_fold_ms": live_fold_ms, "cold_fold_ms": null, "free_space_ms": 0.1,
            "render_ms": 0.1, "process_start_ms": 1.0, "exit_ms": 1.0, "live_runs_folded": 3,
            "live_journal_bytes": 568_855, "cold_journal_bytes": 1000, "read_bytes": 3000,
        })
    };
    serde_json::json!({
        "schema_version": 1,
        "binary": "/usr/local/bin/onepipeline",
        "runs_root": {"run_roots": 100, "live_dispatches": 5, "stale_entries": 4},
        "steady": {
            "build": "release",
            "reads_ms": [12.5, 31.0, 9.5, 14.0, 13.0],
            "median_ms": 13.0,
            "load1": 2.5,
            "breakdown": breakdown(4.0),
            "ended_journals_unreadable": true,
        },
        "cold": {"read_ms": 3600.0, "breakdown": breakdown(9.0), "generation_ms": 4000.0},
        "scaling": {
            "sizes": [
                {"run_roots": 100, "ended_runs": 88, "reads_ms": [10.0], "median_ms": 10.0, "load1": 2.5,
                 "ended_journal_bytes_opened": 0, "runs_opened": ["live-0", "live-1", "live-2"]},
                {"run_roots": 400, "ended_runs": 361, "reads_ms": [18.0], "median_ms": 18.0, "load1": 2.5,
                 "ended_journal_bytes_opened": 0, "runs_opened": ["live-0", "live-1", "live-2"]},
                {"run_roots": 800, "ended_runs": 725, "reads_ms": [30.0], "median_ms": 30.0, "load1": 2.5,
                 "ended_journal_bytes_opened": 0, "runs_opened": ["live-0", "live-1", "live-2"]},
            ],
            "increments": [
                {"from": 100, "to": 400, "ms_per_ended_run": 0.03},
                {"from": 400, "to": 800, "ms_per_ended_run": 0.03},
            ],
        },
    })
}

/// The reporter reports only what the measurement recorded: the slowest steady single read
/// as the value, since each is judged, and the rest as detail. A record that is missing, that
/// the measurement did not write, or that times anything but the release build is refused,
/// and so is a run outside the checker — never a value nobody measured.
#[cfg(unix)]
#[test]
fn the_host_view_reporter_reports_the_slowest_steady_read_of_the_record_alone() {
    let dir = scratch("host-view-report");
    let record = dir.join("target/budget-records/host-view.json");
    std::fs::create_dir_all(record.parent().expect("a directory")).expect("the record's directory");
    let result = dir.join("result.json");

    let mut malformed: Vec<Option<String>> =
        vec![None, Some("{".to_owned()), Some("{}".to_owned())];
    for (path, value) in [
        ("/schema_version", serde_json::json!(2)),
        ("/steady/build", serde_json::json!("debug")),
        ("/steady/reads_ms", serde_json::json!([])),
        ("/steady/reads_ms", serde_json::json!([12.0, -1.0])),
        ("/steady/reads_ms", serde_json::json!(["12"])),
        (
            "/steady/reads_ms",
            serde_json::json!([12.0, 13.0, 14.0, 15.0]),
        ),
        (
            "/steady/ended_journals_unreadable",
            serde_json::json!(false),
        ),
        ("/runs_root/run_roots", serde_json::json!(50)),
        ("/runs_root/live_dispatches", serde_json::json!(0)),
        ("/runs_root/stale_entries", serde_json::json!(null)),
        ("/cold/read_ms", serde_json::json!("slow")),
        ("/cold", serde_json::json!(null)),
        ("/steady/breakdown/live_fold_ms", serde_json::json!(-4.0)),
        (
            "/steady/breakdown/live_runs_folded",
            serde_json::json!("three"),
        ),
        ("/steady/median_ms", serde_json::json!("13")),
        ("/steady/load1", serde_json::json!(-1.0)),
        ("/scaling/sizes", serde_json::json!("all of them")),
        ("/scaling/sizes/0/median_ms", serde_json::json!(null)),
        ("/scaling/sizes/2/run_roots", serde_json::json!(1600)),
        ("/scaling/sizes/1/reads_ms", serde_json::json!([])),
        (
            "/scaling/sizes/1/ended_journal_bytes_opened",
            serde_json::json!(null),
        ),
        (
            "/scaling/sizes/2/ended_journal_bytes_opened",
            serde_json::json!(-1),
        ),
        ("/scaling/sizes/0/runs_opened", serde_json::json!("live-0")),
        ("/scaling/sizes/2/runs_opened", serde_json::json!([""])),
        ("/scaling/increments/1/to", serde_json::json!(1600)),
        (
            "/scaling/increments/0/ms_per_ended_run",
            serde_json::json!(null),
        ),
    ] {
        let mut held = host_view_record();
        *held.pointer_mut(path).expect("a field") = value;
        malformed.push(Some(held.to_string()));
    }
    // A breakdown that lacks a figure — one written or read under another name — too.
    let mut renamed = host_view_record();
    let breakdown = renamed["steady"]["breakdown"]
        .as_object_mut()
        .expect("a breakdown");
    let figure = breakdown.remove("list_ms").expect("a figure");
    breakdown.insert("listing_ms".to_owned(), figure);
    malformed.push(Some(renamed.to_string()));
    for body in malformed {
        let _ = std::fs::remove_file(&record);
        if let Some(body) = &body {
            std::fs::write(&record, body).expect("a record");
        }
        let output = host_view_report(&dir, Some(&result));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{body:?} was reported: {stderr}");
        assert!(!result.exists(), "a refused record reported a value");
        assert!(
            stderr.contains("target/budget-records/host-view.json")
                && stderr.contains("just host-view-budget BINARY"),
            "{stderr}"
        );
    }

    std::fs::write(&record, host_view_record().to_string()).expect("a record");
    let outside = host_view_report(&dir, None);
    assert!(!outside.status.success());
    assert!(
        String::from_utf8_lossy(&outside.stderr).contains("ONEBUDGETSPEC_RESULT is missing"),
        "{}",
        String::from_utf8_lossy(&outside.stderr)
    );

    // A result that cannot be written is a failure, and names what to fix.
    let unwritable = host_view_report(&dir, Some(&dir));
    assert!(!unwritable.status.success());
    assert!(
        String::from_utf8_lossy(&unwritable.stderr)
            .contains("ensure ONEBUDGETSPEC_RESULT names a writable file"),
        "{}",
        String::from_utf8_lossy(&unwritable.stderr)
    );

    let reported = host_view_report(&dir, Some(&result));
    assert!(
        reported.status.success(),
        "{}",
        String::from_utf8_lossy(&reported.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&result).expect("a result")).expect("JSON");
    assert_eq!(
        result["value"], 31.0,
        "the slowest single read is the value: {result}"
    );
    let detail = result["detail"].as_str().expect("a detail");
    for expected in [
        "12.5, 31.0, 9.5, 14.0, 13.0 ms; median 13.0 ms (reported, not the budget)",
        "event registry 2.0 ms",
        "folding 3 live run(s)",
        "every ended run's journal unreadable to the reads",
        "cold read (every ended run's derived documents absent): 3600.0 ms",
        "400 roots median 18.0 ms (18.0 ms), 0 ended-journal bytes opened, \
         journals of 3 run(s) opened: live-0, live-1, live-2",
        "800 roots median 30.0 ms",
        "30.0 µs per ended run 100→400",
    ] {
        assert!(
            detail.contains(expected),
            "the detail lacks {expected:?}: {detail}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The budget's measurement and the journeys that hold the view correct read one root, from
/// one generator: what the release binary is timed over is what this suite's build is proved
/// over. And the measurement refuses to run without the binary it times.
#[test]
fn the_host_view_measurement_and_journeys_share_one_generator() {
    let generator = "tests/e2e/host_view_fixture.mjs";
    assert!(root().join(generator).is_file());
    let measure = std::fs::read_to_string(root().join("scripts/host-view-measure.mjs"))
        .expect("the measurement reads");
    assert!(measure.contains(&format!("from \"../{generator}\"")));
    let journeys =
        std::fs::read_to_string(root().join("tests/e2e/host_view.rs")).expect("the journeys read");
    assert!(journeys.contains(&format!("const GENERATOR: &str = \"{generator}\";")));

    let unasked = std::process::Command::new("node")
        .arg(root().join("scripts/host-view-measure.mjs"))
        .output()
        .expect("node runs the measurement");
    assert_eq!(unasked.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&unasked.stderr)
            .contains("usage: node scripts/host-view-measure.mjs BINARY"),
        "{}",
        String::from_utf8_lossy(&unasked.stderr)
    );
}

/// The measurement records a read only when the read answered: a binary that fails, and one
/// that exits 0 having recorded no phases, each fail the measurement naming what to do next,
/// and neither leaves a record behind.
#[cfg(unix)]
#[test]
fn the_host_view_measurement_records_nothing_from_a_read_that_failed() {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch("host-view-measure");
    let record = root().join("target/budget-records/host-view.json");
    let before = std::fs::metadata(&record)
        .and_then(|about| about.modified())
        .ok();
    for (name, body, says) in [
        ("failing", "echo refused >&2; exit 3", "exited 3"),
        ("silent", "echo '{}'", "recorded no phase line"),
    ] {
        let binary = dir.join(name);
        std::fs::write(&binary, format!("#!/bin/sh\n{body}\n")).expect("a stand-in binary");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        let measured = std::process::Command::new("node")
            .arg(root().join("scripts/host-view-measure.mjs"))
            .arg(&binary)
            .output()
            .expect("node runs the measurement");
        let stderr = String::from_utf8_lossy(&measured.stderr);
        assert_eq!(measured.status.code(), Some(1), "{name}: {stderr}");
        assert!(
            stderr.contains("host-view-measure: the cold read")
                && stderr.contains(says)
                && stderr.contains("next: pass an installed release build"),
            "{name}: {stderr}"
        );
        assert_eq!(
            std::fs::metadata(&record)
                .and_then(|about| about.modified())
                .ok(),
            before,
            "{name}: a failed measurement wrote the record"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// What one tracer-driven read through `scripts/host-view-trace.mjs` answers, over
/// `sh -c script` in `dir`: the journals it opened and refused, and what holding it to a
/// steady read (`old` ended, `live-0` live) answered or refused.
#[cfg(target_os = "linux")]
fn traced_opens(dir: &std::path::Path, script: &str, env: &[(&str, String)]) -> serde_json::Value {
    let module = root().join("scripts/host-view-trace.mjs");
    let driver = format!(
        r#"import {{ journalOpens, steadyOpens }} from {module:?};
const out = {{}};
try {{
  const seen = journalOpens("sh", ["-c", process.argv[1]], "trace.out");
  out.opened = Object.fromEntries(seen.opened);
  out.refused = [...seen.refused].sort();
  out.steady = steadyOpens(seen, ["old"], ["live-0"], "read");
}} catch (error) {{
  out.error = error.message;
}}
console.log(JSON.stringify(out));"#,
        module = module.display().to_string()
    );
    // Found on this process's PATH, so a case may hand the driver another.
    let node = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("node"))
        .find(|node| node.is_file())
        .expect("node on PATH");
    let mut command = std::process::Command::new(node);
    command
        .args(["--input-type=module", "-e", &driver, script])
        .current_dir(dir)
        .envs(env.iter().map(|(key, value)| (key, value)));
    let output = command.output().expect("node runs the tracer module");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the driver's JSON")
}

/// The measurement's record says from the kernel which journals each scaling read opened:
/// `scripts/host-view-trace.mjs` runs a real process under the real `strace` and counts the
/// bytes it read from each run's journal, including from many processes reading at once, whose
/// calls the tracer splits. A read that opened or merely tried an ended run's journal, or opened
/// other runs' than the live ones, is refused, naming them; so is a traced command that failed,
/// a tracer that would not run or left no trace, and a trace with a call it cannot account for.
#[cfg(target_os = "linux")]
#[test]
fn the_host_view_measurement_counts_the_journals_a_traced_read_opened() {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch("host-view-trace");
    let journal = |run: &str| dir.join(run).join("events.jsonl");
    for (run, body) in [("live-0", "a live record\n"), ("old", "an ended record\n")] {
        std::fs::create_dir_all(dir.join(run)).expect("a run root");
        std::fs::write(journal(run), body).expect("a journal");
    }
    std::fs::write(dir.join("other.txt"), "not a journal\n").expect("a file");
    let live_bytes = "a live record\n".len() as u64;
    let unreadable = |yes: bool| {
        std::fs::set_permissions(
            journal("old"),
            std::fs::Permissions::from_mode(if yes { 0o000 } else { 0o644 }),
        )
        .expect("the ended journal's mode");
    };

    // Steady: the live journal read, the ended one never reached.
    let steady = traced_opens(&dir, "cat live-0/events.jsonl other.txt", &[]);
    assert_eq!(
        steady["opened"],
        serde_json::json!({"live-0": live_bytes}),
        "{steady}"
    );
    assert_eq!(
        steady["steady"],
        serde_json::json!({"ended_journal_bytes_opened": 0, "runs_opened": ["live-0"]}),
        "{steady}"
    );

    // A journal opened by a path relative to its own run root is that run's.
    let relative = traced_opens(&dir, "cd live-0 && cat events.jsonl", &[]);
    assert_eq!(
        relative["opened"],
        serde_json::json!({"live-0": live_bytes}),
        "{relative}"
    );

    // A journal under a name the tracer had to escape cannot be read back to its run: refused.
    std::fs::create_dir_all(dir.join("we\u{1}ird")).expect("a run root");
    std::fs::write(dir.join("we\u{1}ird/events.jsonl"), "x\n").expect("a journal");
    let escaped = traced_opens(&dir, "cat we*ird/events.jsonl", &[]);
    assert!(
        escaped["error"].as_str().is_some_and(
            |error| error.contains("a journal path strace escaped cannot be read back")
        ),
        "{escaped}"
    );

    // Twenty readers at once: every byte each read is counted, whichever calls the tracer split.
    let many = traced_opens(
        &dir,
        "for i in $(seq 20); do cat live-0/events.jsonl & done; wait",
        &[],
    );
    assert_eq!(
        many["opened"],
        serde_json::json!({"live-0": 20 * live_bytes}),
        "{many}"
    );

    // An ended journal tried and refused by its mode is refused, as is one opened and read.
    unreadable(true);
    let tried = traced_opens(
        &dir,
        "cat live-0/events.jsonl; cat old/events.jsonl 2>/dev/null; true",
        &[],
    );
    assert_eq!(tried["refused"], serde_json::json!(["old"]), "{tried}");
    assert!(
        tried["error"]
            .as_str()
            .is_some_and(|error| error.contains("tried to open ended runs' journals: old")),
        "{tried}"
    );
    unreadable(false);
    let read = traced_opens(&dir, "cat live-0/events.jsonl old/events.jsonl", &[]);
    assert_eq!(
        read["opened"]["old"],
        serde_json::json!("an ended record\n".len()),
        "{read}"
    );
    assert!(
        read["error"]
            .as_str()
            .is_some_and(|error| error.contains("ended runs' journals: old")),
        "{read}"
    );

    // A read that opened no live journal is not the live runs' read.
    let none = traced_opens(&dir, "cat other.txt", &[]);
    assert!(
        none["error"].as_str().is_some_and(
            |error| error.contains("opened the journals of no run, not the live runs live-0")
        ),
        "{none}"
    );

    // A journal read with `pread64` counts as one read with `read`.
    let pread = traced_opens(
        &dir,
        "python3 -I -c \"import os; os.pread(os.open('live-0/events.jsonl', os.O_RDONLY), 64, 0)\"",
        &[],
    );
    assert_eq!(
        pread["opened"],
        serde_json::json!({"live-0": live_bytes}),
        "{pread}"
    );

    // An open a reader was killed inside has no result to read it by, so it is no observation.
    let fifo = dir.join("live-1");
    std::fs::create_dir_all(&fifo).expect("a run root");
    let made = std::process::Command::new("mkfifo")
        .arg(fifo.join("events.jsonl"))
        .status()
        .expect("mkfifo runs");
    assert!(made.success());
    let killed = traced_opens(
        &dir,
        "cat live-1/events.jsonl & sleep 0.3; kill -9 $!; true",
        &[],
    );
    assert!(
        killed["error"]
            .as_str()
            .is_some_and(|error| error.contains("a traced call on live-1's journal has no result")),
        "{killed}"
    );

    // A traced command that failed answers nothing.
    let failed = traced_opens(&dir, "cat live-0/events.jsonl; exit 3", &[]);
    assert!(
        failed["error"]
            .as_str()
            .is_some_and(|error| error.contains("exited 3")),
        "{failed}"
    );

    // Neither does a tracer that is not there, one that refuses, one that leaves no trace, or a
    // trace that cannot be what a traced read did. The stand-in tracer writes `FAKE_TRACE`'s
    // file where `-o` asks, or refuses where it names none.
    let fake = dir.join("bin");
    std::fs::create_dir_all(&fake).expect("a bin directory");
    std::fs::write(
        fake.join("strace"),
        "#!/bin/sh\n[ -n \"$FAKE_TRACE\" ] || { echo 'ptrace refused' >&2; exit 1; }\n\
         while [ \"$1\" != -o ]; do shift; done\n\
         [ \"$FAKE_TRACE\" = none ] || cp \"$FAKE_TRACE\" \"$2\"\n",
    )
    .expect("a stand-in tracer");
    std::fs::set_permissions(fake.join("strace"), std::fs::Permissions::from_mode(0o755))
        .expect("executable");
    let path = format!(
        "{}:{}",
        fake.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let absent = traced_opens(
        &dir,
        "true",
        &[("PATH", dir.join("nothing").display().to_string())],
    );
    assert!(
        absent["error"]
            .as_str()
            .is_some_and(|error| error.contains("`strace` could not run")),
        "{absent}"
    );
    let refused = traced_opens(&dir, "true", &[("PATH", path.clone())]);
    assert!(
        refused["error"]
            .as_str()
            .is_some_and(|error| error.contains("exited 1: ptrace refused")),
        "{refused}"
    );
    let open = "1 openat(AT_FDCWD</r>, \"live-0/events.jsonl\", O_RDONLY";
    for (name, trace, says) in [
        ("none", None, "the tracer left no trace"),
        (
            "unannotated",
            Some("1 openat(AT_FDCWD, \"live-0/events.jsonl\", O_RDONLY) = 3\n".to_owned()),
            "a traced call names a journal unreadably",
        ),
        (
            "no-open",
            Some("1 read(0</dev/null>, \"\", 64) = 0\n".to_owned()),
            "recorded no open",
        ),
        (
            "orphan",
            Some(format!("{open}) = 3\n2 <... read resumed>\"x\", 64) = 1\n")),
            "a call resumed that never began",
        ),
        (
            "cut",
            Some(format!("{open} <unfinished ...>\n")),
            "a traced call on a journal never finished",
        ),
    ] {
        let fixture = match trace {
            Some(trace) => {
                let file = dir.join(format!("{name}.trace"));
                std::fs::write(&file, trace).expect("a trace");
                file.display().to_string()
            }
            None => "none".to_owned(),
        };
        let answered = traced_opens(
            &dir,
            "true",
            &[("PATH", path.clone()), ("FAKE_TRACE", fixture)],
        );
        assert!(
            answered["error"]
                .as_str()
                .is_some_and(|error| error.contains(says)),
            "{name}: {answered}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
