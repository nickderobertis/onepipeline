//! The wake budget, the watch-terms record and the closure rule — entry 96 of
//! `docs/contract-divergences.md` — driven through the compiled binary.
//!
//! What this exists for is the question a supervising session actually needs
//! answered at the end of its turn: will each run it owes wake it within its wake
//! budget? `unwatched` and `stop-guard` answered only whether a watch process
//! existed, so a watch armed with no deadline counted, and a run nothing was
//! driving — a dead driver, a parked node, a failure hook's ending — owed nothing.
//!
//! Every journey drives real run directories, and a real `onepipeline watch`
//! process holds every lease that is the subject of one. Each case launches its
//! run under a session of its own, so the question asked about it is answered
//! about that run alone. The only files written by hand are the ones no verb
//! produces — a terms record an older engine never wrote, one a writer left
//! half-written, one naming another watch — each marked where it is written.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven here as a
// real compiled binary against a real run store; `harness.rs` carries the same suppression
// and the full rationale. Every claim below is read off that binary's own streams.

use std::path::PathBuf;
use std::process::Child;

use serde_json::{json, Value};

use crate::harness::{agent, plan_of, World, REFUSED, RUNS_UNWATCHED, WATCH_ELAPSED};

use onepipeline::cli::WAKE_BUDGET_ENV;
use onepipeline::views::{
    RunPaths, WatcherRecord, WATCHER_SCHEMA_VERSION, WATCH_TERMS_SCHEMA_VERSION,
};

/// The status that says nothing this session owes is reported.
const SUCCESS: i32 = onepipeline::error::EXIT_SUCCESS;

/// The wake budget every journey asks under, in seconds: thirty minutes, the
/// update interval the user this was written for expects.
const BUDGET: &str = "1800";

/// A watch that gives up well within [`BUDGET`].
const WITHIN: &str = "600";

/// A watch that gives up well past it.
const PAST: &str = "7200";

/// The session variable, as the engine reads it.
const SESSION_ENV: &str = "ONEPIPELINE_LAUNCHER_SESSION";

/// How a journey gives the budget: as the flag, or in the environment.
#[derive(Clone, Copy, Debug)]
enum Given {
    Flag,
    Environment,
}

impl Given {
    /// A view of `world` under `session`, asking under this way of giving the
    /// budget.
    ///
    /// Built in one expression, because a view removes the world's root when it
    /// is dropped: every view a journey makes lives as long as the journey.
    fn world(self, world: &World, session: &str) -> World {
        match self {
            Self::Flag => world.as_session(session),
            Self::Environment => world.as_session(session).with_env(WAKE_BUDGET_ENV, BUDGET),
        }
    }

    /// The arguments the question takes under it.
    fn args(self) -> Vec<&'static str> {
        match self {
            Self::Flag => vec!["--wake-budget", BUDGET],
            Self::Environment => Vec::new(),
        }
    }

    /// The command a reported line names for a run that can still move.
    fn arm(self, run: &str) -> String {
        match self {
            Self::Flag => format!("watch it with: onepipeline watch {run} --timeout {BUDGET}"),
            Self::Environment => format!("watch it with: onepipeline watch {run}\n"),
        }
    }
}

/// A run with a dispatch held open — unsettled, driven, and settled by nothing
/// until the journey releases `<node>.go`.
fn held(world: &World, name: &str, node: &str) -> String {
    world.script(&format!("{node}.wait"), "hold");
    let path = world.plan(name, &plan_of(name, vec![agent(node, &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });
    name.to_string()
}

/// A run driven to settlement by this build and **not** closed, its driver gone.
fn settled(world: &World, name: &str, node: &str, launch: &[&str]) -> String {
    let path = world.plan(name, &plan_of(name, vec![agent(node, &[])]));
    let mut argv = vec!["start", path.as_str(), "--attach"];
    argv.extend_from_slice(launch);
    // Whatever the attached launch exits with — a failed node is a non-zero
    // settlement — the run has settled once its result is on disk.
    let _ = world.run(&argv);
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    world.until("the driver to release the run", |world| {
        !world.run_file(name, "owner.lock").exists()
    });
    name.to_string()
}

fn paths_of(world: &World, run: &str) -> RunPaths {
    RunPaths::under(&world.runs, run)
}

/// Arm a real watch on `run` from `world`'s environment, returning the process
/// once its lease is on disk.
fn watching(world: &World, run: &str, args: &[&str]) -> Child {
    let mut argv = vec!["watch", run];
    argv.extend_from_slice(args);
    let watch = world
        .cmd(&argv)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the watch starts");
    let pid = watch.id();
    world.until("the watch to record itself", |world| {
        lease_of(world, run, pid).is_some()
    });
    watch
}

/// End a watch this journey started, and reap it.
fn end(mut watch: Child) {
    let _ = watch.kill();
    let _ = watch.wait();
}

/// The lease a watch process holds on `run`, when it has written one.
fn lease_of(world: &World, run: &str, pid: u32) -> Option<PathBuf> {
    let mine = format!("{pid}-");
    std::fs::read_dir(world.run_file(run, "watchers"))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&mine) && name.ends_with(".json"))
        })
}

