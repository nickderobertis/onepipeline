//! Who launched a run, who may stop it, what happens when its driver dies, and
//! when an attach returns.
//!
//! Ported from `test_orchestrate_launch_e2e`, `test_attach_settles_e2e`, `test_run_ownership_e2e`, `test_run_adoption_e2e`, `test_relaunch_seed_e2e`, and the driver-liveness half of `test_liveness_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The scenario this journey states is one a real sibling would need paid
// model turns to produce, and `dispatch.rs` is where the real `oneagentgraph` binary is
// driven instead. `harness.rs` carries the same suppression and the full rationale.

use std::path::{Path, PathBuf};

use crate::harness::{agent, human, plan_of, World, NOTHING_DRIVING, REFUSED};
// The journeys that end a process, and those that assert against a process table,
// are `#[cfg(unix)]`, so what only they reach for is imported on the same terms.
// Both names have to be: `end_process` is `#[cfg(unix)]` in `harness.rs`, so an
// unconditional import of it does not resolve for a Windows target at all, and
// `reaped_pid` — which is not gated there — is used in this file only from one of
// those journeys, so importing it unconditionally is an unused import under
// `-D warnings`. Each was a Windows-only build failure this host's own gate,
// which compiles the unix half, cannot see.
#[cfg(unix)]
use crate::harness::{end_process, reaped_pid};
use serde_json::json;

fn start_detached(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    start_detached_announcing(world, name, nodes).0
}

/// The same launch, with an observer graph attached.
///
/// The shipped default is `--dag-graph off` — a run needs no agent — so a
/// journey about what a launched graph is given asks for one explicitly.
fn start_detached_observed(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--detach",
            "--dag-graph",
            &world.shipped_dag_graph(),
        ])
        .exited(0);
    name.to_string()
}

/// The same launch, with the driver pid it announced.
///
/// A detached launch prints its run and the pid it retained, which is how an
/// operator addresses the driver of a run nobody is attached to — so a journey
/// about what that driver's teardown reaches asks for it the same way.
fn start_detached_announcing(
    world: &World,
    name: &str,
    nodes: Vec<serde_json::Value>,
) -> (String, u32) {
    let path = world.plan(name, &plan_of(name, nodes));
    let started = world.run(&["start", &path.to_string_lossy(), "--detach"]);
    started.exited(0);
    let announced: serde_json::Value = serde_json::from_str(started.stdout.trim())
        .unwrap_or_else(|error| panic!("a detached launch announces itself: {error}"));
    let pid = announced["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or_else(|| panic!("the launch announced no driver: {announced}"));
    (name.to_string(), pid)
}

/// The directory every dag-scope launch was given, in launch order.
///
/// Read off the argv the sibling was actually invoked with, because the value
/// under test is the one that *crosses the seam* — it becomes the `--cwd` each
/// member's harness runs in, and a run whose two launch paths disagree about it
/// is a run whose members move when it is adopted.
// llmlint: ignore-block[tests_mirror_real_usage] the argv the double records *is* the
// interface under test: what a launch hands `oneagentgraph` is not something any product
// surface of this crate reports, and it is what decides where a member's harness runs. The
// launch record's own copy is asserted separately, in the same journey; this is the half
// that proves the recorded value is the one that actually crossed.
fn dag_launch_dirs(world: &World) -> Vec<String> {
    world
        .invocations()
        .iter()
        .filter(|call| {
            call["tool"] == "oneagentgraph"
                && call["args"][0] == "run"
                && call["args"][1]
                    .as_str()
                    .is_some_and(|graph| graph.ends_with("dag-scope.yaml"))
        })
        .filter_map(|call| {
            let args = call["args"].as_array()?;
            let at = args.iter().position(|arg| arg == "--dir")?;
            args.get(at + 1)?.as_str().map(str::to_string)
        })
        .collect()
}

/// The `--task` every dag-scope launch was given, in launch order.
///
/// The same reading as [`dag_launch_dirs`] and for the same reason: this value
/// only exists on the wire, and `oneagentgraph` hands it to every member of the
/// graph carrying none of its own.
fn dag_launch_tasks(world: &World) -> Vec<String> {
    world
        .invocations()
        .iter()
        .filter(|call| {
            call["tool"] == "oneagentgraph"
                && call["args"][0] == "run"
                && call["args"][1]
                    .as_str()
                    .is_some_and(|graph| graph.ends_with("dag-scope.yaml"))
        })
        .filter_map(|call| {
            let args = call["args"].as_array()?;
            let at = args.iter().position(|arg| arg == "--task")?;
            args.get(at + 1)?.as_str().map(str::to_string)
        })
        .collect()
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// The role prose the launcher's task is free of, whichever way it is composed.
const ROLE_PROSE: &[&str] = &[
    "Drive",
    "to settlement",
    "Observe",
    "nothing else",
    "run state",
];

fn assert_says_only_what_the_run_is(task: &str, run: &str, goal: &str) {
    assert!(
        task.contains(run) && task.contains(goal),
        "the launched graph's task does not name the run and its goal: {task}"
    );
    for prose in ROLE_PROSE {
        assert!(
            !task.contains(prose),
            "the launched graph's task tells a member what to do ({prose:?}): {task}"
        );
    }
}

/// What the dag-scope graph is launched with names the run and its goal, at
/// `start` and again at `adopt`.
///
/// The adoption half is the one a launch cannot state for itself: a fresh driver
/// composes the description from the run's **projected** plan rather than from
/// the plan file the launch named, because the planner may have edited the graph
/// since and that file may not be there at all.
#[test]
fn the_launched_graphs_task_names_the_run_and_its_goal_at_start_and_at_adoption() {
    let world = World::new("driver-task");
    let run = start_detached_observed(&world, "described", vec![human("approve", &[])]);
    // `plan_of` states this goal, so a task without it is one the plan never
    // reached.
    let goal = "Deliver described";
    let launched = dag_launch_tasks(&world);
    assert_eq!(launched.len(), 1, "{launched:?}");
    assert_says_only_what_the_run_is(&launched[0], &run, goal);

    world.until("the driver to exit", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });
    // A run parked on a person is *awaiting the planner*, not abandoned: the
    // adoption picks it up and returns on the decision it cannot clear.
    world.run(&["adopt", &run]).exited(0);
    let relaunched = dag_launch_tasks(&world);
    assert_eq!(relaunched.len(), 2, "{relaunched:?}");
    assert_says_only_what_the_run_is(&relaunched[1], &run, goal);
}

/// A goal is optional in the plan schema, and the task's shape is not.
///
/// A member composing `{task}` plus its own prose reads the same document either
/// way, so a plan that states no goal says *that* rather than leaving the line
/// out. The absent field is the only case to answer for: a goal that is present
/// and blank never gets this far, because `start` refuses the plan.
#[test]
fn a_run_whose_plan_states_no_goal_is_launched_saying_so() {
    let world = World::new("driver-task-goalless");
    let mut plan = plan_of("goalless", vec![human("approve", &[])]);
    plan.as_object_mut()
        .expect("a plan object")
        .remove("goal")
        .expect("the shared plan states a goal to remove");
    let path = world.plan("goalless", &plan);
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--detach",
            "--dag-graph",
            &world.shipped_dag_graph(),
        ])
        .exited(0);

    let launched = dag_launch_tasks(&world);
    assert_eq!(launched.len(), 1, "{launched:?}");
    assert_says_only_what_the_run_is(&launched[0], "goalless", "(no goal stated)");
}

#[test]
fn start_launches_the_named_dag_scope_graph_and_records_how_to_relaunch_it() {
    let world = World::new("driver-launch");
    world.script("build.wait", "hold");
    let path = world.plan("launched", &plan_of("launched", vec![agent("build", &[])]));
    let started = world.run(&[
        "start",
        &path.to_string_lossy(),
        "--detach",
        "--dag-graph",
        &world.shipped_dag_graph(),
        "--set",
        "members.monitor.agent.model=first value",
        "--set=members.check-in.model=second=value",
        "--node-set",
        "members.worker.agent.model=node value",
    ]);
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
    assert_eq!(launch["heartbeat_interval"], 1_800);
    assert!(
        launch.get("round_budget").is_none(),
        "a retired field survived in the launch record: {launch}"
    );
    assert_eq!(
        launch["dag_sets"],
        json!([
            "members.monitor.agent.model=first value",
            "members.check-in.model=second=value"
        ])
    );
    assert_eq!(
        launch["node_sets"],
        json!(["members.worker.agent.model=node value"])
    );
    assert_eq!(launch["launcher"], "e2e");
    assert!(launch["graph"]
        .as_str()
        .expect("a graph")
        .ends_with("dag-scope.yaml"));
    // The pid recorded is the retained driver's, not this command's: what drives
    // the run is that process, and it is this build rather than any agent.
    assert_ne!(launch["pid"], json!(0));
    world.release("build.go");
}

/// The headline of the roundless contract: a plan runs with no agent at all.
///
/// Default flags, a dependency chain, and nothing launched but the dispatches
/// themselves — every downstream node started by the settlement of the one
/// before it, with no round anywhere in the record.
#[test]
fn a_dependency_chain_runs_to_completion_under_start_with_no_agent_graph() {
    let world = World::new("driver-continuous");
    let path = world.plan(
        "chained",
        &plan_of(
            "chained",
            vec![
                agent("first", &[]),
                agent("second", &["first"]),
                agent("third", &["second"]),
            ],
        ),
    );
    world
        .run(&["start", &path.to_string_lossy()])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");

    // No agent graph was launched: `--dag-graph` defaults to `off`, so the only
    // thing the sibling was asked for is the three node dispatches.
    let dag_launches = world
        .invocations()
        .into_iter()
        .filter(|call| {
            call["args"][1]
                .as_str()
                .is_some_and(|graph| graph.ends_with("dag-scope.yaml"))
        })
        .count();
    assert_eq!(
        dag_launches,
        0,
        "a run with default flags launched an agent graph: {:?}",
        world.invocations()
    );

    // Each node became ready on its dependency settling, and was dispatched from
    // there. The order is the chain's, and there is no round event in it.
    let kinds = world.kinds("chained");
    assert!(
        kinds.iter().all(|kind| !kind.starts_with("round-")),
        "a round event reached the journal: {kinds:?}"
    );
    let ready: Vec<String> = world
        .events_of("chained", "node-ready")
        .iter()
        .map(|event| event["labels"]["node"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(ready, vec!["first", "second", "third"]);

    // Dispatched immediately on settlement, not after any barrier: each node's
    // dispatch follows the previous node's settlement with nothing between them.
    let sequence: Vec<String> = world
        .journal("chained")
        .iter()
        .filter(|event| event["kind"] == "node-dispatched" || event["kind"] == "node-settled")
        .map(|event| {
            format!(
                "{} {}",
                event["kind"].as_str().unwrap_or(""),
                event["labels"]["node"].as_str().unwrap_or("")
            )
        })
        .collect();
    assert_eq!(
        sequence,
        vec![
            "node-dispatched first",
            "node-settled first",
            "node-dispatched second",
            "node-settled second",
            "node-dispatched third",
            "node-settled third",
        ]
    );
    assert_eq!(
        world.run_json("chained", "result.json")["state"],
        "complete"
    );
}

#[test]
fn the_launch_record_exists_before_the_member_the_launcher_starts_reads_the_ledger() {
    let world = World::new("driver-ordering");
    // The observer is held at its first instruction, *after* it has recorded
    // what the ledger held. Holding it there is what makes this about the
    // launcher's ordering and nothing else.
    world.script("observer.wait", "hold");
    world.script("build.wait", "hold");
    let path = world.plan("ordered", &plan_of("ordered", vec![agent("build", &[])]));
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--detach",
            "--dag-graph",
            &world.shipped_dag_graph(),
        ])
        .exited(0);

    world.until("the observer to read the run's ledger", |world| {
        !world.observer_saw().is_empty()
    });
    // A member that opens the run's ledger dies on a file nobody wrote if the
    // record is written second, and the run then sits at `run-started` with
    // nothing driving it: a mysteriously hung run rather than the ordering bug
    // it is.
    let saw = world.observer_saw();
    assert_eq!(saw[0]["run"], "ordered");
    assert_eq!(
        saw[0]["launch_record"],
        json!(true),
        "a launched member was started before the launch record existed: {saw:?}"
    );
    // And the retained driver claimed the run, which `start --detach` waits for
    // and would have refused without.
    assert_ne!(world.run_json("ordered", "launch.json")["pid"], json!(0));
    world.release("observer.go");
    world.release("build.go");
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
    let path = world.plan(
        "unattended",
        &plan_of("unattended", vec![agent("build", &[])]),
    );
    // The one node fails, so nothing is ready, nothing is waiting on a person,
    // and no surface is blocking: the run is simply not being driven any more.
    world.script("build.fail", "1");
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(NOTHING_DRIVING)
        .out_has("\"settlement\":\"unattended\"");
}

