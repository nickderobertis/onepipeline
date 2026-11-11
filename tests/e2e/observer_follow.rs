//! The observer graph follows its run's own journal, and that is the only way a
//! planner's reading restarts one of its clocks.
//!
//! A graph's clock restarts on the records `reset_on` names with no signal from
//! the engine, along all three observer launch paths and across a restart and an
//! adoption; the engine leaves no signal for any member and says nothing about a
//! reset at `start` or at an adoption.

// llmlint: ignore-file[e2e_not_mocked] the graph is the real `oneagentgraph`, linked, retained or at its released binary; only the paid harness below onejudge is doubled, at its subprocess boundary, as `harness.rs` sets out.

#![cfg(unix)]

use serde_json::Value;

#[cfg(target_os = "linux")]
use crate::harness::epoch_millis;
use crate::harness::{agent, oneagentgraph_binary, plan_of, World};

/// The scheduled note every graph below declares.
const NOTE: &str = "progress-report";

/// The lines of `stderr` that call something about a reset deprecated — the
/// warning an engine with a built-in reset printed at `start` and at adoption.
fn reset_deprecations(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.contains("deprecated") && line.contains("reset"))
        .collect()
}

/// The signals left in one graph run's signal directory for a member's clock —
/// what an engine restarting clocks itself writes there, and nothing following
/// the journal ever does.
fn reset_signals(world: &World, graph_run: &str) -> Vec<String> {
    std::fs::read_dir(
        world
            .graph_state()
            .join(graph_run)
            .join(oneagentgraph::run::SIGNAL_DIR),
    )
    .map(|entries| {
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".reset"))
            .collect()
    })
    .unwrap_or_default()
}

/// An observer graph of one held two-party member declaring `clocks`.
fn write_observer(world: &World, clocks: &str) -> String {
    let graphs = world.graphs();
    std::fs::write(
        graphs.join("onejudge.base.yaml"),
        "system_prompt: Watch the run.\nuser:\n  persona: Review it.\n  \
         done_when: the run is complete\n  max_turns: 12\n",
    )
    .expect("the onejudge base config is written");
    let judge = world.root.join("judge.sh");
    std::fs::write(
        &judge,
        "IFS= read -r request\ni=0\nwhile [ \"$i\" -lt 1500 ]; do sleep 0.2; i=$((i+1)); done\n",
    )
    .expect("the judge command is written");
    let path = graphs.join("following.yaml");
    std::fs::write(
        &path,
        format!(
            "version: 11\nname: following\nmembers:\n  monitor:\n    task: {}\n    \
             kind: onejudge\n    base_config: ./onejudge.base.yaml\n    agent:\n      \
             oneharness_config: {}\n    judge:\n      command: [sh, '{}']\n    mode: bypass\n\
             {clocks}env:\n  ONEPIPELINE_RUN_ID: ${{ONEPIPELINE_RUN_ID}}\n  \
             ONEPIPELINE_RUNS_DIR: ${{ONEPIPELINE_RUNS_DIR}}\n",
            serde_json::to_string(crate::harness::MONITOR_TASK).expect("the task is quoted"),
            world.harness_config("monitor"),
            judge.display()
        ),
    )
    .expect("the observer graph is written");
    path.display().to_string()
}

/// A scheduled note that restarts on a planner's reading.
fn resetting_note() -> String {
    format!(
        "    scheduled_notes:\n      - {{id: {NOTE}, every: 2, addressee: supervisor, text: Report., \
         reset_on: {{include: [{{source: pipeline, kind: planner-surfaced}}]}}}}\n"
    )
}

