//! Every test leaves a record of where its wall clock went, and the record says
//! what the test did.
//!
//! The record is `onepipeline_testfakes::profile`'s, written into
//! `target/test-profile/<binary>/` by every test process; these journeys hold it
//! to the work they did, counted exactly where a count is knowable and by
//! category where only a duration is.

use onepipeline_testfakes::profile::process::Command;
use onepipeline_testfakes::profile::{self, Category};
use serde_json::Value;

use crate::harness::{double, git, repo_file, World};

/// The test whose record [`a_journeys_record_carries_what_it_spent_time_on_and_its_neighbours_does_not`]
/// reads for the four categories it spends time in.
const BUSY: &str =
    "profiling::a_profiled_journey_builds_a_fixture_and_a_binary_then_polls_and_writes";

/// Its neighbour, which does nothing at all.
const IDLE: &str = "profiling::a_profiled_journey_that_does_nothing";

/// The record's shape, which `scripts/test-profile-report.mjs` reads: a record
/// written here is held to it, and that script's tests are held to it too, so a
/// field renamed on one side fails on that side.
const GOLDEN: &str = "tests/golden/test-profile-record-v1.json";

/// What a record says, without the values: each object's keys and each leaf's
/// JSON type, with `programs` — one entry per program a test happened to run —
/// reduced to the shape of an entry.
fn shape(value: &Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, field)| {
                    let reduced = match (key.as_str(), field) {
                        ("programs", Value::Object(programs)) => programs.values().next().map_or(
                            Value::Null,
                            |entry| serde_json::json!({ "*": shape(entry) }),
                        ),
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

#[test]
fn a_journeys_processes_git_commands_and_sleeps_are_each_counted_once() {
    let world = World::new("profile-counts");
    // The first command a world runs builds the doubles it wires in, which is
    // the `build` category's; after it, a command is one process and nothing
    // else.
    world.run(&["--version"]).exited(0);
    let checkout = world.root.join("counted");
    profile::fs::create_dir_all(&checkout).expect("a directory to run git in");

    let before = profile::snapshot();
    for _ in 0..3 {
        world.run(&["--version"]).exited(0);
    }
    git(&world, &checkout, &["init", "--initial-branch=main"]);
    git(&world, &checkout, &["status", "--short"]);
    // A wait whose condition holds at its fourth look sleeps between the looks:
    // three times.
    let mut looks = 0;
    world.until("the fourth look", |_| {
        looks += 1;
        looks == 4
    });
    let after = profile::snapshot();

    let counted = |category: Category| after.of(category).count - before.of(category).count;
    assert_eq!(counted(Category::Spawn), 3, "processes started: {after:?}");
    assert_eq!(counted(Category::Git), 2, "git commands run: {after:?}");
    assert_eq!(counted(Category::Wait), 3, "sleeps: {after:?}");
    assert_eq!(after.polls - before.polls, 1, "polling waits: {after:?}");
    assert_eq!(counted(Category::Build), 0, "builds: {after:?}");
    let started = |program: &str| {
        let of = |snapshot: &profile::Snapshot| snapshot.programs.get(program).map_or(0, |p| p.0);
        of(&after) - of(&before)
    };
    assert_eq!(started("onepipeline"), 3, "onepipeline started: {after:?}");
    assert_eq!(started("git"), 2, "git started: {after:?}");
}

/// The test [`BUSY`] names: a world, a built binary, a poll and files written.
#[test]
fn a_profiled_journey_builds_a_fixture_and_a_binary_then_polls_and_writes() {
    let world = World::new("profile-busy");
    assert!(double("fake-gh").is_file(), "the built double is there");
    world.script("profiled.note", "written by the profiled journey");
    profile::fs::write(world.root.join("profiled.txt"), "written").expect("a file is written");
    let mut looks = 0;
    world.until("the third look", |_| {
        looks += 1;
        looks == 3
    });
}

/// The test [`IDLE`] names, which reaches nothing the profile records.
#[test]
fn a_profiled_journey_that_does_nothing() {}

/// Each test is run alone in a process of its own, as nextest runs it, with its
/// record redirected into this world.
#[test]
fn a_journeys_record_carries_what_it_spent_time_on_and_its_neighbours_does_not() {
    let world = World::new("profile-records");
    let records = world.root.join("profiles");
    let suite = std::env::current_exe().expect("this suite's own binary");
    for test in [BUSY, IDLE] {
        let ran = Command::new(&suite)
            .args(["--exact", test, "--nocapture"])
            .env(profile::DIR_ENV, &records)
            // The name nextest gave *this* test, which the child would otherwise
            // read as its own: what it runs is the one `--exact` names.
            .env_remove("NEXTEST_TEST_NAME")
            .output()
            .expect("the suite runs one of its tests");
        assert!(
            ran.status.success(),
            "{test} failed: {}{}",
            String::from_utf8_lossy(&ran.stdout),
            String::from_utf8_lossy(&ran.stderr)
        );
    }

    let mut written: Vec<String> = profile::fs::read_dir(records.join("e2e"))
        .expect("the records were written under the binary's name")
        .map(|entry| {
            entry
                .expect("a record")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    written.sort();
    let file = |test: &str| format!("{}.json", test.replace("::", "."));
    assert_eq!(
        written,
        vec![file(BUSY), file(IDLE)],
        "one record per test, and no other"
    );
    let record = |test: &str| -> Value {
        let text = profile::fs::read_to_string(records.join("e2e").join(file(test)))
            .expect("the record is readable");
        serde_json::from_str(&text).expect("the record is JSON")
    };

    let busy = record(BUSY);
    let golden: Value = serde_json::from_str(
        &profile::fs::read_to_string(repo_file(GOLDEN)).expect("the golden record"),
    )
    .expect("the golden record is JSON");
    assert_eq!(
        shape(&busy),
        shape(&golden),
        "a record no longer has {GOLDEN}'s shape: bump `profile::SCHEMA` and the golden \
         together, and the reader with them"
    );
    assert_eq!(busy["test"], BUSY, "{busy:#}");
    assert_eq!(busy["schema"], profile::SCHEMA, "{busy:#}");
    assert_eq!(
        golden["schema"],
        profile::SCHEMA,
        "{GOLDEN} is another schema's"
    );
    assert_eq!(busy["ended"], "test-thread", "{busy:#}");
    for category in ["fixture", "build", "wait", "fs"] {
        let spent = busy["categories"][category]["s"]
            .as_f64()
            .unwrap_or_default();
        assert!(spent > 0.0, "{BUSY} spent no time in {category}: {busy:#}");
    }
    assert!(
        busy["categories"]["wait"]["polls"].as_u64() >= Some(1),
        "{busy:#}"
    );
    let wall = busy["wall_s"].as_f64().expect("a wall clock");
    let attributed: f64 = Category::ALL
        .iter()
        .map(|category| {
            busy["categories"][category.name()]["s"]
                .as_f64()
                .unwrap_or_default()
        })
        .sum();
    assert!(
        attributed <= wall,
        "the categories sum to {attributed}s of a {wall}s test, so one moment was counted twice: \
         {busy:#}"
    );

    let idle = record(IDLE);
    assert_eq!(idle["test"], IDLE, "{idle:#}");
    assert_eq!(idle["ended"], "process-exit", "{idle:#}");
    for category in Category::ALL {
        let tally = &idle["categories"][category.name()];
        assert_eq!(
            tally["count"],
            0,
            "{IDLE} counted {}: {idle:#}",
            category.name()
        );
        assert_eq!(
            tally["s"],
            0.0,
            "{IDLE} spent time in {}: {idle:#}",
            category.name()
        );
    }
}