/// A run parked on a person is *awaiting the planner*, not abandoned.
///
/// The distinction the exit codes draw: exit 3 sends an operator to intervene in
/// a run nothing is driving, and a run waiting for an attestation is doing
/// exactly what it should.
#[test]
fn an_attach_returns_awaiting_planner_when_a_decision_point_is_all_that_is_left() {
    let world = World::new("driver-awaiting");
    let path = world.plan(
        "awaiting",
        &plan_of("awaiting", vec![human("approve", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0)
        .out_has("\"settlement\":\"awaiting-planner\"");
    assert_eq!(
        world.events_of("awaiting", "decision-pending").len(),
        1,
        "the decision point was never reported: {:?}",
        world.kinds("awaiting")
    );
}

#[test]
fn a_live_driver_that_has_stopped_writing_reads_as_parked_rather_than_dead() {
    let world = World::new("driver-parked");
    // The driver is alive with a dispatch held open, so its pid proves nothing
    // about progress — which is the whole distinction `PARKED` exists to draw.
    world.script("build.wait", "hold");
    let run = start_detached(&world, "quiet", vec![agent("build", &[])]);

    world.until("the run to be reported parked", |world| {
        let mut status = world.cmd(&["status", &run]);
        // A second of silence is enough to call it: what is under test is the
        // verdict, not the threshold.
        status.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
        let out = status.output().expect("the binary runs");
        String::from_utf8_lossy(&out.stdout).contains("PARKED")
    });

    // Parked is not dead: the ledger is intact and the way back is the same.
    let mut parked = world.cmd(&["status", &run]);
    parked.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    let rendered = String::from_utf8_lossy(&parked.output().expect("runs").stdout).to_string();
    assert!(!rendered.contains("DRIVER DEAD"), "{rendered}");
    assert!(rendered.contains("adopt"), "{rendered}");
    world.release("build.go");
}

/// The same silence, with a decision point outstanding, is *not* parked.
///
/// The discriminating counterpart to the journey above: identical held dispatch,
/// identical live pid, identical quiet ledger, and the one difference is a human
/// node nobody has attested. A run waiting on a person is doing exactly what it
/// should, so reporting it `PARKED` sends an operator to adopt work that needs no
/// rescue — and `adopt` may end the driver it finds, which would cost the held
/// dispatch for nothing.
#[test]
fn a_quiet_driver_holding_a_decision_point_reads_as_active_rather_than_parked() {
    let world = World::new("driver-quiet-deciding");
    // The held dispatch keeps the pid alive and the ledger silent; the human node
    // beside it is independent of that dispatch, so it is ready and unattested
    // while the run goes quiet — which is what makes the decision outstanding.
    world.script("build.wait", "hold");
    let run = start_detached(
        &world,
        "deciding",
        vec![agent("build", &[]), human("approve", &[])],
    );

    // Wait for the state the verdict is read against: the dispatch is held open,
    // so from here the ledger has nothing further to write.
    world.until("the held node to be dispatched", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });
    // Past the threshold the neighbouring journey parks at, so silence alone can
    // no longer be what keeps this verdict `ACTIVE`.
    std::thread::sleep(std::time::Duration::from_secs(2));

    let mut status = world.cmd(&["status", &run]);
    status.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    let rendered =
        String::from_utf8_lossy(&status.output().expect("the binary runs").stdout).to_string();
    assert!(
        !rendered.contains("PARKED"),
        "a run waiting on a person was reported parked: {rendered}"
    );
    assert!(
        rendered.contains("ACTIVE"),
        "a run waiting on a person is not reported active: {rendered}"
    );
    world.release("build.go");
}

#[test]
fn a_parked_run_is_adoptable_as_much_as_a_dead_one_is() {
    let world = World::new("driver-adopt-parked");
    // Its driver is alive with a dispatch held open, so this is the *other*
    // undriven verdict: nothing has proved the process gone, and nothing is
    // happening either.
    world.script("build.wait", "hold");
    let run = start_detached(&world, "idle", vec![agent("build", &[])]);
    world.until("the run to be reported parked", |world| {
        let mut status = world.cmd(&["status", &run]);
        status.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
        let out = status.output().expect("the binary runs");
        String::from_utf8_lossy(&out.stdout).contains("PARKED")
    });

    // The hint a parked run prints has to be a hint that works: an `adopt` that
    // refused here would leave the only offered way back closed. It stays
    // attached until the run settles, so the work it picks up is released from
    // beside it: the node the dead driver left in flight is re-dispatched by the
    // fresh one, and this is that dispatch being let go.
    let mut adopt = world.cmd(&["adopt", &run]);
    adopt.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    let adopted = std::thread::scope(|scope| {
        scope.spawn(|| {
            world.until("the fresh driver to re-dispatch the held node", |world| {
                world.events_of(&run, "node-dispatched").len() >= 2
            });
            world.release("build.go");
        });
        adopt.output().expect("the binary runs")
    });
    assert!(
        !String::from_utf8_lossy(&adopted.stderr).contains("still being driven"),
        "a parked run refused the adoption its own status line offers: {}",
        String::from_utf8_lossy(&adopted.stderr)
    );
    assert_eq!(world.events_of(&run, "driver-adopted").len(), 1);
    // Taking the run over ended the driver that was holding it: the loop an
    // adoption starts is the run's single writer, so the parked one could not
    // stay.
    assert!(
        String::from_utf8_lossy(&adopted.stderr).contains("ending it to adopt the run"),
        "the parked driver was left holding the run: {}",
        String::from_utf8_lossy(&adopted.stderr)
    );
    // And the work the dead driver had in flight was offered to the fresh one: a
    // node left recorded as running is a node nothing runs and nothing settles,
    // which is a loop that spins on it for good.
    assert_eq!(
        world.run_json(&run, "result.json")["state"],
        "complete",
        "the adopted run did not finish the work it took over:\n{}",
        world.dump()
    );
}

#[test]
fn a_dead_driver_reads_as_driver_dead_and_adopt_is_the_way_back() {
    let world = World::new("driver-dead");
    let path = world.plan(
        "orphaned",
        &plan_of("orphaned", vec![human("approve", &[])]),
    );
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--detach",
            "--dag-graph",
            &world.shipped_dag_graph(),
            "--set",
            "members.monitor.agent.model=adopted model",
        ])
        .exited(0);
    let run = "orphaned".to_string();
    world.until("the driver to exit", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });

    world
        .run(&["runs"])
        .exited(0)
        .out_has("DRIVER DEAD")
        .out_has("onepipeline adopt orphaned");

    // The ledger is intact, so a fresh driver takes it over.
    let launches_before = world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph" && call["args"][0] == "run")
        .count();
    let adopted = world.run(&["adopt", &run]);
    adopted.exited(0);
    let adoptions = world.events_of(&run, "driver-adopted");
    assert_eq!(adoptions.len(), 1, "{adoptions:?}");
    assert_eq!(world.run_json(&run, "launch.json")["adoptions"], 1);
    // The dead driver's evidence moves aside rather than being truncated.
    assert!(world.run_file(&run, "launch.pre-adopt-1.json").exists());
    let launches = world.invocations();
    let relaunched = launches
        .iter()
        .filter(|call| call["tool"] == "oneagentgraph" && call["args"][0] == "run")
        .nth(launches_before)
        .expect("adopt relaunched the dag graph");
    let args = relaunched["args"].as_array().expect("recorded argv");
    let set_at = args
        .iter()
        .position(|arg| arg == "--set")
        .expect("the adopted launch retained --set");
    assert_eq!(
        args[set_at + 1],
        "members.monitor.agent.model=adopted model"
    );
}

/// One run, one directory — whichever launch path started the driver.
///
/// The two paths used to disagree. Neither passed a directory at all, and the
/// two backends defaulted an absent one differently: the retained-process path
/// inherited `oneagentgraph`'s own CLI default of `.`, resolved against whatever
/// process ends up spawning the graph, and the library path fell back to the
/// launcher's working directory. Both consequences are silent. That directory
/// becomes the `--cwd` every member's harness runs in, so a member can start
/// refusing — or stop refusing — purely because the run was adopted; and
/// `oneharness` derives its history project from it, so one run's records split
/// across two project directories with neither holding the whole run.
///
/// So the two are driven here and compared at the seam, and the adoption is run
/// from **another directory** on purpose: the answer must be the directory the
/// run was launched in, replayed out of the launch record, and not wherever the
/// operator happened to be standing when they typed `adopt`.
#[test]
fn start_and_adopt_give_the_sibling_the_same_directory_for_one_run() {
    let world = World::new("driver-launch-dir");
    // A human action nothing can clear: the loop has nowhere to go and returns,
    // which is what leaves the run adoptable.
    let path = world.plan(
        "relocated",
        &plan_of("relocated", vec![human("approve", &[])]),
    );
    let launched_from = world.project.clone();
    let mut start = world.cmd(&[
        "start",
        &path.to_string_lossy(),
        "--detach",
        "--dag-graph",
        &world.shipped_dag_graph(),
    ]);
    start.current_dir(&launched_from);
    world.run_on(start, "start relocated").exited(0);
    let run = "relocated".to_string();
    world.until("the driver to exit", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });

    // Somewhere else entirely, which is the ordinary case: an operator adopts a
    // run from whatever shell noticed it was undriven.
    let adopted_from = world.root.clone();
    assert_ne!(adopted_from, launched_from);
    let mut adopt = world.cmd(&["adopt", &run]);
    adopt.current_dir(&adopted_from);
    world.run_on(adopt, "adopt relocated").exited(0);
    assert_eq!(world.events_of(&run, "driver-adopted").len(), 1);

    let dirs = dag_launch_dirs(&world);
    assert_eq!(
        dirs.len(),
        2,
        "expected one launch from `start` and one from `adopt`: {dirs:?}"
    );
    assert_eq!(
        dirs[0], dirs[1],
        "`start` and `adopt` sent the sibling two different directories for one run"
    );
    assert_eq!(
        Path::new(&dirs[0]),
        launched_from,
        "the run moved to the directory the adoption was typed in"
    );

    // And a reader of the run's own records can tell which directory it used
    // without inferring it from whichever process launched the driver.
    assert_eq!(
        world.run_json(&run, "launch.json")["dir"],
        json!(launched_from)
    );
    let started = world.events_of(&run, "run-started");
    assert_eq!(started[0]["payload"]["dir"], json!(launched_from));
}

/// A second spelling of one directory: a symlinked route to it.
///
/// The shape macOS ships by default — `/var` is a link to `/private/var`, so
/// every temporary directory there is reached through one — built here because
/// a symlink is a symlink on Linux too. What matters is only that it is a route
/// to the directory rather than the directory's own name, and that a process
/// changed into it reports the directory.
#[cfg(unix)]
fn another_spelling_of(dir: &Path) -> PathBuf {
    let route = dir.with_file_name("routed-through");
    std::os::unix::fs::symlink(dir, &route).expect("a symlinked route to the directory");
    route
}

/// A second spelling of one directory: a detour out through its parent.
///
/// Where a directory symlink needs a privilege no CI account is guaranteed to
/// hold, this is the route every platform offers instead — `SetCurrentDirectory`
/// resolves it away exactly as `chdir` does, so the process still reports the
/// directory rather than the way it was reached.
#[cfg(not(unix))]
fn another_spelling_of(dir: &Path) -> PathBuf {
    let leaf = dir.file_name().expect("the directory has a name");
    dir.join("..").join(leaf)
}