/// The terms record beside a watch's lease: `watch-terms/<the lease's name>`.
fn terms_of(world: &World, run: &str, pid: u32) -> PathBuf {
    let lease = lease_of(world, run, pid).expect("the watch holds a lease");
    world
        .run_file(run, "watch-terms")
        .join(lease.file_name().expect("a lease has a name"))
}

fn read(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("the record")).expect("JSON")
}

/// The keys of one JSON object.
fn keys(object: &Value) -> Vec<String> {
    let mut keys: Vec<String> = object
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The one verdict `stop-guard` answers for `world`'s session.
fn verdict(world: &World, args: &[&str]) -> (Value, String) {
    let mut argv = vec!["stop-guard", "--session", world.session.as_str()];
    argv.extend_from_slice(args);
    let asked = world.run(&argv);
    asked.exited(0);
    let told = serde_json::from_str(asked.stdout.trim()).expect("one verdict object");
    (told, asked.stderr.clone())
}

/// `unwatched` for `world`'s session.
fn unwatched(world: &World, args: &[&str]) -> crate::harness::Run {
    let mut argv = vec!["unwatched", "--session", world.session.as_str()];
    argv.extend_from_slice(args);
    world.run(&argv)
}

/// Both verbs report and block on `run`, the guard's reason being the verb's own
/// lines, each naming `why` and the command that satisfies it.
fn blocked(world: &World, args: &[&str], run: &str, why: &str, satisfied_by: &str) {
    let asked = unwatched(world, args);
    asked
        .exited(RUNS_UNWATCHED)
        .out_has(run)
        .out_has(why)
        .out_has(satisfied_by);
    let (told, _) = verdict(world, args);
    assert_eq!(told["verdict"], json!("block"), "{told}");
    assert_eq!(
        told["reason"],
        json!(asked.stdout),
        "the guard blocked on something other than the verb's own lines: {told}"
    );
}

/// Both verbs pass: nothing reported, nothing unresolved, no verdict.
fn passed(world: &World, args: &[&str]) {
    let asked = unwatched(world, args);
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a run that is watched or closed was written about: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );
    let (told, stderr) = verdict(world, args);
    assert_eq!(told, json!({"verdict": "none"}), "{told} {stderr}");
}

/// Both verbs name `run` as an unknown: `unwatched` reports nothing and names it
/// on standard error, and the guard warns naming it — never a block, and never
/// silence.
fn unknown(world: &World, args: &[&str], run: &str, why: &str) {
    let asked = unwatched(world, args);
    asked.exited(SUCCESS).err_has(run).err_has(why);
    assert!(
        asked.stdout.is_empty(),
        "an unknown was reported as a violation: {}",
        asked.stdout
    );
    let (told, stderr) = verdict(world, args);
    assert_eq!(
        told["verdict"],
        json!("warn"),
        "an unknown did not warn: {told}"
    );
    let message = told["message"].as_str().expect("a warning says something");
    assert!(
        message.contains(run),
        "the warning does not name {run}: {message}"
    );
    assert!(
        stderr.contains(run),
        "the guard's standard error does not name {run}: {stderr}"
    );
}

/// An RFC 3339 instant of the shape this crate writes, as epoch milliseconds.
fn millis(instant: &str) -> i64 {
    let field =
        |from: usize, to: usize| -> i64 { instant[from..to].parse().expect("a digit field") };
    let (year, month, day) = (field(0, 4), field(5, 7), field(8, 10));
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + day_of_year - 719_468;
    ((days * 86_400 + field(11, 13) * 3_600 + field(14, 16) * 60 + field(17, 19)) * 1_000)
        + field(20, 23)
}

fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_millis(),
    )
    .expect("a millisecond count")
}

