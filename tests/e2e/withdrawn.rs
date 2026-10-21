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

pub(crate) fn stored_status(world: &World, project: &str, node: &str) -> Value {
    world
        .store_tasks(project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .map(|task| task["item"]["status"].clone())
        .unwrap_or_else(|| panic!("the board holds no item for {node}"))
}

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
    let before = stored_status(&world, &project, "ship");
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
        stored_status(&world, &project, "ship"),
        before,
        "a refused launch claimed a task"
    );
}

/// An engine cancellation's stored board word keeps the task eligible for a fresh launch.
#[test]
fn an_engine_cancelled_task_resumes_from_the_store() {
    engine_cancelled_continuation(false);
}

/// No current engine writes the legacy shape without a board word; it remains resumable.
#[test]
fn a_legacy_engine_cancelled_task_resumes_without_a_recorded_board_word() {
    engine_cancelled_continuation(true);
}

fn engine_cancelled_continuation(legacy: bool) {
    let world = World::new("withdrawn-engine-cancelled");
    let project = world.plan(
        "engine-cancelled",
        &plan_of("engine-cancelled", vec![agent("build", &[])]),
    );
    let task = withdraw(&world, &project, "build");
    let id = crate::harness::global(&task);
    let key = |name| onetaskgraph_plugin_api::MetadataKey::new(name).expect("a metadata key");
    // Stored input in the shape engine::drain produces via Settlement::plain(Cancelled),
    // projected by writeback::task_document beside Snapshot::word_of's cancelled board word.
    // Those functions are private to the binary. The legacy input omits the additive key,
    // since a current engine cannot generate that older record.
    let mut metadata = std::collections::BTreeMap::from([(
        key("onepipeline.settlement"),
        json!({"status": "cancelled"}),
    )]);
    if !legacy {
        metadata.insert(key("onepipeline.projected_status"), json!("cancelled"));
    }
    let update = onetaskgraph_plugin_api::TaskUpdate {
        metadata_set: metadata,
        ..onetaskgraph_plugin_api::TaskUpdate::default()
    };
    world
        .store_call(|engine| async move { engine.update_task(&id, &update).await })
        .expect("store the engine cancellation record");
    let cold = world.on_host("host-b");
    cold.run(&["start", &project, "--attach"]).settled();
    assert_eq!(dispatched(&cold, "engine-cancelled"), ["build"]);
    assert_eq!(stored_status(&cold, &project, "build")["category"], "done");
}

/// A person's cancellation after a successful settlement removes that task on continuation.
#[test]
fn a_persons_close_after_a_done_settlement_withdraws_the_task() {
    let world = World::new("withdrawn-done");
    let project = world.plan(
        "withdrawn-done",
        &plan_of(
            "withdrawn-done",
            vec![agent("build", &[]), agent("keep", &[])],
        ),
    );
    world.run(&["start", &project, "--attach"]).settled();
    withdraw(&world, &project, "build");
    let cold = world.on_host("host-b");
    cold.run(&["start", &project, "--attach"]).settled();
    let result = cold.run_json("withdrawn-done", "result.json");
    assert_eq!(
        result["nodes"].as_array().expect("nodes").len(),
        1,
        "{result}"
    );
    assert_eq!(result["nodes"][0]["id"], "keep", "{result}");
    assert_eq!(
        stored_status(&cold, &project, "build")["category"],
        "cancelled"
    );
    assert!(dispatched(&cold, "withdrawn-done").is_empty());
}