/// A launch records the directory the process resolves, not the route to it.
///
/// One directory has more than one spelling on every platform, and the record
/// has to carry the one every process agrees on: it is read by a *different*
/// process at every `adopt`, and it reaches each member's harness as its
/// `--cwd`, from which `oneharness` derives the project its history is kept
/// under. Two spellings of one directory is two projects, and a run whose
/// records do not add up.
///
/// So what is recorded is the kernel's own answer — [`std::env::current_dir`],
/// read once by the process the operator ran — and never the argument that
/// process was handed. This is the journey that says so: the launch is typed
/// through a route to the directory, and every place the value lands names the
/// directory instead.
#[test]
fn a_launch_records_the_directory_the_process_resolves_not_the_route_to_it() {
    let world = World::new("driver-launch-dir-route");
    let route = another_spelling_of(&world.project);
    assert_ne!(
        route, world.project,
        "the route is not a second spelling, so this journey proves nothing"
    );

    let path = world.plan("routed", &plan_of("routed", vec![human("approve", &[])]));
    let mut start = world.cmd(&[
        "start",
        &path.to_string_lossy(),
        "--detach",
        "--dag-graph",
        &world.shipped_dag_graph(),
    ]);
    start.current_dir(&route);
    world.run_on(start, "start routed").exited(0);
    let run = "routed".to_string();
    world.until("the driver to exit", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });

    assert_eq!(
        world.run_json(&run, "launch.json")["dir"],
        json!(world.project),
        "the record kept the route the launch was typed through"
    );
    assert_eq!(
        world.events_of(&run, "run-started")[0]["payload"]["dir"],
        json!(world.project)
    );
    assert_eq!(
        Path::new(dag_launch_dirs(&world).first().expect("the launch")),
        world.project,
        "the sibling was sent the route rather than the directory"
    );
}

