//! A task a person withdrew from a board-backed plan — closed on the store as cancelled — is
//! no node of the graph a launch executes, and a task still depending on it refuses the
//! launch rather than being released. Entry 136 of `docs/contract-divergences.md`.
//!
//! Each journey drives the compiled binary over a real `local-md` store, and closes the
//! task through the store's own targeted update, as a person closing it with `onetaskgraph`
//! or on the board does.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary
// over the real `onetaskgraph` and real folders of Markdown. `harness.rs` carries the full
// rationale.

use serde_json::{json, Value};

use crate::harness::{agent, plan_of, World};

/// Close the task carrying node `node` as cancelled, through the store, as a person would.
/// Answers the task's qualified id.
pub(crate) fn withdraw(world: &World, project: &str, node: &str) -> String {
    let task = world
        .store_tasks(project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .unwrap_or_else(|| panic!("the board holds no item for {node}"));
    let whole = task["id"].as_str().expect("an item id").to_owned();
    let id = crate::harness::global(&whole);
    let update = onetaskgraph_plugin_api::TaskUpdate {
        status: Some(onetaskgraph_plugin_api::Status {
            category: onetaskgraph_plugin_api::StatusCategory::Cancelled,
            name: "cancelled".to_owned(),
        }),
        ..onetaskgraph_plugin_api::TaskUpdate::default()
    };
    world
        .store_call(|engine| async move { engine.update_task(&id, &update).await })
        .unwrap_or_else(|why| panic!("the store refused the close: {why}"));
    whole
}

/// The status the store holds the task carrying node `node` at.
pub(crate) fn stored_status(world: &World, project: &str, node: &str) -> Value {
    world
        .store_tasks(project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .map(|task| task["item"]["status"].clone())
        .unwrap_or_else(|| panic!("the board holds no item for {node}"))
}

/// The nodes `run` dispatched, in order.
fn dispatched(world: &World, run: &str) -> Vec<String> {
    world
        .events_of(run, "node-dispatched")
        .iter()
        .filter_map(|event| event["labels"]["node"].as_str().map(str::to_owned))
        .collect()
}

/// `build → ship`, with `spike` beside them withdrawn by a person before the launch: the run
/// executes the two, makes no node of `spike`, and leaves its task cancelled — the launch's
/// projections, which claim every node `queued` before any starts, never touch it.
#[test]
fn a_launch_leaves_out_a_task_a_person_withdrew_and_never_reopens_it() {
    let world = World::new("withdrawn-left-out");
    let project = world.plan(
        "withdrawn",
        &plan_of(
            "withdrawn",
            vec![
                agent("build", &[]),
                agent("spike", &[]),
                agent("ship", &["build"]),
            ],
        ),
    );
    withdraw(&world, &project, "spike");

    world.run(&["start", &project, "--attach"]).settled();
    let mut ran = dispatched(&world, "withdrawn");
    ran.sort();
    assert_eq!(ran, ["build", "ship"]);
    let result = world.run_json("withdrawn", "result.json");
    assert_eq!(result["state"], "complete", "{result:#}");
    let mut nodes: Vec<&str> = result["nodes"]
        .as_array()
        .expect("the result lists its nodes")
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect();
    nodes.sort_unstable();
    assert_eq!(
        nodes,
        ["build", "ship"],
        "the launched graph is not the plan less its withdrawn task"
    );
    assert_eq!(
        stored_status(&world, &project, "spike"),
        json!({"category": "cancelled", "name": "cancelled"}),
        "the launch's projection reopened the withdrawn task"
    );
    assert_eq!(
        stored_status(&world, &project, "ship")["category"],
        "done",
        "the run did not project the nodes it executed"
    );
}

/// `ship` still depends on `design`, which a person withdrew: the launch is refused naming both
/// tasks and the status the store reports `design` cancelled under, and nothing is dispatched —
/// releasing `ship` would start it without the work it was written to wait for.
#[test]
fn a_launch_whose_task_depends_on_a_withdrawn_task_is_refused_naming_both() {
    let world = World::new("withdrawn-dependency");
    let project = world.plan(
        "withdrawn-dep",
        &plan_of(
            "withdrawn-dep",
            vec![agent("design", &[]), agent("ship", &["design"])],
        ),
    );
    let design = withdraw(&world, &project, "design");
    let ship = world
        .store_tasks(&project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == "ship")
        .and_then(|task| task["id"].as_str().map(str::to_owned))
        .expect("ship's item");

    let refused = world.run(&["start", &project, "--attach"]);
    assert_ne!(
        refused.code, 0,
        "the launch was not refused: {}",
        refused.stdout
    );
    refused.err_has(&format!(
        "'{ship}' depends on '{design}', which is withdrawn — the store reports it cancelled, \
         as 'cancelled'"
    ));
    assert!(
        !world.run_file("withdrawn-dep", "events.jsonl").exists(),
        "a refused launch started a run"
    );
    assert!(dispatched(&world, "withdrawn-dep").is_empty());
    assert_eq!(
        stored_status(&world, &project, "ship")["category"],
        "todo",
        "a refused launch claimed a task"
    );
}

/// A stop projects the engine's own cancellation. Its stored board word keeps the task
/// eligible for a fresh launch, whereas a person's cancellation has no such record.
#[test]
fn an_engine_cancelled_task_resumes_from_the_store() {
    let world =
        World::new("withdrawn-engine-cancelled").with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    world.script("build.wait", "hold");
    let project = world.plan(
        "engine-cancelled",
        &plan_of("engine-cancelled", vec![agent("build", &[])]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("build to dispatch", |world| {
        !dispatched(world, "engine-cancelled").is_empty()
    });
    world.run(&["stop", "engine-cancelled"]).exited(0);
    let task = world.store_tasks(&project).remove(0);
    assert_eq!(task["item"]["status"]["category"], "cancelled", "{task}");
    assert_eq!(
        task["item"]["metadata"]["onepipeline.projected_status"], "cancelled",
        "{task}"
    );
    assert_eq!(
        task["item"]["metadata"]["onepipeline.settlement"]["status"], "cancelled",
        "{task}"
    );
    world.unscript("build.wait");
    let cold = world.on_host("host-b");
    cold.run(&["start", &project, "--attach"]).settled();
    assert_eq!(dispatched(&cold, "engine-cancelled"), ["build"]);
    assert_eq!(stored_status(&cold, &project, "build")["category"], "done");
}
