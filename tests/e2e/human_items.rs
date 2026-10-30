//! Human items inside real runs: the compiled binary's reconcile loop over a real
//! run store and plan store, with the host's human-item journal beside it.
//!
//! Each journey launches a plan, acts on its items through `onepipeline
//! human-items` — the operations a person or a manager uses — and reads what the
//! scheduler did off the run's own journal: which nodes it dispatched, which it
//! held, and what became of the human step an item stood for.

use crate::harness::{agent, human, plan_of, World};
use serde_json::{json, Value};

/// Human items enabled for this world, through its host-local configuration.
fn enabled(world: &World) {
    let dir = world.root.join("xdg/onepipeline");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.yaml"), "human_items:\n  enabled: true\n").unwrap();
}

fn items(world: &World, args: &[&str]) -> Vec<Value> {
    let mut all = vec!["human-items", "list", "--json"];
    all.extend_from_slice(args);
    let run = world.run(&all);
    run.exited(0);
    serde_json::from_str::<Value>(&run.stdout)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn dispatched(world: &World, run: &str) -> Vec<String> {
    world
        .events_of(run, "node-dispatched")
        .iter()
        .map(|e| e["labels"]["node"].as_str().unwrap().to_owned())
        .collect()
}

fn settled(world: &World, run: &str, node: &str) -> Option<String> {
    world
        .events_of(run, "node-settled")
        .iter()
        .rev()
        .find(|e| e["labels"]["node"] == node)
        .map(|e| {
            e["payload"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
}

fn id_of(view: &Value) -> String {
    view["item"]["id"].as_str().unwrap().to_owned()
}

#[test]
fn a_human_node_raises_one_item_and_completing_it_releases_only_its_branch() {
    let world = World::new("human-item-node");
    enabled(&world);
    let name = "gated";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                human("approve", &[]),
                agent("build", &["approve"]),
                agent("ship", &["build"]),
                agent("side", &[]),
            ],
        ),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    // The unrelated branch ran; the gated one waits on the person.
    assert_eq!(settled(&world, name, "side").as_deref(), Some("done"));
    assert!(!dispatched(&world, name).contains(&"build".to_owned()));
    let open = items(&world, &[]);
    assert_eq!(open.len(), 1, "{open:?}");
    let item = &open[0];
    assert_eq!(item["item"]["dedup_key"], format!("run:{name}#approve"));
    assert_eq!(item["item"]["kind"], "task");
    assert_eq!(
        item["item"]["priority"], "low",
        "a legacy node with no store priority is Low"
    );
    assert_eq!(
        item["item"]["title"],
        "Do approve, which only a person can do."
    );
    assert_eq!(
        item["item"]["block"],
        json!({"scope": "work", "run": name, "node": "approve"})
    );
    assert_eq!(item["item"]["owner"]["run"], name);
    assert_eq!(item["blocked"]["total"], 2);
    let plan = &item["blocked"]["plans"][0];
    assert_eq!(plan["count"], 2);
    let nodes: Vec<&Value> = plan["preview"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| &n["node"])
        .collect();
    assert_eq!(nodes, [&json!("build"), &json!("ship")]);
    let text = world.run(&["human-items", "list"]);
    text.exited(0).out_has("blocks 2 steps in");
    let detail = world.run(&["human-items", "show", &id_of(item), "--json"]);
    detail.exited(0);
    let detail: Value = serde_json::from_str(&detail.stdout).unwrap();
    assert_eq!(detail["nodes"].as_array().unwrap().len(), 2);

    // Adopting again reads the same eligible node: still one item.
    world.run(&["adopt", name]).exited(0);
    assert_eq!(items(&world, &[]).len(), 1);

    // Completing it attests the step once; the gated branch then runs.
    world
        .run(&["human-items", "complete", &id_of(item)])
        .exited(0);
    world.run(&["adopt", name]).exited(0);
    assert_eq!(settled(&world, name, "build").as_deref(), Some("done"));
    assert_eq!(settled(&world, name, "ship").as_deref(), Some("done"));
    assert_eq!(
        world
            .events_of(name, "edit-committed")
            .iter()
            .filter(|e| e["payload"]["command"]["op"] == "attest")
            .count(),
        1
    );
    assert!(items(&world, &[]).is_empty());
    let done = items(&world, &["--state", "completed"]);
    assert_eq!(done.len(), 1);
}

#[test]
fn dismissing_a_human_items_step_detaches_its_dependents_with_or_without_a_reason() {
    for reason in [Some("Superseded by the new rollout"), None] {
        let world = World::new("human-item-dismiss");
        enabled(&world);
        let name = "dismissed";
        let project = world.plan(
            name,
            &plan_of(
                name,
                vec![human("approve", &[]), agent("build", &["approve"])],
            ),
        );
        world.run(&["start", &project, "--attach"]).exited(0);
        let open = items(&world, &[]);
        let id = id_of(&open[0]);
        let mut dismiss = vec!["human-items", "dismiss", id.as_str()];
        if let Some(reason) = reason {
            dismiss.extend(["--reason", reason]);
        }
        world.run(&dismiss).exited(0);
        world.run(&["adopt", name]).exited(0);
        // The step was dropped, its dependent detached and run: the run did not fail.
        assert_eq!(settled(&world, name, "build").as_deref(), Some("done"));
        let drop = world
            .events_of(name, "edit-committed")
            .into_iter()
            .find(|e| e["payload"]["command"]["op"] == "drop")
            .expect("the step was dropped");
        assert_eq!(drop["payload"]["command"]["dependents"], "detach");
        assert_eq!(
            drop["payload"]["command"]["reason"],
            reason.map_or(Value::Null, |r| json!(r))
        );
        let result: Value = serde_json::from_str(
            &std::fs::read_to_string(world.run_file(name, "result.json")).unwrap(),
        )
        .unwrap();
        assert_ne!(result["state"], "failed", "{result}");
        let dismissed = items(&world, &["--state", "dismissed"]);
        assert_eq!(
            dismissed[0]["item"]["resolution"]["reason"],
            reason.map_or(Value::Null, |r| json!(r))
        );
    }
}
