//! `onepipeline unwatched` — the verb a hook asks at the end of every turn, and
//! the watcher record it decides from.
//!
//! What this exists for is one measured failure: the manager on the host this was
//! written for forgets to arm the watch, and dispatched work then sits for hours
//! with nothing looking at it. Every fix attempted before this was prose in that
//! host's instructions, and prose is remembered by the model or it is not. So the
//! question is asked by a script — which is worth nothing unless the evidence
//! behind the answer is right: **a watcher that died without cleaning up has to
//! read as gone immediately**, or the check waves through exactly the silent state
//! it was built to catch.
//!
//! That is why the journeys here are about *what the host says now* rather than
//! about a record's presence. The sharpest of them kills a watch and deliberately
//! leaves the corpse unreaped, because a reaped one would pass against a liveness
//! reading that only asks whether the pid answers — and an unreaped one answers
//! `kill(pid, 0)` successfully with the very start ticks its token was read from.
//!
//! Everything is driven through the compiled binary and asserted on its exit
//! status, its standard output and its standard error. The only things written by
//! hand are watcher records and summary documents put into states no verb
//! produces — a record another host wrote, one a previous build's schema wrote,
//! one a writer left half-written — each of which is left by a *build*, a *crash*
//! or *another machine* rather than by any interface.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven here as a
// real compiled binary against a real run store; `harness.rs` carries the same suppression
// and the full rationale. Every claim below is read off that binary's own streams.

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] measured rather than
// assumed: every journey here but one drives a handful of runs and they run in about 40
// seconds together. The exception is the scale journey, which takes minutes and writes over
// ten gibibytes, because a bound about a host-sized runs root cannot be stated over a root
// that is not one — the same grounds `tests/e2e/listing.rs` carries for its own. What this
// exercises is `watchers`, `unwatched`, `summary` and `views`, which any change under
// `src/` can move, so a project edged narrower than the crate would drop it out of `nx
// affected` for the very changes it exists to catch.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::harness::{agent, plan_of, World, RUNS_UNWATCHED};

use onepipeline::views::{RunPaths, WATCHER_SCHEMA_VERSION};

/// The status that says nothing this session owns is unwatched.
///
/// Named beside [`RUNS_UNWATCHED`] rather than written as `0`, because the whole
/// point of the pair is that a caller branches on one against the other.
const SUCCESS: i32 = onepipeline::error::EXIT_SUCCESS;

/// A run with a dispatch held open: unsettled, being driven, and settled by
/// nothing until the journey releases it.
///
/// The state this verb exists to find. A run whose graph has *converged* — a
/// complete one, a failed one, a stopped one — is excluded however long it has
/// been unwatched, so a journey about an unwatched run has to hold one open.
fn held(world: &World, name: &str) -> String {
    let path = world.plan(name, &plan_of(name, vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });
    name.to_string()
}

