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

use crate::harness::{agent, double, git, plan_of, repo_file, World};

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
const GOLDEN: &str = "tests/golden/test-profile-record-v3.json";

/// The figures `onepipeline_testfakes::profile` writes as `null` where a platform has
/// none: each program's CPU, the host's CPU count and its load. Every other field's type
/// is held exactly.
const NULLABLE: [&str; 4] = ["cpu_s", "nproc", "load_1m_start", "load_1m_end"];

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
                        (key, Value::Number(_) | Value::Null) if NULLABLE.contains(&key) => {
                            Value::from("number or null")
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

fn golden() -> Value {
    serde_json::from_str(
        &profile::fs::read_to_string(repo_file(GOLDEN)).expect("the golden record"),
    )
    .expect("the golden record is JSON")
}

fn has_the_goldens_shape(record: &Value) {
    assert_eq!(
        shape(record),
        shape(&golden()),
        "a record no longer has {GOLDEN}'s shape: bump `profile::SCHEMA` and the golden \
         together, and the reader with them: {record:#}"
    );
}

/// A record from a platform with no `getrusage` or load average — every Windows
/// record — carries `null` for each program's CPU and for the load, and a host that
/// cannot say how many CPUs it has carries `null` for `nproc`: that is the golden's
/// shape too. A `null` anywhere else, a figure of another type, or a missing key is not.
#[test]
fn a_record_with_a_platforms_missing_figures_has_the_goldens_shape_and_nothing_looser_does() {
    let world = World::new("profile-null-figures");
    let records = world.root.join("profiles");
    passed(BUSY, &run_alone(&["--exact", BUSY], &records));
    let mut record = read_record(&records.join("e2e").join(file_of(BUSY)));
    for field in ["nproc", "load_1m_start", "load_1m_end"] {
        record[field] = Value::Null;
    }
    for program in record["programs"]
        .as_object_mut()
        .expect("programs is an object")
        .values_mut()
    {
        program["cpu_s"] = Value::Null;
    }
    has_the_goldens_shape(&record);

    let golden = shape(&golden());
    let mut typed = record.clone();
    typed["load_1m_end"] = Value::from("high");
    let mut nulled = record.clone();
    nulled["wall_s"] = Value::Null;
    let mut program_nulled = record.clone();
    program_nulled["programs"]["cargo"]["s"] = Value::Null;
    let mut missing = record.clone();
    missing
        .as_object_mut()
        .expect("a record is an object")
        .remove("nproc");
    for (what, wrong) in [
        ("a nullable figure of another type", typed),
        ("a null wall clock", nulled),
        ("a null program wall clock", program_nulled),
        ("a missing nullable key", missing),
    ] {
        assert_ne!(shape(&wrong), golden, "{what} read as the golden's shape");
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

/// Set in the one child process that runs [`FAILING`] so it fails for real.
const FAIL_FOR_REAL: &str = "ONEPIPELINE_PROFILE_FIXTURE_FAILS";

/// The test [`FAILING`] names: it builds a world and then fails, which
/// `should_panic` makes a pass so this suite can run it — unless
/// [`FAIL_FOR_REAL`] is set, when it returns instead, and libtest fails it for
/// not panicking.
#[test]
#[should_panic(expected = "the profiled journey fails")]
fn a_profiled_journey_that_fails() {
    let _world = World::new("profile-failing");
    assert!(
        std::env::var_os(FAIL_FOR_REAL).is_some(),
        "the profiled journey fails"
    );
}

/// Run this suite's own binary the way nextest runs one test — alone, in a
/// process of its own — with its record redirected into `records`.
fn run_alone(args: &[&str], records: &std::path::Path) -> std::process::Output {
    let mut command = Command::new(std::env::current_exe().expect("this suite's own binary"));
    command
        .args(args)
        .arg("--nocapture")
        .env(profile::DIR_ENV, records)
        // The name nextest gave *this* test, which the child would otherwise read
        // as its own: what it runs is what `args` names.
        .env_remove("NEXTEST_TEST_NAME");
    if args.contains(&BUSY) {
        // This probe must observe Cargo even when the enclosing run already
        // built the doubles. Its independent run still uses the real builder.
        let run = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        command.env("NEXTEST_RUN_ID", format!("{run:x}"));
    }
    command.output().expect("the suite runs")
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
    let golden = golden();
    has_the_goldens_shape(&busy);
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

/// A test that builds a fixture with git nested inside it, then runs git and a
/// sleep on a thread it starts: every figure a record carries beyond `s` —
/// `inclusive_s`, `background_count`, `background_s` — has something to hold.
const LAYERED: &str = "profiling::a_profiled_journey_nests_work_and_works_on_a_thread_it_started";

/// Set in the one child process that runs [`LAYERED`]: where it writes the
/// totals it holds in memory once its work is done, for the record to be held to.
const SNAPSHOT_TO: &str = "ONEPIPELINE_PROFILE_FIXTURE_SNAPSHOT";

/// The test [`LAYERED`] names.
#[test]
fn a_profiled_journey_nests_work_and_works_on_a_thread_it_started() {
    let world = World::new("profile-layered");
    let checkout = world.root.join("background");
    profile::fs::create_dir_all(&checkout).expect("a directory to run git in");
    world.repository("local-direct", &[]);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            git(&world, &checkout, &["init", "--initial-branch=main"]);
            profile::thread::sleep(std::time::Duration::from_millis(5));
        });
    });
    drop(world);
    let Some(to) = std::env::var_os(SNAPSHOT_TO) else {
        return;
    };
    let totals = profile::snapshot();
    let categories: serde_json::Map<String, Value> = Category::ALL
        .iter()
        .map(|category| {
            let tally = totals.of(*category);
            (
                category.name().to_owned(),
                serde_json::json!({
                    "count": tally.count,
                    "s": tally.time.as_secs_f64(),
                    "inclusive_s": tally.inclusive.as_secs_f64(),
                    "background_count": tally.background_count,
                    "background_s": tally.background.as_secs_f64(),
                }),
            )
        })
        .collect();
    let programs: serde_json::Map<String, Value> = totals
        .programs
        .iter()
        .map(|(program, (count, time, cpu))| {
            (
                program.clone(),
                serde_json::json!({
                    "count": count,
                    "s": time.as_secs_f64(),
                    "cpu_s": cpu.map(|cpu| cpu.as_secs_f64()),
                }),
            )
        })
        .collect();
    // Through `std::fs` rather than `profile::fs`, so writing the totals down is
    // not itself a filesystem operation the record then holds and they do not.
    std::fs::write(
        to,
        serde_json::json!({ "categories": categories, "programs": programs }).to_string(),
    )
    .expect("the totals are written");
}

