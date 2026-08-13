//! Who launched a run, who may stop it, what happens when its driver dies, and
//! when an attach returns.
//!
//! Ported from `test_orchestrate_launch_e2e`, `test_attach_settles_e2e`, `test_run_ownership_e2e`, `test_round_ownership_e2e`, `test_run_adoption_e2e`, `test_relaunch_seed_e2e`, and the driver-liveness half of `test_liveness_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The scenario this journey states is one a real sibling would need paid
// model turns to produce, and `dispatch.rs` is where the real `oneagentgraph` binary is
// driven instead. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{agent, human, plan_of, World, NOTHING_DRIVING, REFUSED};
use serde_json::json;

fn start_detached(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    name.to_string()
}

/// The directory every dag-scope launch was given, in launch order.
///
/// Read off the argv the sibling was actually invoked with, because the value
/// under test is the one that *crosses the seam* — it becomes the `--cwd` each
/// member's harness runs in, and a run whose two launch paths disagree about it
/// is a run whose members move when it is adopted.
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

#[test]
fn start_launches_the_shipped_dag_scope_graph_and_records_how_to_relaunch_it() {
    let world = World::new("driver-launch");
    world.script("driver.wait", "hold");
    let path = world.plan("launched", &plan_of("launched", vec![agent("build", &[])]));
    let started = world.run(&[
        "start",
        &path.to_string_lossy(),
        "--detach",
        "--set",
        "members.orchestrator.agent.model=first value",
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
    assert_eq!(launch["round_budget"], 14_400);
    assert_eq!(launch["heartbeat_interval"], 1_800);
    assert_eq!(
        launch["dag_sets"],
        json!([
            "members.orchestrator.agent.model=first value",
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
fn a_live_driver_that_has_stopped_writing_reads_as_parked_rather_than_dead() {
    let world = World::new("driver-parked");
    // The driver is alive and holding, so its pid proves nothing about
    // progress — which is the whole distinction `PARKED` exists to draw.
    world.script("driver.wait", "hold");
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
    world.release("driver.go");
}

#[test]
fn a_parked_run_is_adoptable_as_much_as_a_dead_one_is() {
    let world = World::new("driver-adopt-parked");
    // Its driver is alive and holding, so this is the *other* undriven verdict:
    // nothing has proved the process gone, and nothing is happening either.
    world.script("driver.wait", "hold");
    let run = start_detached(&world, "idle", vec![agent("build", &[])]);
    world.until("the run to be reported parked", |world| {
        let mut status = world.cmd(&["status", &run]);
        status.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
        let out = status.output().expect("the binary runs");
        String::from_utf8_lossy(&out.stdout).contains("PARKED")
    });

    // The hint a parked run prints has to be a hint that works: an `adopt` that
    // refused here would leave the only offered way back closed.
    let mut adopt = world.cmd(&["adopt", &run]);
    adopt.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    let adopted = adopt.output().expect("the binary runs");
    assert!(
        !String::from_utf8_lossy(&adopted.stderr).contains("still being driven"),
        "a parked run refused the adoption its own status line offers: {}",
        String::from_utf8_lossy(&adopted.stderr)
    );
    assert_eq!(world.events_of(&run, "driver-adopted").len(), 1);
    world.release("driver.go");
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
            "--set",
            "members.orchestrator.agent.model=adopted model",
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
    adopted.exited(NOTHING_DRIVING);
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
        "members.orchestrator.agent.model=adopted model"
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
    // A human action nothing can clear: the driver settles the round, finds no
    // round it could open, and exits — which is what leaves the run adoptable.
    let path = world.plan(
        "relocated",
        &plan_of("relocated", vec![human("approve", &[])]),
    );
    let launched_from = world.project.clone();
    let mut start = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
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
    world
        .run_on(adopt, "adopt relocated")
        .exited(NOTHING_DRIVING);
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
        std::path::Path::new(&dirs[0]),
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

/// A run recorded by a build that had no directory field keeps the reading it
/// had, and one that records an unusable directory is refused by name.
///
/// The first is the whole compatibility promise: a record written before the
/// field existed carries none, and the driver such a run had ran in its own
/// working directory — so that is what an adoption of it still gives the
/// sibling, rather than a directory this build invented for it. The second is
/// what stops the field from being trusted just because it is there: it is a
/// file on disk, and it decides where every member of the run works.
#[test]
fn a_launch_record_without_a_directory_is_replayed_from_the_adopting_process() {
    let world = World::new("driver-legacy-dir");
    let run = start_detached(&world, "legacy", vec![human("approve", &[])]);
    world.until("the driver to exit", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });

    let rewrite = |record: &mut serde_json::Value| {
        let path = world.run_file(&run, "launch.json");
        std::fs::write(&path, record.to_string()).expect("the record is rewritten");
    };

    // A record from before the field: no `dir` at all.
    let mut record = world.run_json(&run, "launch.json");
    record
        .as_object_mut()
        .expect("the record is an object")
        .remove("dir");
    rewrite(&mut record);

    let adopted_from = world.project.clone();
    let mut adopt = world.cmd(&["adopt", &run]);
    adopt.current_dir(&adopted_from);
    world.run_on(adopt, "adopt legacy").exited(NOTHING_DRIVING);
    let dirs = dag_launch_dirs(&world);
    assert_eq!(
        std::path::Path::new(dirs.last().expect("the adoption relaunched the graph")),
        adopted_from,
        "a record with no directory was not replayed from the adopting process: {dirs:?}"
    );

    // And a record that carries one it cannot honour is refused by name rather
    // than handed on for each member to fail against separately.
    for (dir, reason) in [
        ("relative/place", "relative working directory"),
        ("/no/such/place/here", "which is not a directory"),
    ] {
        let mut record = world.run_json(&run, "launch.json");
        record["dir"] = json!(dir);
        rewrite(&mut record);
        world
            .run(&["adopt", &run])
            .exited(REFUSED)
            .err_has(reason)
            .err_has(dir);
    }
}

/// An adoption re-addresses the pacemaker at the graph run now driving the run.
///
/// An adoption starts a *fresh* graph run with an id of its own, so the id the
/// record carried is a run `oneagentgraph` has already finished with. A reset
/// sent there restarts nothing, which is the same silence the original defect
/// had — the record has to name the run that is driving now.
#[test]
fn adoption_re_addresses_the_pacemaker_at_the_graph_run_now_driving() {
    let world = World::new("driver-adopt-pacemaker");
    let run = start_detached(&world, "readdressed", vec![human("approve", &[])]);
    world.until("the driver to exit", |world| {
        world.run(&["status", &run]).stdout.contains("DRIVER DEAD")
    });
    let before = world.run_json(&run, "launch.json")["graph_run"]
        .as_str()
        .expect("the first driver's graph run")
        .to_string();

    world.run(&["adopt", &run]).exited(NOTHING_DRIVING);
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

/// A run that records no graph run says why its pacemaker was not reset, and
/// still hands the planner the surface they asked for.
///
/// The case a record written before the field existed leaves: there is no
/// address, so there is nothing to send. Saying so is the point — the reset is
/// best-effort, and a silent no-op here is exactly the failure that went
/// unnoticed for as long as it did.
#[test]
fn a_run_with_no_recorded_graph_run_says_why_the_pacemaker_was_not_reset() {
    let world = World::new("driver-no-graph-run");
    world.script("build.wait", "hold");
    let run = start_detached(&world, "unaddressed", vec![agent("build", &[])]);
    world.until("the run to open a round", |world| {
        !world.events_of(&run, "round-started").is_empty()
    });
    world
        .run(&["surface", &run, "--kind", "check-in", "--message", "steady"])
        .exited(0);

    let mut record = world.run_json(&run, "launch.json");
    record
        .as_object_mut()
        .expect("the record is an object")
        .remove("graph_run");
    std::fs::write(world.run_file(&run, "launch.json"), record.to_string())
        .expect("the record is rewritten");

    let read = world.run(&["next", &run]);
    read.exited(0).out_has("steady");
    read.err_has("could not reset the check-in pacemaker")
        .err_has("records no agent-graph run");
    assert!(
        !world.was_invoked("oneagentgraph", &["reset-timer"]),
        "a reset was sent with no run to address it to: {:?}",
        world.invocations()
    );
    world.release("build.go");
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

    // Recording the stop is not stopping it. The ledger says stopped either
    // way — `status` reads `run-stopped` and reports the run undriven without
    // ever looking at a process — so the ledger cannot be the evidence here.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("nothing is driving this run");

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
            !std::path::Path::new(&format!("/proc/{driver}")).exists()
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

#[test]
fn a_detached_driver_outlives_its_own_output_and_keeps_what_it_said() {
    let world = World::new("driver-detached-output");
    // The dispatch emits a line this build cannot read, so the engine verb the
    // driver runs says so — on the driver's own stderr, from a subprocess of a
    // subprocess. That is the one thing a detached driver cannot be given a
    // pipe for: the process that would read it is the launcher, and `--detach`
    // means the launcher has already gone. Written into such a pipe, the verb
    // dies of a broken one mid-round, and the run is left holding an open round
    // with nothing driving it — which surfaces much later, and as something
    // else entirely: a `reply` refused because the run "has settled".
    world.script("build.unreadable", "");
    let run = start_detached(&world, "detachedlog", vec![agent("build", &[])]);

    // On the driver's own record of the round, not on the engine's event for it.
    // `round-finished` is journaled by the `round run` the driver spawned, so it
    // is there while that child is still exiting and before the driver has
    // written down what it exited with — a gap wide enough to lose on a host
    // whose process teardown is slower, where this waited on one fact and then
    // asserted a different one that had not happened yet.
    world.until("the driver to finish its round", |world| {
        !world.events_of(&run, "round-finished").is_empty()
            && world
                .driver_saw()
                .iter()
                .any(|record| record["round_run"].is_number())
    });

    // The driver ran its round to the end rather than dying on its first line,
    // and the verb it ran succeeded.
    let saw = world.driver_saw();
    assert!(
        saw.iter().any(|record| record["round_run"] == json!(0)),
        "the driver's round did not finish: {saw:?}"
    );
    assert_eq!(
        world.run_json(&run, "round-01/result.json")["state"],
        "complete"
    );

    // And what it said is on disk, where a planner who never attached can read
    // it. A detached driver's words are otherwise the run's one unrecoverable
    // output.
    let log = std::fs::read_to_string(world.run_file(&run, "driver.log"))
        .expect("the detached driver's output was kept");
    assert!(
        log.contains("skipped"),
        "the driver's own words were lost: {log}"
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
        world.events_of(&run, "round-finished").is_empty(),
        "the launch did not return until the run had settled: {:?}",
        world.kinds(&run)
    );

    world.release("build.go");
    world.until("the run to settle once it is released", |world| {
        !world.events_of(&run, "round-finished").is_empty()
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
        let started = world.run(&["start", &path.to_string_lossy(), form]);

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
        let mut launch = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
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
        let mut launch = world.cmd(&["start", &path.to_string_lossy(), form]);
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
        // Nothing is driving the run, and the ledger says so rather than naming
        // a driver: the launch is what an `adopt` is offered from.
        world.run(&["status", run]).out_has("DRIVER DEAD");
    }
}

/// A graph that finished before it announced anything still launched.
///
/// The other end of the handshake: the answer came as an exit rather than as an
/// envelope, and it was a *clean* one. The graph ran whatever it was given and
/// stopped, which is a launch that worked and a run with nothing driving it —
/// the state the ledger records and `adopt` is offered from. Reporting it as a
/// refusal would fail the launch over the graph's own verdict.
#[test]
fn a_graph_that_finished_before_announcing_anything_is_a_launch_that_worked() {
    let world = World::new("driver-quiet-exit");
    world.script("run.exit-quietly", "hold");
    let path = world.plan("quiet", &plan_of("quiet", vec![agent("build", &[])]));

    // Detached: the launch reports the run it started, because starting it is
    // all it promised.
    let started = world.run(&["start", &path.to_string_lossy(), "--detach"]);
    started.exited(0).out_has("\"run_id\"");
    world.run(&["status", "quiet"]).out_has("DRIVER DEAD");

    // Attached: the launch stays, finds nothing driving the run, and says so —
    // exit 3 rather than a refusal, because the graph did not refuse anything.
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(NOTHING_DRIVING)
        .out_has("\"settlement\":\"unattended\"");
}
