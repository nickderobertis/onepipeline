//! Ported from `test_plan_e2e`, `test_single_node_plan_e2e`, and
//! `test_scheduling_e2e`.
//!
//! What a plan file may say, what it may not, and the order the engine starts
//! what it says. A plan is external input, so every refusal here happens before
//! any provider time is spent.

use crate::harness::{agent, human, plan_of, World, QUEUED, REFUSED};
use serde_json::json;

/// Run a plan to settlement, attached, and return the run id.
fn settle(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the run to settle", |world| {
        !world.events_of(name, "round-finished").is_empty()
    });
    name.to_string()
}

#[test]
fn a_single_node_plan_runs_to_completion_and_records_its_evidence() {
    let world = World::new("plan-single");
    let run = settle(&world, "single", vec![agent("build", &[])]);

    world.until("the graph to complete", |world| {
        world.run_file(&run, "round-01/result.json").exists()
    });
    let result = world.run_json(&run, "round-01/result.json");
    assert_eq!(result["state"], "complete", "{result}");
    assert_eq!(result["ok"], true);
    assert_eq!(result["nodes"][0]["id"], "build");
    assert_eq!(result["nodes"][0]["status"], "done");

    // The dispatch really went through the `oneagentgraph` seam, with the
    // reserved labels stamped on it.
    assert!(world.was_invoked("oneagentgraph", &["run", "--label", "node=build"]));
    let kinds = world.kinds(&run);
    for expected in [
        "run-started",
        "round-started",
        "node-dispatched",
        "node-settled",
        "round-finished",
    ] {
        assert!(
            kinds.contains(&expected.to_string()),
            "{kinds:?} lacks {expected}"
        );
    }
}

#[test]
fn a_dependent_node_starts_only_after_its_dependency_is_done() {
    let world = World::new("plan-order");
    let run = settle(
        &world,
        "ordered",
        vec![agent("first", &[]), agent("second", &["first"])],
    );

    let order: Vec<String> = world
        .journal(&run)
        .iter()
        .filter(|event| event["kind"] == "node-dispatched" || event["kind"] == "node-settled")
        .filter_map(|event| {
            Some(format!(
                "{}:{}",
                event["labels"]["node"].as_str()?,
                event["kind"].as_str()?
            ))
        })
        .collect();
    let dispatched_second = order
        .iter()
        .position(|entry| entry == "second:node-dispatched")
        .expect("second was dispatched");
    let settled_first = order
        .iter()
        .position(|entry| entry == "first:node-settled")
        .expect("first settled");
    assert!(
        settled_first < dispatched_second,
        "second started before first settled: {order:?}"
    );
}

#[test]
fn a_failed_dependency_skips_its_descendant_rather_than_blocking_it() {
    let world = World::new("plan-skip");
    world.script("build.fail", "1");
    let run = settle(
        &world,
        "skipped",
        vec![agent("build", &[]), agent("ship", &["build"])],
    );

    let result = world.run_json(&run, "round-01/result.json");
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["ok"], false);
    let status = |id: &str| {
        result["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["id"] == id)
            .expect(id)["status"]
            .as_str()
            .expect("a status")
            .to_string()
    };
    assert_eq!(status("build"), "failed");
    assert_eq!(status("ship"), "skipped");
}

#[test]
fn a_ready_human_action_waits_and_blocks_what_it_unblocks() {
    let world = World::new("plan-human");
    let run = settle(
        &world,
        "gated",
        vec![
            agent("build", &[]),
            human("approve", &["build"]),
            agent("ship", &["approve"]),
        ],
    );

    let result = world.run_json(&run, "round-01/result.json");
    assert_eq!(result["state"], "waiting", "{result}");
    let node = |id: &str| {
        result["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["id"] == id)
            .expect(id)
            .clone()
    };
    assert_eq!(node("approve")["status"], "waiting");
    assert_eq!(node("approve")["unblocks"], json!(["ship"]));
    assert_eq!(node("ship")["status"], "blocked");
    assert_eq!(node("ship")["blocked_by"], json!(["approve"]));
    // The harness never guesses that a person acted, so nothing was dispatched
    // for the human node.
    assert!(!world.was_invoked("oneagentgraph", &["run", "--label", "node=approve"]));
}

#[test]
fn an_expects_no_diff_node_settles_without_spending_a_dispatch() {
    let world = World::new("plan-nodiff");
    let run = settle(
        &world,
        "nodiff",
        vec![json!({
            "id": "handoff",
            "task": "## What\nRecord that nothing changes.",
            "expects_no_diff": true,
        })],
    );

    let result = world.run_json(&run, "round-01/result.json");
    assert_eq!(result["nodes"][0]["status"], "done");
    assert_eq!(result["nodes"][0]["outcome"], "no-changes");
    assert!(
        !world.was_invoked("oneagentgraph", &["run", "--label", "node=handoff"]),
        "an expects_no_diff node spent a dispatch"
    );
}

#[test]
fn concurrency_bounds_how_many_nodes_are_in_flight_at_once() {
    let world = World::new("plan-concurrency");
    for node in ["a", "b", "c"] {
        world.script(&format!("{node}.wait"), "hold");
    }
    let mut plan = plan_of(
        "bounded",
        vec![agent("a", &[]), agent("b", &[]), agent("c", &[])],
    );
    plan["concurrency"] = json!(2);
    let path = world.plan("bounded", &plan);
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

    world.until("two dispatches to be in flight", |world| {
        world.events_of("bounded", "node-dispatched").len() == 2
    });
    // Give a third a chance to appear if the bound were not honoured.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        world.events_of("bounded", "node-dispatched").len(),
        2,
        "concurrency 2 dispatched a third node"
    );

    for node in ["a", "b", "c"] {
        world.release(&format!("{node}.go"));
    }
    world.until("the run to settle", |world| {
        !world.events_of("bounded", "round-finished").is_empty()
    });
    assert_eq!(world.events_of("bounded", "node-dispatched").len(), 3);
}