/// A run driven to settlement, with its driver gone.
fn settled(world: &World, name: &str) -> String {
    let path = world.plan(name, &plan_of(name, vec![agent("build", &[])]));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

fn paths_of(world: &World, run: &str) -> RunPaths {
    RunPaths::under(&world.runs, run)
}

/// The watcher directory of one run, which is where every record this verb reads
/// lives.
///
/// Composed here rather than asked of the crate: the directory is deliberately not
/// on the published surface — a reader asks `views::Watchers::of` — so a journey
/// about the files themselves spells the one path the record's own contract fixes.
fn watchers_dir(world: &World, run: &str) -> PathBuf {
    world.run_file(run, "watchers")
}

fn records_under(world: &World, run: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(watchers_dir(world, run))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

/// Arm a real watch on a run, returning the process holding it once its record is
/// on disk.
///
/// `--timeout none` because what the journeys need is a watch that is still
/// watching when the question is asked, and a bounded one would be a race with the
/// clock. Its streams go nowhere: a supervisor's terminal is not what any claim
/// here is about, and a pipe nobody reads is a watch that blocks on its own output.
fn arm(world: &World, run: &str) -> std::process::Child {
    let before = records_under(world, run).len();
    let watching = world
        .cmd(&["watch", run, "--timeout", "none"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the watch starts");
    world.until("the watch to record itself", |world| {
        records_under(world, run).len() > before
    });
    watching
}

/// One watcher record, read back as the writer wrote it.
fn record(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("the watcher record"))
        .expect("a watcher record is JSON")
}

/// Put a record under a run's watcher directory.
///
/// llmlint: ignore-block[tests_mirror_real_usage] no verb writes a *stranger's* watcher
/// record — the only writer is a watch recording itself on this host — and the states below
/// are left by another machine, by a build whose schema predates this one, or by a writer
/// that died mid-write. Every record put back is this build's own writer's, edited only
/// where the state under test is the edit, and every claim afterwards is read off the
/// compiled binary's own streams.
fn put(world: &World, run: &str, name: &str, body: &str) -> PathBuf {
    let dir = watchers_dir(world, run);
    std::fs::create_dir_all(&dir).expect("the watcher directory");
    let path = dir.join(name);
    std::fs::write(&path, body).expect("the watcher record");
    path
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A session that owns no run, and a runs root that is not there, say nothing at
/// all.
///
/// The silence is the contract: this verb runs at the end of every turn, so
/// "nothing to report" has to cost the reader nothing to read. Both halves are
/// here because they are two different absences — a root with runs in it that are
/// somebody else's, and no root at all.
#[test]
fn a_session_owning_no_run_and_a_runs_root_that_is_not_there_say_nothing() {
    let world = World::new("unwatched-quiet");
    world.script("build.wait", "hold");
    let run = held(&world, "unwatchedquiet");

    // A root holding a run that is owned by somebody else, asked about by a
    // session that owns nothing.
    let stranger = world.as_session("another-planner");
    let asked = stranger.run(&["unwatched"]);
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a session owning no run was not answered in silence: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );
    // And the run really was there to be found by the session that owns it, so
    // the silence above is about ownership rather than about an empty root.
    world
        .run(&["unwatched"])
        .exited(RUNS_UNWATCHED)
        .out_has(&run);

    // A runs root that does not exist holds no runs, and is not an error.
    let elsewhere = World::new("unwatched-noroot");
    let missing = elsewhere.root.join("runs-that-are-not-there");
    let asked = elsewhere
        .cmd(&["unwatched"])
        .env("ONEPIPELINE_RUNS_DIR", &missing)
        .output()
        .expect("the binary runs");
    assert_eq!(asked.status.code(), Some(SUCCESS), "{asked:?}");
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a runs root that is not there was not answered in silence: {asked:?}"
    );
    world.release("build.go");
}

/// An unsettled run with no watcher record at all is named on standard output,
/// with the word a listing gives it and the command that watches it.
#[test]
fn an_unsettled_run_nothing_is_watching_is_named_with_its_standing_word() {
    let world = World::new("unwatched-named");
    world.script("build.wait", "hold");
    let run = held(&world, "unwatchednamed");
    assert!(
        !watchers_dir(&world, &run).exists(),
        "a run nobody has watched already holds watcher records"
    );

    let asked = world.run(&["unwatched"]);
    asked
        .exited(RUNS_UNWATCHED)
        .out_has(&run)
        // The word `runs` gives the same run, read from its own document.
        .out_has("ACTIVE")
        .out_has("nothing has recorded a watch on it")
        .out_has(&format!("onepipeline watch {run}"));
    assert!(
        asked.stderr.is_empty(),
        "an answer this verb could give in full still wrote to standard error: {}",
        asked.stderr
    );
    assert_eq!(
        asked.stdout.lines().count(),
        1,
        "one unwatched run is not one line: {}",
        asked.stdout
    );
    // The word is the listing's own rather than a second reading of the same run.
    assert!(
        world.run(&["runs"]).stdout.contains("ACTIVE"),
        "the listing does not give this run the word this verb reported"
    );
    world.release("build.go");
}

/// A run a live process is watching is not reported, and the record that says so
/// is gone once the watch returns.
#[test]
fn a_run_a_live_watch_holds_is_not_reported_and_reads_unwatched_once_it_returns() {
    let world = World::new("unwatched-live");
    world.script("build.wait", "hold");
    let run = held(&world, "unwatchedlive");

    let watching = arm(&world, &run);
    let asked = world.run(&["unwatched"]);
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a watched run was reported: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );

    // The record is this build's own, and it says what it is supposed to say.
    let [only] = &records_under(&world, &run)[..] else {
        panic!("one live watch left something other than one record");
    };
    let held = record(only);
    assert_eq!(
        held["schema_version"],
        json!(WATCHER_SCHEMA_VERSION),
        "{held}"
    );
    assert_eq!(held["run_id"], json!(run), "{held}");
    assert_eq!(
        held["pid"].as_u64(),
        Some(u64::from(watching.id())),
        "the record names a process other than the watch that wrote it: {held}"
    );
    assert!(
        only.file_name()
            .expect("the record has a name")
            .to_string_lossy()
            .starts_with(&format!("{}-", watching.id())),
        "the record is not named after the process holding it: {only:?}"
    );
    assert!(
        held["began_at"].as_str().is_some_and(|at| at.contains('T')),
        "the record does not say when the watch began: {held}"
    );

    // And once the watch has returned, the run reads unwatched again — the same
    // question, a different answer, with nothing having changed but the process.
    let watched = &run;
    world.run(&["stop", watched, "--force"]).exited(0);
    let ended = watching.wait_with_output().expect("the watch returns");
    assert!(
        !ended.status.success(),
        "a watch on a stopped run returned as though the run had settled"
    );
    assert!(
        records_under(&world, &run).is_empty(),
        "a watch that returned left its record behind: {:?}",
        records_under(&world, &run)
    );
    world.release("build.go");
}

/// A watch killed and **left unreaped by its parent** is not a live watch on the
/// very next invocation, with nothing having cleaned up and no interval having
/// elapsed.
///
/// The journey this whole verb turns on. A terminated child nobody has `wait`ed
/// for keeps its pid — that is what the kernel keeps the entry for — so it answers
/// `kill(pid, 0)` successfully, and `/proc/<pid>/stat` still carries the very start
/// ticks its token was read from: only the state field moves, to `Z`. So the two
/// readings a liveness proof is usually made of both say *live*, and a verb that
/// asked only those would report the run as watched. The child is deliberately
/// never reaped here, because reaping it would make this journey pass against
/// exactly the reading it exists to rule out.
///
/// **Unix, and deliberately not skipped elsewhere.** A terminated-but-unreaped
/// process is a Unix state: on Windows a process handle becomes signalled the
/// moment the process ends whoever still holds it, so `process_may_be_live`
/// already answers `false` and there is no state here that reads as alive.
#[cfg(unix)]
#[test]
fn a_watch_killed_and_left_unreaped_is_not_a_live_watch() {
    let world = World::new("unwatched-zombie");
    world.script("build.wait", "hold");
    let run = held(&world, "unwatchedzombie");

    // The child is never `wait`ed on, which is what clippy's lint is for and what
    // this journey is: reaping it would put the pid beyond every reading, and the
    // state under test is the one *before* that — a terminated process still
    // holding its pid because its parent has not collected it. Scoped to this
    // binding, and this is the only place in the suite that leaves one.
    #[allow(clippy::zombie_processes)]
    let mut watching = arm(&world, &run);
    let pid = watching.id();
    world.run(&["unwatched"]).exited(SUCCESS);

    watching.kill().expect("the watch takes the signal");
    // Deliberately no `wait`: this journey is about the state a killed process
    // sits in until its parent reaps it, and this process is that parent.
    world.until(
        "the killed watch to become a terminated process awaiting its parent",
        |_| state_of(pid).starts_with('Z'),
    );
    // The premises, so a failure below is not read as this journey having proved
    // something easier: the record is still there, and the pid still answers.
    assert_eq!(
        records_under(&world, &run).len(),
        1,
        "the killed watch's record was cleaned up by something, which is the whole \
         state this journey is about"
    );
    assert!(
        state_of(pid).starts_with('Z'),
        "the child was reaped, so a liveness reading that only asks whether the pid \
         answers would pass this journey"
    );

    let asked = world.run(&["unwatched"]);
    asked
        .exited(RUNS_UNWATCHED)
        .out_has(&run)
        .out_has("waiting to be reaped");
    world.release("build.go");
}

/// What this host says one process's state is, in `ps`'s own single-letter code.
///
/// Read through `ps` rather than through procfs so the journey holds on every Unix
/// the suite runs on, and read rather than signalled: nothing here touches a
/// process this journey did not start.
#[cfg(unix)]
fn state_of(pid: u32) -> String {
    let listed = std::process::Command::new("ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&listed.stdout).trim().to_string()
}

/// Five records that are **not** live watches, each refused for its own reason and
/// each leaving the run reported.
///
/// One journey rather than five, because the fixture is the expensive part and the
/// claim is the same claim: the record is decided against this host as it stands,
/// so every way of naming a process that is not watching this run reads as nothing
/// watching it. Each record is a copy of the one a real watch wrote, with exactly
/// the field under test changed.
#[test]
fn a_reused_pid_another_host_an_empty_token_another_run_and_another_schema_are_not_live_watches() {
    let world = World::new("unwatched-refused");
    world.script("build.wait", "hold");
    let run = held(&world, "unwatchedrefused");

    // The template: a real watch's own record, taken while it is watching.
    let watching = arm(&world, &run);
    let [only] = &records_under(&world, &run)[..] else {
        panic!("one live watch left something other than one record");
    };
    let template = record(only);
    let leftover = only.clone();
    world.run(&["stop", &run, "--force"]).exited(0);
    let ended = watching.wait_with_output().expect("the watch returns");
    assert!(
        !ended.status.success(),
        "the watch on a stopped run returned 0"
    );
    assert!(
        !leftover.exists(),
        "the watch that returned left its record behind, so the variants below would \
         not be alone"
    );
    // The run has to be unsettled again for anything to be reported at all, and a
    // stopped run never is — so the fixture moves to a second run, and the record
    // is the first one's, which is what a *copied* run root leaves behind.
    let run = held(&world, "unwatchedcopied");

    // This process is alive and is emphatically not the one that wrote the
    // record, which is what a pid the kernel has handed on looks like.
    let live = std::process::id();
    let mine = |edit: &dyn Fn(&mut Value)| -> String {
        let mut held = template.clone();
        held["run_id"] = json!(run);
        held["pid"] = json!(live);
        edit(&mut held);
        held.to_string()
    };
    let cases: [(&str, String, &str); 5] = [
        (
            "a pid this host has since handed to another process",
            mine(&|held| held["started"] = json!("linux-proc-stat:1")),
            "not the process that recorded it",
        ),
        (
            "a record whose token is empty, which never matches",
            mine(&|held| held["started"] = json!("")),
            "not the process that recorded it",
        ),
        (
            "a record another host wrote, where its pid means nothing",
            mine(&|held| held["host"] = json!("another-host")),
            "another host",
        ),
        (
            "a record whose run is not the run it sits under",
            mine(&|held| held["run_id"] = json!("some-other-run")),
            "another run",
        ),
        (
            "a record at a schema version this build does not read",
            mine(&|held| held["schema_version"] = json!(WATCHER_SCHEMA_VERSION + 1)),
            "cannot be read",
        ),
    ];
    for (what, body, expected) in cases {
        let path = put(
            &world,
            &run,
            &format!("{live}-deadbeefcafe0001.json"),
            &body,
        );
        let asked = world.run(&["unwatched"]);
        asked.exited(RUNS_UNWATCHED).out_has(&run).out_has(expected);
        assert!(
            !asked.stdout.contains("watching"),
            "{what} was read as a live watch: {}",
            asked.stdout
        );
        std::fs::remove_file(&path).expect("the record under test");
    }
    world.release("build.go");
}

/// Several concurrent watches on one run are recorded in files that do not
/// collide, and the run reads watched while any one of them is live.
#[test]
fn concurrent_watches_are_recorded_apart_and_hold_the_run_watched_until_the_last_returns() {
    let world = World::new("unwatched-several");
    world.script("build.wait", "hold");
    let run = held(&world, "unwatchedseveral");

    let first = arm(&world, &run);
    let second = arm(&world, &run);
    let third = arm(&world, &run);
    let names: Vec<String> = records_under(&world, &run)
        .iter()
        .map(|path| path.file_name().expect("a name").to_string_lossy().into())
        .collect();
    assert_eq!(
        names.len(),
        3,
        "three concurrent watches did not leave three records: {names:?}"
    );
    world.run(&["unwatched"]).exited(SUCCESS);

    // Two of the three gone, and the run is still watched: one live watch is
    // enough, which is what makes a record per watch rather than per run right.
    for mut watch in [first, second] {
        watch.kill().expect("the watch takes the signal");
        watch.wait().expect("the watch ends");
    }
    world.until("the two killed watches to read as gone", |world| {
        world.run(&["unwatched"]).code == SUCCESS
    });
    let asked = world.run(&["unwatched"]);
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty(),
        "a run one live watch still holds was reported: {}",
        asked.stdout
    );

    // And unwatched once none is.
    let mut third = third;
    third.kill().expect("the watch takes the signal");
    third.wait().expect("the watch ends");
    world
        .run(&["unwatched"])
        .exited(RUNS_UNWATCHED)
        .out_has(&run);
    world.release("build.go");
}

/// A run whose stop is recorded and a run whose graph is complete are never
/// reported, however long they have been unwatched — and a document **behind** its
/// journal is reported all the same.
///
/// The asymmetry is the whole freshness rule. The stamp is required only for
/// exclusion: a run that has stopped writing has a current document, so requiring
/// it costs a settled run nothing — while a document behind its journal is what a
/// run *still recording* looks like, and treating that as proof of settlement is
/// how the one run this verb exists to find would be dropped.
#[test]
fn a_settled_run_is_never_reported_and_a_document_behind_its_journal_is() {
    let world = World::new("unwatched-settled");
    world.script("build.work", "the worker wrote this\n");
    let complete = settled(&world, "unwatchedcomplete");
    // A run whose graph did not complete, stopped by a planner: the other of the
    // two facts a settled run is excluded on.
    world.script("build.fail", "1");
    let stopped = settled(&world, "unwatchedstopped");
    world.run(&["stop", &stopped, "--force"]).exited(0);

    // Both are excluded, and both were excluded on a document whose stamp matches
    // the journal as it stands — which is what the journey has to establish for
    // the staleness half below to mean anything.
    for run in [&complete, &stopped] {
        let paths = paths_of(&world, run);
        let document = document(&paths);
        assert_eq!(
            (
                document["journal_len"].as_u64(),
                document["journal_mtime_ms"].as_u64()
            ),
            stamp_of(&paths),
            "{run}'s document is already behind its journal, so its exclusion proves nothing"
        );
        assert!(
            document["stop_recorded"] == json!(true) || document["graph_complete"] == json!(true),
            "{run} is not a settled run: {document}"
        );
    }
    let asked = world.run(&["unwatched"]);
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a settled run was reported: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );

    // And the same run, with its document behind its journal.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] what this stages is a writer that
    // appended and died before writing the document beside it — a killed process rather than
    // an interface, and the state the freshness rule exists for. The document put back is
    // this build's own with one recorded length moved, which is exactly what that writer
    // would have left.
    let paths = paths_of(&world, &stopped);
    let mut behind = document(&paths);
    behind["journal_len"] = json!(1);
    std::fs::write(paths.summary(), behind.to_string()).expect("the document");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let asked = world.run(&["unwatched"]);
    asked.exited(RUNS_UNWATCHED).out_has(&stopped);
    assert!(
        !asked.stdout.contains(&complete),
        "the run whose document is current was reported beside the one that is not: {}",
        asked.stdout
    );
}

