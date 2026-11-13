//! The observer graph follows its run's own journal, and the engine's built-in
//! clock reset is deprecated beside it.
//!
//! A graph's clock restarts on the records `reset_on` names with no signal from
//! the engine, along all three observer launch paths and across a restart and an
//! adoption; `resettable`, the bare per-member reset, keeps working and reaches
//! only the schedule that declared it.

// llmlint: ignore-file[e2e_not_mocked] the graph is the real `oneagentgraph`, linked, retained or at its released binary; only the paid harness below onejudge is doubled, at its subprocess boundary, as `harness.rs` sets out.

#![cfg(unix)]

use serde_json::Value;

use crate::harness::{agent, oneagentgraph_binary, plan_of, World};

/// The scheduled note every graph below declares.
const NOTE: &str = "progress-report";

/// The deprecation `start` and `adopt` print for a graph declaring `resettable`.
const DEPRECATED: &str = "automatic clock reset on `next` is deprecated";

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
        !started.stderr.contains(DEPRECATED),
        "a graph declaring no `resettable` was warned: {}",
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
    assert!(!adopted.stderr.contains(DEPRECATED), "{}", adopted.stderr);
    let adopting = watching(&world, "detached", &restarted);
    next_resets_the_note(&world, "detached", &adopting, &command, "three");
    wrapping_up(&world, "detached");
}

#[test]
fn a_launch_composing_the_released_sibling_follows_through_its_run_flag() {
    let world = World::new("follow-binary");
    world.write_graphs();
    let path = held_plan(&world, "binary");
    let graph = write_observer(&world, &resetting_note());
    let command = |args: &[&str]| {
        let mut command = world.agentgraph_cmd(args);
        command.env("ONEPIPELINE_ONEAGENTGRAPH_BIN", oneagentgraph_binary());
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
    world.run_on(command(&["stop", "binary"]), "stop").exited(0);
}

/// The built-in reset reaches the schedule that declared `resettable` and
/// nothing else, and `start` and `adopt` say it is on its way out.
///
/// The note's clock fires once, well after the reading: a firing beside an
/// untaken copy would withdraw that copy as `superseded`, which is what this
/// journey says the reading must not cause.
#[test]
fn the_built_in_reset_restarts_only_the_resettable_schedule_and_warns() {
    let world = World::new("deprecated-reset");
    world.write_graphs();
    let path = held_plan(&world, "deprecated");
    let graph = write_observer(
        &world,
        &format!(
            "    background: false\n    \
             schedule: {{every: 3600, start_after: 3600, resettable: true}}\n    \
             scheduled_notes:\n      - {{id: {NOTE}, every: 3600, start_after: 8, addressee: \
             supervisor, text: Report., reset_on: {{include: [{{source: pipeline, kind: \
             nothing-journals-this}}]}}}}\n"
        ),
    );
    let started = world.run_on_agentgraph(&["start", &path, "--detach", "--dag-graph", &graph]);
    started.exited(0);
    assert_eq!(
        started.stderr.matches(DEPRECATED).count(),
        1,
        "{}",
        started.stderr
    );
    assert!(started.stderr.contains("reset_on"), "{}", started.stderr);

    let run = watching(&world, "deprecated", "");
    world
        .run_on_agentgraph(&[
            "surface",
            "deprecated",
            "--kind",
            "check-in",
            "--message",
            "m",
        ])
        .exited(0);
    world.run_on_agentgraph(&["next", "deprecated"]).exited(0);
    world.until("the resettable schedule to restart", |world| {
        world.graph_journal(&run).iter().any(|event| {
            event["kind"] == "cron-reset" && event["payload"]["scheduled_note"].is_null()
        })
    });
    let reset_at = |journal: &[Value]| {
        journal
            .iter()
            .position(|event| event["kind"] == "cron-reset")
            .expect("the schedule restarted")
    };
    world.until("the note's clock to go on to its firing", |world| {
        let journal = world.graph_journal(&run);
        journal[reset_at(&journal)..].iter().any(|event| {
            event["kind"] == "cron-fired" && event["payload"]["scheduled_note"] == NOTE
        })
    });
    let journal = world.graph_journal(&run);
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
        "the bare reset reached the note: {touched:?}"
    );

    wrapping_up(&world, "deprecated");
    let adopted = world.run_on_agentgraph(&["adopt", "deprecated", "--detach"]);
    adopted.exited(0);
    assert_eq!(
        adopted.stderr.matches(DEPRECATED).count(),
        1,
        "{}",
        adopted.stderr
    );
    wrapping_up(&world, "deprecated");

    // Attached, the adopter is a process of this test's own, with its warning on
    // a file because it stays to drive.
    let log = world.root.join("attached-adopt.stderr");
    let mut attached = world
        .agentgraph_cmd(&["adopt", "deprecated"])
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&log).expect("a log for the adoption"))
        .spawn()
        .expect("the attached adoption starts");
    world.until("the attached adoption to warn", |_| {
        std::fs::read_to_string(&log).is_ok_and(|said| said.matches(DEPRECATED).count() == 1)
    });
    world.release("observer.go");
    world.release("turn.go");
    attached.kill().expect("the attached adoption is ended");
    attached.wait().expect("the attached adoption is reaped");
}