/// Metadata is external input: malformed words refuse before the run claims or dispatches.
#[test]
fn a_cancelled_task_with_malformed_projected_status_refuses_launch() {
    for value in [json!(17), json!("canceled"), Value::Null] {
        let world = World::new("withdrawn-malformed");
        let project = world.plan(
            "withdrawn-malformed",
            &plan_of("withdrawn-malformed", vec![agent("build", &[])]),
        );
        let task = withdraw(&world, &project, "build");
        let id = crate::harness::global(&task);
        let key = onetaskgraph_plugin_api::MetadataKey::new("onepipeline.projected_status")
            .expect("a metadata key");
        world
            .store_call(|engine| async move { engine.set_task_metadata(&id, &key, &value).await })
            .expect("write external metadata");
        let refused = world.run(&["start", &project, "--attach"]);
        assert_ne!(refused.code, 0, "{}", refused.stdout);
        refused.err_has(&task);
        refused.err_has("`onepipeline.projected_status` is not a projected status");
        assert!(dispatched(&world, "withdrawn-malformed").is_empty());
        assert_eq!(
            stored_status(&world, &project, "build")["category"],
            "cancelled"
        );
    }
}

/// A withdrawn board item need not have executable metadata: it claims no ID or edges.
#[test]
fn a_withdrawn_task_without_a_node_id_or_valid_edges_is_left_out() {
    let world = World::new("withdrawn-unplanned");
    let project = world.plan(
        "withdrawn-unplanned",
        &plan_of(
            "withdrawn-unplanned",
            vec![agent("build", &[]), agent("spike", &[])],
        ),
    );
    let task = withdraw(&world, &project, "spike");
    let id = crate::harness::global(&task);
    let key = |name| onetaskgraph_plugin_api::MetadataKey::new(name).expect("a metadata key");
    let update = onetaskgraph_plugin_api::TaskUpdate {
        metadata_remove: std::collections::BTreeSet::from([key("onepipeline.id")]),
        metadata_set: std::collections::BTreeMap::from([(
            key("onepipeline.deps"),
            json!(["missing"]),
        )]),
        ..onetaskgraph_plugin_api::TaskUpdate::default()
    };
    world
        .store_call(|engine| async move { engine.update_task(&id, &update).await })
        .expect("leave withdrawn work without executable metadata");
    world.run(&["start", &project, "--attach"]).settled();
    assert_eq!(dispatched(&world, "withdrawn-unplanned"), ["build"]);
    let result = world.run_json("withdrawn-unplanned", "result.json");
    assert_eq!(
        result["nodes"].as_array().expect("nodes").len(),
        1,
        "{result}"
    );
    let stored = world
        .store_tasks(&project)
        .into_iter()
        .find(|item| item["id"] == task)
        .expect("the withdrawn task remains on the board");
    assert_eq!(stored["item"]["status"]["category"], "cancelled");
}

/// A retry clears the old settlement's projected word before its replacement settles.
#[test]
fn a_retry_removes_the_previous_settlements_recorded_board_word() {
    let world = World::new("withdrawn-retry");
    world.script("build.fail", "1");
    let run = "withdrawn-retry";
    let project = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world.run(&["start", &project, "--attach"]).settled();
    let task = world.store_tasks(&project).remove(0);
    assert_eq!(
        task["item"]["metadata"]["onepipeline.projected_status"],
        "failed"
    );
    world.script("replacement.wait", "hold");
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{
                "op": "retry", "id": "build", "node": {
                    "id": "replacement", "persona": "engineer",
                    "task": "## What\nRetry it.\n\n## Acceptance criteria\n- It is done."
                }
            }]})
            .to_string(),
        )
        .exited(0);
    world.run(&["adopt", run, "--detach"]).exited(0);
    world.until_store(
        "the replacement to reach the board without the old settlement",
        |world| {
            let task = world.store_tasks(&project).remove(0);
            let metadata = &task["item"]["metadata"];
            metadata["onepipeline.node"] == "replacement"
                && metadata.get("onepipeline.settlement").is_none()
        },
    );
    let task = world.store_tasks(&project).remove(0);
    assert!(
        task["item"]["metadata"]
            .get("onepipeline.projected_status")
            .is_none(),
        "{task}"
    );
    world.release("replacement.go");
    world.until_store("the replacement to settle", |world| {
        world.store_tasks(&project)[0]["item"]["metadata"]["onepipeline.projected_status"] == "done"
    });
}