fn graph_run(world: &World, run: &str) -> String {
    // A look before the launcher has written the record, or one meeting a rename,
    // finds nothing yet and looks again.
    std::fs::read_to_string(world.run_file(run, "launch.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|launch| launch["graph_run"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Wait for a graph run the launch record names, other than `earlier`, to have
/// reached its member's wave, so its clocks and its follower both exist.
fn watching(world: &World, run: &str, earlier: &str) -> String {
    world.until("the observer graph's member to come up", |world| {
        let now = graph_run(world, run);
        !now.is_empty()
            && now != earlier
            && world
                .graph_journal(&now)
                .iter()
                .any(|event| event["kind"] == "member-started")
    });
    graph_run(world, run)
}

fn note_reset(event: &Value) -> bool {
    event["kind"] == "cron-reset" && event["payload"]["scheduled_note"] == NOTE
}

/// Read one surface with `next`, through `command`, and wait for `graph_run` to
/// restart the note's clock because of it.
fn next_resets_the_note(
    world: &World,
    run: &str,
    graph_run: &str,
    command: &dyn Fn(&[&str]) -> onepipeline_testfakes::profile::process::Command,
    message: &str,
) {
    let answered = world.run_on(
        command(&["surface", run, "--kind", "check-in", "--message", message]),
        "surface",
    );
    answered.exited(0);
    let read = world.run_on(command(&["next", run]), "next");
    read.exited(0).out_has(message);
    world.until("the note's clock to restart on the reading", |world| {
        world.graph_journal(graph_run).iter().any(|event| {
            note_reset(event) && event["payload"]["cause"]["kind"] == "planner-surfaced"
        })
    });
    let caused: Vec<Value> = world
        .graph_journal(graph_run)
        .into_iter()
        .filter(note_reset)
        .collect();
    assert!(
        caused
            .iter()
            .all(|event| event["payload"]["cause"]["source"] == "pipeline"),
        "a reset named another cause: {caused:?}"
    );
}

/// Launch a run with its work held, so its driver and graph stay up for the read.
fn held_plan(world: &World, run: &str) -> String {
    world.script("turn.hold", "hold");
    world.script("observer.wait", "hold");
    world.plan(run, &plan_of(run, vec![agent("build", &[])]))
}

fn wrapping_up(world: &World, run: &str) {
    world.run_on_agentgraph(&["stop", run]).exited(0);
}

#[test]
fn an_attached_launch_resets_its_note_on_a_reading_through_the_linked_library() {
    let world = World::new("follow-attached");
    world.write_graphs();
    let path = held_plan(&world, "attached");
    let graph = write_observer(&world, &resetting_note());
    let mut child = world
        .agentgraph_cmd(&["start", &path, "--attach", "--dag-graph", &graph])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the attached launch starts");
    let run = watching(&world, "attached", "");
    next_resets_the_note(
        &world,
        "attached",
        &run,
        &|args| world.agentgraph_cmd(args),
        "one",
    );
    // This launch's own child, and the doubles it holds, released rather than
    // signalled through `stop`: an attached driver hosts the graph in-process.
    world.release("observer.go");
    world.release("turn.go");
    child.kill().expect("the attached launch is ended");
    child.wait().expect("the attached launch is reaped");
}

#[test]
fn a_detached_launch_resets_its_note_and_so_do_a_restart_and_an_adoption() {
    let world = World::new("follow-detached");
    world.write_graphs();
    let path = held_plan(&world, "detached");
    let graph = write_observer(&world, &resetting_note());
    let started = world.run_on_agentgraph(&["start", &path, "--detach", "--dag-graph", &graph]);
    started.exited(0);
    assert!(
        reset_deprecations(&started.stderr).is_empty(),
        "`start` warned about a reset: {}",
        started.stderr
    );
    let command = |args: &[&str]| world.agentgraph_cmd(args);
    let first = watching(&world, "detached", "");
    next_resets_the_note(&world, "detached", &first, &command, "one");

    // The process that owns the first graph run, as the run's own lock names it:
    // the retained `drive` child, killed so the driver starts another observer.
    let lock = std::fs::read_to_string(
        world
            .graph_state()
            .join(&first)
            .join(oneagentgraph::liveness::OWNER_LOCK_FILE),
    )
    .expect("the graph run records who owns its state");
    let owner: i32 = lock
        .split_whitespace()
        .next()
        .and_then(|pid| pid.parse().ok())
        .unwrap_or_else(|| panic!("the owner lock names no process: {lock:?}"));
    // SAFETY: `kill` takes a pid and a signal and reports failure in its return
    // value; the pid is the one this run's own ownership record names.
    assert_eq!(unsafe { libc::kill(owner, libc::SIGKILL) }, 0);
    let restarted = watching(&world, "detached", &first);
    next_resets_the_note(&world, "detached", &restarted, &command, "two");

    wrapping_up(&world, "detached");
    let adopted = world.run_on_agentgraph(&["adopt", "detached", "--detach"]);
    adopted.exited(0);
    assert!(
        reset_deprecations(&adopted.stderr).is_empty(),
        "an adoption warned about a reset: {}",
        adopted.stderr
    );
    let adopting = watching(&world, "detached", &restarted);
    next_resets_the_note(&world, "detached", &adopting, &command, "three");
    wrapping_up(&world, "detached");
}

/// The released sibling behind a launcher that appends each argv it is run with
/// to `log`, one line per invocation, before running it unchanged.
fn sibling_recording_its_invocations(world: &World, log: &std::path::Path) -> String {
    let launcher = world.root.join("oneagentgraph-recorded");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh
printf '%s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            oneagentgraph_binary().display()
        ),
    )
    .expect("the launcher is written");
    let mut permissions = std::fs::metadata(&launcher)
        .expect("the launcher exists")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&launcher, permissions).expect("the launcher is executable");
    launcher.display().to_string()
}

