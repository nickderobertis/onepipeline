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

use crate::harness::{agent, let_writeback_settle, plan_of, World, NODE_SETTLED, RUNS_UNWATCHED};

use onepipeline::views::{RunPaths, SUMMARY_SCHEMA_VERSION, WATCHER_SCHEMA_VERSION};

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
    arm_until(world, run, "surface")
}

/// The same, told what to return on — for the one journey that needs a watch to
/// **end** without the run it was watching ending with it.
fn arm_until(world: &World, run: &str, until: &str) -> std::process::Child {
    let watching = world
        .cmd(&["watch", run, "--timeout", "none", "--until", until])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the watch starts");
    // Waited on by **this watch's own name** rather than by a count, because a
    // writer arming a watch also sweeps the records it has proved are not live: a
    // count can come out where it started, and did.
    let mine = format!("{}-", watching.id());
    world.until("the watch to record itself", |world| {
        records_under(world, run)
            .iter()
            .any(|path| named(path).starts_with(&mine))
    });
    watching
}

/// One record's file name.
fn named(path: &std::path::Path) -> String {
    path.file_name()
        .expect("a watcher record has a name")
        .to_string_lossy()
        .into_owned()
}

/// One watcher record, read back as the writer wrote it.
fn record(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("the watcher record"))
        .expect("a watcher record is JSON")
}

/// Put a record under a run's watcher directory.
///
// llmlint: ignore-block[tests_mirror_real_usage] no verb writes a *stranger's* watcher
// record — the only writer is a watch recording itself on this host — and the states below
// are left by another machine, by a build whose schema predates this one, or by a writer
// that died mid-write. Every record put back is this build's own writer's, edited only
// where the state under test is the edit, and every claim afterwards is read off the
// compiled binary's own streams.
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

/// A run a live process is watching is not reported, and reads unwatched the
/// moment that watch returns.
///
/// The same question twice with nothing changed but the process, which is the whole
/// of what the record is for. The watch is ended by a **condition it was told to
/// return on** rather than by stopping the run, because a stopped run is excluded
/// from this verb whatever is watching it — so the second answer would prove
/// nothing about watching at all.
#[test]
fn a_run_a_live_watch_holds_is_not_reported_and_reads_unwatched_once_it_returns() {
    let world = World::new("unwatched-live");
    // Two dispatches held open, so releasing the first ends the watch while the run
    // it was watching is still going.
    world.script("build.wait", "hold");
    world.script("docs.wait", "hold");
    let run = "unwatchedlive";
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), agent("docs", &["build"])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });

    let watching = arm_until(&world, run, "node=build");
    let asked = world.run(&["unwatched"]);
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a watched run was reported: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );

    // The record is this build's own, and it says what it is supposed to say.
    let [only] = &records_under(&world, run)[..] else {
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
        named(only).starts_with(&format!("{}-", watching.id())),
        "the record is not named after the process holding it: {only:?}"
    );
    assert!(
        held["began_at"].as_str().is_some_and(|at| at.contains('T')),
        "the record does not say when the watch began: {held}"
    );

    // The condition the watch was told to return on, and nothing else about the
    // run: its second node is still held open, so the run goes on.
    world.release("build.go");
    let ended = watching.wait_with_output().expect("the watch returns");
    assert_eq!(
        ended.status.code(),
        Some(NODE_SETTLED),
        "the watch did not return on the node it was told to return on: {}",
        String::from_utf8_lossy(&ended.stderr)
    );
    assert!(
        records_under(&world, run).is_empty(),
        "a watch that returned left its record behind: {:?}",
        records_under(&world, run)
    );

    let asked = world.run(&["unwatched"]);
    asked.exited(RUNS_UNWATCHED).out_has(run);
    world.release("docs.go");
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

