//! What one `onepipeline` process spent its time on, as `ONEPIPELINE_PROFILE_SPANS`
//! has it say: one record per process, its verb, its wall and CPU, and its spans.
//!
//! Every journey drives the compiled binary against this world's real store and a
//! run it launched through the CLI, and reads back the record the process wrote.
//! The record's shape is `tests/golden/span-profile-record-v1.json`, which
//! `scripts/test-profile-report.mjs`'s tests read too. No assertion holds a figure
//! to a fixed time: each is held to another figure of the same record, or of the
//! child process that did the work.

use std::path::{Path, PathBuf};

use onepipeline_testfakes::profile;
use serde_json::Value;

use crate::harness::{agent, double, plan_of, repo_file, Run, World};

/// The binary's switch: the directory a process writes its record into.
const SPANS_ENV: &str = "ONEPIPELINE_PROFILE_SPANS";

/// The record's shape, which the report's tests read as well.
const GOLDEN: &str = "tests/golden/span-profile-record-v1.json";

/// The phase line `onepipeline host` writes for the `host-view-time` budget.
const HOST_PHASES_ENV: &str = "ONEPIPELINE_HOST_PHASES";

/// What a record says, without the values: each object's keys and each leaf's JSON
/// type, with `spans` — one entry per span a process happened to enter — reduced to
/// the shape of an entry.
fn shape(value: &Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, field)| {
                    let reduced = match (key.as_str(), field) {
                        ("spans", Value::Array(spans)) => {
                            Value::Array(spans.first().map(shape).into_iter().collect())
                        }
                        _ => shape(field),
                    };
                    (key.clone(), reduced)
                })
                .collect(),
        ),
        Value::Number(_) => Value::from("number"),
        Value::String(_) => Value::from("string"),
        Value::Bool(_) => Value::from("bool"),
        Value::Null => Value::from("null"),
        Value::Array(_) => Value::from("array"),
    }
}

/// Every record a directory holds; a file still being staged (`.`-named) is not one.
fn records(dir: &Path) -> Vec<Value> {
    let Ok(entries) = profile::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .map(|entry| entry.expect("a directory entry").path())
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            assert!(
                !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.')),
                "a staged record was left behind: {}",
                path.display()
            );
            let text = profile::fs::read_to_string(path).expect("the record is readable");
            serde_json::from_str(&text).expect("the record is JSON")
        })
        .collect()
}

/// The one record `dir` holds, held to the golden's shape.
fn the_record(dir: &Path) -> Value {
    let mut written = records(dir);
    assert_eq!(written.len(), 1, "one process, one record: {written:#?}");
    let record = written.remove(0);
    let golden: Value = serde_json::from_str(
        &profile::fs::read_to_string(repo_file(GOLDEN)).expect("the golden record"),
    )
    .expect("the golden record is JSON");
    assert_eq!(
        shape(&record),
        shape(&golden),
        "a span record no longer has {GOLDEN}'s shape: bump `spans::SCHEMA` and the golden \
         together, and the report's reader with them: {record:#}"
    );
    assert_eq!(record["schema"], golden["schema"], "{record:#}");
    record
}

fn span<'a>(record: &'a Value, name: &str) -> Option<&'a Value> {
    record["spans"]
        .as_array()
        .expect("spans is a list")
        .iter()
        .find(|span| span["name"] == name)
}

fn names(record: &Value) -> Vec<String> {
    record["spans"]
        .as_array()
        .expect("spans is a list")
        .iter()
        .map(|span| span["name"].as_str().expect("a span's name").to_owned())
        .collect()
}

fn figure(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("{value} is not a figure"))
}

/// Each span's own time is within its whole, and the spans' own times fit inside
/// the process: they are one thread's and never overlap.
fn spans_fit_the_process(record: &Value) {
    let mut own = 0.0;
    for span in record["spans"].as_array().expect("spans is a list") {
        let (self_wall, wall) = (figure(&span["self_wall_ms"]), figure(&span["wall_ms"]));
        assert!(
            self_wall <= wall,
            "{} spent {self_wall}ms of its own in {wall}ms: {record:#}",
            span["name"]
        );
        own += self_wall;
    }
    let wall = figure(&record["wall_ms"]);
    assert!(
        own <= wall,
        "the spans hold {own}ms of a {wall}ms process: {record:#}"
    );
}