/// Composing the released executable, a reading restarts the note through the
/// journal the graph run follows, and the engine never runs the sibling for
/// anything but that run: no `reset-timer`, and no signal file in the run.
#[test]
fn a_launch_composing_the_released_sibling_follows_through_its_run_flag() {
    let world = World::new("follow-binary");
    world.write_graphs();
    let path = held_plan(&world, "binary");
    let graph = write_observer(&world, &resetting_note());
    let invocations = world.root.join("sibling-invocations.log");
    let sibling = sibling_recording_its_invocations(&world, &invocations);
    let command = |args: &[&str]| {
        let mut command = world.agentgraph_cmd(args);
        command.env("ONEPIPELINE_ONEAGENTGRAPH_BIN", &sibling);
        command
    };
    world
        .run_on(
            command(&["start", &path, "--detach", "--dag-graph", &graph]),
            "start",
        )
        .exited(0);
    let run = watching(&world, "binary", "");
    next_resets_the_note(&world, "binary", &run, &command, "one");
    // llmlint: ignore-block[tests_mirror_real_usage] which verbs the engine ran the
    // sibling's executable with is on no product surface — `next` hands the surface
    // over either way — so the launcher's own record of its argv, and the sibling's
    // signal directory, are where a reset the engine sent would show.
    let ran = std::fs::read_to_string(&invocations).expect("the sibling was run");
    assert!(
        ran.lines().any(|line| line.starts_with("run ")),
        "the launcher recorded no `run`, so it is not what the engine composed: {ran}"
    );
    assert!(
        !ran.lines().any(|line| line.starts_with("reset-timer")),
        "the engine ran `reset-timer`:\n{ran}"
    );
    assert_eq!(
        reset_signals(&world, &run),
        Vec::<String>::new(),
        "the engine left a reset signal in the graph run"
    );
    // llmlint: ignore-end[tests_mirror_real_usage]
    world.run_on(command(&["stop", "binary"]), "stop").exited(0);
}

/// A shared object that runs its process's wall clock slower than its monotonic
/// one, by a tenth, from the first instant the process reads it: the host a
/// time daemon is pulling back toward its source, which `ts` is stamped on while
/// a span is waited on the monotonic clock.
#[cfg(target_os = "linux")]
const SLOWED_WALL_CLOCK: &str = r#"
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdint.h>
#include <time.h>

typedef int (*clock_gettime_fn)(clockid_t, struct timespec *);

static int64_t base = -1;

int clock_gettime(clockid_t clock, struct timespec *ts) {
    static clock_gettime_fn real;
    if (!real) real = (clock_gettime_fn)dlsym(RTLD_NEXT, "clock_gettime");
    int answered = real(clock, ts);
    if (answered != 0 || clock != CLOCK_REALTIME) return answered;
    int64_t now = (int64_t)ts->tv_sec * 1000000000 + ts->tv_nsec;
    int64_t unset = -1;
    __atomic_compare_exchange_n(&base, &unset, now, 0, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST);
    int64_t from = __atomic_load_n(&base, __ATOMIC_SEQ_CST);
    int64_t slowed = from + (now - from) / 10 * 9;
    ts->tv_sec = slowed / 1000000000;
    ts->tv_nsec = slowed % 1000000000;
    return 0;
}
"#;