/// Six records that are **not** live watches, each refused for its own reason and
/// each leaving the run reported.
///
/// One journey rather than six, because the fixture is the expensive part and the
/// claim is the same claim: the record is decided against this host as it stands,
/// so every way of naming a process that is not watching this run reads as nothing
/// watching it. Each record is a copy of the one a real watch wrote, with exactly
/// the field under test changed.
#[test]
fn a_reused_pid_another_host_an_empty_token_another_run_a_schema_and_a_stamp_are_not_live() {
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
    let cases: [(&str, String, &str); 6] = [
        (
            "a pid this host has since handed to another process",
            mine(&|held| held["started"] = json!("linux-proc-stat:1")),
            "not the process that recorded it",
        ),
        (
            "a record whose token is empty, which never matches",
            mine(&|held| held["started"] = json!("")),
            "nothing can say whether it is the process that recorded it",
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
        (
            "a record whose stamp is not the instant it says it is",
            mine(&|held| held["began_at"] = json!("some time yesterday")),
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
        .map(|path| named(path))
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
/// reported, however long they have been unwatched. A document that **records
/// settlement while behind its journal** is named on standard error as
/// undecidable rather than reported (clause 3), and a document recording **no**
/// settlement while behind its journal *is* reported (clause 4).
///
/// The asymmetry is the whole freshness rule, and the stamp decides two different
/// questions on the two sides of it. For **exclusion** the stamp must be current:
/// a run that has stopped writing has a current document, so requiring it costs a
/// settled run nothing. But a stale stamp is not proof the run is *unwatched*
/// either — `6` means *proven* unwatched — so a document that records settlement
/// while behind its journal proves nothing in either direction and is undecidable,
/// while one recording no settlement is exactly what a run *still recording* looks
/// like and stays the one run this verb exists to find.
#[test]
fn a_settled_run_is_excluded_and_a_document_recording_settlement_behind_its_journal_is_undecidable()
{
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

    // A run whose store is not on disk at all is excluded on the same terms, and
    // this is the one stamp that is not a length and a time: `(0, 0)` is what the
    // writer records for a run whose journal it cannot stat — a real state, since a
    // run root exists before its first record lands — so a document carrying it
    // describes that run exactly rather than being behind it.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] no verb takes a run's store away, and
    // the state is left by a sweep or a restore that did not copy it. The document put back
    // is this build's own with the stamp its own writer records for a journal it cannot
    // stat, which is what that run would carry.
    let paths = paths_of(&world, &complete);
    let mut storeless = document(&paths);
    storeless["journal_len"] = json!(0);
    storeless["journal_mtime_ms"] = json!(0);
    std::fs::write(paths.summary(), storeless.to_string()).expect("the document");
    std::fs::remove_file(paths.journal()).expect("the run's merged store");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let asked = world.run(&["unwatched"]);
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a settled run whose store is gone was written about: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );

    // Clause 3: the stopped run, its document still recording that stop but now
    // **behind its journal**. Its own record says it settled, so a stale stamp
    // leaves that undecidable rather than proving the run unwatched — it is named
    // on standard error with the reason and changes no status, and with nothing
    // else to report the verb answers `0`.
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
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty(),
        "a document recording settlement while behind its journal was reported: {}",
        asked.stdout
    );
    asked
        .err_has(&stopped)
        .err_has("records that it settled but is behind its journal");

    // Clause 4: a document recording **no** settlement while behind its journal is
    // the opposite case — that is exactly what a run still recording looks like, so
    // with nothing watching it, it is reported at `6`. The same stopped run whose
    // driver is gone, its settlement fields now cleared and its stamp still behind
    // the journal that is really there beside it, so nothing rewrites it underneath
    // the assertion.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] the same killed-mid-write writer as
    // above, its document now recording no settlement — the run this verb exists to find.
    // What is put back is this build's own document with its settlement fields and one
    // recorded length moved, and every claim afterwards is read off the compiled binary.
    let mut recording = document(&paths);
    recording["graph_complete"] = json!(false);
    recording["stop_recorded"] = json!(false);
    recording["journal_len"] = json!(1);
    std::fs::write(paths.summary(), recording.to_string()).expect("the document");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let asked = world.run(&["unwatched"]);
    asked.exited(RUNS_UNWATCHED).out_has(&stopped);
}

/// The four words a reported run's line can carry, as entry 68 states them.
///
/// Named as a set because the scale journey asserts the word without being able
/// to predict which of the four it will get: it is the guard its whole bound rests
/// on, since a fixture can go degenerate — a clone that carried no run state would
/// be excluded or refused, every other assertion would pass, and the bound would
/// be met over rows with nothing in them.
const STANDING_WORDS: [&str; 4] = ["ACTIVE", "PARKED", "DRIVER DEAD", "UNDRIVEN"];

/// A document at a schema this build has **moved past** is *refreshed* — folded
/// once and rewritten at this build's schema, as `runs` refreshes the same run —
/// and then decided from the run's own record: a settled run is excluded, a run
/// still recording is reported, and the document each leaves behind is this
/// build's own and current for its journal.
///
/// Such a document is what **every run the previous release settled** carries
/// until something refreshes it, and the rule this replaces reported each of them
/// at `6` on the first turn after an upgrade: that is the measured report
/// `a_run_the_previous_release_settled_is_refreshed_and_excluded_rather_than_reported`
/// reproduces over the measured shape. The half kept from that rule is that an
/// answer nobody could take must never *silence* a run: a run still being driven by
/// the previous release's binary carries the same document, and it is reported here
/// from what its store really says rather than passed over. The only cost is the
/// one fold, paid once per run per schema bump, which is why the third half below
/// asks twice and reads the second answer off a document nothing had to fold.
///
/// The settled fixture is chosen so that the two readings **agree** only after the
/// fold: the document says the run settled, and at the superseded version this
/// build may not read it that way — so an answer of `0` proves the run was decided
/// from its store rather than from those fields. A run whose writer has finished is
/// used for that half for the same reason a settled run is used everywhere the
/// document is edited by hand — nothing is going to rewrite the file underneath the
/// assertion. The still-recording half holds a dispatch open instead, and asserts
/// only what is true whether or not its driver rewrote the document first: it is
/// reported, with the word the listing gives it, and the document is this build's.
///
// llmlint: ignore-block[tests_mirror_real_usage] no verb writes a document at a schema
// this build has moved past, and none could: the writer stamps its own version. The state
// is left by a run recorded under an earlier build and read under this one — measured on
// the host as the previous release's document, current for its journal — and what is put
// back is this build's own document with that one field moved. Every claim afterwards is
// read off the compiled binary's own streams and the files it leaves.
#[test]
fn a_document_at_a_superseded_schema_is_refreshed_and_decided_from_the_run() {
    let world = World::new("unwatched-superseded");
    world.script("build.work", "the worker wrote this\n");
    let run = settled(&world, "unwatchedsuperseded");
    let paths = paths_of(&world, &run);
    let written = document(&paths);

    // At its own version the run is excluded, which is what the change below has
    // to be measured against.
    let asked = world.run(&["unwatched"]);
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "the fixture is not an excluded run: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );
    assert!(
        written["stop_recorded"] == json!(true) || written["graph_complete"] == json!(true),
        "the document does not say the run settled, so refusing to read it proves nothing: \
         {written}"
    );

    let mut older = written.clone();
    older["schema_version"] = json!(SUMMARY_SCHEMA_VERSION - 1);
    std::fs::write(paths.summary(), older.to_string()).expect("the document");

    // Refreshed and excluded: nothing on either stream, and the document left
    // behind is this build's own, current for the journal.
    let asked = world.run(&["unwatched"]);
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a settled run the previous release recorded was written about: stdout {:?}, \
         stderr {:?}",
        asked.stdout,
        asked.stderr
    );
    let refreshed = document(&paths);
    assert_eq!(refreshed["schema_version"], json!(SUMMARY_SCHEMA_VERSION));
    assert_eq!(
        (
            refreshed["journal_len"].as_u64(),
            refreshed["journal_mtime_ms"].as_u64()
        ),
        stamp_of(&paths),
        "the refreshed document is behind its journal: {refreshed}"
    );

    // The other half of the rule: a run still recording under the previous
    // release's document is reported from what its store says, never silenced.
    // The run the mixed-root half below leaves undecidable is settled first, while
    // the worker script still runs to completion.
    let undecided = settled(&world, "unwatchedalongside");
    let undecided_paths = paths_of(&world, &undecided);
    world.script("build.wait", "hold");
    let recording = held(&world, "unwatchedrecording");
    let recording_paths = paths_of(&world, &recording);
    let mut older = document(&recording_paths);
    older["schema_version"] = json!(SUMMARY_SCHEMA_VERSION - 1);
    std::fs::write(recording_paths.summary(), older.to_string()).expect("the document");
    let asked = world.run(&["unwatched"]);
    asked
        .exited(RUNS_UNWATCHED)
        .out_has(&recording)
        .out_has("ACTIVE")
        .out_has("nothing has recorded a watch on it")
        .out_has(&format!("onepipeline watch {recording}"));
    assert!(
        !asked.stdout.contains(&run),
        "the settled run reached standard output beside the recording one: {}",
        asked.stdout
    );
    assert!(
        asked.stderr.is_empty(),
        "a run this verb decided about was also named as one it could not: {}",
        asked.stderr
    );
    assert_eq!(
        document(&recording_paths)["schema_version"],
        json!(SUMMARY_SCHEMA_VERSION)
    );

    // And beside a run nothing could be decided about, each answer lands where it
    // belongs: the recording one on standard output and in the status, the
    // undecided one on standard error and in neither.
    std::fs::remove_file(undecided_paths.summary()).expect("the document");
    let asked = world.run(&["unwatched"]);
    asked
        .exited(RUNS_UNWATCHED)
        .out_has(&recording)
        .err_has(&undecided);
    assert!(
        !asked.stdout.contains(&undecided),
        "the undecided run reached standard output: {}",
        asked.stdout
    );
    world.release("build.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// One state a summary document can be left in, and the way it is reached: what it
/// is called, and what to do to the file to put it there.
type Undecidable = (&'static str, fn(&std::path::Path, &Value));

/// The five states a run's summary document can be in that leave its settlement
/// undecidable, each with the way that state is reached.
///
/// Function pointers rather than a table of data, because what distinguishes them
/// is what is done to the file: a build that never wrote a document, a writer that
/// recorded a settlement and died before its stamp caught the journal up, a writer
/// killed mid-write, a build that knows more than this one, and a run root copied
/// from another run's.
///
/// **A schema this build has moved past is deliberately not here.** It is the
/// previous release's document, which this build refreshes — folds once and
/// rewrites at its own schema — and then decides from the run's own record;
/// `a_document_at_a_superseded_schema_is_refreshed_and_decided_from_the_run` is
/// that half. A version *ahead* of this build is the one that stays here, and it
/// is a different fact: a document written by a build that knows things this one
/// does not, which this one has no reading of at all.
///
// llmlint: ignore-block[tests_mirror_real_usage] no verb removes or corrupts the document
// its run's journal writer maintains, and none could — each of the five is left by a
// build, a crash, or a copied run root rather than by an interface. What is put back is
// this build's own document, edited only where the state under test is the edit, and every
// claim afterwards is read off the compiled binary's own streams.
const UNDECIDABLE: [Undecidable; 5] = [
    ("no document at all", |path, _written| {
        std::fs::remove_file(path).expect("the document");
    }),
    (
        "a document recording settlement while behind its journal",
        |path, written| {
            let mut behind = written.clone();
            behind["journal_len"] = json!(0);
            std::fs::write(path, behind.to_string()).expect("the document");
        },
    ),
    ("a document a writer left half-written", |path, _written| {
        std::fs::write(path, "{\"schema_ver").expect("the document");
    }),
    (
        "a document at the schema a later build wrote",
        |path, written| {
            let mut ahead = written.clone();
            ahead["schema_version"] = json!(SUMMARY_SCHEMA_VERSION + 1);
            std::fs::write(path, ahead.to_string()).expect("the document");
        },
    ),
    ("a document that is another run's", |path, written| {
        let mut copied = written.clone();
        copied["run_id"] = json!("some-other-run");
        std::fs::write(path, copied.to_string()).expect("the document");
    }),
];
// llmlint: ignore-end[tests_mirror_real_usage]

/// The verb decides a run whose merged store **cannot be read at all** exactly as it
/// decides one whose store is readable — reporting the unsettled run, excluding the
/// settled one, with each document current about what is at the store's path.
///
/// The direct proof that nothing on this path folds, and it does not depend on a
/// clock. A stopwatch over a bigger journal can only ask whether the store was read
/// by inference; this asks the filesystem, which refuses outright. A verb that
/// folded would meet an error where a run's records are and decide out of an empty
/// graph — reporting the settled run, or dropping the unsettled one's word — and a
/// verb that reads the stored summary cannot tell the difference.
///
/// **Unreadable rather than absent**, so the document stays current on the ordinary
/// terms: a real length and a real modification time it still matches, which is what
/// every live run's document carries. The store is made unreadable by putting a
/// **directory** where the records go — a path `stat` answers for and no process on
/// any platform can read as a file, not a privileged one, which is what a mode of
/// `000` cannot promise. `results` over the same run is the discriminator: it folds
/// by design, and it loses what this verb keeps.
#[test]
fn the_verb_decides_runs_whose_merged_store_cannot_be_read() {
    let world = World::new("unwatched-unreadablestore");
    world.script("build.work", "the worker wrote this\n");
    let settled_run = settled(&world, "unwatchedsettledstore");
    world.script("build.wait", "hold");
    let reported = held(&world, "unwatchedheldstore");

    let before = world.run(&["unwatched"]);
    before.exited(RUNS_UNWATCHED).out_has(&reported);
    let answer = before.stdout.clone();
    assert!(
        !answer.contains(&settled_run),
        "the settled run was reported before its store went anywhere: {answer}"
    );
    let folded_before = world.run(&["results", &reported]).exited(0).stdout.clone();

    for run in [&settled_run, &reported] {
        store_unreadable(&paths_of(&world, run));
        assert!(
            std::fs::read(paths_of(&world, run).journal()).is_err(),
            "{run}'s store can still be read, so nothing below is a claim"
        );
    }

    let after = world.run(&["unwatched"]);
    after.exited(RUNS_UNWATCHED);
    assert_eq!(
        after.stdout, answer,
        "the verb answered differently once the stores it must not read could not be read"
    );
    assert!(
        after.stderr.is_empty(),
        "a run whose settlement was decided from a current document was reported \
         undecidable once its store became unreadable: {}",
        after.stderr
    );
    // And the stores really were where a fold would have gone: the view that folds
    // cannot render them any more.
    let folded_after = world.run(&["results", &reported]).exited(0).stdout.clone();
    assert_ne!(
        folded_after, folded_before,
        "the detail read was unchanged by the store becoming unreadable, so this journey \
         proved nothing: {folded_after}"
    );
    world.release("build.go");
}

/// Put something at the store's path that **cannot be read as a file**, leaving the
/// document current about it.
///
/// A directory, because it is the one form of "unreadable" every platform agrees on
/// and no privilege overrides: `open` refuses it everywhere and `stat` still answers,
/// so the length and modification time the document is stamped with are real
/// readings of what is at that path.
///
// llmlint: ignore-block[tests_mirror_real_usage] no verb puts a directory where a run's
// records go, and none could: what it stands in for is a store this process cannot read — a
// permission, a device that is refusing, a file another mount has taken over — and the
// document beside it is this build's own carrying the stamp this build's own writer records
// for what the path now holds. Every claim afterwards is read off the compiled binary's own
// streams.
fn store_unreadable(paths: &RunPaths) {
    std::fs::remove_file(paths.journal()).expect("the run's merged store");
    std::fs::create_dir(paths.journal()).expect("something unreadable at the store's path");
    let about = std::fs::metadata(paths.journal()).expect("what is at the store's path");
    let modified = about
        .modified()
        .expect("a modification time")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("an instant past the epoch")
        .as_millis();
    let mut summary = document(paths);
    summary["journal_len"] = json!(about.len());
    summary["journal_mtime_ms"] = json!(u64::try_from(modified).expect("a millisecond count"));
    std::fs::write(paths.summary(), summary.to_string()).expect("the document");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A run whose settlement cannot be decided at all is named on **standard error**
/// with the reason, is absent from standard output, and changes no exit status.
///
/// Five ways to reach it, because they are one fact to a reader and five different
/// things on disk: no document, one that cannot be read, one that **records
/// settlement while behind its journal** — its own record says it settled but a
/// stale stamp proves nothing it says — one at a schema version **ahead** of this
/// build, and one that is *another run's* — which is what a copied run root
/// leaves, and is no more a description of this run than a document nobody wrote.
/// Such a run is most often an old settled run whose document is gone, and
/// blocking on it would never clear by watching it — so it is said out loud and
/// passed over.
///
/// A schema this build has **moved past** is not one of the five, and the reason it
/// is not is stated where the five are.
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

/// A run root with no readable launch record — absent, or half-written — and one
/// naming no session are passed over with nothing said about them on either stream.
///
/// Ownership is a **positive claim**. The host this was written for holds seventy
/// run roots with no launch record at all, and a verb that ran at the end of every
/// turn and named them would be noise on every turn.
#[test]
fn roots_with_no_readable_launch_record_and_one_naming_no_session_are_passed_over() {
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

    // And two directories that claim to be runs and record no launch this build can
    // read: one with no record at all, and one whose record is not a record.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] no verb makes a run root without a
    // launch record — a `start` writes one before anything else — and none leaves half of
    // one behind, so both states are reached by making them. The first is the state seventy
    // roots on the host this verb was written for are in, left by builds and sweeps that
    // predate the record; the second is a writer that died mid-write.
    let orphan = world.runs.join("unwatchedorphan");
    std::fs::create_dir_all(&orphan).expect("a run root");
    let torn = world.runs.join("unwatchedtorn");
    std::fs::create_dir_all(&torn).expect("a run root");
    std::fs::write(torn.join("launch.json"), "{\"run_id\": \"unwatchedt").expect("half a record");
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
    for passed_over in [
        &nameless,
        &"unwatchedorphan".to_string(),
        &"unwatchedtorn".to_string(),
    ] {
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

    // An option carrying nothing is a refusal rather than a fall-through: the
    // environment here names a session that owns a reportable run, and answering
    // out of it would be reporting on whichever session the hook happens to run
    // under — the one mistake the option exists to make impossible.
    let blank = world.run(&["unwatched", "--session", ""]);
    assert!(
        blank.code != SUCCESS && blank.code != RUNS_UNWATCHED,
        "`--session \"\"` was answered out of the environment: {} {}",
        blank.code,
        blank.stdout
    );
    blank.err_has("no session");
    assert!(
        blank.stdout.is_empty(),
        "a refusal wrote to standard output: {}",
        blank.stdout
    );

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
/// A caller has to tell "no run is unwatched" from "this verb could not answer":
/// the hook treats every status but `0` and the unwatched answer as silence, and an
/// operator in a plain shell is told why on standard error.
#[test]
fn a_question_with_no_session_at_all_is_refused_rather_than_answered() {
    let world = World::new("unwatched-nosession");
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
}

/// The other question it cannot ask: a runs root that is there and cannot be read
/// as a whole.
///
/// Answering it as "nothing is unwatched" is the silence the whole verb exists to
/// end, so it is the same refusal the missing session gets rather than a `0` — and
/// the refusal names the root, because an operator pointed at the wrong path has
/// nothing else to go on.
///
/// **Staged as a runs root that is a file**, which is a state every platform has
/// and every process meets the same way: `read_dir` refuses it with something that
/// is not "no such directory", which is the whole of what this path distinguishes.
/// A directory whose mode forbids listing is the same refusal by a different
/// errno, and staging *that* is what this journey used to do — but a mode is a
/// thing Windows does not have and the kernel ignores for a privileged process, so
/// the journey could return having asserted nothing, which is not a journey.
#[test]
fn a_runs_root_that_cannot_be_read_is_refused_rather_than_answered() {
    let world = World::new("unwatched-unreadableroot");
    // llmlint: ignore-block[tests_mirror_real_usage] no verb makes a runs root out of a
    // file, and none could: what this stands in for is an operator or a harness pointing
    // `ONEPIPELINE_RUNS_DIR` at something that is not a directory of runs, and the binary
    // meets it exactly as it meets a directory it may not list. Everything asserted after
    // it is read off the compiled binary's own streams.
    let unreadable = world.root.join("runs-that-are-a-file");
    std::fs::write(&unreadable, "not a directory of runs").expect("something in the way");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let asked = world
        .cmd(&["unwatched"])
        .env("ONEPIPELINE_RUNS_DIR", &unreadable)
        .output()
        .expect("the binary runs");
    let code = asked.status.code();
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

/// How many run roots the scale journey assembles.
///
/// Four hundred, because that is the shape the bound is about: a supervisory host
/// accumulates run roots and never sheds them, and the one this verb was written
/// for held 491 when the work began.
const SCALED_RUNS: usize = 400;

/// How many bytes of journal those roots hold together at the first measurement.
///
/// One gibibyte, spread evenly. The host this was written for held 11 GB across
/// its roots; a gibibyte is the smallest total at which reading them would be
/// unmistakably the thing being paid for rather than process start.
const SCALED_JOURNAL_BYTES: u64 = 1 << 30;

/// What the second measurement multiplies that total by, with the run count held
/// fixed.
///
/// The two measurements together are the claim: the first says this verb is fast
/// enough to ask at the end of every turn, and the second says what it is fast
/// *because of* — a tenfold change in the one input a fold is linear in barely
/// moves it.
const JOURNAL_MULTIPLE: u64 = 10;

/// How many of those runs the asked-about session owns.
///
/// **A handful**, because that is the shape: a manager holds a few live runs on a
/// host that has accumulated hundreds, and everything this verb does past
/// discovery is proportional to the few.
const SCALED_OWNED: usize = 3;

/// What the median of five consecutive invocations may take.
///
/// Half a second, because of *where* this runs: at the end of every manager turn,
/// in a hook the harness waits for. A check that cost seconds would be turned off,
/// and a check that is turned off is the prose it replaced.
const ASKING_BOUND: std::time::Duration = std::time::Duration::from_millis(500);

/// What no single one of those five may take.
///
/// A second — twice the median bound, so one unlucky invocation on a loaded host
/// is a bound rather than a flake, and a verb that occasionally took seconds still
/// fails.
const SINGLE_BOUND: std::time::Duration = std::time::Duration::from_secs(1);

/// A **regression guard**, and deliberately not the evidence for anything.
///
/// The same value and the same reasoning as the listing journey's constant of this
/// name, which is where both are stated: a ratio between medians this small
/// measures the host, five clears the worst noise yet seen on unchanged code and
/// still fails a fold. What proves the journal goes unread is
/// [`the_verb_decides_runs_whose_merged_store_cannot_be_read`] — a store that
/// cannot be read at all, which no clock enters into.
const GROWTH_GUARD: u32 = 5;

/// How many invocations each median is taken over, after one warm-up.
///
/// The warm-up is discarded because what it measures is mostly a debug binary
/// nobody has paged in, and a median rather than a mean because the outlier this
/// has to survive is another test on the same host, not a slow invocation.
const TIMED: usize = 5;

/// One run root cloned from a real one, under a new id and a new owner.
///
/// Everything but the journal is the template's own file, because everything but
/// the journal is what this verb reads: the launch record and the summary document
/// are this build's own, written by a real run driven through the binary, and only
/// the three facts that must name *this* run rather than the one it was copied from
/// are rewritten.
// llmlint: ignore-block[tests_mirror_real_usage] four hundred run roots is a *host*, not a
// command: no verb makes one, and the only honest way to hold the shape is to assemble it.
// What is assembled is this build's own output — one real run driven through the compiled
// binary, cloned — rather than documents invented here.
fn cloned_run(template: &RunPaths, root: &std::path::Path, id: &str, session: &str) -> RunPaths {
    let paths = RunPaths::under(root, id);
    std::fs::create_dir_all(&paths.dir).expect("a run root");
    let journal = template
        .journal()
        .file_name()
        .expect("the journal has a name")
        .to_owned();
    for entry in std::fs::read_dir(&template.dir).expect("the template run") {
        let entry = entry.expect("an entry of the template run");
        if !entry.file_type().expect("its kind").is_file() || entry.file_name() == journal {
            continue;
        }
        std::fs::copy(entry.path(), paths.dir.join(entry.file_name())).expect("a copied file");
    }
    for path in [paths.launch(), paths.summary()] {
        let mut held: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("a copied document"))
                .expect("a document");
        held["run_id"] = json!(id);
        held["session"] = json!(session);
        // A pid means nothing across machines, so a run recorded elsewhere reads as
        // the live work it is — which is the **expensive** row: it is not excluded,
        // so it goes on to be decided, to have its watchers read, and to be given a
        // word.
        held["host"] = json!("another-host");
        std::fs::write(&path, held.to_string()).expect("the document");
    }
    paths
}

/// Grow one run's journal to `bytes` and stamp its document for the store that
/// leaves.
///
/// The filler is the template run's **own records**, repeated: real journal lines,
/// so a reader that folded one of these would do the work a fold of a real run
/// does. The stamp is written after the bytes, so what the document claims is what
/// the file holds — which is what makes it *current*, and the state the bound is
/// about.
fn journal_of(paths: &RunPaths, filler: &[u8], bytes: u64) {
    use std::io::Write;
    let mut file = std::io::BufWriter::new(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.journal())
            .expect("the run's journal"),
    );
    let mut held = std::fs::metadata(paths.journal()).map_or(0, |about| about.len());
    while held < bytes {
        file.write_all(filler).expect("journal records");
        held += filler.len() as u64;
    }
    file.flush().expect("journal records");
    // Forced to the device before this returns, rather than left as dirty pages for
    // the kernel to write back **while the next measurement runs**: ten gibibytes of
    // writeback competing with the reads being timed is the host's clock rather than
    // this verb's.
    file.get_ref().sync_all().expect("journal records on disk");
    drop(file);

    let about = std::fs::metadata(paths.journal()).expect("the run's journal");
    let modified = about
        .modified()
        .expect("a modification time")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("an instant past the epoch")
        .as_millis();
    let mut summary = document(paths);
    summary["journal_len"] = json!(about.len());
    summary["journal_mtime_ms"] = json!(u64::try_from(modified).expect("a millisecond count"));
    std::fs::write(paths.summary(), summary.to_string()).expect("the document");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// One invocation's stdout and status, taken **without** the harness's own look at
/// the world.
///
/// `World::run` captures a dump of every run root beside the output, which is
/// exactly the read this journey is about the binary not making — over four hundred
/// roots it is the harness that would be reading the gigabytes, and every figure
/// below would be its.
fn asked(world: &World, argv: &[&str]) -> (i32, String) {
    let out = world.cmd(argv).output().expect("the binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn median(world: &World, argv: &[&str], expected: i32) -> std::time::Duration {
    let mut took: Vec<std::time::Duration> = Vec::with_capacity(TIMED);
    for nth in 0..=TIMED {
        let began = std::time::Instant::now();
        let out = world.cmd(argv).output().expect("the binary runs");
        let elapsed = began.elapsed();
        assert_eq!(
            out.status.code(),
            Some(expected),
            "`onepipeline {}` answered {:?}: {}",
            argv.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        // The first is the warm-up: what it measures is mostly a debug binary
        // nobody has paged in.
        if nth > 0 {
            took.push(elapsed);
        }
    }
    took.sort_unstable();
    println!("  {:<16} {took:?}", argv.join(" "));
    assert!(
        took[TIMED - 1] < SINGLE_BOUND,
        "one of five invocations took {:?}, past the {SINGLE_BOUND:?} no single one may take",
        took[TIMED - 1]
    );
    took[TIMED / 2]
}

/// Over a host-sized runs root this verb answers in well under a second.
///
/// **The bound is the claim**, and it is about where this runs: at the end of every
/// manager turn, in a hook the harness waits for, so a check that cost seconds
/// would be turned off — and a check that is turned off is the prose it replaced.
/// Both measurements are held to it, over the unmultiplied root and over ten times
/// the journal bytes.
///
/// The ratio between those two is kept as [`GROWTH_GUARD`] and is **not** evidence
/// that the journal goes unread; that constant says why, and
/// [`the_verb_decides_runs_whose_merged_store_cannot_be_read`] is where the property
/// is actually proven — over a store that cannot be read at all, which no clock
/// enters into.
///
/// **What the rows are is asserted, not assumed.** A clock over a degenerate
/// fixture is the one way this journey could report green while measuring nothing,
/// so before it times anything it reads the reported rows back and holds them to
/// the shape the bound is set against.
#[test]
fn a_host_sized_runs_root_is_answered_in_well_under_a_second_whatever_its_journals_hold() {
    // Every run on this root was last written moments ago, so the threshold that
    // decides `PARKED` is moved down to make the rows take the **whole** path a
    // real host's older runs take — the run's channel read included. A bound
    // measured over rows that skipped it would be a bound about the cheap case.
    let world = World::new("unwatched-scale").with_env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    world.script("build.wait", "hold");

    // The template: a real run with a dispatch held open, which is what an
    // unwatched run *is*. Cloned before it is stopped, so every clone carries an
    // unsettled document; the template itself is then stopped, which excludes it
    // and leaves nothing writing to this root while the clock runs.
    let template = paths_of(&world, &held(&world, "unwatchedtemplate"));
    let filler = std::fs::read(template.journal()).expect("the template's own records");
    assert!(!filler.is_empty(), "the template run recorded nothing");

    let per_run = SCALED_JOURNAL_BYTES / SCALED_RUNS as u64;
    let stranger = "another-planner";
    let mut assembled: Vec<RunPaths> = Vec::new();
    for nth in 0..SCALED_RUNS {
        let owner = if nth < SCALED_OWNED {
            world.session.as_str()
        } else {
            stranger
        };
        let paths = cloned_run(&template, &world.runs, &format!("scaled-{nth:04}"), owner);
        journal_of(&paths, &filler, per_run);
        assembled.push(paths);
    }
    world.run(&["stop", &template.run, "--force"]).exited(0);
    world.release("build.go");

    let held_bytes = |assembled: &[RunPaths]| -> u64 {
        assembled
            .iter()
            .map(|paths| std::fs::metadata(paths.journal()).map_or(0, |about| about.len()))
            .sum()
    };
    let bytes = held_bytes(&assembled);
    assert!(
        bytes >= SCALED_JOURNAL_BYTES,
        "the root holds {bytes} journal byte(s), short of the {SCALED_JOURNAL_BYTES} this \
         measures over"
    );
    assert!(
        std::fs::read_dir(&world.runs)
            .expect("the runs root")
            .count()
            >= SCALED_RUNS,
        "the root does not hold the {SCALED_RUNS} run roots this bound is about"
    );

    // Exactly the runs this session owns, out of the four hundred on the root, and
    // each row is the shape the bound is set against.
    let (code, rows) = asked(&world, &["unwatched"]);
    assert_eq!(code, RUNS_UNWATCHED, "{rows}");
    assert_eq!(
        rows.lines().count(),
        SCALED_OWNED,
        "over {SCALED_RUNS} run roots this verb reported something other than the \
         {SCALED_OWNED} unwatched runs this session owns:\n{rows}"
    );
    for row in rows.lines() {
        assert!(
            STANDING_WORDS.iter().any(|word| row.contains(word)),
            "a reported row carries none of {STANDING_WORDS:?}, so this fixture is not the \
             shape it measures: {row}"
        );
        assert!(
            row.contains("onepipeline watch"),
            "a reported row does not say what to do about it: {row}"
        );
    }
    // Said out loud, because the whole bound below is a clock over these rows and a
    // reader who cannot see what they are cannot weigh it.
    println!("  the rows this bound is measured over:\n{rows}");

    // Before the first measurement, so both are taken over a disk that has finished
    // with the fixture rather than one still writing it: what is being compared is
    // the verb, and the ten gibibytes written below would otherwise be in the second
    // figure and not the first.
    let_writeback_settle();
    binary_in_cache();
    let before = median(&world, &["unwatched"], RUNS_UNWATCHED);
    assert!(
        before < ASKING_BOUND,
        "`onepipeline unwatched` took {before:?} over {SCALED_RUNS} run roots holding {bytes} \
         journal byte(s), past the {ASKING_BOUND:?} a check asked at the end of every turn is \
         held to"
    );

    // The same root, with the one input a fold is linear in multiplied by ten and
    // the run count held exactly where it was.
    for paths in &assembled {
        let held = std::fs::metadata(paths.journal())
            .expect("the journal")
            .len();
        journal_of(paths, &filler, held * JOURNAL_MULTIPLE);
    }
    // The documents this verb reads, back in the cache the first measurement found
    // them in. Writing ten gibibytes evicts them, and a median taken over cold
    // documents is a reading of the host's page cache rather than of the verb.
    for paths in &assembled {
        for document in [paths.summary(), paths.launch()] {
            let _ = std::fs::read(document);
        }
    }
    // And again, for the write that just happened — this is the one the ratio is
    // about, and `fsync` per file only puts that file's pages on the device.
    let_writeback_settle();
    let grown = held_bytes(&assembled);
    assert!(
        grown >= bytes * JOURNAL_MULTIPLE,
        "the grown root holds {grown} journal byte(s), short of {JOURNAL_MULTIPLE} times the \
         {bytes} it held"
    );
    let (code, grown_rows) = asked(&world, &["unwatched"]);
    assert_eq!(code, RUNS_UNWATCHED);
    assert_eq!(
        grown_rows, rows,
        "the answer changed when the journals grew, so the run count or the fixture moved"
    );

    binary_in_cache();
    let after = median(&world, &["unwatched"], RUNS_UNWATCHED);
    assert!(
        after <= before * GROWTH_GUARD,
        "`onepipeline unwatched` took {after:?} over {grown} journal byte(s) against {before:?} \
         over {bytes} — {JOURNAL_MULTIPLE} times the bytes moved it past the {GROWTH_GUARD}x \
         regression guard, which no run of this has approached"
    );
    // And the grown root is held to the same absolute bound the unmultiplied one
    // is, which is the statement that does not depend on a ratio at all: ten
    // gibibytes of journal do not put this verb anywhere near what a hook can wait
    // for.
    assert!(
        after < ASKING_BOUND,
        "`onepipeline unwatched` took {after:?} over {grown} journal byte(s), past the \
         {ASKING_BOUND:?} a check asked at the end of every turn is held to"
    );
    println!(
        "  unwatched     {before:?} over {SCALED_RUNS} roots holding {bytes} journal byte(s), \
         {after:?} over {grown}"
    );
}

/// Every path the process **opened**, as the kernel recorded it, whatever status it
/// returned.
///
/// The command is the one `World` composes — same binary, same environment — with
/// the tracer wrapped around it, so what is observed is the invocation a user makes
/// rather than a second one assembled here. The status is not asserted, because
/// this verb's whole answer *is* a status and the trace is about what it read to
/// reach one.
///
/// It **refuses** rather than passes where the tracer will not run: an observation
/// nobody made is not an observation of nothing, and this is the one journey whose
/// claim is entirely about what the process did.
#[cfg(target_os = "linux")]
fn opened_by(world: &World, argv: &[&str], into: &std::path::Path) -> Vec<String> {
    let inner = world.cmd(argv);
    let mut traced = std::process::Command::new("strace");
    traced
        // Children too: a verb that shelled out to read a store would have read it
        // just the same.
        .arg("-f")
        .arg("-qq")
        .arg("-e")
        .arg("trace=openat,open")
        .arg("-o")
        .arg(into)
        .arg(inner.get_program())
        .args(inner.get_args())
        .stdin(std::process::Stdio::null());
    for (key, value) in inner.get_envs() {
        match value {
            Some(value) => traced.env(key, value),
            None => traced.env_remove(key),
        };
    }
    traced.output().unwrap_or_else(|error| {
        panic!(
            "this journey's whole claim is what the process opened, and the tracer would not \
             run: strace: {error}. Install strace, or run the suite where ptrace is permitted \
             — a journey that cannot observe is not a journey that observed nothing."
        )
    });
    let trace = std::fs::read_to_string(into).expect("the trace the tracer wrote");
    // Each line names its path in the first quoted field, and a line carrying no
    // path is a resumption or a signal rather than an open.
    let opened: Vec<String> = trace
        .lines()
        .filter_map(|line| line.split_once('"'))
        .filter_map(|(_, rest)| rest.split_once('"'))
        .map(|(path, _)| path.to_owned())
        .collect();
    assert!(
        !opened.is_empty(),
        "the tracer recorded no open at all, so it observed nothing: {trace}"
    );
    opened
}

/// `unwatched` opens **no run's merged event store**, over a root where every store
/// is present — including the runs whose summary document is absent and stale,
/// which are exactly the two a reader is tempted to fold.
///
/// What the wall clock cannot establish. A bound over a host-sized root rules out a
/// fold that dominates the clock; it would still pass a process that opened a store
/// and threw the bytes away, and on a host holding eleven gigabytes of journals
/// throwing them away is the whole cost. So this asks the **kernel** what the
/// process opened.
///
/// The **positive control** is what makes the silence a measurement: `results` over
/// one of the same runs is a detail read, is traced the same way in the same
/// journey, and does open the store. A tracer that saw nothing at all would pass the
/// first half and fail the second.
///
/// **Linux, and deliberately not skipped anywhere.** There is no portable way to ask
/// another process what it opened, so the observation is made on the platform this
/// crate's deterministic tier and coverage floor are measured on, where it runs every
/// time and refuses if it cannot.
#[cfg(target_os = "linux")]
#[test]
fn unwatched_opens_no_run_store_that_is_there() {
    let world = World::new("unwatched-traced");
    world.script("build.work", "the worker wrote this\n");
    // Three runs this session owns, in the three states this verb's settlement
    // reading meets: a current document, none at all, and one behind its journal.
    let current = settled(&world, "unwatchedcurrent");
    let absent = settled(&world, "unwatchedabsent");
    let stale = settled(&world, "unwatchedstale");
    world.script("build.wait", "hold");
    let reported = held(&world, "unwatchedheld");

    // llmlint: ignore-block[tests_mirror_real_usage] no verb removes the document its run's
    // journal writer maintains, and none moves it behind its journal: a build that never
    // wrote one and a writer killed between its append and the document beside it are the two
    // states under test. The stale document is this build's own with one recorded length
    // moved, which is exactly what that writer would have left.
    std::fs::remove_file(paths_of(&world, &absent).summary()).expect("the document");
    let paths = paths_of(&world, &stale);
    let mut behind = document(&paths);
    behind["journal_len"] = json!(1);
    std::fs::write(paths.summary(), behind.to_string()).expect("the document");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let watched: Vec<String> = vec![current, absent, stale, reported.clone()];
    for run in &watched {
        assert!(
            paths_of(&world, run).journal().is_file(),
            "{run}'s merged store is not there, so this journey would be about nothing"
        );
    }

    let opened = opened_by(&world, &["unwatched"], &world.root.join("trace.txt"));
    // The trace is of the read under test: the launch records are what discovery
    // opens, and this verb opened them.
    for run in &watched {
        let launch = paths_of(&world, run).launch().display().to_string();
        assert!(
            opened.contains(&launch),
            "`onepipeline unwatched` never opened {run}'s launch record, so this trace is not \
             of the read under test: {opened:?}"
        );
    }
    for run in &watched {
        let journal = paths_of(&world, run).journal();
        assert!(
            !opened.contains(&journal.display().to_string()),
            "`onepipeline unwatched` opened {}: this verb may not read a run's merged event \
             store, for any run and least of all for one whose document is absent or stale",
            journal.display()
        );
    }
    // And nothing named like one, however it was reached — a store opened through a
    // relative path or another run's root is the same read.
    let named_like_a_store = std::path::Path::new(&paths_of(&world, &reported).journal())
        .file_name()
        .expect("the journal has a name")
        .to_owned();
    assert!(
        !opened
            .iter()
            .any(|path| std::path::Path::new(path).file_name() == Some(&*named_like_a_store)),
        "`onepipeline unwatched` opened a run's merged event store: {opened:?}"
    );

    // The control. `results` folds by design, so it opens exactly what the verb
    // above must not — which is what says the tracer was watching this binary's
    // opens rather than recording an empty room.
    let opened = opened_by(
        &world,
        &["results", &reported],
        &world.root.join("trace-detail.txt"),
    );
    let journal = paths_of(&world, &reported).journal().display().to_string();
    assert!(
        opened.contains(&journal),
        "`results` did not open the store it folds, so nothing above was observed: {opened:?}"
    );
    world.release("build.go");
}

/// Arming a watch sweeps the records this host has **proved the process of is
/// gone**, and removes nothing else.
///
/// The bound on a directory that would otherwise grow for ever: a run watched a
/// thousand times over a week would hold a thousand records, all but one of them
/// about processes that are gone. What makes the sweep safe is what it refuses to
/// touch, and the four kinds it refuses are each here — a record naming another
/// host, whose pid means nothing here; one this build could not read at all; one
/// whose pid **is** live; and the one this journey exists for, a record nothing can
/// judge because it carries no start token.
///
/// **That last one is the distinction between reporting and removing**, and they
/// are two different bars. The run is *reported* unwatched on it, because an
/// absence of evidence resolves that way and being wrong costs one re-armed watch.
/// The record is *not deleted* on it, because deletion cannot be taken back and
/// would destroy the only thing that could ever have said the watcher was alive: a
/// single failed reading of a live watch's start would erase its record for ever,
/// its supervisor would read the run unwatched and arm a second watch, and a hook
/// refusing to end a turn on that reading would refuse over a run that was never
/// unwatched at all.
///
/// **The sweep is not what makes a dead watcher read as gone**, and this journey is
/// careful to say so: the run reads unwatched *before* anything is swept, on the
/// strength of the reading alone.
#[test]
fn arming_a_watch_sweeps_what_it_proved_is_gone_and_keeps_what_it_could_not_judge() {
    let world = World::new("unwatched-sweep");
    world.script("build.wait", "hold");
    let run = held(&world, "unwatchedsweep");

    // A record this host can prove is not a live watch: a real watch's own, whose
    // process has since been reaped.
    let mut watching = arm(&world, &run);
    let dead_pid = watching.id();
    watching.kill().expect("the watch takes the signal");
    watching.wait().expect("the watch ends");
    let [dead] = &records_under(&world, &run)[..] else {
        panic!("the killed watch did not leave exactly one record");
    };
    let dead = dead.clone();
    let template = record(&dead);

    // Beside it, the records a sweep may not touch.
    let mut elsewhere = template.clone();
    elsewhere["host"] = json!("another-host");
    let foreign = put(
        &world,
        &run,
        "4243-0badc0ffee000002.json",
        &elsewhere.to_string(),
    );
    let unreadable = put(&world, &run, "4244-0badc0ffee000003.json", "{\"schema_ver");
    // The one this journey exists for: a live pid, and no start token to judge it
    // by — which is what a watch armed on a host that will not report one leaves.
    // This process is the live pid, because a test process is emphatically alive.
    //
    // **The half this environment cannot stage** is the mirror of it: a live pid
    // whose token this host will not give *now*, against a record that carries one.
    // On Linux a live process's `/proc/<pid>/stat` is always readable, so there is
    // no such pid to point a record at — and a reading that came back empty would
    // mean the process is gone, which is a different standing decided two lines
    // earlier. That half is driven directly, on the function that decides it, by
    // `src/watchers.rs`'s `a_token_this_host_would_not_give_is_not_a_mismatch`;
    // both halves are the same standing, so a regression folding either back into
    // a proven mismatch fails one of the two.
    let mut unjudgeable = template.clone();
    unjudgeable["pid"] = json!(std::process::id());
    unjudgeable["started"] = json!("");
    let unproven = put(
        &world,
        &run,
        &format!("{}-0badc0ffee000004.json", std::process::id()),
        &unjudgeable.to_string(),
    );
    // A proven mismatch beside it, so the two readings are told apart by what they
    // are rather than by which file they are in: the same live pid, with a token
    // that was read and disagrees.
    let mut handed_on = unjudgeable.clone();
    handed_on["started"] = json!("linux-proc-stat:1");
    let mismatched = put(
        &world,
        &run,
        &format!("{}-0badc0ffee000005.json", std::process::id()),
        &handed_on.to_string(),
    );

    // The run reads unwatched now, before anything has swept anything: the reading
    // is what decides it, and the sweep is only housekeeping.
    world
        .run(&["unwatched"])
        .exited(RUNS_UNWATCHED)
        .out_has(&run);
    assert!(
        dead.exists(),
        "reading the run removed a record, and the reading side removes nothing"
    );

    // A fresh watch, which is the writer that sweeps.
    let watching = arm(&world, &run);
    assert!(
        !dead.exists(),
        "the record of the reaped watch (pid {dead_pid}) was not swept, so a long-watched \
         run accumulates them without bound"
    );
    assert!(
        foreign.exists(),
        "the sweep removed a record naming another host, about which this host has proved \
         nothing"
    );
    assert!(
        unreadable.exists(),
        "the sweep removed a record it could not read, which is another build's evidence"
    );
    assert!(
        unproven.exists(),
        "the sweep removed a record nothing could judge — a live pid with no start token \
         to compare — which is a live watcher's record erased for ever on the strength of a \
         reading nobody took"
    );
    assert!(
        !mismatched.exists(),
        "the sweep kept a record whose token was read and disagreed, so removal is not \
         deciding on the evidence it has: the pid has been handed to another process and \
         the one that wrote this record is gone"
    );
    // And the run is watched again, by the watch that did the sweeping — over a
    // directory that still holds the record nothing could judge.
    world.run(&["unwatched"]).exited(SUCCESS);
    assert!(unproven.exists(), "the second reading removed it instead");

    world.run(&["stop", &run, "--force"]).exited(0);
    watching.wait_with_output().expect("the watch returns");
    world.release("build.go");
}

/// A watch whose record cannot be written **runs exactly as it does without one**,
/// and the run it is watching reads unwatched with the reason said out loud.
///
/// The write is best effort and silent, and this is both halves of what that has to
/// mean. A runs root this process may not write costs a reader the knowledge that
/// this watch exists and costs the watch itself nothing — so the verb's status and
/// its records are the same as a watch that recorded itself, asserted against a
/// control run in the same journey rather than against a remembered value. And the
/// question asked about the run answers **unwatched**, because that is where every
/// unknown here resolves: a watch nothing recorded is indistinguishable from no
/// watch, and re-arming one is what clears it.
#[test]
fn a_watch_that_cannot_write_its_record_runs_as_it_does_with_one() {
    let world = World::new("unwatched-unwritable");
    world.script("build.wait", "hold");
    let blocked = held(&world, "unwatchedblocked");
    let control = held(&world, "unwatchedcontrol");

    // A file where the directory of records goes, so creating it fails on every
    // platform this builds for.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] no verb puts a file there, and what
    // this stands in for is a runs root the watching process may not write — a permission
    // this crate cannot set portably, and one no interface offers. What is asserted
    // afterwards is read off the compiled binary's own streams.
    std::fs::write(watchers_dir(&world, &blocked), "not a directory")
        .expect("something where the records go");
    // llmlint: ignore-end[tests_mirror_real_usage]

    // `--timeout 0` reads the run once and returns, which is a whole watch: it arms,
    // reads, and reports. Both runs are in the same state, so both must answer the
    // same way.
    let watched = world.run(&["watch", &blocked, "--timeout", "0"]);
    let same = world.run(&["watch", &control, "--timeout", "0"]);
    assert_eq!(
        watched.code, same.code,
        "a watch that could not record itself returned {} where the same watch on a \
         comparable run returned {}",
        watched.code, same.code
    );
    let condition = |run: &crate::harness::Run| -> Value {
        let last: Value = serde_json::from_str(
            run.stdout
                .lines()
                .rfind(|line| !line.trim().is_empty())
                .unwrap_or_else(|| panic!("`onepipeline {}` wrote nothing", run.args)),
        )
        .expect("the watch's last record is JSON");
        json!({"watch": last["watch"], "condition": last["condition"], "exit": last["exit"]})
    };
    assert_eq!(
        condition(&watched),
        condition(&same),
        "a watch that could not record itself wrote a different return record"
    );
    // The control did record itself and cleaned up after returning, which is what
    // says the comparison above was against a watch that took the path this one
    // could not.
    assert!(
        watchers_dir(&world, &control).is_dir(),
        "the control watch never created the directory of records, so nothing distinguishes \
         it from the blocked one"
    );

    // And the run whose watch left no record reads unwatched, with what could not be
    // resolved named on standard error.
    let asked = world.run(&["unwatched"]);
    asked.exited(RUNS_UNWATCHED).out_has(&blocked);
    asked.err_has(&blocked).err_has("watcher directory");
    world.release("build.go");
}

/// A run root whose name is **not a run id** is passed over, launch record and
/// all.
///
/// The boundary every externally supplied run id in this crate crosses, at the one
/// place this verb takes one from a stranger: what discovery joins onto the runs
/// root is a directory name somebody else chose, and what a reported line does with
/// it is name it back to a caller that will type it at another verb. A root this
/// build cannot name is one it cannot report, whatever its launch record says.
///
/// **Unix**, because that is where the state exists: the separator a run id may not
/// contain is a character Windows does not allow in a file name at all, so there is
/// no such directory there to meet.
#[cfg(unix)]
#[test]
fn a_run_root_whose_name_is_not_a_run_id_is_passed_over() {
    let world = World::new("unwatched-badname");
    world.script("build.wait", "hold");
    let real = held(&world, "unwatchedrealname");

    // A directory that claims to be a run, carrying a launch record naming this
    // very session — so ownership is not what keeps it off the list.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] no verb makes a run root whose name is
    // not a run id, because `start` mints its own from an alphabet that has none of these
    // characters. What it stands in for is a directory somebody else put beside the runs,
    // and what it carries is this build's own launch record, copied.
    let odd = world.runs.join(r"od\dname");
    std::fs::create_dir_all(&odd).expect("a run root");
    std::fs::copy(paths_of(&world, &real).launch(), odd.join("launch.json"))
        .expect("a copied launch record");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let asked = world.run(&["unwatched"]);
    asked.exited(RUNS_UNWATCHED).out_has(&real);
    assert!(
        !asked.stdout.contains("od") && !asked.stderr.contains("od"),
        "a root this build cannot name was written about: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );
    world.release("build.go");
}

/// Put the binary back in the page cache before a measurement.
///
/// Measurement hygiene, and stated as no more than that: both measurements in a
/// journey read the binary first, so neither begins from a cache state the other
/// did not have. **What that is worth is unestablished** — no run has been taken
/// with this and without it, all else equal — and nothing this suite asserts rests
/// on it. It is here because a quarter-gigabyte binary and a ten-gibibyte fixture
/// share one page cache, and a measurement that begins by not knowing which of the
/// two is resident is one nobody can read.
fn binary_in_cache() {
    let read = std::fs::read(crate::harness::binary()).expect("the binary under test");
    assert!(!read.is_empty(), "the binary under test is empty");
}

/// A run this engine drove to settlement under a real observer graph — its monitor
/// turn held **in flight** (`observer.wait`) as the graph settles — leaves a
/// document current for its journal, read off the files before any view, so
/// `unwatched` excludes it. Clause 2 of the verb's rule, for an **attached** driver.
#[test]
fn a_run_settled_under_an_observer_mid_turn_leaves_a_current_document_and_is_excluded() {
    let world = World::new("unwatched-observed");
    world.write_graphs();
    world.script("observer.wait", "");
    world.script("turn.hold", "hold");
    world.script("build.work", "the worker wrote this\n");
    let run = "unwatchedobserved";
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    let driver = world
        .agentgraph_cmd(&[
            "start",
            &path,
            "--attach",
            "--dag-graph",
            &world.dag_graph(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the driver starts");
    // The monitor's turn is in flight when the graph settles: it has begun (its
    // first act is to read the run's ledger, which the sighting below records) and
    // is held on `observer.wait` while the worker runs to completion and the graph
    // settles under it — released only after `unwatched` is asked.
    world.until("the observer's turn to be in flight", |world| {
        !world.observer_saw().is_empty()
    });
    world.release("turn.go");
    world.release("turn.settle");
    driver
        .wait_with_output()
        .expect("the driver exits at settlement");

    // The document the driver left, off the files before any view touches the run.
    let paths = paths_of(&world, run);
    let left = document(&paths);
    assert_eq!(left["graph_complete"], json!(true), "{left}");
    assert_eq!(
        (
            left["journal_len"].as_u64(),
            left["journal_mtime_ms"].as_u64()
        ),
        stamp_of(&paths),
        "the driver left a document behind its journal at handback: {left}"
    );

    let asked = world.run(&["unwatched"]);
    world.release("observer.go");
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a run its own driver settled was reported: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );
}

/// The same clause-2 guarantee for a **detached** driver, whose only journal
/// appender is the engine loop: a run started detached that the retained driver
/// drives to settlement leaves a document current for its journal, read off the
/// files before any view, so `unwatched` excludes it — the documented happy path
/// `onepipeline start --detach` returns on, with no observer relay beside the
/// engine loop.
#[test]
fn a_detached_driver_that_settles_leaves_a_current_document_and_is_excluded() {
    let world = World::new("unwatched-detached");
    world.script("build.work", "the worker wrote this\n");
    let run = "unwatcheddetached";
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    // Detached, with the shipped default `--dag-graph off`: no observer, so the
    // engine loop is the only writer of this run's journal.
    world.run(&["start", &path, "--detach"]).exited(0);
    // The retained driver settles the run on its own. `result.json` is a direct
    // file check rather than a view, so nothing refreshes the summary document
    // between the driver writing it and this journey reading it — the engine loop
    // wrote the document at its last append, before this file exists.
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });

    let paths = paths_of(&world, run);
    let left = document(&paths);
    assert_eq!(left["graph_complete"], json!(true), "{left}");
    assert_eq!(
        (
            left["journal_len"].as_u64(),
            left["journal_mtime_ms"].as_u64()
        ),
        stamp_of(&paths),
        "the detached driver left a document behind its journal at handback: {left}"
    );

    let asked = world.run(&["unwatched"]);
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a detached run its own driver settled was reported: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );
}

/// The **measured shape**, driven to settlement and handed back: a first driver
/// with a real observer graph, killed mid-turn and recovered by a real `adopt`
/// that settles the run under a fresh observer whose monitor turn is in flight
/// (`observer.wait`) as the graph settles, over a lifecycle node publishing
/// through the real `onevcs` seam. Answers the run's paths once the adopting
/// driver has exited, with **no view having touched the run since**: what the
/// document then holds is what the driver left.
///
/// The observer's held turn is released by the caller (`observer.go`), after
/// whatever it asks with the run in this state.
fn settled_over_the_measured_shape(world: &World, run: &str) -> RunPaths {
    world.write_graphs();
    world.repository("local-direct", &[]);
    world.script("observer.wait", "");
    world.script("turn.hold", "hold");
    world.script("service.work", "the worker wrote this\n");
    let path = world.plan(
        run,
        &plan_of(run, vec![crate::harness::lifecycle("service", &[])]),
    );
    // A detached first driver with a real observer, its worker turn held so the run
    // keeps a driver to kill.
    world
        .run_on(
            world.agentgraph_cmd(&[
                "start",
                &path,
                "--detach",
                "--dag-graph",
                &world.dag_graph(),
            ]),
            "start adopted",
        )
        .exited(0);
    world.until("the observer to record itself", |world| {
        !world.observer_saw().is_empty()
    });
    world.until("the worker to be dispatched", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    // Kill the first driver, leaving the run for adoption.
    let pid = world.run_json(run, "launch.json")["pid"]
        .as_u64()
        .expect("the launch record names a pid") as u32;
    crate::harness::end_process(pid);
    world.until("the driver to exit", |world| {
        world.run(&["status", run]).stdout.contains("DRIVER DEAD")
    });
    // Let the held turn go, then adopt attached: the adopting driver records the
    // settlement and is the one whose closeout leaves the document.
    world.release("turn.go");
    world.release("turn.settle");
    world
        .agentgraph_cmd(&["adopt", run])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the adopting driver starts")
        .wait_with_output()
        .expect("the adopting driver exits at settlement");
    paths_of(world, run)
}

/// The clause-2 guarantee for an **adopted** driver, over
/// [`settled_over_the_measured_shape`].
#[test]
fn an_adopted_run_over_a_lifecycle_node_leaves_a_current_document_and_is_excluded() {
    let world = World::new("unwatched-adopted");
    let run = "unwatchedadopted";
    let paths = settled_over_the_measured_shape(&world, run);

    let left = document(&paths);
    assert_eq!(left["graph_complete"], json!(true), "{left}");
    assert_eq!(
        (
            left["journal_len"].as_u64(),
            left["journal_mtime_ms"].as_u64()
        ),
        stamp_of(&paths),
        "the adopted driver left a document behind its journal at handback: {left}"
    );

    let asked = world.run(&["unwatched"]);
    world.release("observer.go");
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "an adopted run its own driver settled was reported: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );
}

/// The measured report, reproduced: a run the **previous release** drove to
/// settlement is not reported by this one. Entry 68 records the measurement.
///
/// The fixture is what that run's driver left — a document current for its
/// journal, recording `graph_complete: true`, at the schema this build has moved
/// past — reached by driving [`settled_over_the_measured_shape`], putting the
/// document where the previous release's writer leaves it, and asking the verb
/// with no view between. The observer, the adoption and the lifecycle streams are
/// driven because the measured run had them; none is necessary to reach the
/// report. The verb refreshes the document and excludes the run, and what it
/// leaves behind is this build's own document, current for the journal.
///
// llmlint: ignore-block[tests_mirror_real_usage] no verb of this build writes a document
// at a schema it has moved past, and none could: the writer stamps its own version. The
// state is left by the previous release's driver — measured on the host, a 22-field
// document at `schema_version: 1` beside a journal it is current for — and what is put
// back is this build's own document with that one field moved, on the terms
// `a_document_at_a_superseded_schema_is_refreshed_and_decided_from_the_run` states.
// Every claim afterwards is read off the compiled binary's own streams and the files.
#[test]
fn a_run_the_previous_release_settled_is_refreshed_and_excluded_rather_than_reported() {
    let world = World::new("unwatched-previous-release");
    let run = "unwatchedprevious";
    let paths = settled_over_the_measured_shape(&world, run);

    // What the driver left: current for its journal and recording settlement, which
    // is what the previous release's driver left too. Established first, so that the
    // report below cannot be the stale-document or still-recording case.
    let left = document(&paths);
    assert_eq!(left["graph_complete"], json!(true), "{left}");
    assert_eq!(
        (
            left["journal_len"].as_u64(),
            left["journal_mtime_ms"].as_u64()
        ),
        stamp_of(&paths),
        "the adopted driver left a document behind its journal at handback: {left}"
    );
    let mut previous = left.clone();
    previous["schema_version"] = json!(SUMMARY_SCHEMA_VERSION - 1);
    std::fs::write(paths.summary(), previous.to_string()).expect("the document");

    let asked = world.run(&["unwatched"]);
    world.release("observer.go");
    asked.exited(SUCCESS);
    assert!(
        asked.stdout.is_empty() && asked.stderr.is_empty(),
        "a run the previous release settled was written about: stdout {:?}, stderr {:?}",
        asked.stdout,
        asked.stderr
    );
    // And the document is now this build's own, current for the journal: the
    // question was answered from the run's record, and asking again reads nothing.
    let refreshed = document(&paths);
    assert_eq!(refreshed["schema_version"], json!(SUMMARY_SCHEMA_VERSION));
    assert_eq!(refreshed["graph_complete"], json!(true), "{refreshed}");
    assert_eq!(
        (
            refreshed["journal_len"].as_u64(),
            refreshed["journal_mtime_ms"].as_u64()
        ),
        stamp_of(&paths),
        "the refreshed document is behind its journal: {refreshed}"
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]