/// One state a summary document can be left in, and the way it is reached: what it
/// is called, and what to do to the file to put it there.
type Undecidable = (&'static str, fn(&std::path::Path, &Value));

/// The three states a run's summary document can be in that leave its settlement
/// undecidable, each with the way that state is reached.
///
/// Function pointers rather than a table of data, because what distinguishes them
/// is what is done to the file: a build that never wrote a document, a writer
/// killed mid-write, and a build whose schema predates this one.
///
/// llmlint: ignore-block[tests_mirror_real_usage] no verb removes or corrupts the document
/// its run's journal writer maintains, and none could — each of the three is left by a
/// build or a crash rather than by an interface. What is put back is this build's own
/// document, edited only where the state under test is the edit, and every claim afterwards
/// is read off the compiled binary's own streams.
const UNDECIDABLE: [Undecidable; 3] = [
    ("no document at all", |path, _written| {
        std::fs::remove_file(path).expect("the document");
    }),
    ("a document a writer left half-written", |path, _written| {
        std::fs::write(path, "{\"schema_ver").expect("the document");
    }),
    (
        "a document at the schema a previous build wrote",
        |path, written| {
            let mut older = written.clone();
            older["schema_version"] = json!(1);
            std::fs::write(path, older.to_string()).expect("the document");
        },
    ),
];
// llmlint: ignore-end[tests_mirror_real_usage]

/// A run whose settlement cannot be decided at all is named on **standard error**
/// with the reason, is absent from standard output, and changes no exit status.
///
/// Three ways to reach it, because they are one fact to a reader and three
/// different things on disk: no document, one that cannot be read, and one at a
/// schema version this build refuses. Such a run is most often an old settled run
/// whose document is gone, and blocking on it would never clear by watching it —
/// so it is said out loud and passed over.
#[test]
fn a_run_whose_settlement_cannot_be_decided_is_named_on_standard_error_and_changes_no_status() {
    let world = World::new("unwatched-undecidable");
    world.script("build.work", "the worker wrote this\n");
    let run = settled(&world, "unwatchedundecided");
    let paths = paths_of(&world, &run);
    let written = document(&paths);

    for (what, stage) in UNDECIDABLE {
        stage(&paths.summary(), &written);
        let asked = world.run(&["unwatched"]);
        asked.exited(SUCCESS);
        assert!(
            asked.stdout.is_empty(),
            "a run with {what} was reported as unwatched: {}",
            asked.stdout
        );
        asked
            .err_has(&run)
            .err_has("its settlement cannot be decided");
    }

    // And beside a run that *is* reported, it changes nothing: the status is the
    // answer about the other run, and each is on its own stream.
    world.script("build.wait", "hold");
    let reported = held(&world, "unwatchedreported");
    let asked = world.run(&["unwatched"]);
    asked
        .exited(RUNS_UNWATCHED)
        .out_has(&reported)
        .err_has(&run);
    assert!(
        !asked.stdout.contains(&run),
        "the undecidable run reached standard output: {}",
        asked.stdout
    );
    world.release("build.go");
}

/// A proven-unsettled run whose only watcher record cannot be read is reported.
///
/// The inversion, at the one place it costs something: for a *driver* an
/// unreadable input resolves toward "still working", and here it resolves the
/// other way, because a run reported watched while nothing is watching it is the
/// silence this verb exists to end and arming a watch is what clears it.
#[test]
fn a_run_whose_only_watcher_record_cannot_be_read_is_reported() {
    let world = World::new("unwatched-unreadable");
    world.script("build.wait", "hold");
    let run = held(&world, "unwatchedunreadable");
    let path = put(&world, &run, "4242-0badc0ffee000001.json", "{\"schema_ver");

    let asked = world.run(&["unwatched"]);
    asked
        .exited(RUNS_UNWATCHED)
        .out_has(&run)
        .out_has("a record that cannot be read");
    // The record itself is named where everything unresolved is named, so an
    // operator can go and look at it.
    asked.err_has(&path.display().to_string());
    world.release("build.go");
}

/// A run root with no readable launch record, and one naming no session, are
/// passed over with nothing said about them on either stream.
///
/// Ownership is a **positive claim**. The host this was written for holds seventy
/// run roots with no launch record at all, and a verb that ran at the end of every
/// turn and named them would be noise on every turn.
#[test]
fn a_root_with_no_launch_record_and_one_naming_no_session_are_passed_over() {
    let world = World::new("unwatched-unowned");
    world.script("build.wait", "hold");

    // A launch nothing could attribute: the same runs root, with no session in the
    // environment the launch is made from, which is what a run recorded outside a
    // harness looks like.
    let anonymous = world
        .as_session("ignored")
        .with_env("ONEPIPELINE_LAUNCHER_SESSION", "");
    let nameless = held(&anonymous, "unwatchednameless");
    // What a launch with nothing to attribute it to records: the word this crate
    // reserves for a launcher nothing identifies, which every reader treats as
    // nobody's — including a reader whose own session is that same word.
    assert_eq!(
        document(&paths_of(&world, &nameless))["session"],
        json!("unknown"),
        "the anonymous launch recorded an attributable session after all"
    );

    // And a directory that claims to be a run and records no launch at all.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] no verb makes a run root without a
    // launch record — a `start` writes one before anything else — so the state is reached by
    // making the directory. It is the state seventy roots on the host this verb was written
    // for are in, left by builds and sweeps that predate the record.
    let orphan = world.runs.join("unwatchedorphan");
    std::fs::create_dir_all(&orphan).expect("a run root");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let asked = world.run(&["unwatched"]);
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a run belonging to nobody was written about: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );
    // And the anonymous run really is an unsettled, unwatched run — so what kept
    // it off both streams is ownership rather than anything else about it.
    let mine = held(&world, "unwatchedmine");
    let asked = world.run(&["unwatched"]);
    asked.exited(RUNS_UNWATCHED).out_has(&mine);
    for passed_over in [&nameless, &"unwatchedorphan".to_string()] {
        assert!(
            !asked.stdout.contains(passed_over) && !asked.stderr.contains(passed_over),
            "{passed_over} was written about: stdout {:?}, stderr {:?}",
            asked.stdout,
            asked.stderr
        );
    }
    anonymous.release("build.go");
    world.release("build.go");
}