/// The released sibling, behind a launcher that runs it — and everything it
/// starts — on [`SLOWED_WALL_CLOCK`].
#[cfg(target_os = "linux")]
fn sibling_on_a_slowed_wall_clock(world: &World) -> String {
    let source = world.root.join("slowed-wall-clock.c");
    let shim = world.root.join("slowed-wall-clock.so");
    std::fs::write(&source, SLOWED_WALL_CLOCK).expect("the shim's source is written");
    let built = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-O2", "-o"])
        .arg(&shim)
        .arg(&source)
        .arg("-ldl")
        .output()
        .expect("a C compiler runs");
    assert!(
        built.status.success(),
        "the shim did not build: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let launcher = world.root.join("oneagentgraph-slowed");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nLD_PRELOAD='{}' exec '{}' \"$@\"\n",
            shim.display(),
            oneagentgraph_binary().display()
        ),
    )
    .expect("the launcher is written");
    let mut permissions = std::fs::metadata(&launcher)
        .expect("the launcher exists")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&launcher, permissions).expect("the launcher is executable");
    launcher.display().to_string()
}

/// The progress report a planner's reading restarts is next stamped a whole
/// interval after the restart, on a host whose wall clock runs behind its
/// monotonic one — the run monitor's report the user hears only when they would
/// not otherwise have heard about the run.
///
/// Both instants are the graph's own `ts`, so the shim slows the clock each is
/// read on alike; a clock counting its span on the monotonic clock alone stamps
/// the report a tenth of the interval early.
#[cfg(target_os = "linux")]
#[test]
fn a_planner_reading_holds_the_next_report_a_whole_interval_away() {
    let world = World::new("follow-interval");
    world.write_graphs();
    let path = held_plan(&world, "interval");
    let graph = write_observer(&world, &resetting_note());
    let sibling = sibling_on_a_slowed_wall_clock(&world);
    let command = |args: &[&str]| {
        let mut command = world.agentgraph_cmd(args);
        command.env("ONEPIPELINE_ONEAGENTGRAPH_BIN", &sibling);
        command
    };
    world
        .run_on(
            command(&["start", &path, "--detach", "--dag-graph", &graph]),
            "start",
        )
        .exited(0);
    let run = watching(&world, "interval", "");
    next_resets_the_note(&world, "interval", &run, &command, "one");

    let restarted = |journal: &[Value]| {
        journal
            .iter()
            .position(|event| {
                note_reset(event) && event["payload"]["cause"]["kind"] == "planner-surfaced"
            })
            .expect("the reading restarted the note")
    };
    world.until("the note's next report after the restart", |world| {
        let journal = world.graph_journal(&run);
        journal[restarted(&journal)..].iter().any(|event| {
            event["kind"] == "cron-fired" && event["payload"]["scheduled_note"] == NOTE
        })
    });
    let journal = world.graph_journal(&run);
    let from = restarted(&journal);
    let reset = &journal[from];
    let fired = journal[from..]
        .iter()
        .find(|event| event["kind"] == "cron-fired" && event["payload"]["scheduled_note"] == NOTE)
        .expect("the note fired after the restart");
    let stamp = |event: &Value| epoch_millis(event["ts"].as_str().expect("a stamped envelope"));
    let waited = stamp(fired) - stamp(reset);
    assert!(
        waited >= 2_000,
        "the report came {waited} ms after the reading restarted its 2 s clock: \
         {reset} then {fired}"
    );
    world
        .run_on(command(&["stop", "interval"]), "stop")
        .exited(0);
}