/// A running watch writes its v1 lease exactly as before and its terms beside
/// it — every field, with the deadline, the conditions and the session it was
/// armed with — and both are gone once the watch returns.
#[test]
fn a_running_watch_writes_its_unchanged_lease_and_its_terms_and_removes_both_when_it_returns() {
    let world = World::new("wake-terms");
    let run = held(&world, "waketerms", "build");

    let before = now_ms();
    let watch = watching(&world, &run, &["--timeout", "8", "--until", "node-settled"]);
    let after = now_ms();
    let pid = watch.id();
    let lease_path = lease_of(&world, &run, pid).expect("a lease");
    let lease = read(&lease_path);

    // The lease: the six fields entry 68 names and no other, at version 1, and the
    // very bytes the published record type writes for those values — which is
    // what an older engine reading it closed parses.
    assert_eq!(
        keys(&lease),
        [
            "began_at",
            "host",
            "pid",
            "run_id",
            "schema_version",
            "started"
        ],
        "the lease is not the v1 record an older engine reads: {lease}"
    );
    assert_eq!(lease["schema_version"], json!(WATCHER_SCHEMA_VERSION));
    let typed: WatcherRecord = serde_json::from_value(lease.clone()).expect("a v1 lease");
    assert_eq!(
        std::fs::read_to_string(&lease_path).expect("the lease"),
        serde_json::to_string_pretty(&typed).expect("a lease renders"),
        "the lease's bytes are not the ones the v1 record writes"
    );

    // The terms: every field the entry names, the lease's own identity, and what
    // this watch was armed with.
    let terms = read(&terms_of(&world, &run, pid));
    assert_eq!(
        keys(&terms),
        [
            "deadline",
            "pid",
            "run_id",
            "schema_version",
            "session",
            "started",
            "until"
        ],
        "{terms}"
    );
    assert_eq!(terms["schema_version"], json!(WATCH_TERMS_SCHEMA_VERSION));
    for field in ["run_id", "pid", "started"] {
        assert_eq!(
            terms[field], lease[field],
            "the terms name another {field}: {terms}"
        );
    }
    let deadline = millis(
        terms["deadline"]
            .as_str()
            .expect("a bounded watch records a deadline"),
    );
    assert!(
        (before + 8_000 - 1_000..=after + 8_000 + 1_000).contains(&deadline),
        "the deadline is not eight seconds from arming: {terms}"
    );
    assert_eq!(
        terms["until"],
        json!(["node-settled", "settled", "nothing-driving"]),
        "the terms do not spell the conditions the watch resolved: {terms}"
    );
    assert_eq!(terms["session"], json!(world.session));

    // Both go when the watch returns.
    let terms_path = terms_of(&world, &run, pid);
    let returned = watch.wait_with_output().expect("the watch returns");
    assert_eq!(returned.status.code(), Some(WATCH_ELAPSED));
    assert!(!lease_path.exists(), "the lease outlived its watch");
    assert!(!terms_path.exists(), "the terms outlived their watch");

    // Unbounded, and armed under no session: both recorded as `null`, never
    // omitted.
    let nobody = world.as_session("").with_env(SESSION_ENV, "  ");
    let watch = watching(&nobody, &run, &["--timeout", "none"]);
    let terms = read(&terms_of(&world, &run, watch.id()));
    assert_eq!(terms["deadline"], Value::Null, "{terms}");
    assert_eq!(terms["session"], Value::Null, "{terms}");
    assert_eq!(
        terms["until"],
        json!(["surface", "settled", "nothing-driving"])
    );
    end(watch);
    world.release("build.go");
}

/// Under a wake budget — given as the flag, and separately in the environment —
/// both verbs agree on every owed run: each watch that cannot wake the session in
/// time is reported and blocked on, a run nothing drives is too, and a run a
/// compliant watch holds passes, a second non-compliant one beside it or not.
fn under_a_budget(given: Given, name: &str) {
    let world = World::new(name);
    let args = given.args();

    // The owner of each case, and the stranger whose watch is not the owner's.
    let cases: Vec<World> = [
        "unbounded",
        "late",
        "blind",
        "stranger",
        "dead",
        "notpid",
        "notstarted",
        "notrun",
        "bounded",
        "mixed",
    ]
    .iter()
    .map(|case| given.world(&world, &format!("{name}-{case}")))
    .collect();
    let stranger = world.as_session("somebody-else");
    let [unbounded, late, blind, strange, dead, notpid, notstarted, notrun, bounded, mixed] =
        &cases[..]
    else {
        unreachable!("ten cases")
    };

    // `--timeout none`: it never gives up, so it wakes nobody on a clock.
    let run = held(unbounded, &format!("{name}unbounded"), "a");
    let watch = watching(unbounded, &run, &["--timeout", "none"]);
    blocked(
        unbounded,
        &args,
        &run,
        "it has no deadline (`--timeout none`)",
        &given.arm(&run),
    );
    end(watch);

    // A deadline past the budget.
    let run = held(late, &format!("{name}late"), "b");
    let watch = watching(late, &run, &["--timeout", PAST]);
    blocked(
        late,
        &args,
        &run,
        "is past the wake budget",
        &given.arm(&run),
    );
    end(watch);

    // Bounded, but blind to surfaces.
    let run = held(blind, &format!("{name}blind"), "c");
    let watch = watching(
        blind,
        &run,
        &["--timeout", WITHIN, "--until", "node-settled"],
    );
    blocked(
        blind,
        &args,
        &run,
        "it does not return on a surface (`--until node-settled settled nothing-driving`)",
        &given.arm(&run),
    );
    end(watch);

    // Compliant in every way but whose it is: armed under another session.
    let run = held(strange, &format!("{name}stranger"), "d");
    let watch = watching(&stranger, &run, &["--timeout", WITHIN]);
    blocked(
        strange,
        &args,
        &run,
        "it is another session's ('somebody-else')",
        &given.arm(&run),
    );
    end(watch);

    // An owed run nothing drives, and nothing watches.
    let run = held(dead, &format!("{name}dead"), "e");
    let pid = dead.run_json(&run, "launch.json")["pid"]
        .as_u64()
        .expect("the launch record names a pid") as u32;
    crate::harness::end_process(pid);
    dead.until("the driver to read as dead", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });
    blocked(
        dead,
        &args,
        &run,
        "nothing has recorded a watch on it",
        &given.arm(&run),
    );

    // A terms record that is not its lease's, on each of the three it must match.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] no verb writes a terms record naming
    // another watch — the only writer is the watch recording its own — and what these stand
    // in for is one left by a restored backup or a copied run root beside a live lease. What
    // is put back is the live watch's own record with that one field moved, and every claim
    // afterwards is read off the compiled binary's own streams.
    for (owner, node, field, moved) in [
        (notpid, "f", "pid", json!(1)),
        (notstarted, "g", "started", json!("linux-proc-stat:1")),
        (notrun, "h", "run_id", json!("another-run")),
    ] {
        let run = held(owner, &format!("{name}not{field}").replace('_', ""), node);
        let watch = watching(owner, &run, &["--timeout", WITHIN]);
        let path = terms_of(owner, &run, watch.id());
        let mut terms = read(&path);
        terms[field] = moved;
        std::fs::write(&path, terms.to_string()).expect("the terms");
        blocked(
            owner,
            &args,
            &run,
            "its terms record is not this watch's",
            &given.arm(&run),
        );
        end(watch);
    }
    // llmlint: ignore-end[tests_mirror_real_usage]

    // Compliant: this session's, bounded within the budget, returning on a surface.
    let run = held(bounded, &format!("{name}bounded"), "i");
    let watch = watching(bounded, &run, &["--timeout", WITHIN]);
    passed(bounded, &args);
    end(watch);

    // And compliant beside one that is not: one watch that wakes the session is
    // enough, whatever else is live.
    let run = held(mixed, &format!("{name}mixed"), "j");
    let careless = watching(mixed, &run, &["--timeout", "none"]);
    let careful = watching(mixed, &run, &["--timeout", WITHIN]);
    passed(mixed, &args);
    end(careful);
    end(careless);

    for node in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"] {
        world.release(&format!("{node}.go"));
    }
    drop(stranger);
    drop(cases);
}

