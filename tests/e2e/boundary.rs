//! A dispatch that produced *nothing* and failed is asked again. An attempt that
//! answered has already answered, whatever its exit status, so only the silent
//! one is retried — and every retry reaches the run's own record as what it is:
//! the node being dispatched again, with the attempt it is on.
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
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

#[test]
fn a_dispatch_that_produced_nothing_is_asked_again_and_each_attempt_is_journalled() {
    let world = World::new("boundary-retry");
    world.script("build.silent", "");
    // It refuses twice and answers on the third attempt, inside the default
    // budget of three.
    world.script("build.recover-after", "3");
    let run = settle(&world, "retried", vec![agent("build", &[])]);

    // Three attempts, three dispatches: asking again *is* dispatching again, so
    // that is what the record says, with the attempt each one is on.
    let dispatches = world.events_of(&run, "node-dispatched");
    assert_eq!(dispatches.len(), 3, "{dispatches:?}");
    let attempts: Vec<u64> = dispatches
        .iter()
        .map(|event| event["payload"]["attempt"].as_u64().unwrap_or(0))
        .collect();
    assert_eq!(attempts, vec![1, 2, 3]);
    assert_eq!(dispatches[1]["labels"]["node"], "build");
    assert_eq!(dispatches[1]["payload"]["attempts"], 3);
    assert!(
        dispatches[1]["payload"]["reason"]
            .as_str()
            .expect("a reason")
            .contains("provider refused"),
        "{dispatches:?}"
    );

    // The recovery is what it is for: the node settled.
    assert_eq!(world.run_json(&run, "result.json")["state"], "complete");
}

#[test]
fn a_dispatch_that_answered_is_not_asked_again_whatever_its_exit_status() {
    let world = World::new("boundary-answered");
    world.script("build.fail", "1");
    let run = settle(&world, "answered", vec![agent("build", &[])]);

    assert_eq!(
        world.events_of(&run, "node-dispatched").len(),
        1,
        "an attempt that answered was retried; the next budget goes the same way"
    );
    let result = world.run_json(&run, "result.json");
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
        world.run_file("spent", "result.json").is_file()
    });
    // Two attempts means one retry between them.
    assert_eq!(world.events_of("spent", "node-dispatched").len(), 2);
    let result = world.run_json("spent", "result.json");
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
        world.run_file("fallback", "result.json").is_file()
    });
    assert_eq!(
        world.events_of("fallback", "node-dispatched").len(),
        2,
        "an unusable value disabled the recovery it configures"
    );
    assert_eq!(
        world.run_json("fallback", "result.json")["state"],
        "complete"
    );
}

/// A dispatch that died for a reason that is **not the agent's verdict** settles
/// under a word of its own, having produced nothing at all.
///
/// The incident with no branch to recover: a node's run root was deleted
/// underneath a live dispatch, so every identity in both its chains refused to
/// start and none of them ran a turn. Reported `task-failed`, it says the agent
/// tried this task and could not do it — which is a lie about a dispatch that
/// never reached the task, and it sends whoever reads it to re-run work rather
/// than to look at the host.
///
/// Everything the classification is made from is on the dispatch's own stderr,
/// which is where `oneagentgraph` writes what ended a member, and the word is
/// chosen from that and never from the branch: this node has none.
#[test]
fn a_dispatch_that_died_producing_nothing_is_not_reported_as_an_agent_that_failed() {
    let world = World::new("boundary-diednothing");
    world.script("build.silent", "");
    world.script(
        "build.refused",
        "- - claude-code spawn-error\n- - codex spawn-error\n",
    );
    world.script(
        "build.died",
        "no candidate ran the turn: claude-code [spawn-error], codex [spawn-error]",
    );
    let run = settle(&world, "diednothing", vec![agent("build", &[])]);

    // Not retried: the dispatch answered — those refusals are on the record — so
    // another budget would go the same way. That is the policy this word was
    // added *beside* rather than instead of.
    assert_eq!(
        world.events_of(&run, "node-dispatched").len(),
        1,
        "a dispatch that answered was retried"
    );
    assert!(
        world
            .journal(&run)
            .iter()
            .all(|event| event["kind"] != "turn-started"),
        "this journey's dispatch opened a turn"
    );

    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "failed", "{node}");
    assert_eq!(
        node["outcome"], "dispatch-died",
        "a dispatch that never reached its task was reported as one that failed it: {node}"
    );
    assert_eq!(node["cause"], "spawn-error", "{node}");
    // No branch and no commit, because there was neither — which is exactly how
    // this case has to read. The word does not depend on them.
    assert!(node["branch"].is_null(), "{node}");
    assert!(node["head"].is_null(), "{node}");

    // And a manager reads all of that without opening the store.
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("dispatch-died")
        .out_has("the dispatch died (spawn-error) rather than failing its task")
        .out_has("it left no branch, so nothing of it survived");
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("the dispatch died (spawn-error) rather than failing its task");
}

/// The same failure with a verdict of the agent's own settles exactly as it did.
///
/// The pair is the point: a word that is *distinct* is only distinct if the
/// ordinary case still reads the way it always has. A classifier that answered
/// the same way for both would have relabelled every failure in the run and told
/// a manager to go and look at a harness that was never the problem.
#[test]
fn a_dispatch_whose_agent_failed_its_task_still_settles_as_a_task_failure() {
    let world = World::new("boundary-diedverdict");
    // The double's `.fail` is the agent's own verdict — "the node failed its
    // gate" — which names no machinery and carries no classification.
    world.script("build.fail", "1");
    let run = settle(&world, "diedverdict", vec![agent("build", &[])]);

    let node = world.run_json(&run, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "failed", "{node}");
    assert_eq!(node["outcome"], "task-failed", "{node}");
    assert!(
        node["cause"].is_null(),
        "a task failure was classified as a dispatch that died: {node}"
    );
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("task-failed")
        .out_lacks("the dispatch died");
}
