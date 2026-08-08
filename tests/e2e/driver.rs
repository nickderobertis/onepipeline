//! Ported from `test_orchestrate_launch_e2e`, `test_attach_settles_e2e`,
//! `test_run_ownership_e2e`, `test_round_ownership_e2e`,
//! `test_run_adoption_e2e`, `test_relaunch_seed_e2e`, and the driver-liveness
//! half of `test_liveness_e2e`.
//!
//! Who launched a run, who may stop it, what happens when its driver dies, and
//! when an attach returns.

use crate::harness::{agent, human, plan_of, World, NOTHING_DRIVING, REFUSED};
use serde_json::json;

fn start_detached(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    name.to_string()
}

#[test]
fn start_launches_the_shipped_dag_scope_graph_and_records_how_to_relaunch_it() {
    let world = World::new("driver-launch");
    world.script("driver.wait", "hold");
    let path = world.plan("launched", &plan_of("launched", vec![agent("build", &[])]));
    let started = world.run(&["start", &path.to_string_lossy(), "--detach"]);
    started.exited(0).out_has("\"run_id\"");

    let record = started.json();
    assert_eq!(record["run_id"], "launched");
    assert!(record["commands"]["next"].as_str().is_some());

    // The graph really was launched through the sibling, and it is the shipped
    // dag-scope config. It records itself from its own process, so wait for it.
    world.until("the graph process to record itself", |world| {
        !world.invocations().is_empty()
    });
    assert!(
        world.invocations().iter().any(|invocation| {
            invocation["tool"] == "oneagentgraph"
                && invocation["args"][0] == "run"
                && invocation["args"][1]
                    .as_str()
                    .is_some_and(|graph| graph.ends_with("dag-scope.yaml"))
        }),
        "the dag-scope graph was not launched: {:?}",
        world.invocations()
    );

    // The relaunch record is what `adopt` replays from.
    let launch = world.run_json("launched", "launch.json");
    assert_eq!(launch["round_budget"], 14_400);
    assert_eq!(launch["heartbeat_interval"], 1_800);
    assert_eq!(launch["launcher"], "e2e");
    assert!(launch["graph"]
        .as_str()
        .expect("a graph")
        .ends_with("dag-scope.yaml"));
    // The pid recorded is the graph process's, not this command's: what drives
    // the run is that process.
    assert_ne!(launch["pid"], json!(0));
    world.release("driver.go");
}