/// `--session` decides which session is asked about even when the environment
/// names another, and the environment decides when the option is absent.
///
/// Both directions, because the consumer is a hook: it is handed the session it
/// must ask about on standard input while the environment it runs in carries
/// somebody else's, so a verb that read only one of the two would answer
/// confidently about the wrong session.
#[test]
fn the_option_decides_which_session_is_asked_about_and_the_environment_decides_without_it() {
    let world = World::new("unwatched-session");
    world.script("build.wait", "hold");
    let mine = held(&world, "unwatchedsession");
    let stranger = world.as_session("another-planner");
    let theirs = held(&stranger, "unwatchedtheirs");

    // The environment, with no option: each session is answered about its own run.
    let asked = world.run(&["unwatched"]);
    asked.exited(RUNS_UNWATCHED).out_has(&mine);
    assert!(!asked.stdout.contains(&theirs), "{}", asked.stdout);
    let asked = stranger.run(&["unwatched"]);
    asked.exited(RUNS_UNWATCHED).out_has(&theirs);
    assert!(!asked.stdout.contains(&mine), "{}", asked.stdout);

    // The option, against an environment naming the other session — both ways
    // round, so what decides is the option rather than the pair happening to
    // agree.
    let asked = world.run(&["unwatched", "--session", &stranger.session]);
    asked.exited(RUNS_UNWATCHED).out_has(&theirs);
    assert!(!asked.stdout.contains(&mine), "{}", asked.stdout);
    let asked = stranger.run(&["unwatched", "--session", &world.session]);
    asked.exited(RUNS_UNWATCHED).out_has(&mine);
    assert!(!asked.stdout.contains(&theirs), "{}", asked.stdout);

    world.release("build.go");
    stranger.release("build.go");
}