/// A reading restarts a **schedule** that declares `reset_on` matching
/// `planner-surfaced`, through the journal alone: the engine leaves no reset
/// signal in the graph run, a note whose own `reset_on` matches nothing is not
/// touched, and neither `start` nor either form of adoption says anything about
/// a reset.
///
/// The note's clock fires once, well after the reading: a firing beside an
/// untaken copy would withdraw that copy as `superseded`, which is what this
/// journey says the reading must not cause.
#[test]
fn a_reading_restarts_a_schedule_through_the_journal_and_the_engine_signals_nothing() {
    let world = World::new("journal-reset");
    world.write_graphs();
    let path = held_plan(&world, "journalled");
    let graph = write_observer(
        &world,
        &format!(
            "    background: false\n    \
             schedule: {{every: 3600, start_after: 3600, reset_on: {{include: [{{source: \
             pipeline, kind: planner-surfaced}}]}}}}\n    \
             scheduled_notes:\n      - {{id: {NOTE}, every: 3600, start_after: 8, addressee: \
             supervisor, text: Report., reset_on: {{include: [{{source: pipeline, kind: \
             nothing-journals-this}}]}}}}\n"
        ),
    );
    let started = world.run_on_agentgraph(&["start", &path, "--detach", "--dag-graph", &graph]);
    started.exited(0);
    assert!(
        reset_deprecations(&started.stderr).is_empty(),
        "`start` warned about a reset: {}",
        started.stderr
    );

    let run = watching(&world, "journalled", "");
    world
        .run_on_agentgraph(&[
            "surface",
            "journalled",
            "--kind",
            "check-in",
            "--message",
            "m",
        ])
        .exited(0);
    world
        .run_on_agentgraph(&["next", "journalled"])
        .exited(0)
        .out_has("\"m\"");
    let schedule_reset = |event: &Value| {
        event["kind"] == "cron-reset" && event["payload"]["scheduled_note"].is_null()
    };
    world.until("the schedule to restart on the reading", |world| {
        world.graph_journal(&run).iter().any(schedule_reset)
    });
    let reset_at = |journal: &[Value]| {
        journal
            .iter()
            .position(schedule_reset)
            .expect("the schedule restarted")
    };
    world.until("the note's clock to go on to its firing", |world| {
        let journal = world.graph_journal(&run);
        journal[reset_at(&journal)..].iter().any(|event| {
            event["kind"] == "cron-fired" && event["payload"]["scheduled_note"] == NOTE
        })
    });
    let journal = world.graph_journal(&run);
    let resets: Vec<&Value> = journal
        .iter()
        .filter(|event| schedule_reset(event))
        .collect();
    assert_eq!(
        resets.len(),
        1,
        "one reading restarted the schedule {resets:?}"
    );
    assert_eq!(
        resets[0]["payload"]["cause"]["kind"], "planner-surfaced",
        "the schedule restarted for another cause: {resets:?}"
    );
    assert_eq!(resets[0]["payload"]["cause"]["source"], "pipeline");
    let touched: Vec<&Value> = journal
        .iter()
        .filter(|event| {
            matches!(
                event["kind"].as_str(),
                Some("cron-reset" | "note-withdrawn")
            ) && event["payload"]["scheduled_note"] == NOTE
        })
        .collect();
    assert!(
        touched.is_empty(),
        "the reading reached a note that does not reset on it: {touched:?}"
    );
    // llmlint: ignore[tests_mirror_real_usage] a reset signal is the sibling's own file under the graph run's directory and appears on no product surface; its absence is the claim that the engine sent none.
    assert_eq!(
        reset_signals(&world, &run),
        Vec::<String>::new(),
        "the engine left a reset signal in the graph run"
    );

    wrapping_up(&world, "journalled");
    let adopted = world.run_on_agentgraph(&["adopt", "journalled", "--detach"]);
    adopted.exited(0);
    assert!(
        reset_deprecations(&adopted.stderr).is_empty(),
        "a detached adoption warned about a reset: {}",
        adopted.stderr
    );
    let adopting = watching(&world, "journalled", &run);
    wrapping_up(&world, "journalled");

    // Attached, the adopter is a process of this test's own, with its stderr on
    // a file because it stays to drive; it has said what it will say at launch
    // once its graph run is up.
    let log = world.root.join("attached-adopt.stderr");
    let mut attached = world
        .agentgraph_cmd(&["adopt", "journalled"])
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&log).expect("a log for the adoption"))
        .spawn()
        .expect("the attached adoption starts");
    watching(&world, "journalled", &adopting);
    let said = std::fs::read_to_string(&log).expect("the adoption's stderr");
    world.release("observer.go");
    world.release("turn.go");
    attached.kill().expect("the attached adoption is ended");
    attached.wait().expect("the attached adoption is reaped");
    assert!(
        reset_deprecations(&said).is_empty(),
        "an attached adoption warned about a reset: {said}"
    );
}