#[test]
fn under_a_wake_budget_flag_both_verbs_hold_every_owed_run_to_a_watch_that_wakes_the_session() {
    under_a_budget(Given::Flag, "wakeflag");
}

#[test]
fn under_a_wake_budget_in_the_environment_both_verbs_hold_every_owed_run_the_same_way() {
    under_a_budget(Given::Environment, "wakeenv");
}

/// A live watch nothing here can judge is an **unknown**: `unwatched` names it on
/// standard error and reports nothing, and the guard warns — never a block,
/// never silence. Three of them, and one beside a watch that positively fails.
#[test]
fn a_watch_the_budget_cannot_judge_makes_the_guard_warn_and_never_block() {
    let world = World::new("wake-unknown");
    let args = ["--wake-budget", BUDGET];
    let untermed = world.as_session("unknown-untermed");
    let sessionless = world.as_session("unknown-sessionless");
    let garbled = world.as_session("unknown-garbled");
    let mixed = world.as_session("unknown-mixed");
    let acknowledged = world.as_session("unknown-acknowledged");
    // A watch armed with no session in its environment, of a run `sessionless`
    // owns.
    let nobody = world.as_session("").with_env(SESSION_ENV, "");

    // llmlint: ignore-block[tests_mirror_real_usage] no verb of this build leaves a lease
    // without its terms, or terms it cannot read: the first is what an older engine's watch
    // leaves, which predates the record, and the second what a writer killed mid-write
    // leaves. Each is staged on a live watch's own files by taking its terms away or
    // cutting them short; every claim afterwards is read off the compiled binary.
    let run = held(&untermed, "unknownuntermed", "a");
    let watch = watching(&untermed, &run, &["--timeout", WITHIN]);
    std::fs::remove_file(terms_of(&untermed, &run, watch.id())).expect("the terms");
    unknown(&untermed, &args, &run, "recorded no terms");
    // Without a budget the same lease is what it always was: a live watch.
    passed(&untermed, &[]);
    end(watch);

    let run = held(&sessionless, "unknownsessionless", "b");
    let watch = watching(&nobody, &run, &["--timeout", WITHIN]);
    unknown(&sessionless, &args, &run, "its terms name no session");
    end(watch);

    let run = held(&garbled, "unknowngarbled", "c");
    let watch = watching(&garbled, &run, &["--timeout", WITHIN]);
    std::fs::write(
        terms_of(&garbled, &run, watch.id()),
        "{\"schema_version\": 1, \"run_",
    )
    .expect("the terms");
    unknown(&garbled, &args, &run, "its terms record cannot be read");
    end(watch);

    // One unknown, and otherwise only a watch that positively fails: still an
    // unknown, because nothing positively determined the violation.
    let run = held(&mixed, "unknownmixed", "d");
    let failing = watching(&mixed, &run, &["--timeout", "none"]);
    let older = watching(&mixed, &run, &["--timeout", WITHIN]);
    std::fs::remove_file(terms_of(&mixed, &run, older.id())).expect("the terms");
    unknown(&mixed, &args, &run, "recorded no terms");
    end(older);
    end(failing);
    // llmlint: ignore-end[tests_mirror_real_usage]

    // An acknowledgement that cannot be read may be the word that closes a run,
    // so the run it sits beside is an unknown too, budget or none.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] no verb writes an acknowledgement it
    // cannot read back; what this stands in for is a writer killed mid-write or a record a
    // later build wrote. Every claim afterwards is read off the compiled binary.
    let run = settled(&acknowledged, "unknownacknowledged", "e", &[]);
    let dir = acknowledged.run_file(&run, "acknowledgements");
    std::fs::create_dir_all(&dir).expect("the acknowledgements");
    std::fs::write(
        dir.join("0123456789abcdef0000.json"),
        "{\"schema_version\": 1",
    )
    .expect("the acknowledgement");
    unknown(
        &acknowledged,
        &[],
        &run,
        "an acknowledgement of it cannot be read",
    );
    // llmlint: ignore-end[tests_mirror_real_usage]

    for node in ["a", "b", "c", "d"] {
        world.release(&format!("{node}.go"));
    }
    drop((untermed, sessionless, garbled, mixed, nobody, acknowledged));
}