/// A question this verb cannot ask at all is **refused**, with a status that is
/// neither of its two answers.
///
/// Both refusals, because a caller has to tell "no run is unwatched" from "this
/// verb could not answer": the hook treats every status but `0` and the unwatched
/// answer as silence, and an operator in a plain shell is told why on standard
/// error.
#[test]
fn a_question_this_verb_cannot_ask_is_refused_rather_than_answered() {
    let world = World::new("unwatched-refusals");

    // No session anywhere: no option, and nothing in the environment.
    let asked = world
        .cmd(&["unwatched"])
        .env_remove("ONEPIPELINE_LAUNCHER_SESSION")
        .output()
        .expect("the binary runs");
    let code = asked.status.code();
    assert!(
        code != Some(SUCCESS) && code != Some(RUNS_UNWATCHED),
        "a question this verb could not ask was answered: {asked:?}"
    );
    assert!(
        asked.stdout.is_empty(),
        "a refusal wrote to standard output: {asked:?}"
    );
    assert!(
        String::from_utf8_lossy(&asked.stderr).contains("no session"),
        "the refusal does not say what is missing: {asked:?}"
    );

    // A runs root that exists and cannot be read as a whole.
    let unreadable = world.root.join("unreadable-runs");
    std::fs::create_dir_all(&unreadable).expect("a runs root");
    if !unreadable_to_us(&unreadable) {
        // llmlint: ignore[no_skipped_tests] not a skip: the first refusal above is
        // asserted on every platform, and this second one is about a *host* condition —
        // a directory this process may not list. A root nobody can read cannot be staged
        // where the test process is privileged or where the platform has no such mode, and
        // asserting over a directory that is in fact readable would assert the opposite of
        // the journey's claim.
        return;
    }
    let asked = world
        .cmd(&["unwatched"])
        .env("ONEPIPELINE_RUNS_DIR", &unreadable)
        .output()
        .expect("the binary runs");
    let code = asked.status.code();
    let _ = std::fs::set_permissions(&unreadable, readable());
    assert!(
        code != Some(SUCCESS) && code != Some(RUNS_UNWATCHED),
        "a runs root this process cannot read was answered rather than refused: {asked:?}"
    );
    assert!(
        asked.stdout.is_empty(),
        "a refusal wrote to standard output: {asked:?}"
    );
    assert!(
        String::from_utf8_lossy(&asked.stderr).contains(&unreadable.display().to_string()),
        "the refusal does not name the root it could not read: {asked:?}"
    );
}

