//! A dispatch that produced *nothing* and failed is asked again. An attempt that
//! answered has already answered, whatever its exit status, so only the silent
//! one is retried — and every retry reaches the run's own record rather than
//! only a log.
//!
//! Ported from `test_boundary_retry_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The scenario this journey states is one a real sibling would need paid
// model turns to produce, and `dispatch.rs` is where the real `oneagentgraph` binary is
// driven instead. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{agent, plan_of, World};

fn settle(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .settled();
    world.until("the run to settle", |world| {
        !world.events_of(name, "round-finished").is_empty()
    });
    name.to_string()
}

#[test]
fn a_dispatch_that_produced_nothing_is_asked_again_and_the_retry_is_journalled() {
    let world = World::new("boundary-retry");
    world.script("build.silent", "");
    // It refuses twice and answers on the third attempt, inside the default
    // budget of three.
    world.script("build.recover-after", "3");
    let run = settle(&world, "retried", vec![agent("build", &[])]);

    let retries = world.events_of(&run, "boundary-retried");
    assert_eq!(retries.len(), 2, "{retries:?}");
    assert_eq!(retries[0]["payload"]["attempt"], 1);
    assert_eq!(retries[0]["payload"]["attempts"], 3);
    assert_eq!(retries[0]["labels"]["node"], "build");
    assert!(
        retries[0]["payload"]["reason"]
            .as_str()
            .expect("a reason")
            .contains("provider refused"),
        "{retries:?}"
    );

    // The recovery is what it is for: the node settled.
    assert_eq!(
        world.run_json(&run, "round-01/result.json")["state"],
        "complete"
    );
}

#[test]
fn a_dispatch_that_answered_is_not_asked_again_whatever_its_exit_status() {
    let world = World::new("boundary-answered");
    world.script("build.fail", "1");
    let run = settle(&world, "answered", vec![agent("build", &[])]);

    assert!(
        world.events_of(&run, "boundary-retried").is_empty(),
        "an attempt that answered was retried; the next budget goes the same way"
    );
    let result = world.run_json(&run, "round-01/result.json");
    assert_eq!(result["nodes"][0]["status"], "failed");
    assert_eq!(result["nodes"][0]["outcome"], "task-failed");
}

#[test]
fn a_budget_that_runs_out_settles_the_node_rather_than_retrying_forever() {
    let world = World::new("boundary-spent");
    world.script("build.silent", "");
    let path = world.plan("spent", &plan_of("spent", vec![agent("build", &[])]));
    let mut command = world.cmd(&["start", &path.to_string_lossy(), "--attach"]);
    command.env("ONEPIPELINE_BOUNDARY_ATTEMPTS", "2");
    command.output().expect("the binary runs");

    world.until("the run to settle", |world| {
        !world.events_of("spent", "round-finished").is_empty()
    });
    // Two attempts means one retry between them.
    assert_eq!(world.events_of("spent", "boundary-retried").len(), 1);
    let result = world.run_json("spent", "round-01/result.json");
    assert_eq!(result["nodes"][0]["status"], "failed");
    // Named apart from an ordinary task failure: the budget was spent without
    // the agent producing anything, and the two want opposite responses.
    assert_eq!(result["nodes"][0]["outcome"], "no-agent-progress");
    world
        .run(&["results", "spent"])
        .exited(0)
        .out_has("provider refused before the first turn");
}

#[test]
fn an_unusable_attempt_budget_falls_back_rather_than_disabling_the_recovery() {
    let world = World::new("boundary-unusable");
    world.script("build.silent", "");
    world.script("build.recover-after", "2");
    let path = world.plan("fallback", &plan_of("fallback", vec![agent("build", &[])]));
    let mut command = world.cmd(&["start", &path.to_string_lossy(), "--attach"]);
    command.env("ONEPIPELINE_BOUNDARY_ATTEMPTS", "not a number");
    command.output().expect("the binary runs");

    world.until("the run to settle", |world| {
        !world.events_of("fallback", "round-finished").is_empty()
    });
    assert_eq!(
        world.events_of("fallback", "boundary-retried").len(),
        1,
        "an unusable value disabled the recovery it configures"
    );
    assert_eq!(
        world.run_json("fallback", "round-01/result.json")["state"],
        "complete"
    );
}