/// With neither `--wake-budget` nor `ONEPIPELINE_WAKE_BUDGET`, any live lease
/// still counts as watching, exactly as before — and the same run under a budget
/// is reported.
#[test]
fn with_no_budget_anywhere_any_live_watch_still_counts() {
    let world = World::new("wake-none");
    let run = held(&world, "wakenone", "build");
    let watch = watching(
        &world,
        &run,
        &["--timeout", "none", "--until", "node-settled"],
    );
    passed(&world, &[]);
    blocked(
        &world,
        &["--wake-budget", BUDGET],
        &run,
        "it has no deadline",
        &Given::Flag.arm(&run),
    );
    end(watch);
    world.release("build.go");
}

/// `watch`'s default `--timeout` is the wake budget where the environment names
/// one, and 300 seconds where it names none.
#[test]
fn a_watch_given_no_timeout_waits_the_budget_the_environment_names() {
    let world = World::new("wake-default");
    let run = held(&world, "wakedefault", "build");
    let budgeted = world
        .as_session(&world.session)
        .with_env(WAKE_BUDGET_ENV, "900");
    for (asker, seconds) in [(&budgeted, 900), (&world, 300)] {
        let before = now_ms();
        let watch = watching(asker, &run, &[]);
        let after = now_ms();
        let terms = read(&terms_of(asker, &run, watch.id()));
        let deadline = millis(terms["deadline"].as_str().expect("a default is bounded"));
        assert!(
            (before + seconds * 1_000 - 1_000..=after + seconds * 1_000 + 1_000)
                .contains(&deadline),
            "a watch given no timeout under a budget of {seconds}s recorded {terms}"
        );
        end(watch);
    }
    // And `--help` says what the default follows.
    world
        .run(&["watch", "--help"])
        .exited(0)
        .out_has(WAKE_BUDGET_ENV);
    world.release("build.go");
    drop(budgeted);
}

/// A wake budget the environment names and that is not one is refused naming the
/// variable, by every verb that reads it — `unwatched` and `watch` with the
/// refusal status, `stop-guard` with a warning, since its status is always `0`.
#[test]
fn a_malformed_wake_budget_is_refused_naming_the_variable() {
    let world = World::new("wake-malformed");
    let run = held(&world, "wakemalformed", "build");
    let mut askers = Vec::new();
    for malformed in ["0", "-5", "abc", "1.5", ""] {
        let asker = world
            .as_session(&world.session)
            .with_env(WAKE_BUDGET_ENV, malformed);
        unwatched(&asker, &[])
            .exited(REFUSED)
            .err_has(WAKE_BUDGET_ENV);
        asker
            .run(&["watch", &run, "--timeout", "0"])
            .exited(REFUSED)
            .err_has(WAKE_BUDGET_ENV);
        let (told, _) = verdict(&asker, &[]);
        assert_eq!(told["verdict"], json!("warn"), "{malformed:?}: {told}");
        assert!(
            told["message"]
                .as_str()
                .is_some_and(|message| message.contains(WAKE_BUDGET_ENV)),
            "{malformed:?}: the warning does not name the variable: {told}"
        );
        // The flag, where given, is the budget, and the variable is not read.
        unwatched(&asker, &["--wake-budget", BUDGET]).exited(RUNS_UNWATCHED);
        askers.push(asker);
    }
    // The flag itself takes only a positive whole number.
    for refused in ["--wake-budget=0", "--wake-budget=-5", "--wake-budget=abc"] {
        unwatched(&world, &[refused])
            .exited(REFUSED)
            .err_has("--wake-budget");
    }
    world.release("build.go");
    drop(askers);
}