/// A run recorded by a build that had no directory field keeps the reading it
/// had, and one that records an unusable directory is refused by name.
///
/// The first is the whole compatibility promise: a record written before the
/// field existed carries none, and the driver such a run had ran in its own
/// working directory — so that is what an adoption of it still gives the
/// sibling, rather than a directory this build invented for it. The second is
/// what stops the field from being trusted just because it is there: it is a
/// file on disk, and it decides where every member of the run works.
// llmlint: ignore-block[tests_mirror_real_usage] neither state has an engine-side
// constructor: `start` resolves an absolute launch directory and `adopt` replays it, so no
// verb writes a launch record with **no `dir`**, nor one whose **`dir` is relative or is
// not a directory**. Nothing here is assembled by hand — what is mutated is the record
// `start` itself wrote, one field at a time, and the removal asserts it removed something
// so a renamed field cannot leave this journey arranging nothing. Every other step —
// `start`, `status`, `adopt` — is the real binary.
#[test]
fn a_launch_record_without_a_directory_is_replayed_from_the_adopting_process() {
    let world = World::new("driver-legacy-dir");
    let run = start_detached_observed(&world, "legacy", vec![human("approve", &[])]);
    world.until("the driver to exit", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });

    let rewrite = |record: &mut serde_json::Value| {
        let path = world.run_file(&run, "launch.json");
        std::fs::write(&path, record.to_string()).expect("the record is rewritten");
    };

    // A record from before the field: no `dir` at all. Taking away a field this
    // build no longer writes would arrange nothing, and every assertion below
    // would then be made against an ordinary record — so the removal has to have
    // removed something.
    let mut record = world.run_json(&run, "launch.json");
    assert!(
        record
            .as_object_mut()
            .expect("the record is an object")
            .remove("dir")
            .is_some(),
        "the record this build wrote carries no `dir` to take away: {record}"
    );
    rewrite(&mut record);

    let adopted_from = world.project.clone();
    let mut adopt = world.cmd(&["adopt", &run]);
    adopt.current_dir(&adopted_from);
    world.run_on(adopt, "adopt legacy").exited(0);
    let dirs = dag_launch_dirs(&world);
    assert_eq!(
        Path::new(dirs.last().expect("the adoption relaunched the graph")),
        adopted_from,
        "a record with no directory was not replayed from the adopting process: {dirs:?}"
    );

    // And a record that carries one it cannot honour is refused by name rather
    // than handed on for each member to fail against separately. One case per
    // guard, and each spelled so it means the same thing wherever this runs:
    // whether a path is absolute is a platform's own rule, so a fixture that
    // reads as absolute here and relative there reaches a different guard on
    // each and proves neither. `/no/such/place/here` is exactly that — absolute
    // on Unix, and relative on Windows, where an absolute path carries a drive.
    let occupied = world.root.join("not-a-directory");
    std::fs::write(
        &occupied,
        "a file, so the record names something that is not a directory",
    )
    .expect("a file for the record to point at");
    for (dir, absolute, reason) in [
        (
            PathBuf::from("relative/place"),
            false,
            "relative working directory",
        ),
        (occupied, true, "which is not a directory"),
    ] {
        assert_eq!(
            dir.is_absolute(),
            absolute,
            "'{}' does not reach the guard this case is written for on this platform",
            dir.display()
        );
        let mut record = world.run_json(&run, "launch.json");
        record["dir"] = json!(dir);
        rewrite(&mut record);
        world
            .run(&["adopt", &run])
            .exited(REFUSED)
            .err_has(reason)
            .err_has(&dir.to_string_lossy());
    }
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// An adoption re-addresses the pacemaker at the graph run now driving the run.
///
/// An adoption starts a *fresh* graph run with an id of its own, so the id the
/// record carried is a run `oneagentgraph` has already finished with. A reset
/// sent there restarts nothing, which is the same silence the original defect
/// had — the record has to name the run that is driving now.
// llmlint: ignore-block[tests_mirror_real_usage] the id the reset is *addressed by* is not
// on any product surface — `next` prints the surface, and a reset that failed is a line on
// stderr — so the argv the double recorded is the only place the address exists. The
// record's own two values are read through the product's file, and the claim that the old
// one was not used has nowhere else to be observed.
#[test]
fn adoption_re_addresses_the_pacemaker_at_the_graph_run_now_driving() {
    let world = World::new("driver-adopt-pacemaker");
    let run = start_detached_observed(&world, "readdressed", vec![human("approve", &[])]);
    world.until("the driver to exit", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });
    let before = world.run_json(&run, "launch.json")["graph_run"]
        .as_str()
        .expect("the first driver's graph run")
        .to_string();

    world.run(&["adopt", &run]).exited(0);
    let after = world.run_json(&run, "launch.json")["graph_run"]
        .as_str()
        .expect("the adopted driver's graph run")
        .to_string();
    assert_ne!(
        after, before,
        "the record still names the graph run that died"
    );

    world
        .run(&["surface", &run, "--kind", "check-in", "--message", "steady"])
        .exited(0);
    world.run(&["next", &run]).exited(0);
    assert!(
        world.was_invoked("oneagentgraph", &["reset-timer", &after, "check-in"]),
        "the reset was not addressed at the graph run driving the run now: {:?}",
        world.invocations()
    );
    assert!(
        !world.was_invoked("oneagentgraph", &["reset-timer", &before]),
        "the reset went to the graph run that had already died: {:?}",
        world.invocations()
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A run that records no graph run says why its pacemaker was not reset, and
/// still hands the planner the surface they asked for.
///
/// The case a record written before the field existed leaves: there is no
/// address, so there is nothing to send. Saying so is the point — the reset is
/// best-effort, and a silent no-op here is exactly the failure that went
/// unnoticed for as long as it did.
// llmlint: ignore-block[tests_mirror_real_usage] no verb writes a launch record with **no
// `graph_run`**, nor one whose **`graph_run` is a path the sibling would refuse**: every
// launch records the run it started and every `adopt` rewrites it. As above the record is
// the one `start` wrote and the mutation is one field of it, with the removal asserting it
// removed something. What is mostly asserted is the product's own answer — `next`'s exit
// code, its surface, and its stderr — but the closing claim is that *nothing was sent*, and
// the only place a value that never crossed the seam can be observed is the log of what
// did. A product surface reporting "no reset was attempted" does not exist, and inventing
// one to make the assertion product-shaped would be a surface nobody asked for.
#[test]
fn a_run_with_no_recorded_graph_run_says_why_the_pacemaker_was_not_reset() {
    let world = World::new("driver-no-graph-run");
    world.script("build.wait", "hold");
    let run = start_detached_observed(&world, "unaddressed", vec![agent("build", &[])]);
    world.until("the run to dispatch its node", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });
    world
        .run(&["surface", &run, "--kind", "check-in", "--message", "steady"])
        .exited(0);

    // As above, the removal has to have removed something: a record this build
    // never wrote the field into would make every claim below vacuous.
    let mut record = world.run_json(&run, "launch.json");
    assert!(
        record
            .as_object_mut()
            .expect("the record is an object")
            .remove("graph_run")
            .is_some(),
        "the record this build wrote carries no `graph_run` to take away: {record}"
    );
    std::fs::write(world.run_file(&run, "launch.json"), record.to_string())
        .expect("the record is rewritten");

    let read = world.run(&["next", &run]);
    read.exited(0).out_has("steady");
    read.err_has("could not reset the check-in pacemaker")
        .err_has("records no agent-graph run");

    // And one that carries a value the sibling would never answer to — a run
    // store is a directory, and this field is joined onto it. Refused with the
    // value named, not sent, and the surface is still the planner's.
    let mut record = world.run_json(&run, "launch.json");
    record["graph_run"] = json!("../elsewhere");
    std::fs::write(world.run_file(&run, "launch.json"), record.to_string())
        .expect("the record is rewritten");
    world
        .run(&["surface", &run, "--kind", "check-in", "--message", "again"])
        .exited(0);
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("again");
    read.err_has("could not reset the check-in pacemaker")
        .err_has("../elsewhere");

    assert!(
        !world.was_invoked("oneagentgraph", &["reset-timer"]),
        "a reset was sent with no run to address it to: {:?}",
        world.invocations()
    );
    world.release("build.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

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

    // Recording the stop is not stopping it. The ledger says stopped either
    // way — `status` reads `run-stopped` and reports the run undriven without
    // ever looking at a process — so the ledger cannot be the evidence here.
    let status = world.run(&["status", &run]);
    status.exited(0).out_has("nothing is driving this run");

    // And what became of the work in flight is said, in both views that report
    // it. A stop ends the run's whole dispatch tree, and the process that would
    // have settled this node was in that tree — so the last thing the record
    // holds for it is that it started, and a reader with nothing else to go on
    // takes a worker that was *ended mid-edit* for one that produced nothing.
    status.out_has("build: worker ended when the run was stopped");
    assert!(
        !status.stdout.contains("build: running for"),
        "a node whose worker the stop ended is still reported as working:\n{}",
        status.stdout
    );
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("worker ended when the run was stopped");

    // The process itself, where this host can see one. Asserted under `cfg`
    // rather than through a path that simply does not exist elsewhere: a probe
    // that is vacuously true off Linux would read as coverage and prove
    // nothing.
    #[cfg(target_os = "linux")]
    {
        let driver = world.run_json(&run, "launch.json")["pid"]
            .as_u64()
            .expect("a recorded driver");
        world.until("the driver process to end", |_| {
            !Path::new(&format!("/proc/{driver}")).exists()
        });
    }
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
fn the_driving_process_is_a_single_writer() {
    let world = World::new("driver-lock");
    world.script("build.wait", "hold");
    let run = start_detached(&world, "locked", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });

    // The driving process holds the run's ownership lock for as long as it is
    // driving, and `adopt` is the only documented way to point a second loop at
    // a run that already has one. It refuses, rather than interleaving.
    world
        .run(&["adopt", &run])
        .exited(REFUSED)
        .err_has("still being driven");
    // And the run really was still being written while it refused: the graph is
    // where a second writer would have shown up, and it holds exactly the one
    // dispatch this driver started.
    assert_eq!(
        world.events_of(&run, "node-dispatched").len(),
        1,
        "something wrote to the run beside its driver: {:?}",
        world.kinds(&run)
    );
    world.release("build.go");
}

#[test]
fn a_detached_launch_drives_the_whole_chain_and_records_one_result() {
    let world = World::new("driver-detached-drive");
    let run = start_detached(
        &world,
        "unattendedchain",
        vec![agent("first", &[]), agent("second", &["first"])],
    );

    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    let result = world.run_json(&run, "result.json");
    assert_eq!(result["state"], "complete");
    assert_eq!(result["ok"], json!(true));
    // One document for the run, not one per round: there are no rounds, and
    // nothing beside it claims to be the frontier.
    assert!(
        !world.run_file(&run, "round-01").exists(),
        "a round directory was written"
    );
    let ids: Vec<&str> = result["nodes"]
        .as_array()
        .expect("the result names every node")
        .iter()
        .map(|node| node["id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(ids, vec!["first", "second"]);
}

#[test]
fn a_detached_run_settles_and_its_driver_is_left_a_log_to_write_to() {
    let world = World::new("driver-detached-output");
    let path = world.plan(
        "detachedlog",
        &plan_of("detachedlog", vec![agent("build", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    let run = "detachedlog".to_string();

    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    assert_eq!(world.run_json(&run, "result.json")["state"], "complete");

    let log = world.run_file(&run, "driver.log");
    assert!(
        log.exists(),
        "the detached driver was given no log to write"
    );
}

#[test]
fn a_detached_start_returns_while_the_run_it_launched_is_still_in_flight() {
    let world = World::new("driver-detach-returns");
    // The one dispatch this run has is held, so the run cannot settle until
    // this test releases it.
    world.script("build.wait", "hold");
    let run = start_detached(&world, "inflight", vec![agent("build", &[])]);

    // `start --detach` has already returned. A launcher that returns only once
    // the run has finished has not detached from it at all — which is what a
    // launcher does when the driver it starts inherits, and holds open, the
    // streams its own caller is reading.
    assert!(
        !world.run_file(&run, "result.json").is_file(),
        "the launch did not return until the run had settled: {:?}",
        world.kinds(&run)
    );

    world.release("build.go");
    world.until("the run to settle once it is released", |world| {
        world.run_file(&run, "result.json").is_file()
    });
}

/// A refusal that takes its time is still a refusal.
///
/// This is the shape a launcher gets wrong most easily. A graph validates before
/// it announces itself, and how long that takes is the host's business — a
/// config fetched over the network, a machine under load, a process the
/// scheduler has not run yet. A launcher that waited a fixed moment and then
/// called a still-running process started would report exactly this refusal as a
/// running driver, print its pid, and leave the run undriven with the reason in
/// a file nobody opens.
///
/// So the launch is held until the graph *answers* — and the answer here comes
/// well after any window a launcher might have waited instead.
#[test]
fn a_refusal_slower_than_any_launch_window_still_fails_the_launch() {
    let world = World::new("driver-slow-refusal");
    world.script("run.refuse-after", "1500");
    let path = world.plan("stalled", &plan_of("stalled", vec![agent("build", &[])]));

    // Both launch forms: the graph's own words are in a different place for
    // each, and a launcher that reported this one as started would do it for
    // whichever form it did not wait on.
    for form in ["--detach", "--attach"] {
        let started = world.run(&[
            "start",
            &path.to_string_lossy(),
            form,
            "--dag-graph",
            &world.shipped_dag_graph(),
        ]);

        started.exited(REFUSED);
        started.err_has("oneagentgraph");
        started.err_has("a member that does not exist");
        assert!(
            !started.stdout.contains("\"pid\""),
            "`start {form}` reported a pid for a graph that went on to refuse:\n{}",
            started.stdout
        );
    }

    // An unusable bound falls back to the default rather than to nothing. Read
    // as zero, the wait would end before any graph could answer and every launch
    // would fail as unanswered — so this refusal, which arrives long after a
    // zero bound would have given up, has to come back as the graph's own words
    // and not as a launch that waited no time at all.
    //
    // Both ways a value is unusable, because they are unusable for different
    // reasons and only one of them is caught by reading it: `0` is a perfectly
    // good number, and the only thing standing between it and a wait that ends
    // before it starts is the crate declining to take it.
    for unusable in ["however long it takes", "0"] {
        let mut launch = world.cmd(&[
            "start",
            &path.to_string_lossy(),
            "--detach",
            "--dag-graph",
            &world.shipped_dag_graph(),
        ]);
        launch.env(crate::harness::STARTUP_TIMEOUT_ENV, unusable);
        let started = world.run_on(launch, "start --detach");
        started.exited(REFUSED);
        assert!(
            !started.stderr.contains("neither started nor exited"),
            "`{unusable}` was taken as the bound, so the launch gave up before \
             the graph answered:\n{}",
            started.stderr
        );
        started.err_has("a member that does not exist");
    }
}

/// A graph that says nothing and does not exit is not a driver either.
///
/// The third answer to the startup handshake, and the only one that is not an
/// answer at all. A launch cannot wait on it forever, so the wait is bounded —
/// and reaching that bound fails the launch rather than passing it, which is the
/// difference between this and the fixed window it replaced. The process is
/// ended with it: nothing would ever collect one the launcher has just disowned.
///
/// Both launch forms wait on different things — a file one launcher polls, a
/// pipe the other reads — so a silence that only one of them gave up on would
/// hang the other for as long as the graph did.
#[test]
fn a_graph_that_neither_starts_nor_exits_fails_the_launch_rather_than_outlasting_it() {
    let world = World::new("driver-silent-graph");
    world.script("run.hang", "hold");
    let path = world.plan("silent", &plan_of("silent", vec![agent("build", &[])]));

    // The second launch mints a run of its own beside the first.
    for (form, run) in [("--detach", "silent"), ("--attach", "silent-2")] {
        let mut launch = world.cmd(&[
            "start",
            &path.to_string_lossy(),
            form,
            "--dag-graph",
            &world.shipped_dag_graph(),
        ]);
        launch.env(crate::harness::STARTUP_TIMEOUT_ENV, "1");
        let began = std::time::Instant::now();
        let started = world.run_on(launch, form);
        let took = began.elapsed();

        started.exited(REFUSED);
        started.err_has("neither started nor exited");
        // And it gave up on *this* bound rather than on the default. The suite
        // keeps its own copy of the variable's name, so this is what stands
        // between that copy and a rename: an inert override leaves the launch
        // waiting out a backstop many times longer than the one it asked for.
        assert!(
            took < crate::harness::OVERRIDE_TOOK_EFFECT,
            "`start {form}` was given a 1s backstop and took {took:?}, so it \
             waited out the default instead — is `{}` still the name the binary \
             reads?",
            crate::harness::STARTUP_TIMEOUT_ENV
        );
        assert!(
            !started.stdout.contains("\"pid\""),
            "`start {form}` never got an answer and still printed a pid:\n{}",
            started.stdout
        );
        // The launch refused, so nothing recorded a driver for the run at all:
        // the ledger is what an `adopt` is offered from.
        world.run(&["status", run]).out_has("DRIVER DEAD");
    }
}

/// A graph that finished before it announced anything still launched.
///
/// The other end of the handshake: the answer came as an exit rather than as an
/// envelope, and it was a *clean* one. The graph ran whatever it was given and
/// stopped, and the run settles either way — the observer it launched never
/// drove it. Reporting that exit as a refusal would fail the launch over a
/// verdict the observer was never asked for.
#[test]
fn a_graph_that_finished_before_announcing_anything_is_a_launch_that_worked() {
    let world = World::new("driver-quiet-exit");
    world.script("run.exit-quietly", "hold");
    let path = world.plan("quiet", &plan_of("quiet", vec![agent("build", &[])]));

    // Detached: the launch reports the run it started, because starting it is
    // all it promised — and the observer having stopped watching costs the run
    // nothing, because the observer never drove it.
    let started = world.run(&[
        "start",
        &path.to_string_lossy(),
        "--detach",
        "--dag-graph",
        &world.shipped_dag_graph(),
    ]);
    started.exited(0).out_has("\"run_id\"");
    world.until("the run to settle", |world| {
        world.run_file("quiet", "result.json").is_file()
    });
    assert_eq!(world.run_json("quiet", "result.json")["state"], "complete");

    // Attached: the launch stays, drives the run itself, and settles it — the
    // observer's clean exit is reported and changes nothing.
    let attached = world.run(&[
        "start",
        &path.to_string_lossy(),
        "--attach",
        "--dag-graph",
        &world.shipped_dag_graph(),
    ]);
    attached.exited(0).out_has("\"settlement\":\"complete\"");
    attached.err_has("has stopped watching");
}

/// The `(pid, parent pid)` pairs this host reports.
///
/// The test's own oracle, deliberately not the crate's: asking the teardown to
/// describe the tree it reached would be asking the answer of the thing under
/// test. Strict where the crate's reader degrades, because an oracle that
/// silently returned a short table would report a survivor as gone.
#[cfg(unix)]
fn process_table() -> Vec<(u32, u32)> {
    let listed = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
        .expect("this host lists its processes");
    assert!(
        listed.status.success(),
        "`ps` refused to list this host's processes: {}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let text = String::from_utf8(listed.stdout).expect("`ps` wrote a listing this host can decode");
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut columns = line.split_whitespace();
            let mut id = |what: &str| {
                columns
                    .next()
                    .unwrap_or_else(|| panic!("`ps` wrote a row with no {what}: {line:?}"))
                    .parse::<u32>()
                    .unwrap_or_else(|_| panic!("`ps` wrote an unreadable {what}: {line:?}"))
            };
            let pair = (id("pid"), id("parent pid"));
            assert!(
                columns.next().is_none(),
                "`ps` wrote a row with more than the two ids it was asked for: {line:?}"
            );
            pair
        })
        .collect()
}

#[cfg(unix)]
fn descendants(pid: u32) -> Vec<u32> {
    let table = process_table();
    let mut found: Vec<u32> = Vec::new();
    let mut frontier = vec![pid];
    while let Some(parent) = frontier.pop() {
        for (child, _) in table.iter().filter(|(_, ppid)| *ppid == parent) {
            if *child != pid && !found.contains(child) {
                found.push(*child);
                frontier.push(*child);
            }
        }
    }
    found
}

#[cfg(unix)]
fn still_listed(pid: u32) -> bool {
    process_table().iter().any(|(listed, _)| *listed == pid)
}

/// When this host says a process started, asked the way the crate asks it.
///
/// The test's own oracle again, and the environment matters: `lstart` is a
/// rendering, so a reading taken in another zone or locale is a different string
/// for the same process, and a journey comparing one against a stamp the crate
/// recorded would be comparing two renderings rather than two processes.
#[cfg(unix)]
fn started_at_of(pid: u32) -> String {
    let listed = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .output()
        .expect("this host says when a process started");
    assert!(
        listed.status.success(),
        "`ps` refused to describe pid {pid}"
    );
    String::from_utf8(listed.stdout)
        .expect("`ps` wrote a start this host can decode")
        .trim()
        .to_string()
}

/// A live process this run never started, standing in for whatever the host gave
/// a reissued pid to — and one this host describes differently from `stamp`.
///
/// `lstart` is reported to the **second**, so a process started inside the same
/// second as the driver carries the driver's own stamp and would be a pid the
/// record still proves rather than a stranger. Retried until the host's clock has
/// left that second behind, so the stand-in is a stranger by construction rather
/// than by luck.
#[cfg(unix)]
fn stranger_started_after(stamp: &str) -> std::process::Child {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let mut child = std::process::Command::new("sleep")
            .arg("300")
            .spawn()
            .expect("this host starts a process of its own");
        if started_at_of(child.id()) != stamp {
            return child;
        }
        child.kill().expect("this test ends its own process");
        child.wait().expect("it is reaped");
        assert!(
            std::time::Instant::now() < deadline,
            "this host kept starting processes inside the second {stamp:?} names"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Ending a run ends everything it started, and nothing beside it.
///
/// Both halves, because a teardown can be wrong in either direction: the
/// expensive process is levels below the pid the ledger holds, and the run
/// beside this one is nobody's descendant.
///
/// Unix-only; the Windows arm hands the same boundary to `taskkill /T`, and
/// `the_owner_stops_its_own_run_without_force` holds the ledger half everywhere.
#[cfg(unix)]
#[test]
fn stopping_a_run_ends_its_whole_dispatch_tree_and_leaves_the_run_beside_it_alone() {
    let world = World::new("driver-stop-tree");
    world.script("build.wait", "hold");
    let (run, driver) = start_detached_announcing(&world, "treed", vec![agent("build", &[])]);
    let (beside, untouched) =
        start_detached_announcing(&world, "untouched", vec![agent("build", &[])]);
    for run in [&run, &beside] {
        world.until("a node to be in flight", |world| {
            !world.events_of(run, "node-dispatched").is_empty()
        });
    }

    // Read before the stop, and only once the dispatch has actually started: a
    // tree of one process is a journey that would pass without the fix, because
    // the expensive process is a level below the pid the ledger holds.
    world.until("the dispatch to be a process below the driver", |_| {
        !descendants(driver).is_empty()
    });
    let tree: Vec<u32> = std::iter::once(driver).chain(descendants(driver)).collect();
    assert!(
        !tree.contains(&untouched),
        "the run beside it is inside the tree, so this journey proves nothing: {tree:?}"
    );

    world.run(&["stop", &run]).exited(0);

    world.until("every process the run started to end", |_| {
        tree.iter().all(|pid| !still_listed(*pid))
    });
    assert!(
        still_listed(untouched),
        "stopping one run ended the driver of another: pid {untouched}"
    );

    world.run(&["stop", &beside]).exited(0);
    world.release("build.go");
}

/// A **forced** stop reaches the tree too.
///
/// `--force` is the other way an operator ends a run: it overrides ownership, so
/// it is the one a person reaches for when the session that launched the run is
/// gone — which is exactly when nobody is left watching what the run started.
/// The ownership half is held by
/// `stop_refuses_another_sessions_run_and_force_names_the_owner`; this is the
/// half that says the override ends the same tree the owner's own stop does,
/// rather than only the pid the ledger holds.
#[cfg(unix)]
#[test]
fn a_forced_stop_ends_the_whole_dispatch_tree_of_another_sessions_run() {
    let world = World::new("driver-stop-tree-forced");
    world.script("build.wait", "hold");
    let (run, driver) = start_detached_announcing(&world, "forced", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });
    world.until("the dispatch to be a process below the driver", |_| {
        !descendants(driver).is_empty()
    });
    let tree: Vec<u32> = std::iter::once(driver).chain(descendants(driver)).collect();

    let stranger = world.as_session("session-forcing");
    stranger
        .run(&["stop", &run, "--force"])
        .exited(0)
        .err_has("belongs to");

    world.until("every process the forced stop was aimed at to end", |_| {
        tree.iter().all(|pid| !still_listed(*pid))
    });
    world.release("build.go");
}

/// A stop that cannot see what it must end refuses, changes nothing, and works
/// on the retry.
///
/// All three ways this host can fail to say what the run is running: a `ps` that
/// cannot be spawned, one that runs and exits non-zero, and one that answers with
/// a listing holding a row nobody can read. The third is the subtle one — the
/// rows around it look fine, and reading them as the whole tree would signal
/// some of it and call that done.
///
/// The refusal is the whole point. Reporting a clean stop here would be the
/// original defect wearing a success code — the expensive processes left running
/// and writing, with the run's own record saying they were ended. And nothing is
/// signalled, so the tree is intact and the same ask works once the host answers:
/// killing the driver alone would have orphaned everything under it permanently,
/// since descent is the only handle a later stop has on them.
#[cfg(unix)]
#[test]
fn a_stop_that_cannot_read_the_process_table_refuses_and_leaves_the_run_retryable() {
    for (fault, path) in [
        ("no ps to spawn", World::empty_path as fn(&World) -> PathBuf),
        ("a ps that fails", World::path_whose_ps_fails),
        (
            "a ps whose listing has a row nobody can read",
            World::path_whose_ps_garbles_a_row,
        ),
    ] {
        let world = World::new(&format!("driver-stop-blind-{}", fault.replace(' ', "-")));
        world.script("build.wait", "hold");
        let (run, driver) = start_detached_announcing(&world, "blind", vec![agent("build", &[])]);
        world.until("a node to be in flight", |world| {
            !world.events_of(&run, "node-dispatched").is_empty()
        });
        world.until("the dispatch to be a process below the driver", |_| {
            !descendants(driver).is_empty()
        });
        let tree: Vec<u32> = std::iter::once(driver).chain(descendants(driver)).collect();

        let mut command = world.cmd(&["stop", &run]);
        command.env("PATH", path(&world));
        let refused = world.run_on(command, &format!("stop with {fault}"));

        // Not a success, and not a claim to have stopped anything.
        refused.exited(REFUSED).err_has("was not stopped");
        assert!(
            !refused.stdout.contains("\"stopped\":true"),
            "a stop that reached nothing still announced a clean stop:\n{}",
            refused.stdout
        );

        // And it really did leave the run alone — every process, driver
        // included. This is what makes the refusal honest rather than merely
        // pessimistic, and what makes the retry below possible at all.
        for pid in &tree {
            assert!(
                still_listed(*pid),
                "a refused stop signalled pid {pid} anyway, orphaning what it could not find"
            );
        }

        // The run's own record says a stop happened and says it was not a clean
        // one, so no reader takes this for a run whose work was ended.
        let stopped = world.events_of(&run, "run-stopped");
        assert_eq!(stopped.len(), 1, "the attempt went unrecorded");
        assert_eq!(
            stopped[0]["payload"]["teardown"],
            json!("not-attempted"),
            "a stop that established nothing was recorded as a clean one: {}",
            stopped[0]
        );

        // Neither view claims the worker was ended, because it is still running.
        let status = world.run(&["status", &run]);
        status.exited(0).out_has("worker may still be running");
        assert!(
            !status
                .stdout
                .contains("worker ended when the run was stopped"),
            "a view reported a worker as ended while it was still running:\n{}",
            status.stdout
        );
        world
            .run(&["results", &run])
            .exited(0)
            .out_has("worker may still be running");

        // The recovery: the same ask, on a host that answers, ends the whole
        // tree and reports the clean stop it actually made.
        world
            .run(&["stop", &run])
            .exited(0)
            .out_has("\"stopped\":true")
            .out_has("\"teardown\":\"signalled\"");
        world.until("every process the run started to end", |_| {
            tree.iter().all(|pid| !still_listed(*pid))
        });
        world
            .run(&["status", &run])
            .exited(0)
            .out_has("worker ended when the run was stopped");
        world.release("build.go");
    }
}

/// A host that says more than it was asked is a host that has not answered, and
/// a stop says so rather than reading it as somebody else's pid.
///
/// The fourth way this host can fail a teardown, and the one that hides. A start
/// token is one process and one line, so an answer carrying anything beside it is
/// a different string from the one the driver recorded for that very process — and
/// read as a token, a *different* string is not "this host is unwell", it is "the
/// pid was handed on". That reading is silent by design: a pid the host reissued
/// is nobody's to signal and nothing to report, so the stop would walk past the
/// run's own driver, find nothing else it could prove, and answer
/// `{"stopped":true,"teardown":"nothing-to-stop"}` over a tree that is still
/// running.
///
/// So the answer is refused instead: nothing is signalled, the run is left exactly
/// as it was, and the same ask works once the host answers what it was asked. The
/// listing this stand-in gives is the real one throughout, which is what keeps the
/// fault to the one question under test.
#[cfg(unix)]
#[test]
fn a_stop_whose_host_says_more_than_it_was_asked_about_a_pid_refuses_and_signals_nothing() {
    let world = World::new("driver-stop-talkative-ps");
    world.script("build.wait", "hold");
    let (run, driver) = start_detached_announcing(&world, "talkative", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });
    world.until("the dispatch to be a process below the driver", |_| {
        !descendants(driver).is_empty()
    });
    let tree: Vec<u32> = std::iter::once(driver).chain(descendants(driver)).collect();

    let mut command = world.cmd(&["stop", &run]);
    command.env("PATH", world.path_whose_ps_says_more_than_it_was_asked());
    let refused = world.run_on(command, "stop with a ps that says more than it was asked");
    refused
        .exited(REFUSED)
        .err_has("was not stopped")
        .err_has("will not say when it started");
    assert!(
        !refused.stdout.contains("\"stopped\":true"),
        "a stop that could not place a single pid still announced a clean stop:\n{}",
        refused.stdout
    );
    for pid in &tree {
        assert!(
            still_listed(*pid),
            "a refused stop signalled pid {pid} anyway, orphaning what it could not find"
        );
    }
    let stopped = world.events_of(&run, "run-stopped");
    assert_eq!(stopped.len(), 1, "the attempt went unrecorded");
    assert_eq!(
        stopped[0]["payload"]["teardown"],
        json!("not-attempted"),
        "a stop that established nothing was recorded as a clean one: {}",
        stopped[0]
    );

    // And the same ask, on a host that answers what it was asked, ends the tree.
    world
        .run(&["stop", &run])
        .exited(0)
        .out_has("\"stopped\":true")
        .out_has("\"teardown\":\"signalled\"");
    world.until("every process the run started to end", |_| {
        tree.iter().all(|pid| !still_listed(*pid))
    });
    world.release("build.go");
}

/// A stop aimed at another host's driver says it reached nothing.
///
/// A pid means nothing across machines, so this host will not signal one it did
/// not start. The ledger record is still written — that is what stops a run
/// across hosts — but the teardown says `elsewhere`, and no view claims the
/// worker was ended, because nothing here established that.
///
/// The same run then stops properly from the host its driver is recorded on,
/// which is both the contrast and this journey's cleanup.
#[cfg(unix)]
#[test]
fn a_stop_aimed_at_another_hosts_driver_reports_that_it_reached_nothing() {
    const ELSEWHERE: &str = "a-host-this-is-not";
    let world = World::new("driver-stop-elsewhere");
    world.script("build.wait", "hold");

    let path = world.plan("afar", &plan_of("afar", vec![agent("build", &[])]));
    let mut launch = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
    launch.env("HOSTNAME", ELSEWHERE);
    let started = world.run_on(launch, "start recorded on another host");
    started.exited(0);
    let driver = u32::try_from(
        serde_json::from_str::<serde_json::Value>(started.stdout.trim())
            .expect("a detached launch announces itself")["pid"]
            .as_u64()
            .expect("a driver pid"),
    )
    .expect("a pid");
    world.until("a node to be in flight", |world| {
        !world.events_of("afar", "node-dispatched").is_empty()
    });

    // From this host, which is not the one the driver is recorded on.
    world
        .run(&["stop", "afar"])
        .exited(0)
        .out_has("\"teardown\":\"elsewhere\"");
    assert!(
        still_listed(driver),
        "a stop signalled a pid recorded on another host, where it means nothing"
    );
    let status = world.run(&["status", "afar"]);
    status.exited(0).out_has("worker may still be running");
    assert!(
        !status
            .stdout
            .contains("worker ended when the run was stopped"),
        "a view claimed a worker on another host was ended:\n{}",
        status.stdout
    );

    // And from the host it *is* recorded on, which ends it for real.
    let mut here = world.cmd(&["stop", "afar"]);
    here.env("HOSTNAME", ELSEWHERE);
    world
        .run_on(here, "stop from the recorded host")
        .exited(0)
        .out_has("\"teardown\":\"signalled\"");
    world.until("the driver to end", |_| !still_listed(driver));
    world.release("build.go");
}

/// A stop finds the run's tree through the ownership lock, not through a
/// recorded pid that has died.
///
/// The launch record names the driver a run was launched or last adopted with,
/// which is a claim about the past: a driver that died leaves that pid behind
/// it, and a stop aimed there alone signals nothing, finds nothing, and reports
/// a clean teardown over a dispatch tree that is still running and still
/// spending. The **lock** is the claim made now, and its start token is what
/// makes acting on it safe — so the stop reaches the tree the run is actually
/// made of.
///
/// The record is edited rather than arranged, because no verb produces this
/// state on demand: a driver that has died and been taken over rewrites the
/// record as it takes the lock, and the window where the two disagree is one a
/// journey cannot stand in. What is edited is the one fact under test — the pid
/// the record names — and everything else about the run is real: a live driver
/// holding the lock it took, with a dispatch genuinely in flight below it.
// llmlint: ignore-block[tests_mirror_real_usage] the one value set by hand is the pid the
// launch record names, and no product surface sets it: the verbs that write that field —
// `start`, `drive-run`, `adopt` — all write their *own* pid as they take the lock, so a run
// whose record has fallen behind its lock is a state a user reaches by having a driver die,
// not by typing anything. The rest of the journey is the real binary end to end, and the
// assertion is about processes on this host rather than about the file that was edited.
#[cfg(unix)]
#[test]
fn stopping_a_run_ends_the_tree_its_lock_names_when_the_record_names_a_dead_driver() {
    let world = World::new("driver-stop-stale-record");
    world.script("build.wait", "hold");
    let (run, driver) = start_detached_announcing(&world, "stale", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });
    world.until("the dispatch to be a process below the driver", |_| {
        !descendants(driver).is_empty()
    });
    let tree: Vec<u32> = std::iter::once(driver).chain(descendants(driver)).collect();

    let record = world.run_file(&run, "launch.json");
    let mut named = world.run_json(&run, "launch.json");
    let dead = reaped_pid();
    named["pid"] = json!(dead);
    std::fs::write(&record, named.to_string()).expect("the launch record is rewritten");
    assert!(
        !still_listed(dead),
        "the record now names pid {dead}, which is live, so this journey proves nothing"
    );

    world
        .run(&["stop", &run])
        .exited(0)
        .out_has("\"stopped\":true")
        .out_has("\"teardown\":\"signalled\"");
    world.until("every process the run started to end", |_| {
        tree.iter().all(|pid| !still_listed(*pid))
    });
    world.release("build.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A stop never signals a pid the host has since given to another process.
///
/// The other thing a stale launch record can be, and the dangerous one. The
/// record above named a pid nothing answers to, which costs a signal and nothing
/// else; here the host has reissued that pid, so the record names a **live**
/// process that this run never started — somebody else's editor, somebody else's
/// build — and a teardown taking the record at its word ends it. That is not a
/// hypothetical for a record that outlives every driver it names: it is the
/// oldest claim a run holds, a driver that dies leaves its pid sitting in it, and
/// nothing rewrites it until an adoption does.
///
/// So the stranger stands in for whatever the host handed that pid to. It is
/// this test's own process, deliberately started outside the run's tree, and the
/// journey holds both halves at once: it is still running afterwards, and the
/// run's real tree — found through the claims that *can* prove themselves — is
/// gone.
///
/// The record is edited for the reason the journey above edits one: no verb
/// produces this state on demand, and what is edited is the one fact under test.
/// The stamp beside the pid is left exactly as the driver wrote it, which is what
/// makes this a reissued pid rather than a rewritten record.
// llmlint: ignore-block[tests_mirror_real_usage] the one value set by hand is the pid the
// launch record names, and no product surface sets it: the verbs that write that field —
// `start`, `drive-run`, `adopt` — write their own pid and their own stamp together, so a
// record whose pid the host has reissued is a state a user reaches by having a driver die and
// the host reuse its pid, not by typing anything. The rest of the journey is the real binary
// end to end, and the assertions are about processes on this host.
#[cfg(unix)]
#[test]
fn a_stop_never_signals_a_pid_the_host_has_given_to_another_process() {
    let world = World::new("driver-stop-reissued-pid");
    world.script("build.wait", "hold");
    let (run, driver) = start_detached_announcing(&world, "reissued", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });
    world.until("the dispatch to be a process below the driver", |_| {
        !descendants(driver).is_empty()
    });
    let tree: Vec<u32> = std::iter::once(driver).chain(descendants(driver)).collect();

    // The stranger the host handed the pid to: a real process, started by this
    // test rather than by the run, so it is nobody's descendant in the tree a
    // teardown walks — and one this host describes differently from the stamp
    // the driver recorded, which is what makes it a stranger and not the driver.
    let record = world.run_file(&run, "launch.json");
    let mut named = world.run_json(&run, "launch.json");
    let recorded = named["started"]
        .as_str()
        .expect("a driver records the stamp that proves its pid")
        .to_string();
    let mut stranger = stranger_started_after(&recorded);
    let taken = stranger.id();
    assert!(
        !tree.contains(&taken),
        "the stranger {taken} is part of the run's own tree, so this journey proves nothing"
    );

    named["pid"] = json!(taken);
    std::fs::write(&record, named.to_string()).expect("the launch record is rewritten");

    let stopped = world.run(&["stop", &run]);
    // The half this journey exists for, asked of the process itself rather than
    // of a listing: a signalled process is still *listed* while nobody has
    // reaped it, and this test is what would reap this one.
    assert!(
        stranger
            .try_wait()
            .expect("this host answers about this test's own process")
            .is_none(),
        "a stop ended pid {taken}, which the host had given to a process this run never started"
    );
    // And the run itself was stopped, through the claims that can prove
    // themselves — the stale record cost it nothing but a sentence on stderr.
    stopped
        .exited(0)
        .out_has("\"stopped\":true")
        .out_has("\"teardown\":\"signalled\"")
        .err_has("since given to another process");
    world.until("every process the run started to end", |_| {
        tree.iter().all(|pid| !still_listed(*pid))
    });
    stranger.kill().expect("this test ends its own process");
    stranger.wait().expect("it is reaped");
    world.release("build.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A stop reaches a dispatch whose driver is gone.
///
/// The launch record and the ownership lock name a driver, and a dispatch
/// outlives the driver that started it — so before the registry this state
/// answered `{"stopped":true,"teardown":"nothing-to-stop"}` with an exit 0 while
/// the dispatch went on running. The driver here is ended the way a host ends one
/// it has run out of memory for, and what it started is left behind.
#[cfg(unix)]
#[test]
fn stopping_a_run_reaches_a_dispatch_whose_driver_and_lock_holder_are_dead() {
    let world = World::new("driver-stop-orphaned-dispatch");
    world.script("build.wait", "hold");
    let (run, driver) = start_detached_announcing(&world, "orphaned", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });
    world.until("the dispatch to be a process below the driver", |_| {
        !descendants(driver).is_empty()
    });
    let dispatch = descendants(driver);

    // The driver goes the way a host ends a process it has run out of memory
    // for, and what it started does not go with it.
    end_process(driver);
    assert!(
        dispatch.iter().all(|pid| still_listed(*pid)),
        "the driver took its dispatch {dispatch:?} with it, so this journey proves nothing"
    );
    // Both of the run's records named that driver, and the views say so in the
    // one way an operator reads it: nothing is driving the run, and no row here
    // claims a dispatch — which is the whole of what those two records can find.
    world.until("the run to read as undriven", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });
    world
        .run(&["host"])
        .exited(0)
        .out_has("no live dispatches")
        .out_has(&run);

    let stopped = world.run(&["stop", &run]);
    stopped.exited(0).out_has("\"stopped\":true");
    assert_eq!(
        stopped.json()["teardown"],
        json!("signalled"),
        "a stop that reached a live dispatch did not report reaching one:\n{}",
        stopped.stdout
    );
    let surviving: Vec<u32> = dispatch
        .iter()
        .copied()
        .filter(|pid| still_listed(*pid))
        .collect();
    assert!(
        surviving.is_empty(),
        "the stop left {surviving:?} of the dispatch {dispatch:?} running"
    );
    // Reported as reached because it *was* reached: the teardown watched it go
    // rather than reporting on the signal it sent.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("worker ended when the run was stopped");
    world.release("build.go");
}

/// A stop that found nothing running says that, rather than claiming it reached
/// a tree.
///
/// The two are opposite answers to what an operator is asking, and they were one
/// value: `ESRCH` — no such process — counted as a process reached, so `stop`
/// answered `signalled` for a run whose work had ended hours earlier and for one
/// it had genuinely just ended. The run is stopped either way, which is why this
/// is a success and not a refusal: the ledger record is what stops a run.
#[test]
fn stopping_a_run_whose_work_is_over_says_there_was_nothing_to_stop() {
    let world = World::new("driver-stop-nothing");
    let run = start_detached(&world, "finished", vec![agent("build", &[])]);
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });

    let stopped = world.run(&["stop", &run]);
    stopped.exited(0).out_has("\"stopped\":true");
    assert_eq!(
        stopped.json()["teardown"],
        json!("nothing-to-stop"),
        "a stop that signalled nothing reported what a stop that ended a run reports:\n{}",
        stopped.stdout
    );
    // And the run's own record says the same, so a later reader is not left to
    // take this for a run whose workers were ended by it.
    let recorded = world.events_of(&run, "run-stopped");
    assert_eq!(recorded.len(), 1, "the stop went unrecorded");
    assert_eq!(recorded[0]["payload"]["teardown"], json!("nothing-to-stop"));
}

/// And a stop that found nothing running never reports the run's workers as
/// ended by it.
///
/// The node here is recorded in flight and its worker is genuinely gone — the
/// driver died and took its dispatch with it, which is the state a run is left
/// in when a host reboots under it. Nothing was signalled, so "worker ended when
/// the run was stopped" would be a claim about a signal nobody sent; the view
/// says what this stop actually established about that worker, which is nothing.
#[cfg(unix)]
#[test]
fn a_stop_that_found_nothing_running_never_reports_its_workers_as_ended() {
    let world = World::new("driver-stop-nothing-inflight");
    world.script("build.wait", "hold");
    let (run, driver) = start_detached_announcing(&world, "vanished", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });
    world.until("the dispatch to be a process below the driver", |_| {
        !descendants(driver).is_empty()
    });
    for pid in std::iter::once(driver).chain(descendants(driver)) {
        end_process(pid);
    }

    let stopped = world.run(&["stop", &run]);
    stopped.exited(0).out_has("\"stopped\":true");
    assert_eq!(
        stopped.json()["teardown"],
        json!("nothing-to-stop"),
        "a stop with nothing left to aim at reported reaching a tree:\n{}",
        stopped.stdout
    );

    let status = world.run(&["status", &run]);
    status.exited(0).out_has("worker may still be running");
    assert!(
        !status
            .stdout
            .contains("worker ended when the run was stopped"),
        "a view reported a worker as ended by a stop that signalled nothing:\n{}",
        status.stdout
    );
    world.release("build.go");
}

/// A dispatch this run cannot record does not run.
///
/// The registry is a trust boundary rather than bookkeeping around one: an entry
/// that was not written is a process no view will show and no `stop` will reach,
/// on a run whose own records say it has nothing running. So a dispatch that
/// cannot be registered is taken back down and the node settles as an
/// infrastructure failure naming what could not be written — the same ending a
/// dispatch that could not start at all takes, and the same one the loop retries.
///
/// Nothing races. One node is held open, which keeps the driver in its loop with
/// nothing to do, and the node under test waits on a person — so the registry is
/// broken while the run is idle and the attestation is what releases the dispatch
/// into it. The held dispatch beside it is the control: it was registered before
/// the fault and it is still running afterwards, so what disappears is the
/// dispatch that was refused and nothing else.
#[cfg(unix)]
// llmlint: ignore-block[tests_mirror_real_usage] a run whose registry cannot be written is a
// filesystem condition, not something a user types: the directory is created with the run and
// the only writers under it are its own dispatches. What it stands for — a full disk, a
// revoked permission, a volume that went read-only under a live run — is reachable only by
// arranging the filesystem, so the fixture removes exactly that one thing and everything else
// in the journey is the real binary end to end.
#[test]
fn a_dispatch_this_run_cannot_record_is_refused_and_does_not_run() {
    let world = World::new("driver-dispatch-unrecordable");
    world.script("held.wait", "hold");
    let (run, driver) = start_detached_announcing(
        &world,
        "unrecordable",
        vec![
            agent("held", &[]),
            human("approve", &[]),
            agent("build", &["approve"]),
        ],
    );
    world.until(
        "the run to ask the person and hold a dispatch open",
        |world| {
            world
                .events_of(&run, "node-settled")
                .iter()
                .any(|event| event["labels"]["node"] == json!("approve"))
                && !world.events_of(&run, "node-dispatched").is_empty()
        },
    );
    world.until("the held dispatch to be a process below the driver", |_| {
        !descendants(driver).is_empty()
    });
    let held = descendants(driver);

    // The registry cannot be written: a file where its directory has to be, which
    // no host will create a directory under.
    let registry = world.run_file(&run, "dispatches");
    std::fs::remove_dir_all(&registry).expect("the registry is taken away");
    std::fs::write(&registry, "not a directory").expect("something in the way");

    world.run(&["attest", &run, "approve"]).exited(0);

    // The dispatch is made, refused, and taken back down — and the node says so
    // where an operator reads it.
    world.until("the node to settle", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == json!("build"))
    });
    let settled = world.events_of(&run, "node-settled");
    let build = settled
        .iter()
        .find(|event| event["labels"]["node"] == json!("build"))
        .expect("the node settles");
    assert_eq!(
        build["payload"]["outcome"],
        json!("infrastructure-failure"),
        "a dispatch nothing could record settled as something else: {build}"
    );
    let said = build["payload"]["detail"].as_str().unwrap_or_default();
    assert!(
        said.contains("dispatches"),
        "the settlement does not say what could not be recorded: {build}"
    );
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("build")
        .out_has("failed");

    // And the work it started really is gone. The refused dispatch was held open
    // by its double like the control beside it, so a process still under the
    // driver would be one this run cannot find and nobody asked for.
    world.until(
        "the refused dispatch to be gone from under the driver",
        |_| descendants(driver) == held,
    );
    world.release("held.go");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A stop whose run holds a **lock** this build cannot read says so, and still
/// ends the tree it can find.
///
/// An unreadable lock names nobody, so it adds no root — but it is not the same
/// as a run nothing holds, and a stop that swallowed the difference would leave
/// an operator reading a narrower teardown than they asked for with nothing
/// saying why. It is not refused either: the lock can only *widen* what a
/// teardown reaches, so losing it costs reach, and refusing over it would leave a
/// live run running. The registry below is the opposite case, and the journey
/// after this one is where the difference is stated.
#[cfg(unix)]
// llmlint: ignore-block[tests_mirror_real_usage] a held lock this build cannot read is not a
// state any command produces: the only writer of that file is a live driver taking the run,
// and it writes a record of its own schema. What it stands for — a lock written by a build
// this one does not understand — is reachable only across versions, so the fixture writes the
// one fact under test and everything else in the journey is the real binary end to end.
#[test]
fn stopping_a_run_whose_lock_cannot_be_read_says_so_and_still_ends_what_it_finds() {
    let world = World::new("driver-stop-unreadable-lock");
    world.script("build.wait", "hold");
    let (run, driver) = start_detached_announcing(&world, "unreadable", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });
    world.until("the dispatch to be a process below the driver", |_| {
        !descendants(driver).is_empty()
    });
    let tree: Vec<u32> = std::iter::once(driver).chain(descendants(driver)).collect();

    std::fs::write(
        world.run_file(&run, "owner.lock"),
        "not a lock this build knows",
    )
    .expect("the lock is rewritten");

    let stopped = world.run(&["stop", &run]);
    stopped
        .exited(0)
        .out_has("\"teardown\":\"signalled\"")
        .err_has("ownership lock")
        .err_has("cannot be read");
    world.until("every process the run started to end", |_| {
        tree.iter().all(|pid| !still_listed(*pid))
    });
    world.release("build.go");
}

// llmlint: ignore-end[tests_mirror_real_usage]

/// A stop whose run holds a **registry** this build cannot read refuses, signals
/// nothing, and works on the retry.
///
/// The other half of the pair, and the opposite answer. Every other record a stop
/// consults can only add a root, so one it cannot read costs reach. The registry
/// is what says whether the run has work running at all — so a reader that met an
/// entry it could not parse and carried on would report `nothing-to-stop` about a
/// run it never managed to ask, which is the false completion this verb exists to
/// refuse, one layer further in.
///
/// Both shapes an unreadable entry takes: one that is not a record at all, and
/// one that is a record carrying a field this build does not know — a newer
/// writer's, which a reader that shrugged would silently act on half of. Nothing
/// is signalled for either, so the run is intact and the same ask works once the
/// entry is gone.
#[cfg(unix)]
// llmlint: ignore-block[tests_mirror_real_usage] the only writer of a registry entry is a
// live dispatch recording the process it is in, and it writes a record of its own schema, so
// neither shape below is a state a command produces. What they stand for — an entry from a
// build this one does not understand, and one a crash left half-written — is reachable only
// across versions or across a power cut, so the fixture writes the one fact under test and
// everything else in the journey is the real binary end to end.
#[test]
fn stopping_a_run_whose_registry_cannot_be_read_refuses_and_leaves_the_run_retryable() {
    for (shape, entry) in [
        (
            "a record that is not one",
            Some("not an entry this build knows".to_string()),
        ),
        (
            "a record from a writer this build does not know",
            Some(
                json!({
                    "node": "build",
                    "pid": 4_242,
                    "host": "this-host",
                    "dispatched_at": "2026-08-17T00:00:00.000Z",
                    "started": "a start this host once reported",
                    "reaped_by": "a build that came later",
                })
                .to_string(),
            ),
        ),
        (
            "a record whose stamp proves nothing",
            Some(
                json!({
                    "node": "build",
                    "pid": 4_242,
                    "host": "this-host",
                    "dispatched_at": "2026-08-17T00:00:00.000Z",
                    "started": "",
                })
                .to_string(),
            ),
        ),
        // And the registry gone altogether. Every run this build creates has
        // one, so this is not "a run with nothing running" — it is a run whose
        // record of what it is running has been taken away.
        ("no registry at all", None),
    ] {
        let world = World::new(&format!(
            "driver-stop-unreadable-registry-{}",
            shape.replace(' ', "-")
        ));
        world.script("build.wait", "hold");
        let (run, driver) =
            start_detached_announcing(&world, "unreadable", vec![agent("build", &[])]);
        world.until("a node to be in flight", |world| {
            !world.events_of(&run, "node-dispatched").is_empty()
        });
        world.until("the dispatch to be a process below the driver", |_| {
            !descendants(driver).is_empty()
        });
        let tree: Vec<u32> = std::iter::once(driver).chain(descendants(driver)).collect();

        let planted = world.run_file(&run, "dispatches/planted.json");
        match &entry {
            Some(entry) => std::fs::write(&planted, entry).expect("an entry nobody can read"),
            None => std::fs::remove_dir_all(world.run_file(&run, "dispatches"))
                .expect("the registry is taken away"),
        }

        let refused = world.run(&["stop", &run]);
        refused
            .exited(REFUSED)
            .err_has("was not stopped")
            .err_has("cannot establish what it is running");
        assert!(
            !refused.stdout.contains("\"stopped\":true"),
            "a stop that established nothing announced a clean stop with {shape}:\n{}",
            refused.stdout
        );
        assert!(
            !refused.stdout.contains("nothing-to-stop"),
            "a stop that could not read the registry reported the run as idle:\n{}",
            refused.stdout
        );

        // And it really did leave the run alone, which is what makes the refusal
        // honest rather than merely pessimistic — and the retry below possible.
        for pid in &tree {
            assert!(
                still_listed(*pid),
                "a refused stop signalled pid {pid} anyway, with {shape} in the registry"
            );
        }
        assert!(
            world.events_of(&run, "run-stopped").is_empty(),
            "a stop that refused recorded one anyway:\n{}",
            world.dump()
        );

        // The recovery: with what it could not read put right, the same ask ends
        // the run and says what it reached.
        match &entry {
            Some(_) => std::fs::remove_file(&planted).expect("the entry is removed"),
            None => std::fs::create_dir_all(world.run_file(&run, "dispatches"))
                .expect("the registry is put back"),
        }
        world
            .run(&["stop", &run])
            .exited(0)
            .out_has("\"stopped\":true")
            .out_has("\"teardown\":\"signalled\"");
        world.until("every process the run started to end", |_| {
            tree.iter().all(|pid| !still_listed(*pid))
        });
        world.release("build.go");
    }
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A stop watches the tree it signalled and refuses when it is still there.
///
/// `Signalled` was never a process that had exited — `kill` reports a delivered
/// signal and nothing more — and the type said so, deferring the liveness probe
/// to a caller that never performed it. So a worker that took the ask and stayed
/// was reported as a clean stop, which is how an operator walks away from a run
/// still burning a CPU.
///
/// The worker that stays is the run's own dispatch, scripted to keep working
/// through the polite ask: a real process, started by the driver, inside the
/// tree the teardown walks for itself. The `SIGTERM` it ignores is the one the
/// stop actually sent it, and the forceful ask this journey ends with is the one
/// no process can ignore.
#[cfg(unix)]
#[test]
fn a_stop_whose_tree_takes_the_ask_and_stays_refuses_rather_than_reporting_a_clean_stop() {
    let world = World::new("driver-stop-deaf");
    world.script("build.wait", "hold");
    world.script("build.ignores-the-ask", "yes");
    let (run, driver) = start_detached_announcing(&world, "deaf", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });
    world.until("the dispatch to be a process below the driver", |_| {
        !descendants(driver).is_empty()
    });
    let worker = descendants(driver);

    let refused = world.run(&["stop", &run]);
    refused.exited(REFUSED).err_has("only partly stopped");
    assert!(
        !refused.stdout.contains("\"stopped\":true"),
        "a run with a process still running was announced as a clean stop:\n{}",
        refused.stdout
    );
    let surviving: Vec<u32> = worker
        .iter()
        .copied()
        .filter(|pid| still_listed(*pid))
        .collect();
    assert_eq!(
        surviving, worker,
        "the dispatch {worker:?} ended on the polite ask, so the refusal above proves nothing"
    );

    // The record says which of the answers it was, so no reader takes this for a
    // run whose work was ended.
    let stopped = world.events_of(&run, "run-stopped");
    assert_eq!(stopped.len(), 1);
    assert_eq!(
        stopped[0]["payload"]["teardown"],
        json!("partly-signalled"),
        "a stop that left a process running was recorded as something else: {}",
        stopped[0]
    );
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("worker may still be running");

    // The ask no worker can ignore, which is also this journey's cleanup: the
    // dispatch it left running is the one thing a `World` going out of scope
    // cannot take with it.
    for pid in &surviving {
        end_process(*pid);
    }
    world.release("build.go");
}

/// A stop that reaches part of a tree says so, and does not call it clean.
///
/// The third answer a teardown can give, and the one that is neither of the
/// others: the tree *was* listed, so this is not a run left untouched, and part
/// of it was signalled, so it is not a clean stop either. Something in it could
/// not be signalled and is still running — a process belonging to somebody else,
/// in the case this stands in for — and no retry of `stop` will change that.
///
/// The stand-in adds one child to the real listing under an id no signal can be
/// sent to, so the case is produced without this suite ever signalling a process
/// it does not own.
#[cfg(unix)]
#[test]
fn a_stop_that_reaches_part_of_the_tree_refuses_and_names_what_it_left() {
    let world = World::new("driver-stop-partial");
    world.script("build.wait", "hold");
    let (run, driver) = start_detached_announcing(&world, "partial", vec![agent("build", &[])]);
    world.until("a node to be in flight", |world| {
        !world.events_of(&run, "node-dispatched").is_empty()
    });
    world.until("the dispatch to be a process below the driver", |_| {
        !descendants(driver).is_empty()
    });
    let tree: Vec<u32> = std::iter::once(driver).chain(descendants(driver)).collect();

    let mut command = world.cmd(&["stop", &run]);
    command.env(
        "PATH",
        world.path_whose_ps_invents_an_unreachable_child(driver),
    );
    let refused = world.run_on(command, "stop with an unreachable child in the listing");
    refused.exited(REFUSED).err_has("only partly stopped");
    assert!(
        !refused.stdout.contains("\"stopped\":true"),
        "a partly stopped run was announced as a clean stop:\n{}",
        refused.stdout
    );

    // The record says which of the three it was, so a reader is not left to
    // guess between "untouched" and "ended".
    let stopped = world.events_of(&run, "run-stopped");
    assert_eq!(stopped.len(), 1);
    assert_eq!(
        stopped[0]["payload"]["teardown"],
        json!("partly-signalled"),
        "a partial teardown was recorded as something else: {}",
        stopped[0]
    );
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("worker may still be running");

    // The processes it *could* reach were still reached: this is a report about
    // what was left, not a teardown that gave up.
    world.until("the processes it could reach to end", |_| {
        tree.iter().all(|pid| !still_listed(*pid))
    });
    world.release("build.go");
}

/// A worker of a **detached** run can put a question to its manager, with no
/// observer graph to have named the run for it.
///
/// The whole of what makes that direction open is the run id arriving *at the
/// dispatched process*: the operator's `ask-manager` wrapper reads it out of its
/// own environment, is told none and infers none, and refuses without one — so a
/// worker that carried nothing is a worker that cannot ask. The doubles ask
/// exactly that way, and what each journey below states is that the question
/// reached the channel its manager reads.
///
/// This is the shape that had no run id at all: a detached driver launches its
/// observer as a subprocess, so the pair reached that child and never the
/// driver's own environment — and with no dag-scope graph, the shipped default,
/// nothing exported anything anywhere.
#[test]
fn a_detached_runs_worker_can_ask_its_manager() {
    let world = World::new("driver-detached-run-id");
    world.script(
        "first.asks",
        "first: the origin refuses me. Do I wait or fail?",
    );
    world.script(
        "second.asks",
        "second: first left no branch. Do I open one?",
    );
    let run = start_detached(
        &world,
        "askabledetached",
        vec![agent("first", &[]), agent("second", &["first"])],
    );

    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    // Both dispatches asked, and both questions are on the stream the manager
    // watches. A dispatch that carried no run id refuses its own ask, which fails
    // the node — so a run that settled complete with both questions on it is two
    // workers that each reached this run and no other.
    assert_eq!(
        world.questions_on_the_stream(&run),
        vec![
            "first: the origin refuses me. Do I wait or fail?".to_string(),
            "second: first left no branch. Do I open one?".to_string(),
        ],
        "a worker of a detached run could not ask its manager: {}",
        world.dump()
    );
    assert_eq!(
        world.run_json(&run, "result.json")["state"],
        "complete",
        "the run did not settle: {}",
        world.dump()
    );

    // And the manager reads one: the newest, because a check-in replaces the
    // queued one at the channel rather than queueing behind it.
    assert_eq!(
        world.question_for_the_manager(&run),
        "second: first left no branch. Do I open one?"
    );
}

/// The same of an **attached** launch, which is the shape every other journey
/// here takes.
#[test]
fn an_attached_runs_worker_can_ask_its_manager() {
    let world = World::new("driver-attached-run-id");
    world.script(
        "build.asks",
        "build: the gate is red on main. Do I fix it here?",
    );
    let path = world.plan(
        "askableattached",
        &plan_of("askableattached", vec![agent("build", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0)
        .settled();

    assert_eq!(
        world.question_for_the_manager("askableattached"),
        "build: the gate is red on main. Do I fix it here?"
    );
}

/// And of an **adoption**, the third process a run can be driven in. It reaches
/// one the way a run does: a human gate attested, leaving work to do and nobody
/// driving it.
#[test]
fn an_adopted_runs_worker_can_ask_its_manager() {
    let world = World::new("driver-adopted-run-id");
    world.script(
        "build.asks",
        "build: the approval says nothing about the schema. Which one?",
    );
    let path = world.plan(
        "askableadopted",
        &plan_of(
            "askableadopted",
            vec![human("approve", &[]), agent("build", &["approve"])],
        ),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);
    world
        .run(&["attest", "askableadopted", "approve"])
        .exited(0);
    assert!(
        world.questions_on_the_stream("askableadopted").is_empty(),
        "something asked before the adopted run dispatched anything: {}",
        world.dump()
    );

    world.run(&["adopt", "askableadopted"]).exited(0).settled();
    assert_eq!(
        world.question_for_the_manager("askableadopted"),
        "build: the approval says nothing about the schema. Which one?"
    );
}

/// The prefix every branch those records name carries.
///
/// One prefix, so the journey can tell what it wrote from what the run did
/// without knowing which malformation is which — and so a line of `results`
/// carrying any of them fails the assertion that reads it.
const FORGED: &str = "onevcs/forged";

/// A branch name carrying what reads as another node's line in `results`.
///
/// The forgery is the point: `results` is read line by line, so a value that
/// carries a newline would put a record about a node nobody dispatched into a
/// view a manager reads.
const FORGED_LINE: &str = "onevcs/forged-line\n  audit                    running";

/// The branch a record about a *different* session names.
const FORGED_ELSEWHERE: &str = "onevcs/forged-elsewhere";

/// What the dispatch puts on its session's own stream: records the merged
/// store's reader cannot act on, one per refusal it makes.
///
/// A session's stream is **a file any process holding the token appends to** —
/// `src/vcs.rs` says so where it reads a change request off one — and the
/// dispatch is a process holding the token, so these arrive the way every
/// session record does. Each is written after the real record, so a reader that
/// took any of them would take it over the real one and every assertion in the
/// journey would then be about the wrong session.
fn unusable_session_records() -> String {
    json!([
        {"branch": FORGED_LINE},
        // The bound read off the reader's own declaration of it: a build that
        // lowered the bound and left a number here would stop testing anything.
        {
            "branch": format!(
                "{FORGED}-long{}",
                "g".repeat(onepipeline::event::MAX_PAYLOAD_TEXT_BYTES),
            ),
        },
        // Half a record is worse than none — the branch says where work is and
        // the token is how the worktree holding it is found — so a branch a
        // manager could act on under a token nothing can be addressed by is
        // refused as whole as the others.
        {"token": "  ", "branch": format!("{FORGED}-token")},
        {"token": "s-elsewhere", "branch": FORGED_ELSEWHERE},
        // Named, and named nothing: a value a producer wrote as empty is not the
        // same record as one that names no field at all, and neither is a
        // pointer at work.
        {"token": "", "branch": format!("{FORGED}-empty")},
        {"branch": ""},
        // `null` is how a record says a field is *absent*, which is a different
        // record from one naming it empty.
        {"branch": null},
    ])
    .to_string()
}

/// Adopting a run that had a dispatch in flight, which is what `adopt` is for.
///
/// The driver dies mid-dispatch and the work does not: `onevcs` commits the
/// worktree onto the session's branch before it gates, so the branch holds what
/// the worker had done and the session is the only thing that knows where.
/// Adoption used to drop the record and nothing else, which left that branch
/// unreferenced and unnamed anywhere a manager looks — so the branch is named
/// where the adoption is recorded and where an operator reads results, and the
/// node is pinned to it, which makes `onevcs` take that session up rather than
/// cut a second one on the same work.
///
/// The second half is the reader's boundary: the same run's stream carries
/// records nothing can be acted on, and none of them may take the real one's
/// place. See [`unusable_session_records`].
#[test]
fn adopting_a_run_whose_dispatch_was_in_flight_leaves_that_dispatchs_work_reachable() {
    let world = World::new("driver-adopt-inflight");
    // A gate this journey holds, which is how a dispatch is caught in flight
    // with its work already committed.
    let go = world.fakes.join("gate.go");
    let held = crate::harness::gate_script(&world, &["wait-for", &go.to_string_lossy()]);
    let repo = world.repository(
        "local-direct",
        &held.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    world.script("service.work", "the worker wrote this\n");
    // The other half of this journey: the dispatch puts records on its own
    // session's stream that no producer writes — see
    // [`unusable_session_records`] — so what the adoption folds is a store that
    // carries them beside the real one.
    world.script("service.session-records", &unusable_session_records());

    // No branch pin at all — the measured case. The session names the branch, so
    // nothing in the plan says where the dispatch's work is.
    let run = "inflight".to_string();
    let path = world.plan(
        &run,
        &plan_of(&run, vec![crate::harness::lifecycle("service", &[])]),
    );
    let mut owner = world
        .cmd(&["start", &path.to_string_lossy(), "--attach"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the run's first driver starts");
    world.until("the dispatch to reach its gate", |world| {
        world
            .journal(&run)
            .iter()
            .any(|event| event["source"] == "vcs" && event["kind"] == "gate-started")
    });
    let abandoned = world
        .events_of(&run, "session-opened")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the dispatch opened no session:\n{}", world.dump()));
    let token = abandoned["payload"]["token"]
        .as_str()
        .expect("the session names its token")
        .to_owned();
    let branch = abandoned["payload"]["branch"]
        .as_str()
        .expect("the session names its branch")
        .to_owned();

    owner.kill().expect("the run's first driver is terminated");
    owner.wait().expect("the run's first driver exits");
    world.until("the driver to exit", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });

    // The gate the dead driver was held at, released so the adoption's own
    // publication can finish.
    world.release("gate.go");

    let adopted = world.run(&["adopt", &run]);
    adopted.exited(0);
    // The operator who typed `adopt` is told at the moment it happens, because
    // this is when they can still act on it.
    adopted
        .err_has("had a dispatch in flight")
        .err_has(&branch)
        .err_has(&token);

    let recorded = world.events_of(&run, "driver-adopted");
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(
        recorded[0]["payload"]["abandoned"],
        json!([{"node": "service", "session": token, "branch": branch}]),
        "the adoption did not name the dispatch it cleared, or named a session \
         nothing in this stack opened: {}",
        recorded[0]
    );
    let results = world.run(&["results", &run]);
    results.exited(0).out_has(&branch).out_has(&token);
    for line in results.stdout.lines() {
        assert!(
            !line.contains("audit") && !line.contains(FORGED) && !line.contains("s-elsewhere"),
            "a record the store was handed put a line of its own into results:\n{}",
            results.stdout
        );
    }

    // The re-dispatch continued that session rather than cutting a second one on
    // the same work. Both halves are asserted, because either alone passes for
    // the wrong reason: every session this run ever opened is the one the
    // abandoned dispatch was working in, and `onevcs` — which is what decides
    // whether a session is taken up — says it took one up.
    let recorded_openings = world.events_of(&run, "session-opened");
    let branch_of = |event: &serde_json::Value| event["payload"]["branch"].clone();
    let forged = |event: &serde_json::Value| {
        branch_of(event)
            .as_str()
            .is_none_or(|branch| branch.is_empty() || branch.starts_with(FORGED))
    };
    // The scripted records reached the merged store the ordinary way — through
    // the session's own stream and this crate's follow of it — so what the
    // assertions below are about is a reader that met them and refused them,
    // rather than a producer path that quietly dropped them on the way.
    let arrived: std::collections::BTreeSet<String> = recorded_openings
        .iter()
        .filter(|event| forged(event))
        .map(|event| branch_of(event).to_string())
        .collect();
    assert_eq!(
        arrived.len(),
        7,
        "the records the dispatch put on its session's stream did not all reach the \
         merged store — one per refusal, each arriving at least once — so nothing \
         here is about what a reader does with them: {arrived:?}"
    );
    let openings: Vec<serde_json::Value> = recorded_openings
        .into_iter()
        .filter(|event| !forged(event))
        .collect();
    let tokens: Vec<String> = openings
        .iter()
        .map(|event| {
            event["payload"]["token"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert!(
        tokens.len() > 1,
        "the adopted run's own dispatch opened no session at all: {tokens:?}"
    );
    assert!(
        tokens.iter().all(|opened| *opened == token),
        "the adopted run cut a session beside the one holding the work: {tokens:?}"
    );
    assert!(
        openings
            .iter()
            .any(|event| event["payload"]["reused"] == json!(true)),
        "the sibling never recorded taking a session up, so the re-dispatch reached \
         that branch some other way: {openings:?}"
    );
    // Which is the only thing an operator cares about: the work the dead driver
    // left on that branch reached the base.
    assert_eq!(
        repo.base_file("service.md").as_deref().map(str::trim),
        Some("the worker wrote this"),
        "the abandoned dispatch's work never reached the base"
    );
}
