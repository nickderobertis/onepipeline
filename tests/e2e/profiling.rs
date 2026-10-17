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

/// A test that fails, whose record is written all the same.
const FAILING: &str = "profiling::a_profiled_journey_that_fails";

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

/// The test [`IDLE`] names, which reaches nothing the profile records — so
/// nothing is recorded, on any thread.
#[test]
fn a_profiled_journey_that_does_nothing() {
    let recorded = profile::snapshot();
    for category in Category::ALL {
        assert_eq!(
            recorded.of(category),
            profile::Tally::default(),
            "{recorded:?}"
        );
    }
    assert_eq!(recorded.polls, 0, "{recorded:?}");
    assert!(recorded.programs.is_empty(), "{recorded:?}");
}

/// The test [`FAILING`] names: it fails after building a world, which is the
/// verdict `should_panic` turns into a pass so this suite can run it.
#[test]
#[should_panic(expected = "the profiled journey fails")]
fn a_profiled_journey_that_fails() {
    let _world = World::new("profile-failing");
    panic!("the profiled journey fails");
}

/// Run this suite's own binary the way nextest runs one test — alone, in a
/// process of its own — with its record redirected into `records`.
fn run_alone(args: &[&str], records: &std::path::Path) -> std::process::Output {
    Command::new(std::env::current_exe().expect("this suite's own binary"))
        .args(args)
        .arg("--nocapture")
        .env(profile::DIR_ENV, records)
        // The name nextest gave *this* test, which the child would otherwise read
        // as its own: what it runs is what `args` names.
        .env_remove("NEXTEST_TEST_NAME")
        .output()
        .expect("the suite runs")
}

fn passed(test: &str, ran: &std::process::Output) {
    assert!(
        ran.status.success(),
        "{test} failed: {}{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
}

fn file_of(test: &str) -> String {
    format!("{}.json", test.replace("::", "."))
}

fn read_record(path: &std::path::Path) -> Value {
    let text = profile::fs::read_to_string(path).expect("the record is readable");
    serde_json::from_str(&text).expect("the record is JSON")
}

/// Each test is run alone in a process of its own, as nextest runs it, with its
/// record redirected into this world.
#[test]
fn a_journeys_record_carries_what_it_spent_time_on_and_its_neighbours_does_not() {
    let world = World::new("profile-records");
    let records = world.root.join("profiles");
    for test in [BUSY, IDLE] {
        passed(test, &run_alone(&["--exact", test], &records));
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
    assert_eq!(
        written,
        vec![file_of(BUSY), file_of(IDLE)],
        "one record per test, and no other"
    );
    let record = |test: &str| read_record(&records.join("e2e").join(file_of(test)));

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

/// A fixture's own time is what it spent outside the work nested in it, and the
/// work nested in it is recorded where it happened: the two add up to the
/// fixture's whole time to the nanosecond, so no moment is in two categories.
#[test]
fn a_fixtures_nested_git_and_file_work_is_its_own_and_not_the_fixtures() {
    let world = World::new("profile-nested");
    // Built before the count starts, as in the journey above.
    world.run(&["--version"]).exited(0);
    let before = profile::snapshot();
    world.repository("local-direct", &[]);
    let after = profile::snapshot();

    let delta = |category: Category| {
        let (a, b) = (after.of(category), before.of(category));
        (
            a.count - b.count,
            a.time - b.time,
            a.inclusive - b.inclusive,
        )
    };
    let (fixtures, own, whole) = delta(Category::Fixture);
    assert_eq!(fixtures, 1, "one repository is one fixture: {after:?}");
    let nested: std::time::Duration = Category::ALL
        .iter()
        .filter(|category| **category != Category::Fixture)
        .map(|category| delta(*category).1)
        .sum();
    assert!(nested > std::time::Duration::ZERO, "{after:?}");
    assert_eq!(
        own + nested,
        whole,
        "the fixture's time and its nested work: {after:?}"
    );
    // `init`, `clone`, `add`, `commit` and `push`; the registration through the
    // linked `onevcs`; and the `remote get-url` that registration runs.
    assert_eq!(delta(Category::Git).0, 7, "git operations: {after:?}");
}

/// A poll inside a poll is one polling wait, and every sleep in either counts.
#[test]
fn a_poll_inside_a_poll_is_one_poll_and_every_sleep_is_counted() {
    let world = World::new("profile-polls");
    let before = profile::snapshot();
    let mut outer = 0;
    world.until("the outer second look", |world| {
        outer += 1;
        let mut inner = 0;
        world.until("the inner second look", |_| {
            inner += 1;
            inner == 2
        });
        outer == 2
    });
    let after = profile::snapshot();
    assert_eq!(after.polls - before.polls, 1, "{after:?}");
    // One sleep in each of the two inner polls, and one between the outer looks.
    let sleeps = after.of(Category::Wait).count - before.of(Category::Wait).count;
    assert_eq!(sleeps, 3, "{after:?}");
}

/// Work on a thread the test started is counted, and its time is kept apart from
/// the test thread's, which it overlaps.
#[test]
fn a_thread_the_test_started_is_counted_apart_from_the_test_thread() {
    let world = World::new("profile-background");
    let checkout = world.root.join("background");
    profile::fs::create_dir_all(&checkout).expect("a directory to run git in");
    let before = profile::snapshot();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            git(&world, &checkout, &["init", "--initial-branch=main"]);
            profile::thread::sleep(std::time::Duration::from_millis(5));
        });
    });
    let after = profile::snapshot();
    for category in [Category::Git, Category::Wait] {
        let (a, b) = (after.of(category), before.of(category));
        assert_eq!(a.background_count - b.background_count, 1, "{after:?}");
        assert!(a.background > b.background, "{after:?}");
        assert_eq!(
            a.count, b.count,
            "the test thread did none of it: {after:?}"
        );
        assert_eq!(a.time, b.time, "the test thread did none of it: {after:?}");
    }
}