/// Runs this release drives are **owed until closed**, in every standing — a
/// failed graph after its failure hook, a finished one whose success hook
/// launched a follow-up, a parked run and a host-shutdown one — and each is
/// closed by the session's acknowledgement, which no other session can make.
#[cfg(unix)]
#[test]
fn a_run_this_release_drives_is_owed_in_every_standing_until_it_is_acknowledged() {
    let world = World::new("wake-owed");
    let failed = world.as_session("owed-failed");
    let followed = world.as_session("owed-followed");
    let shut = world.as_session("owed-shutdown");
    let stranger = world.as_session("owed-stranger");
    // Its own world, because the threshold that reads a quiet run as parked is
    // every run's in the world that sets it.
    let parked = World::new("wake-owed-parked").with_env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    let parked_stranger = parked.as_session("owed-stranger");

    // A failure hook, and a success hook that launches a follow-up run.
    let failure_hook = world.root.join("failure-hook.sh");
    onepipeline_testfakes::executable(&failure_hook, "#!/bin/sh\nexit 0\n");
    world.script("next.wait", "hold");
    let next = world.plan(
        "owedfollowup",
        &plan_of("owedfollowup", vec![agent("next", &[])]),
    );
    let success_hook = world.root.join("success-hook.sh");
    onepipeline_testfakes::executable(
        &success_hook,
        format!(
            "#!/bin/sh\n\"${}\" start '{next}' --detach >/dev/null 2>&1\nexit 0\n",
            onepipeline_testfakes::CLI_BIN_ENV
        ),
    );

    world.script("broken.fail", "1");
    let failing = settled(
        &failed,
        "owedfailed",
        "broken",
        &["--failure-hook", &failure_hook.display().to_string()],
    );
    failed.until("the failure hook to fire", |world| {
        !world.events_of(&failing, "run-hook-fired").is_empty()
    });
    let finished = settled(
        &followed,
        "owedfinished",
        "fine",
        &["--success-hook", &success_hook.display().to_string()],
    );
    followed.until("the follow-up run to launch", |world| {
        world.run_file("owedfollowup", "launch.json").is_file()
    });

    let parking = held(&parked, "owedparked", "idle");
    parked.until("the run to read as parked", |world| {
        world.run(&["status", &parking]).stdout.contains("PARKED")
    });

    world.script("halted.stops-when-interrupted", "");
    let halted = held(&shut, "owedshutdown", "halted");
    let _ = shut.run(&["shutdown", &halted, "--grace", "4"]);
    shut.until("the run to read as shut down", |world| {
        world
            .run(&["status", &halted])
            .stdout
            .contains("HOST SHUTDOWN")
    });

    let close = |run: &str| format!("onepipeline unwatched --acknowledge {run} --reason <TEXT>");
    for (owner, other, run, standing, satisfied_by) in [
        (&failed, &stranger, &failing, "", close(&failing)),
        (&followed, &stranger, &finished, "", close(&finished)),
        (
            &parked,
            &parked_stranger,
            &parking,
            "PARKED",
            format!("onepipeline watch {parking}"),
        ),
        (
            &shut,
            &stranger,
            &halted,
            "",
            format!("onepipeline watch {halted}"),
        ),
    ] {
        blocked(owner, &[], run, standing, &satisfied_by);

        // Another session's acknowledgement is refused, writes nothing, and
        // leaves the owner's view as it was.
        other
            .run(&["unwatched", "--acknowledge", run, "--reason", "not mine"])
            .exited(REFUSED);
        assert!(
            !owner.run_file(run, "acknowledgements").exists(),
            "a refused acknowledgement left a directory behind"
        );
        unwatched(owner, &[]).exited(RUNS_UNWATCHED).out_has(run);

        // The owner's own acknowledgement closes it, for both verbs.
        owner
            .run(&[
                "unwatched",
                "--acknowledge",
                run,
                "--reason",
                "the follow-up carries it",
            ])
            .exited(0)
            .out_has(run)
            .out_has("the follow-up carries it");
        let after = unwatched(owner, &[]);
        assert!(
            !after.stdout.contains(run.as_str()),
            "an acknowledged run was reported: {}",
            after.stdout
        );
        // The follow-up is its own run, owed in its own right; every other
        // session here owns nothing else.
        if run != &finished {
            passed(owner, &[]);
        }
    }
    parked.release("idle.go");
    world.release("halted.go");
    world.release("next.go");
    drop((failed, followed, shut, stranger, parked_stranger));
}

/// A closed run re-opens on a later `edit-committed`, and separately on a later
/// `driver-adopted`: the acknowledgement before either closes nothing after it.
#[test]
fn a_later_edit_or_adoption_reopens_an_acknowledged_run() {
    let world = World::new("wake-reopen");
    let edited = world.as_session("reopen-edited");
    let adopted = world.as_session("reopen-adopted");
    world.script("first.work", "the worker wrote this\n");
    world.script("second.work", "the worker wrote this\n");

    let run = settled(&edited, "reopenedited", "first", &[]);
    edited
        .run(&[
            "unwatched",
            "--acknowledge",
            &run,
            "--reason",
            "done with it",
        ])
        .exited(0);
    passed(&edited, &[]);
    edited
        .run_with_stdin(
            &["reply", &run],
            &json!({"version": 3, "commands": [{"op": "add", "node": agent("later", &[])}]})
                .to_string(),
        )
        .exited(0);
    assert!(!edited.events_of(&run, "edit-committed").is_empty());
    unwatched(&edited, &[]).exited(RUNS_UNWATCHED).out_has(&run);

    let run = settled(&adopted, "reopenadopted", "second", &[]);
    adopted
        .run(&[
            "unwatched",
            "--acknowledge",
            &run,
            "--reason",
            "done with it",
        ])
        .exited(0);
    passed(&adopted, &[]);
    let _ = adopted.run(&["adopt", &run]);
    assert!(!adopted.events_of(&run, "driver-adopted").is_empty());
    unwatched(&adopted, &[])
        .exited(RUNS_UNWATCHED)
        .out_has(&run)
        .out_has("--acknowledge");
    drop((edited, adopted));
}