#[test]
fn a_plan_the_schema_refuses_never_starts_a_run() {
    let world = World::new("plan-refuse");
    let cases: &[(&str, &str, &str)] = &[
        (
            "cycle",
            r#"{"schema_version":1,"tasks":[
                {"id":"a","persona":"e","task":"t","deps":["b"]},
                {"id":"b","persona":"e","task":"t","deps":["a"]}]}"#,
            "cycle",
        ),
        (
            "dangling",
            r#"{"schema_version":1,"tasks":[{"id":"a","persona":"e","task":"t","deps":["nowhere"]}]}"#,
            "not in the plan",
        ),
        (
            "duplicate",
            r#"{"schema_version":1,"tasks":[{"id":"a","persona":"e","task":"t"},{"id":"a","persona":"e","task":"t"}]}"#,
            "duplicate node id",
        ),
        (
            "typo",
            r#"{"schema_version":1,"concurency":2,"tasks":[{"id":"a","persona":"e","task":"t"}]}"#,
            "concurency",
        ),
        (
            "notmapping",
            "[1, 2, 3]",
            "must be a JSON mapping, got list",
        ),
        (
            "humanpersona",
            r#"{"schema_version":1,"tasks":[{"id":"a","kind":"human","task":"t","persona":"e"}]}"#,
            "no persona or done_when",
        ),
        (
            "nodiffpersona",
            r#"{"schema_version":1,"tasks":[{"id":"a","task":"t","expects_no_diff":true,"persona":"e"}]}"#,
            "takes no persona or done_when",
        ),
        (
            "version",
            r#"{"schema_version":7,"tasks":[{"id":"a","persona":"e","task":"t"}]}"#,
            "schema_version",
        ),
    ];

    for (name, body, expected) in cases {
        let path = world.raw_plan(&format!("{name}.plan.json"), body);
        world
            .run(&["start", &path.to_string_lossy()])
            .exited(REFUSED)
            .err_has(expected);
        assert!(
            !world.runs.join(name).exists(),
            "a refused plan left a run directory behind"
        );
    }
}

#[test]
fn a_json_plan_keeps_json_escape_semantics_all_the_way_to_the_dispatch() {
    let world = World::new("plan-emoji");
    // What a JSON writer emits for one emoji is a surrogate pair. Read as YAML
    // it is two unpaired halves and the node fails on its own task prose.
    let path = world.raw_plan(
        "emoji.plan.json",
        r#"{"schema_version":1,"name":"emoji","tasks":[
            {"id":"build","persona":"engineer","task":"😀 ship it"}]}"#,
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the run to settle", |world| {
        !world.events_of("emoji", "round-finished").is_empty()
    });

    let relayed = world
        .journal("emoji")
        .into_iter()
        .find(|event| event["source"] == "agentgraph")
        .expect("the dispatch relayed an envelope");
    let task = relayed["payload"]["task"].as_str().expect("the task prose");
    assert!(
        task.starts_with('\u{1f600}'),
        "the surrogate pair did not survive as one character: {task:?}"
    );
}

#[test]
fn a_round_that_settles_unfinished_exits_one_rather_than_zero() {
    let world = World::new("plan-exit");
    world.script("build.fail", "1");
    let path = world.plan("exit", &plan_of("exit", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the round to finish", |world| {
        !world.events_of("exit", "round-finished").is_empty()
    });
    // Driving the same round again is the engine verb a caller reads the exit
    // status off: a failed graph is unfinished, not an error.
    world.run(&["round", "run", "exit"]).exited(QUEUED);
}