/// Make a directory one this process cannot list, answering whether it worked.
#[cfg(unix)]
fn unreadable_to_us(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).is_err() {
        return false;
    }
    // Root ignores the mode, so the state under test was not reached.
    std::fs::read_dir(dir).is_err()
}

#[cfg(not(unix))]
fn unreadable_to_us(_dir: &std::path::Path) -> bool {
    false
}

#[cfg(unix)]
fn readable() -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    std::fs::Permissions::from_mode(0o755)
}

#[cfg(not(unix))]
fn readable() -> std::fs::Permissions {
    std::fs::metadata(".")
        .expect("the working directory")
        .permissions()
}

/// One run's summary document, as its own writer wrote it.
fn document(paths: &RunPaths) -> Value {
    serde_json::from_str(&std::fs::read_to_string(paths.summary()).expect("the document"))
        .expect("a summary document")
}

/// A journal's length and modification time, in the millisecond the crate stamps a
/// summary document with.
fn stamp_of(paths: &RunPaths) -> (Option<u64>, Option<u64>) {
    let about = std::fs::metadata(paths.journal()).expect("the run's merged store");
    let modified = about
        .modified()
        .expect("a modification time")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("an instant past the epoch")
        .as_millis();
    (
        Some(about.len()),
        Some(u64::try_from(modified).expect("a millisecond count")),
    )
}