/// The happy path needs no acknowledgement: a `complete` verdict sent with
/// `onepipeline reply` to a settled run nothing drives closes it — and a stopped
/// run is closed as it always was.
#[test]
fn a_complete_verdict_closes_a_settled_run_and_a_stop_closes_any() {
    let world = World::new("wake-complete");
    let completing = world.as_session("complete-verdict");
    let stopping = world.as_session("complete-stop");
    world.script("build.work", "the worker wrote this\n");

    let run = settled(&completing, "completeverdict", "build", &[]);
    blocked(
        &completing,
        &[],
        &run,
        "SETTLED",
        &format!("onepipeline reply {run}, or acknowledge it"),
    );
    // Where a driver still holds it — here a lock nobody can be named as holding,
    // which is a claim on the run all the same — the verdict is refused as any
    // reply to a settled run is, and nothing is written.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] no verb leaves an ownership lock
    // nobody can read beside a settled run; it stands in for a driver still closing the
    // run out, a window no journey can hold open without racing it. It is removed again
    // before the verdict the rest of the journey sends.
    let lock = completing.run_file(&run, "owner.lock");
    std::fs::write(&lock, "not a lock this build wrote").expect("the lock");
    completing
        .run_with_stdin(
            &["reply", &run],
            r#"{"completion":true,"reason":"too soon"}"#,
        )
        .exited(REFUSED)
        .err_has("has settled");
    assert!(
        completing
            .events_of(&run, "completion-requested")
            .is_empty(),
        "a verdict refused over a held run was journalled"
    );
    std::fs::remove_file(&lock).expect("the lock");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let replied = completing.run_with_stdin(
        &["reply", &run],
        r#"{"completion":true,"reason":"the goal is met"}"#,
    );
    replied
        .exited(0)
        .out_has(r#"{"reply":0,"state":"delivered","verdict":"delivered"}"#);
    assert!(
        !completing
            .events_of(&run, "completion-requested")
            .is_empty(),
        "the verdict journalled no completion request: {:?}",
        completing.kinds(&run)
    );
    passed(&completing, &[]);

    let run = held(&stopping, "completestop", "halt");
    stopping.run(&["stop", &run]).exited(0);
    passed(&stopping, &[]);
    world.release("halt.go");
    drop((completing, stopping));
}

/// Strip the closure rule's marker from a run's journal, and fold it afresh:
/// the run as an engine before this release leaves it.
///
// llmlint: ignore-block[tests_mirror_real_usage] no verb of this build writes a journal
// without the marker — every `run-started` and `driver-adopted` it writes carries it — and
// the state under test is a run an earlier release drove, measured on the host as dozens of
// a resumed session's historical runs. What is put back is this build's own journal with
// that one payload field removed, its summary and checkpoint left for the reader to refold,
// and every claim afterwards is read off the compiled binary.
fn as_an_earlier_release_left_it(world: &World, run: &str) {
    let paths = paths_of(world, run);
    let journal = std::fs::read_to_string(paths.journal()).expect("the journal");
    assert!(
        journal.contains("\"owed_until_closed\":true"),
        "no marker to strip"
    );
    std::fs::write(
        paths.journal(),
        journal
            .replace(",\"owed_until_closed\":true", "")
            .replace("\"owed_until_closed\":true,", ""),
    )
    .expect("the journal");
    for derived in ["summary.json", "checkpoint.json"] {
        let _ = std::fs::remove_file(world.run_file(run, derived));
    }
    // The listing folds the run once and leaves its document, as it would on the
    // first look after an upgrade.
    world.run(&["runs"]).exited(0);
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A settled run, and a stopped run, that no driver of this release drove are
/// decided by the rule they were driven under and are not owed; the same settled
/// run, once a driver of this release adopts it, is held to the closure rule.
#[test]
fn a_run_no_driver_of_this_release_drove_keeps_the_rule_it_was_driven_under() {
    let world = World::new("wake-history");
    world.script("old.work", "the worker wrote this\n");
    let historical = settled(&world, "historysettled", "old", &[]);
    let stopped = held(&world, "historystopped", "halt");
    world.run(&["stop", &stopped]).exited(0);
    world.release("halt.go");

    // Driven by this release, the settled one is owed.
    unwatched(&world, &[])
        .exited(RUNS_UNWATCHED)
        .out_has(&historical);

    for run in [&historical, &stopped] {
        as_an_earlier_release_left_it(&world, run);
    }
    passed(&world, &[]);

    // Adopted by a driver of this release: owed from then on.
    let _ = world.run(&["adopt", &historical]);
    unwatched(&world, &[])
        .exited(RUNS_UNWATCHED)
        .out_has(&historical)
        .out_has("--acknowledge");
}

/// `--acknowledge` refuses, and leaves neither a record nor a directory, for a
/// blank reason, a run id that is not one, a run with no root, and a run
/// another session owns.
#[test]
fn an_acknowledgement_is_refused_and_leaves_nothing_behind() {
    let world = World::new("wake-refused");
    let stranger = world.as_session("refused-stranger");
    world.script("build.work", "the worker wrote this\n");
    let run = settled(&world, "refusedrun", "build", &[]);
    let acknowledgements = world.run_file(&run, "acknowledgements");

    for (asker, target, reason, why) in [
        (&world, run.as_str(), "   ", "reason"),
        (&world, "../escaped", "a reason", "not a run id"),
        (&world, "norunhere", "a reason", "norunhere"),
        (&stranger, run.as_str(), "a reason", &run),
    ] {
        asker
            .run(&["unwatched", "--acknowledge", target, "--reason", reason])
            .exited(REFUSED)
            .err_has(why);
        assert!(
            !acknowledgements.exists(),
            "a refused acknowledgement left {acknowledgements:?}"
        );
    }
    assert!(!world.runs.join("norunhere").exists());
    assert!(!world.runs.join("..").join("escaped").exists());
    unwatched(&world, &[]).exited(RUNS_UNWATCHED).out_has(&run);
    drop(stranger);
}

/// The verdict does not depend on how a run's observer is built or what it
/// raises: a compliant bounded watch passes, and an unbounded one blocks,
/// identically on a run launched with no observer graph and on one whose
/// observer graph declares arbitrarily named members.
#[test]
fn the_verdict_is_the_same_whatever_observes_the_run() {
    let world = World::new("wake-observers");
    world.write_graphs();
    world.script("observer.wait", "");
    let graph = world.write_observer_graph_with_clocks(&[("zebra-herder", false), ("q7", true)]);
    let bare = world.as_session("observed-bare");
    let observed = world.as_session("observed-graph");
    let args = ["--wake-budget", BUDGET];

    let plain = held(&bare, "observedbare", "a");
    // A run launched over the written graphs dispatches through their worker,
    // whose turn is held by `turn.hold` rather than by the node's own script.
    world.script("turn.hold", "hold");
    let path = observed.plan(
        "observedgraph",
        &plan_of("observedgraph", vec![agent("b", &[])]),
    );
    observed
        .run_on(
            observed.agentgraph_cmd(&["start", &path, "--detach", "--dag-graph", &graph]),
            "start observed",
        )
        .exited(0);
    observed.until("the observer to record itself", |world| {
        !world.observer_saw().is_empty()
    });
    observed.until("the worker to be dispatched", |world| {
        !world
            .events_of("observedgraph", "node-dispatched")
            .is_empty()
    });
    let graphed = "observedgraph".to_owned();

    let mut answers = Vec::new();
    for (owner, run) in [(&bare, &plain), (&observed, &graphed)] {
        let careful = watching(owner, run, &["--timeout", WITHIN]);
        passed(owner, &args);
        end(careful);
        let careless = watching(owner, run, &["--timeout", "none"]);
        let asked = unwatched(owner, &args);
        asked.exited(RUNS_UNWATCHED);
        let (told, _) = verdict(owner, &args);
        assert_eq!(told["verdict"], json!("block"), "{told}");
        end(careless);
        // Identical but for the run's name, the pid and the standing column.
        let line = asked.stdout.replace(run.as_str(), "RUN");
        let why = line
            .split_once("—")
            .map(|(_, rest)| rest.to_owned())
            .unwrap_or_default();
        answers.push((
            line.split("pid ")
                .nth(1)
                .and_then(|rest| rest.split_once(':'))
                .map(|(_, why)| why.to_owned()),
            why,
        ));
    }
    assert_eq!(
        answers[0], answers[1],
        "the two runs were decided differently: {answers:?}"
    );
    world.release("a.go");
    world.release("turn.go");
    world.release("turn.settle");
    world.release("observer.go");
    drop((bare, observed));
}

/// What `docs/stop-guard.md` tells a host to set is what the binary reads: the
/// variable is this build's, every flag the section names is one the verbs
/// offer, and the command it says closes a settled run is the one a blocked
/// line names for it.
#[test]
fn the_stop_guard_page_names_the_budget_the_binary_reads() {
    let world = World::new("wake-page");
    let page = std::fs::read_to_string(crate::harness::repo_file("docs/stop-guard.md"))
        .expect("the page ships");
    let section = page
        .split("## The wake budget")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the page has a wake budget section")
        .to_owned();
    assert!(
        section.contains(&format!("`{WAKE_BUDGET_ENV}=<SECONDS>`")),
        "{section}"
    );
    let mut offered = std::collections::BTreeSet::new();
    for verb in ["stop-guard", "watch", "unwatched"] {
        let help = world.run(&[verb, "--help"]);
        help.exited(0);
        offered.extend(crate::stop_guard::flags_of(&help.stdout));
    }
    let named = crate::stop_guard::flags_of(&section);
    assert!(
        named.is_subset(&offered),
        "the page names flags no verb offers: {:?}",
        named.difference(&offered).collect::<Vec<_>>()
    );

    // The closing command, as a blocked line spells it for a settled run.
    let run = settled(&world, "wakepage", "build", &[]);
    let line = unwatched(&world, &[]).exited(RUNS_UNWATCHED).stdout.clone();
    let spelled = "onepipeline unwatched --acknowledge <RUN> --reason <TEXT>";
    assert!(section.contains(spelled), "{section}");
    assert!(
        line.contains(&spelled.replace("<RUN>", &run)),
        "a blocked settled run's line is not the page's command: {line}"
    );
}