/// What a record says of each category is what the test held in memory when it
/// ended — its own and inclusive time, and the count and time of the work on a
/// thread it started — and so is each program's count and time; none of it is
/// zero where the test did that work. What it leaves unattributed is its wall
/// clock less every category's own time, the rest of the test's time.
#[test]
fn a_records_inclusive_and_background_figures_are_the_ones_the_test_measured() {
    let world = World::new("profile-layered-record");
    let records = world.root.join("profiles");
    let totals = world.root.join("totals.json");
    let ran = Command::new(std::env::current_exe().expect("this suite's own binary"))
        .args(["--exact", LAYERED, "--nocapture"])
        .env(profile::DIR_ENV, &records)
        .env(SNAPSHOT_TO, &totals)
        .env_remove("NEXTEST_TEST_NAME")
        .output()
        .expect("the suite runs");
    passed(LAYERED, &ran);
    let record = read_record(&records.join("e2e").join(file_of(LAYERED)));
    let measured = read_record(&totals);
    for category in Category::ALL {
        let name = category.name();
        for field in [
            "count",
            "s",
            "inclusive_s",
            "background_count",
            "background_s",
        ] {
            assert_eq!(
                record["categories"][name][field], measured["categories"][name][field],
                "{LAYERED}'s record says other than it measured of {name}.{field}: {record:#}"
            );
        }
    }
    assert_eq!(
        record["programs"], measured["programs"],
        "{LAYERED}'s record says other than it measured of the programs it ran: {record:#}"
    );
    let git = &record["programs"]["git"];
    assert!(
        git["count"].as_u64().unwrap_or_default() >= 1
            && git["s"].as_f64().unwrap_or_default() > 0.0,
        "the git the test ran was not recorded against it: {record:#}"
    );
    let figure = |category: &str, field: &str| {
        record["categories"][category][field]
            .as_f64()
            .unwrap_or_default()
    };
    assert!(
        figure("fixture", "inclusive_s") > figure("fixture", "s"),
        "the fixture's git was not inside its inclusive time: {record:#}"
    );
    for category in ["git", "wait"] {
        assert!(
            figure(category, "background_count") >= 1.0,
            "the started thread's {category} was not counted: {record:#}"
        );
        assert!(
            figure(category, "background_s") > 0.0,
            "the started thread's {category} took no time: {record:#}"
        );
    }
    let wall = record["wall_s"]
        .as_f64()
        .expect("a record's wall_s is a number");
    let attributed: f64 = Category::ALL
        .iter()
        .map(|category| figure(category.name(), "s"))
        .sum();
    let unattributed = record["unattributed_s"]
        .as_f64()
        .expect("a record's unattributed_s is a number");
    // A microsecond is far above the f64 rounding of a sum of seconds and far
    // below any error the writer could make in what it subtracts.
    assert!(
        (unattributed - (wall - attributed)).abs() < 1e-6,
        "{LAYERED}'s unattributed_s {unattributed} is not its wall_s {wall} less its \
         categories' {attributed}: {record:#}"
    );
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
    // `init`, `fast-import` and `clone`; the registration through the linked
    // `onevcs`; and the `remote get-url` that registration runs.
    assert_eq!(delta(Category::Git).0, 5, "git operations: {after:?}");
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

/// A test that fails leaves its record like any other: one whose panic is its
/// pass, and one the test binary itself reports failed.
#[test]
fn a_failing_journey_leaves_its_record_too() {
    let world = World::new("profile-failing-record");
    for fails in [false, true] {
        let records = world.root.join(format!("profiles-{fails}"));
        let mut child = Command::new(std::env::current_exe().expect("this suite's own binary"));
        child
            .args(["--exact", FAILING, "--nocapture"])
            .env(profile::DIR_ENV, &records)
            .env_remove("NEXTEST_TEST_NAME");
        if fails {
            child.env(FAIL_FOR_REAL, "1");
        }
        let ran = child.output().expect("the suite runs");
        assert_eq!(
            ran.status.success(),
            !fails,
            "{}{}",
            String::from_utf8_lossy(&ran.stdout),
            String::from_utf8_lossy(&ran.stderr)
        );
        let failing = read_record(&records.join("e2e").join(file_of(FAILING)));
        assert_eq!(failing["test"], FAILING, "{failing:#}");
        assert_eq!(failing["ended"], "test-thread", "{failing:#}");
        assert_eq!(failing["categories"]["fixture"]["count"], 1, "{failing:#}");
    }
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

/// A test name that is not a test's path — one that could reach outside the
/// record directory — writes nothing anywhere, and says so.
#[test]
fn a_test_name_that_is_not_a_tests_path_writes_no_record() {
    let world = World::new("profile-escaping");
    let records = world.root.join("profiles");
    let ran = Command::new(std::env::current_exe().expect("this suite's own binary"))
        .args(["--exact", IDLE, "--nocapture"])
        .env(profile::DIR_ENV, &records)
        .env("NEXTEST_TEST_NAME", "../../escaped")
        .output()
        .expect("the suite runs");
    passed(IDLE, &ran);
    let said = String::from_utf8_lossy(&ran.stderr);
    assert!(
        said.contains(r#"no test profile is written for "../../escaped""#),
        "{said}"
    );
    assert!(!records.exists(), "a record directory was made");
    assert!(
        !world.root.join("escaped.json").exists(),
        "a record escaped its directory"
    );
}

/// A fixture built on a thread the test started is counted as it would be on the
/// test thread, and its time is the fixture's once: what nested inside it is not
/// added to the background time again.
#[test]
fn nested_work_on_a_thread_the_test_started_is_counted_once() {
    let world = World::new("profile-background-nested");
    world.run(&["--version"]).exited(0);
    let before = profile::snapshot();
    std::thread::scope(|scope| {
        scope.spawn(|| world.repository("local-direct", &[]));
    });
    let after = profile::snapshot();
    let delta = |category: Category| {
        let (a, b) = (after.of(category), before.of(category));
        (
            a.background_count - b.background_count,
            a.background - b.background,
        )
    };
    let (fixtures, fixture_time) = delta(Category::Fixture);
    assert_eq!(fixtures, 1, "{after:?}");
    assert!(fixture_time > std::time::Duration::ZERO, "{after:?}");
    let (gits, git_time) = delta(Category::Git);
    assert_eq!(
        gits, 5,
        "the same git operations as on the test thread: {after:?}"
    );
    assert_eq!(
        git_time,
        std::time::Duration::ZERO,
        "git inside the fixture is in the fixture's time already: {after:?}"
    );
    assert_eq!(
        after.of(Category::Fixture).count,
        before.of(Category::Fixture).count
    );
}

/// Every filesystem operation the profile replaces is one operation of `fs`,
/// whether it succeeds or is refused.
#[test]
fn each_filesystem_operation_counts_once_whether_or_not_it_succeeds() {
    use profile::fs;

    let world = World::new("profile-fs");
    let at = world.root.join("fs");
    let before = profile::snapshot();
    fs::create_dir(&at).expect("create_dir");
    fs::create_dir_all(at.join("a/b")).expect("create_dir_all");
    fs::write(at.join("one"), "one").expect("write");
    assert_eq!(fs::read(at.join("one")).expect("read"), b"one");
    assert_eq!(
        fs::read_to_string(at.join("one")).expect("read_to_string"),
        "one"
    );
    fs::copy(at.join("one"), at.join("two")).expect("copy");
    fs::rename(at.join("two"), at.join("three")).expect("rename");
    fs::hard_link(at.join("one"), at.join("linked")).expect("hard_link");
    assert!(fs::metadata(at.join("linked")).expect("metadata").is_file());
    fs::canonicalize(&at).expect("canonicalize");
    assert_eq!(fs::read_dir(&at).expect("read_dir").count(), 4);
    fs::remove_file(at.join("linked")).expect("remove_file");
    fs::remove_dir(at.join("a/b")).expect("remove_dir");
    fs::remove_dir_all(at.join("a")).expect("remove_dir_all");
    assert!(
        fs::read(at.join("never-written")).is_err(),
        "a read of nothing fails"
    );
    let after = profile::snapshot();

    let (a, b) = (after.of(Category::Fs), before.of(Category::Fs));
    assert_eq!(a.count - b.count, 15, "{after:?}");
    assert!(a.time > b.time, "{after:?}");
}

/// A process run to its status is one process, held for all of it; and a
/// program named by its path is the program it is, so a git found on `PATH` and
/// named in full is still git.
#[test]
fn a_status_and_a_program_named_by_its_path_are_counted_as_what_they_are() {
    let world = World::new("profile-status");
    world.run(&["--version"]).exited(0);
    let git_on_path = std::env::split_paths(&std::env::var_os("PATH").expect("a PATH"))
        .map(|dir| dir.join(format!("git{}", std::env::consts::EXE_SUFFIX)))
        .find(|candidate| candidate.is_file())
        .expect("git is on PATH");
    let before = profile::snapshot();
    let status = world
        .cmd(&["--version"])
        .stdout(std::process::Stdio::null())
        .status()
        .expect("the binary runs");
    assert!(status.success());
    let named = Command::new(&git_on_path)
        .arg("--version")
        .output()
        .expect("git runs");
    assert!(named.status.success());
    let after = profile::snapshot();

    let delta = |category: Category| after.of(category).count - before.of(category).count;
    assert_eq!(delta(Category::Spawn), 1, "{after:?}");
    assert_eq!(
        delta(Category::Git),
        1,
        "{} is git: {after:?}",
        git_on_path.display()
    );
    let held = |program: &str| {
        let of = |s: &profile::Snapshot| s.programs.get(program).copied().unwrap_or_default();
        (of(&after).0 - of(&before).0, of(&after).1 - of(&before).1)
    };
    let (count, time) = held("onepipeline");
    assert_eq!(count, 1, "{after:?}");
    assert_eq!(
        time,
        after.of(Category::Spawn).time - before.of(Category::Spawn).time,
        "the status is all of the spawn time: {after:?}"
    );
    assert_eq!(held("git").0, 1, "{after:?}");
}

/// A record carries the run and attempt nextest names, and none when a test
/// binary is run by hand.
#[test]
fn a_record_names_the_run_and_attempt_it_belongs_to() {
    let world = World::new("profile-run-id");
    for (run, attempt) in [(Some("a-run-of-this-suite"), Some("2")), (None, None)] {
        let records = world.root.join(format!("profiles-{}", run.is_some()));
        let mut child = Command::new(std::env::current_exe().expect("this suite's own binary"));
        child
            .args(["--exact", IDLE, "--nocapture"])
            .env(profile::DIR_ENV, &records)
            .env_remove("NEXTEST_TEST_NAME")
            .env_remove("NEXTEST_RUN_ID")
            .env_remove("NEXTEST_ATTEMPT");
        if let (Some(run), Some(attempt)) = (run, attempt) {
            child
                .env("NEXTEST_RUN_ID", run)
                .env("NEXTEST_ATTEMPT", attempt);
        }
        passed(IDLE, &child.output().expect("the suite runs"));
        let record = read_record(&records.join("e2e").join(file_of(IDLE)));
        assert_eq!(record["run_id"], serde_json::json!(run), "{record:#}");
        assert_eq!(
            record["attempt"],
            serde_json::json!(attempt.map(|a| a.parse::<u64>().expect("a number"))),
            "{record:#}"
        );
    }
}

/// Set in the one child process that runs [`HANGING`] so it hangs.
const HANG_FOR_REAL: &str = "ONEPIPELINE_PROFILE_FIXTURE_HANGS";

/// A test that builds a world and then, in the child that sets
/// [`HANG_FOR_REAL`], never returns — as a test nextest ends at its
/// `terminate-after` does not.
const HANGING: &str = "profiling::a_profiled_journey_that_hangs_until_it_is_ended";

/// The test [`HANGING`] names. Run by the suite itself it builds its world and
/// returns.
#[test]
fn a_profiled_journey_that_hangs_until_it_is_ended() {
    let world = World::new("profile-hanging");
    if std::env::var_os(HANG_FOR_REAL).is_some() {
        world.until_within(std::time::Duration::from_secs(3600), "nothing", |_| false);
    }
}

/// A test ended by the runner before it finishes leaves a record all the same:
/// the heartbeat's, marked `unfinished`, holding what it had spent.
#[test]
fn a_journey_ended_before_it_finishes_leaves_an_unfinished_record() {
    let world = World::new("profile-ended");
    let records = world.root.join("profiles");
    let path = records.join("e2e").join(file_of(HANGING));
    let mut hanging = Command::new(std::env::current_exe().expect("this suite's own binary"))
        .args(["--exact", HANGING, "--nocapture"])
        .env(profile::DIR_ENV, &records)
        .env(HANG_FOR_REAL, "1")
        .env_remove("NEXTEST_TEST_NAME")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the hanging journey starts");
    // A heartbeat taken once the journey is polling: its world built, its wait
    // under way.
    world.until_within(
        profile::HEARTBEAT * 6,
        "a heartbeat from inside the hanging journey's wait",
        |_| {
            profile::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .is_some_and(|record| record["categories"]["wait"]["s"].as_f64() > Some(0.0))
        },
    );
    hanging.kill().expect("the hanging journey is ended");
    assert!(!hanging.wait().expect("it is reaped").success());

    let record = read_record(&path);
    assert_eq!(record["test"], HANGING, "{record:#}");
    assert_eq!(record["ended"], "unfinished", "{record:#}");
    assert_eq!(record["categories"]["fixture"]["count"], 1, "{record:#}");
    assert!(record["wall_s"].as_f64() > Some(0.0), "{record:#}");
}

/// The check [`a_profiled_journey_that_runs_onepipeline_verbs`] starts spends the CPU
/// its script asks for and says what it spent, and refuses, by name and with its
/// double's failure code, a burn that is no count and a CPU file it cannot write.
#[test]
fn the_working_check_reports_its_cpu_and_refuses_what_it_cannot_do() {
    let world = World::new("reviewer-bar-burn");
    world.script("reviewer-bar.fingerprint", "bar");
    let bar = || {
        let mut command = Command::new(double("reviewer-bar"));
        command.env(onepipeline_testfakes::SCRIPT_DIR_ENV, &world.fakes);
        command.output().expect("the double runs")
    };
    let cpu = world.fakes.join("reviewer-bar.cpu");

    world.script("reviewer-bar.burn", "20");
    let ran = bar();
    assert!(ran.status.success(), "{ran:?}");
    assert_eq!(ran.stdout, b"bar\n");
    let spent: Value =
        serde_json::from_str(&std::fs::read_to_string(&cpu).expect("the CPU it spent"))
            .expect("JSON");
    if cfg!(unix) {
        let ms = spent
            .as_f64()
            .expect("a count of milliseconds on a platform with getrusage");
        assert!(ms >= 20.0, "it spent the CPU it was asked to: {ms}");
    } else {
        assert!(spent.is_null(), "{spent}");
    }
    std::fs::remove_file(&cpu).expect("the CPU file goes");

    world.script("reviewer-bar.burn", "soon");
    let ran = bar();
    assert_eq!(ran.status.code(), Some(78), "{ran:?}");
    assert!(
        String::from_utf8_lossy(&ran.stderr)
            .contains("reviewer-bar.burn is not a count of milliseconds"),
        "{ran:?}"
    );
    assert!(ran.stdout.is_empty(), "{ran:?}");
    assert!(!cpu.exists(), "a refused burn writes no CPU");

    world.script("reviewer-bar.burn", "1");
    std::fs::create_dir(&cpu).expect("a directory where the CPU file goes");
    let ran = bar();
    assert_eq!(ran.status.code(), Some(78), "{ran:?}");
    assert!(
        String::from_utf8_lossy(&ran.stderr).contains("reviewer-bar.cpu is not written"),
        "{ran:?}"
    );
    assert!(ran.stdout.is_empty(), "{ran:?}");
}

/// A test that runs `onepipeline` verbs crossing the binary's outer boundaries: a
/// plan check reading the store and starting a registered check that does real
/// work, twice, and a listing of runs.
const SPANNED: &str = "profiling::a_profiled_journey_that_runs_onepipeline_verbs";

/// Set in the one child process that runs [`SPANNED`]: a directory it has two more
/// `onepipeline` runs write their own span records into, by setting the binary's
/// variable itself — and one more run where it removes the variable.
const OWN_SPANS: &str = "ONEPIPELINE_PROFILE_FIXTURE_OWN_SPANS";

#[test]
fn a_profiled_journey_that_runs_onepipeline_verbs() {
    let world = World::new("profile-spanned");
    let project = world.plan(
        "profile-spanned",
        &plan_of("profile-spanned", vec![agent("build", &[])]),
    );
    world.script("reviewer-bar.fingerprint", r#"{"refusals": []}"#);
    world.script("reviewer-bar.burn", "50");
    let check = double("reviewer-bar");
    let check = check.to_str().expect("a UTF-8 path");
    for _ in 0..2 {
        world
            .run(&["plan", "check", &project, "--check", check, "--json"])
            .exited(0);
    }
    world.run(&["runs"]).exited(0);
    let Some(own) = std::env::var_os(OWN_SPANS) else {
        return;
    };
    for _ in 0..2 {
        let mut command = world.cmd(&["runs"]);
        command.env("ONEPIPELINE_PROFILE_SPANS", &own);
        world.run_on(command, "runs").exited(0);
    }
    let mut command = world.cmd(&["runs"]);
    command.env_remove("ONEPIPELINE_PROFILE_SPANS");
    world.run_on(command, "runs").exited(0);
}

fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("{value} is not a number"))
}

/// Under [`profile::SPANS_ENV`], every `onepipeline` a test starts writes its span
/// record into that test's own directory beside its record — one per process — and
/// the record is schema 3, carrying each program's CPU and the host's load. The
/// report folds those span records into the test's module, per verb: each verb's
/// runs, wall and CPU are the sums of its records', and the store-read and
/// child-process spans keep their names and their summed walls.
#[test]
fn a_journeys_onepipeline_runs_leave_their_span_records_with_it_and_the_report_folds_them() {
    let world = World::new("profile-spans");
    let records = world.root.join("profiles");
    let spans_dir = records
        .join("e2e")
        .join(file_of(SPANNED).replace(".json", ".spans"));
    // What an earlier attempt of the test left, which its first spawn clears.
    profile::fs::create_dir_all(&spans_dir).expect("an earlier attempt's span directory");
    let earlier = spans_dir.join("onepipeline-1-1.json");
    profile::fs::write(&earlier, "an earlier attempt's").expect("an earlier attempt's record");
    let own = world.root.join("own-spans");
    let ran = Command::new(std::env::current_exe().expect("this suite's own binary"))
        .args(["--exact", SPANNED, "--nocapture"])
        .env(profile::DIR_ENV, &records)
        .env(profile::SPANS_ENV, "1")
        .env(OWN_SPANS, &own)
        .env_remove("ONEPIPELINE_PROFILE_SPANS")
        .env_remove("NEXTEST_TEST_NAME")
        .output()
        .expect("the suite runs");
    passed(SPANNED, &ran);

    let record = read_record(&records.join("e2e").join(file_of(SPANNED)));
    assert_eq!(record["schema"], 3, "{record:#}");
    let started = &record["programs"]["onepipeline"];
    assert_eq!(started["count"], 6, "{record:#}");
    if cfg!(unix) {
        assert!(number(&started["cpu_s"]) > 0.0, "{record:#}");
        assert!(number(&record["load_1m_start"]) >= 0.0, "{record:#}");
        assert!(number(&record["load_1m_end"]) >= 0.0, "{record:#}");
    } else {
        assert!(started["cpu_s"].is_null(), "{record:#}");
    }
    assert!(number(&record["nproc"]) >= 1.0, "{record:#}");

    assert!(!earlier.exists(), "the earlier attempt's record was kept");
    let kept = profile::fs::read_dir(&own)
        .expect("the directory the test named itself")
        .count();
    assert_eq!(
        kept, 2,
        "a run the test pointed elsewhere was not left there"
    );
    let spans: Vec<Value> = profile::fs::read_dir(&spans_dir)
        .expect("the test's span directory")
        .map(|entry| read_record(&entry.expect("a span record").path()))
        .collect();
    assert_eq!(
        spans.len(),
        3,
        "one span record per onepipeline the test started and left the switch to, and none \
         where it set the variable itself or removed it: {spans:#?}"
    );

    let out = world.root.join("report.json");
    let reported = Command::new("node")
        .arg(repo_file("scripts/test-profile-report.mjs"))
        .args(["--json", "--top-spans", "1000", "--out"])
        .arg(&out)
        .arg(format!("linux={}", records.display()))
        .output()
        .expect("node runs the report");
    assert!(
        reported.status.success(),
        "{}",
        String::from_utf8_lossy(&reported.stderr)
    );
    let report = read_record(&out);
    let folded = &report["platforms"][0]["spans"];
    assert_eq!(folded["records"], 3, "{folded:#}");
    let module = folded["modules"]
        .as_array()
        .expect("modules")
        .iter()
        .find(|module| module["name"] == "e2e::profiling")
        .unwrap_or_else(|| panic!("the test's module is not in the fold: {folded:#}"));
    let close = |a: f64, b: f64, what: &str| {
        assert!(
            (a - b).abs() <= 1e-6 * b.abs().max(1.0),
            "{what}: the fold says {a} and the records {b}: {module:#}"
        );
    };
    for verb in ["plan check", "runs"] {
        let theirs: Vec<&Value> = spans.iter().filter(|span| span["verb"] == verb).collect();
        let folded = module["verbs"]
            .as_array()
            .expect("verbs")
            .iter()
            .find(|folded| folded["verb"] == verb)
            .unwrap_or_else(|| panic!("`{verb}` is not in the fold: {module:#}"));
        assert_eq!(folded["invocations"], theirs.len(), "{module:#}");
        let sum =
            |field: &dyn Fn(&Value) -> f64| theirs.iter().map(|span| field(span)).sum::<f64>();
        close(
            number(&folded["wall_ms"]),
            sum(&|span| number(&span["wall_ms"])),
            "wall",
        );
        if cfg!(unix) {
            close(
                number(&folded["cpu_ms"]),
                sum(&|span| number(&span["user_cpu_ms"]) + number(&span["system_cpu_ms"])),
                "cpu",
            );
        }
        let named = |name: &str| -> Vec<&Value> {
            theirs
                .iter()
                .flat_map(|span| span["spans"].as_array().expect("spans"))
                .filter(|span| span["name"] == name)
                .collect()
        };
        let boundaries: Vec<String> = theirs
            .iter()
            .flat_map(|span| span["spans"].as_array().expect("spans"))
            .filter_map(|span| span["name"].as_str())
            .filter(|name| name.starts_with("child:") || name.starts_with("onetaskgraph:"))
            .map(str::to_owned)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if verb == "plan check" {
            assert!(
                boundaries.iter().any(|name| name == "child:reviewer-bar")
                    && boundaries
                        .iter()
                        .any(|name| name.starts_with("onetaskgraph:")),
                "`plan check` crossed no library call and no child process: {boundaries:?}"
            );
        }
        for name in boundaries {
            let held = folded["spans"]
                .as_array()
                .expect("a verb's spans")
                .iter()
                .find(|span| span["name"] == name.as_str())
                .unwrap_or_else(|| panic!("{name} lost its name in the fold: {folded:#}"));
            for field in ["self_wall_ms", "wall_ms"] {
                close(
                    number(&held[field]),
                    named(&name).iter().map(|span| number(&span[field])).sum(),
                    &format!("{name}.{field}"),
                );
            }
        }
    }
}