/// A world with one settled run, launched through the CLI, and the project it was
/// read from.
fn settled(name: &str) -> (World, String, String) {
    let world = World::new(name);
    let project = world.plan(name, &plan_of(name, vec![agent("build", &[])]));
    world.run(&["start", &project, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    (world, project, name.to_owned())
}

/// `args` run from `cwd`, with the switch set to `spans` — or, for `None`, set to
/// nothing, then not set at all, both of which must record nothing.
fn run_profiled(world: &World, cwd: &Path, spans: Option<&Path>, args: &[&str]) -> Run {
    let mut command = world.cmd(args);
    command.current_dir(cwd);
    match spans {
        Some(dir) => command.env(SPANS_ENV, dir),
        None => command.env(SPANS_ENV, ""),
    };
    world.run_on(command, &args.join(" "))
}

fn run_unset(world: &World, cwd: &Path, args: &[&str]) -> Run {
    let mut command = world.cmd(args);
    command.current_dir(cwd).env_remove(SPANS_ENV);
    world.run_on(command, &args.join(" "))
}

/// Two verbs, each run with the switch set, empty and unset: set, each writes one
/// record naming its verb with its wall, CPU and the spans a verb's start is made
/// of; empty or unset, nothing is written — not even into the directory it ran in,
/// which an empty value read as a path would name — and what each prints is the same.
#[test]
fn each_verb_profiled_writes_one_record_of_its_startup_configuration_and_reads() {
    let (world, project, run) = settled("span-verbs");
    let quiet = world.root.join("quiet");
    profile::fs::create_dir_all(&quiet).expect("a directory to run in");
    let verbs: [(&str, Vec<&str>, &str); 2] = [
        ("status", vec!["status", "--json", &run], "journal:read"),
        (
            "plan check",
            vec!["plan", "check", &project, "--json"],
            "onetaskgraph:",
        ),
    ];
    for (verb, args, read) in &verbs {
        let spans = world.root.join(format!("spans-{}", verb.replace(' ', "-")));
        let profiled = run_profiled(&world, &quiet, Some(&spans), args);
        let empty = run_profiled(&world, &quiet, None, args);
        let unset = run_unset(&world, &quiet, args);
        for other in [&empty, &unset] {
            assert_eq!(
                (other.code, &other.stdout),
                (profiled.code, &profiled.stdout),
                "`{verb}` answered differently with the profile on: {}",
                profiled.stderr
            );
        }
        assert_eq!(profiled.code, 0, "{}", profiled.stderr);
        assert!(
            records(&quiet).is_empty(),
            "an empty or unset switch wrote into the directory `{verb}` ran in"
        );

        let record = the_record(&spans);
        assert_eq!(record["verb"], *verb, "{record:#}");
        assert_eq!(record["exit_status"], profiled.code, "{record:#}");
        assert!(figure(&record["wall_ms"]) > 0.0, "{record:#}");
        if cfg!(unix) {
            let cpu = figure(&record["user_cpu_ms"]) + figure(&record["system_cpu_ms"]);
            assert!(cpu > 0.0, "{record:#}");
            assert!(figure(&record["peak_rss_kb"]) > 0.0, "{record:#}");
        }
        let held = names(&record);
        for wanted in ["startup", "dispatch"] {
            assert!(
                held.iter().any(|name| name == wanted),
                "`{verb}` has no {wanted} span: {record:#}"
            );
        }
        assert!(
            held.iter().any(|name| name.starts_with("configuration:")),
            "`{verb}` has no configuration span: {record:#}"
        );
        assert!(
            held.iter().any(|name| name.starts_with(read)),
            "`{verb}` has no {read} span: {record:#}"
        );
        let startup = span(&record, "startup").expect("a startup span");
        assert!(figure(&startup["wall_ms"]) > 0.0, "{record:#}");
        spans_fit_the_process(&record);
    }
}

/// `plan check` with a registered check crosses both of the binary's outer
/// boundaries: the store it reads through the linked `onetaskgraph`, and the check
/// it starts as a child. The check does real work and says what it cost, and that
/// cost is the record's children CPU — the process's own is what the same check
/// costs it without the work.
#[test]
fn a_profiled_plan_check_times_its_store_read_and_its_check_and_counts_the_checks_cpu_apart() {
    let world = World::new("span-boundaries");
    let project = world.plan(
        "span-boundaries",
        &plan_of("span-boundaries", vec![agent("build", &[])]),
    );
    world.script("reviewer-bar.fingerprint", r#"{"refusals": []}"#);
    let check = double("reviewer-bar");
    let check = check.to_str().expect("a UTF-8 path");
    let args = ["plan", "check", &project, "--check", check, "--json"];

    let idle = world.root.join("spans-idle");
    run_profiled(&world, &world.root, Some(&idle), &args).exited(0);
    let idle = the_record(&idle);

    world.script("reviewer-bar.burn", "400");
    let busy = world.root.join("spans-busy");
    run_profiled(&world, &world.root, Some(&busy), &args).exited(0);
    let record = the_record(&busy);
    assert_eq!(record["verb"], "plan check", "{record:#}");
    spans_fit_the_process(&record);

    let store = names(&record)
        .into_iter()
        .find(|name| name.starts_with("onetaskgraph:"))
        .unwrap_or_else(|| panic!("no span of the store read: {record:#}"));
    let store = span(&record, &store).expect("the store span");
    assert!(figure(&store["count"]) >= 1.0, "{record:#}");
    let child = span(&record, "child:reviewer-bar")
        .unwrap_or_else(|| panic!("no span of the check it started: {record:#}"));
    assert_eq!(child["count"], 1, "{record:#}");

    let reported: Value = serde_json::from_str(
        &profile::fs::read_to_string(world.fakes.join("reviewer-bar.cpu"))
            .expect("the check said what it spent"),
    )
    .expect("a figure");
    if !cfg!(unix) {
        assert!(reported.is_null() && record["children_user_cpu_ms"].is_null());
        return;
    }
    let reported = figure(&reported);
    let cpu = |record: &Value, which: &str| {
        figure(&record[format!("{which}user_cpu_ms")])
            + figure(&record[format!("{which}system_cpu_ms")])
    };
    let children = cpu(&record, "children_");
    assert!(
        children >= reported,
        "the check spent {reported}ms and the record's children {children}ms: {record:#}"
    );
    let (own, own_idle) = (cpu(&record, ""), cpu(&idle, ""));
    assert!(
        own - own_idle < reported,
        "the process's own CPU grew from {own_idle}ms to {own}ms with a check that spent \
         {reported}ms, so the child's was counted as its own: {record:#}"
    );
    let waited = figure(&child["self_cpu_ms"]);
    assert!(
        waited < reported,
        "the span waiting on the check spent {waited}ms of the {reported}ms the check did: \
         {record:#}"
    );
}

/// `onepipeline host`'s phase line is the `host-view-time` budget's, and the span
/// record beside it changes none of it: with both switches set, the line has the
/// keys, phases and quantities it has with its own switch alone.
#[test]
fn the_host_phase_line_is_unchanged_beside_a_span_record() {
    let world = World::new("span-host-phases");
    let shape_of = |line: &str| {
        let line: Value = serde_json::from_str(line).expect("the phase line is JSON");
        let keys = |value: &Value| -> Vec<String> {
            value
                .as_object()
                .expect("an object")
                .keys()
                .cloned()
                .collect()
        };
        let phases: Vec<(String, Vec<String>)> = line["phases"]
            .as_object()
            .expect("phases")
            .iter()
            .map(|(name, phase)| (name.clone(), keys(phase)))
            .collect();
        (keys(&line), phases, keys(&line["quantities"]))
    };
    let mut lines = Vec::new();
    for spans in [None, Some(world.root.join("spans"))] {
        let phases = world.root.join(format!("phases-{}", spans.is_some()));
        let mut command = world.cmd(&["host"]);
        command.env(HOST_PHASES_ENV, &phases);
        match &spans {
            Some(dir) => command.env(SPANS_ENV, dir),
            None => command.env_remove(SPANS_ENV),
        };
        world.run_on(command, "host").exited(0);
        let text = profile::fs::read_to_string(&phases).expect("the phase line");
        let written: Vec<&str> = text.lines().collect();
        assert_eq!(written.len(), 1, "one read, one line: {text}");
        lines.push(shape_of(written[0]));
        if let Some(dir) = spans {
            assert_eq!(the_record(&dir)["verb"], "host");
        }
    }
    assert_eq!(
        lines[0], lines[1],
        "the phase line changed with the span profile on"
    );
}

/// A record that cannot be written is said on stderr, and costs the verb nothing:
/// it answers, and exits, exactly as it does with the profile off.
#[test]
fn a_span_record_that_cannot_be_written_leaves_the_verbs_answer_alone() {
    let world = World::new("span-unwritable");
    let in_the_way = world.root.join("spans-is-a-file");
    profile::fs::write(&in_the_way, "not a directory").expect("a file in the way");
    let profiled = run_profiled(&world, &world.root, Some(&in_the_way), &["runs"]);
    let unset = run_unset(&world, &world.root, &["runs"]);
    assert_eq!(
        (profiled.code, &profiled.stdout),
        (unset.code, &unset.stdout),
        "{}",
        profiled.stderr
    );
    assert!(
        profiled
            .stderr
            .contains("onepipeline: the span profile was not written to"),
        "{}",
        profiled.stderr
    );
    assert_eq!(
        profile::fs::read_to_string(&in_the_way).expect("the file in the way"),
        "not a directory"
    );
}