#[test]
fn the_launch_record_is_written_before_the_driver_that_reads_it_is_started() {
    let world = World::new("driver-ordering");
    // The driver is held at its first instruction, *after* it has recorded what
    // the ledger held. Holding it there is what makes this about the launcher's
    // ordering and nothing else: whatever the driver does next cannot be the
    // reason the record is there.
    world.script("driver.wait", "hold");
    let path = world.plan("ordered", &plan_of("ordered", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

    world.until("the driver to read the run's ledger", |world| {
        !world.driver_saw().is_empty()
    });
    // A driver that wins the race against its own launcher dies on a file
    // nobody wrote, and the run then sits at `run-started` with nothing driving
    // it — which reads as a mysteriously hung run rather than as the ordering
    // bug it is.
    let saw = world.driver_saw();
    assert_eq!(saw[0]["run"], "ordered");
    assert_eq!(
        saw[0]["launch_record"],
        json!(true),
        "the driver was started before its launch record existed: {saw:?}"
    );
    world.release("driver.go");
}

#[test]
fn an_attached_start_returns_when_the_graph_completes() {
    let world = World::new("driver-attach");
    let path = world.plan("attached", &plan_of("attached", vec![agent("build", &[])]));
    let started = world.run(&["start", &path.to_string_lossy(), "--attach"]);
    started.exited(0).out_has("\"settlement\":\"complete\"");
}

#[test]
fn an_attach_returns_exit_three_when_nothing_is_driving_the_run() {
    let world = World::new("driver-unattended");
    // A human action nothing can clear: the driver settles the round, finds no
    // round it could open, and exits with the graph unfinished.
    let path = world.plan(
        "unattended",
        &plan_of("unattended", vec![human("approve", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(NOTHING_DRIVING)
        .out_has("\"settlement\":\"unattended\"");
}

#[test]
fn a_dead_driver_reads_as_driver_dead_and_adopt_is_the_way_back() {
    let world = World::new("driver-dead");
    let run = start_detached(&world, "orphaned", vec![human("approve", &[])]);
    world.until("the driver to exit", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });

    world
        .run(&["runs"])
        .exited(0)
        .out_has("DRIVER DEAD")
        .out_has("onepipeline adopt orphaned");

    // The ledger is intact, so a fresh driver takes it over.
    let adopted = world.run(&["adopt", &run]);
    adopted.exited(NOTHING_DRIVING);
    let adoptions = world.events_of(&run, "driver-adopted");
    assert_eq!(adoptions.len(), 1, "{adoptions:?}");
    assert_eq!(world.run_json(&run, "launch.json")["adoptions"], 1);
    // The dead driver's evidence moves aside rather than being truncated.
    assert!(world.run_file(&run, "launch.pre-adopt-1.json").exists());
}

#[test]
fn adopt_refuses_another_sessions_run_and_has_no_force() {
    let world = World::new("driver-adopt-owner");
    let run = start_detached(&world, "someone-elses", vec![human("approve", &[])]);
    world.until("the driver to exit", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });

    let stranger = world.as_session("session-other");
    stranger
        .run(&["adopt", &run])
        .exited(REFUSED)
        .err_has("belongs to")
        .err_has("not to this session");

    // There is no `--force`: adopting takes over ongoing work, which is exactly
    // where a second opinion beats an override.
    let output = stranger.run(&["adopt", &run, "--force"]);
    assert_eq!(output.code, crate::harness::USAGE_ERROR);
}

#[test]
fn adopt_refuses_a_run_something_is_still_driving() {
    let world = World::new("driver-adopt-live");
    world.script("build.wait", "hold");
    let run = start_detached(&world, "still-live", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });

    world
        .run(&["adopt", &run])
        .exited(REFUSED)
        .err_has("still being driven")
        .err_has("onepipeline stop still-live");
    world.release("build.go");
}

#[test]
fn a_run_belongs_to_the_session_that_launched_it() {
    let world = World::new("driver-ownership");
    let run = start_detached(&world, "owned", vec![human("approve", &[])]);
    world.until("the run to be recorded", |world| {
        world.run_file(&run, "launch.json").exists()
    });

    world.run(&["runs"]).exited(0).out_has("[mine]");
    world.run(&["runs", "--mine"]).exited(0).out_has("owned");

    let stranger = world.as_session("session-other");
    let listed = stranger.run(&["runs"]);
    listed.exited(0).out_has("owned");
    assert!(!listed.stdout.contains("[mine]"), "{}", listed.stdout);
    assert!(
        !listed.stdout.contains("session-driver-ownership"),
        "the session id leaked into a view:\n{}",
        listed.stdout
    );
    stranger
        .run(&["runs", "--mine"])
        .exited(0)
        .out_has("no runs recorded");
}

#[test]
fn stop_refuses_another_sessions_run_and_force_names_the_owner() {
    let world = World::new("driver-stop");
    world.script("build.wait", "hold");
    let run = start_detached(&world, "stoppable", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });

    let stranger = world.as_session("session-other");
    stranger
        .run(&["stop", &run])
        .exited(REFUSED)
        .err_has("belongs to");
    assert!(
        world.events_of(&run, "run-stopped").is_empty(),
        "a refused stop still ended the run"
    );

    stranger
        .run(&["stop", &run, "--force"])
        .exited(0)
        .err_has("belongs to")
        .out_has("\"stopped\":true");
    assert_eq!(world.events_of(&run, "run-stopped").len(), 1);
    world.release("build.go");
}

#[test]
fn the_owner_stops_its_own_run_without_force() {
    let world = World::new("driver-stop-mine");
    world.script("build.wait", "hold");
    let run = start_detached(&world, "mine", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });

    world.run(&["stop", &run]).exited(0).out_has("[mine]");
    assert_eq!(world.events_of(&run, "run-stopped").len(), 1);
    world.release("build.go");
}

#[test]
fn a_run_nobody_can_attribute_is_nobodys() {
    let world = World::new("driver-unknown");
    let path = world.plan("anon", &plan_of("anon", vec![human("approve", &[])]));
    let mut command = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
    command.env_remove("ONEPIPELINE_LAUNCHER_SESSION");
    command.output().expect("the binary runs");

    world.until("the run to be recorded", |world| {
        world.run_file("anon", "launch.json").exists()
    });
    world.run(&["runs"]).exited(0).out_has("[unknown]");
    // A provenance-less run never displays as the caller's, and never accepts a
    // command that ownership guards.
    world
        .run(&["stop", "anon"])
        .exited(REFUSED)
        .err_has("[unknown]");
}

#[test]
fn the_engine_verbs_are_a_single_writer() {
    let world = World::new("driver-lock");
    world.script("build.wait", "hold");
    let run = start_detached(&world, "locked", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });

    // The driver holds the run's ownership lock for the whole round, so a
    // second writer loses the race rather than interleaving with it.
    world
        .run(&["round", "run", &run])
        .exited(REFUSED)
        .err_has("is being written by pid");
    world
        .run(&["round", "next", &run])
        .exited(REFUSED)
        .err_has("is being written by pid");
    world.release("build.go");
}

#[test]
fn the_engine_verbs_drive_a_run_the_planner_launched_detached() {
    let world = World::new("driver-verbs");
    // The driver is held back, so this test *is* the orchestrator: it runs the
    // same two verbs under the same lock, which is what the shipped persona
    // instructs and the only way run state may change.
    world.script("driver.wait", "hold");
    let run = start_detached(
        &world,
        "verbs",
        vec![agent("first", &[]), agent("second", &["first"])],
    );

    world.run(&["round", "run", &run]).exited(0);
    assert_eq!(
        world.run_json(&run, "round-01/result.json")["state"],
        "complete"
    );

    // Nothing is left to do, so the transition reports completion rather than
    // opening a round that would dispatch nothing.
    let transitioned = world.run(&["round", "next", &run]);
    transitioned.exited(0).out_has("\"complete\"");
    assert_eq!(transitioned.json()["next_round"], serde_json::Value::Null);
    assert!(!world.run_file(&run, "round-02").exists());
    world.release("driver.go");
}