/// A process started and then waited on is one process, held for both; one that
/// could not be started is still one attempt.
#[test]
fn a_process_waited_on_is_counted_once_and_held_for_the_wait() {
    let world = World::new("profile-waits");
    world.run(&["--version"]).exited(0);
    let before = profile::snapshot();
    let mut waited = world
        .cmd(&["--version"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("the binary starts");
    assert!(waited.wait().expect("it exits").success());
    let collected = world
        .cmd(&["--version"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the binary starts")
        .wait_with_output()
        .expect("it exits");
    assert!(collected.status.success());
    let refused = Command::new(world.root.join("no-such-program")).output();
    assert!(
        refused.is_err(),
        "a program that is not there does not start"
    );
    let after = profile::snapshot();

    let started = |program: &str| {
        let of = |snapshot: &profile::Snapshot| {
            snapshot.programs.get(program).copied().unwrap_or_default()
        };
        let (a, b) = (of(&after), of(&before));
        (a.0 - b.0, a.1 - b.1)
    };
    let (count, held) = started("onepipeline");
    assert_eq!(count, 2, "two processes, each counted once: {after:?}");
    let spawned = after.of(Category::Spawn).time - before.of(Category::Spawn).time;
    assert!(
        held > std::time::Duration::ZERO && held <= spawned,
        "{after:?}"
    );
    assert_eq!(started("no-such-program").0, 1, "{after:?}");
    assert_eq!(
        after.of(Category::Spawn).count - before.of(Category::Spawn).count,
        3,
        "{after:?}"
    );
}

/// A failing test's record is written when its thread ends, like any other.
#[test]
fn a_failing_journey_leaves_its_record_too() {
    let world = World::new("profile-failing-record");
    let records = world.root.join("profiles");
    passed(FAILING, &run_alone(&["--exact", FAILING], &records));
    let failing = read_record(&records.join("e2e").join(file_of(FAILING)));
    assert_eq!(failing["test"], FAILING, "{failing:#}");
    assert_eq!(failing["ended"], "test-thread", "{failing:#}");
    assert_eq!(failing["categories"]["fixture"]["count"], 1, "{failing:#}");
}

/// A process running several tests writes nothing: no one test owns its totals.
#[test]
fn a_process_running_several_journeys_writes_no_record() {
    let world = World::new("profile-several");
    let records = world.root.join("profiles");
    // Two tests that say nothing about what else their process did.
    let ran = run_alone(&["--exact", BUSY, FAILING], &records);
    passed("the profiled journeys", &ran);
    assert!(
        String::from_utf8_lossy(&ran.stdout).contains("2 passed"),
        "{}",
        String::from_utf8_lossy(&ran.stdout)
    );
    assert!(
        !records.exists(),
        "a record was written for a process of several tests"
    );
}

/// A record that cannot be written is said, and the test's verdict stands.
#[test]
fn a_record_that_cannot_be_written_leaves_the_verdict_alone() {
    let world = World::new("profile-unwritable");
    let records = world.root.join("profiles-is-a-file");
    profile::fs::write(&records, "not a directory").expect("a file in the way");
    let ran = run_alone(&["--exact", IDLE], &records);
    passed(IDLE, &ran);
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(
        said.contains(&format!("the test profile for {IDLE} was not written")),
        "{said}"
    );
}

/// A test run again replaces its record rather than leaving two.
#[test]
fn a_journey_run_again_replaces_its_record() {
    let world = World::new("profile-again");
    let records = world.root.join("profiles");
    let path = records.join("e2e").join(file_of(IDLE));
    passed(IDLE, &run_alone(&["--exact", IDLE], &records));
    let first = read_record(&path);
    passed(IDLE, &run_alone(&["--exact", IDLE], &records));
    let second = read_record(&path);
    let written: Vec<_> = profile::fs::read_dir(records.join("e2e"))
        .expect("the records")
        .collect();
    assert_eq!(written.len(), 1, "one record for one test");
    assert!(
        second["started_at"].as_f64() > first["started_at"].as_f64(),
        "the second run's record replaced the first's: {first:#} {second:#}"
    );
}
