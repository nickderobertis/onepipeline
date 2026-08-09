//! A `run:<id>#<node>` dependency: how it resolves, what it records when it
//! does, and what it reports when the upstream moves afterwards.
//!
//! The reference is another run's ledger, not this one's graph, so the edge is
//! resolved by reading that run — an unknown run, an unfinished node, or a
//! failed one all leave the consumer blocked rather than failing it, because the
//! upstream may still arrive. Once it does arrive the consumer records how far
//! the upstream had got, and reports later movement past that point without
//! rerunning work.
//!
//! Ported from the cross-DAG halves of `test_tracked_graph_e2e`, whose semantics
//! `ai-orchestrator`'s `docs/orchestration.md` states.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. There is no alternative today: both sibling crates are at their own
// interface-only stage and refuse every invocation with exit 70. `harness.rs` carries the
// same suppression and the full rationale.

use crate::harness::{agent, human, plan_of, World};
use serde_json::{json, Value};

/// A node depending on another run's node.
fn consumer(id: &str, upstream: &str) -> Value {
    let mut node = agent(id, &[]);
    node["deps"] = json!([upstream]);
    node
}

/// Run a plan to settlement, attached, and return the run id.
fn settle(world: &World, name: &str, nodes: Vec<Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path.to_string_lossy(), "--attach"]);
    world.until("the run to settle", |world| {
        !world.events_of(name, "round-finished").is_empty()
    });
    name.to_string()
}

fn status_of(world: &World, run: &str, round: &str, id: &str) -> Value {
    world.run_json(run, round)["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|node| node["id"] == id)
        .unwrap_or_else(|| panic!("{id} is missing from {run}/{round}"))["status"]
        .clone()
}

#[test]
fn a_dependency_on_a_finished_upstream_node_resolves_and_the_consumer_runs() {
    let world = World::new("crossdag-resolves");
    // The upstream settles first, so its node is genuinely `done` in a ledger
    // the consumer has to go and read.
    settle(&world, "produced", vec![agent("build", &[])]);

    let run = settle(
        &world,
        "consumes",
        vec![consumer("ship", "run:produced#build")],
    );

    assert_eq!(
        status_of(&world, &run, "round-01/result.json", "ship"),
        "done"
    );
    assert_eq!(
        world.run_json(&run, "round-01/result.json")["state"],
        "complete"
    );

    // Resolving is recorded, with how far the upstream had got when it was: the
    // consumer has to know where to measure later movement from.
    let satisfied = world.events_of(&run, "cross-dag-satisfied");
    assert_eq!(satisfied.len(), 1, "{satisfied:?}");
    assert_eq!(satisfied[0]["payload"]["dependency"], "run:produced#build");
    assert_eq!(satisfied[0]["labels"]["node"], "ship");
    assert!(
        satisfied[0]["payload"]["last_seq"].as_u64().is_some(),
        "the baseline records no upstream position: {satisfied:?}"
    );
}

#[test]
fn an_upstream_that_failed_leaves_its_consumer_blocked_rather_than_failing_it() {
    let world = World::new("crossdag-failed");
    world.script("build.fail", "1");
    settle(&world, "broke", vec![agent("build", &[])]);

    let run = settle(&world, "waits", vec![consumer("ship", "run:broke#build")]);

    // Blocked, not failed: the upstream may still be retried by its own planner.
    assert_eq!(
        status_of(&world, &run, "round-01/result.json", "ship"),
        "blocked"
    );
    assert!(
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .all(|event| event["labels"]["node"] != "ship"),
        "a consumer of a failed upstream was dispatched anyway"
    );
    assert!(
        world.events_of(&run, "cross-dag-satisfied").is_empty(),
        "an unresolved edge recorded a baseline"
    );
}

#[test]
fn an_upstream_that_arrives_later_unblocks_the_consumer_in_the_next_round() {
    let world = World::new("crossdag-recovers");
    // Nothing named `arrives` exists yet: an unknown run is exactly the case an
    // unresolved edge waits through.
    let run = settle(
        &world,
        "patient",
        vec![consumer("ship", "run:arrives#build")],
    );
    assert_eq!(
        status_of(&world, &run, "round-01/result.json", "ship"),
        "blocked"
    );

    // The upstream arrives.
    settle(&world, "arrives", vec![agent("build", &[])]);

    // The transition has to open a round for it. A ready check that could not
    // resolve the edge would find nothing ready and park the run for good,
    // which is the run sitting on work that is now perfectly startable.
    world.run(&["round", "next", &run]).exited(0);
    assert!(
        world.run_file(&run, "round-02/plan.json").exists(),
        "no round was opened for an upstream that had arrived"
    );
    world.run(&["round", "run", &run]).exited(0);

    assert_eq!(
        status_of(&world, &run, "round-02/result.json", "ship"),
        "done"
    );
    assert_eq!(
        world.run_json(&run, "round-02/result.json")["state"],
        "complete"
    );
}

#[test]
fn an_upstream_that_moves_after_it_was_recorded_is_reported_without_rerunning_work() {
    let world = World::new("crossdag-modified");
    // The upstream's own run is not finished — its human action is still
    // outstanding — but the node the consumer names is `done`, which is what
    // the edge is about.
    settle(
        &world,
        "moving",
        vec![agent("build", &[]), human("approve", &[])],
    );

    // The consumer's round is held open by a second node, so the upstream can
    // move while the watch is live.
    world.script("late.wait", "hold");
    let path = world.plan(
        "watcher",
        &plan_of(
            "watcher",
            vec![consumer("ship", "run:moving#build"), agent("late", &[])],
        ),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the edge to resolve", |world| {
        !world.events_of("watcher", "cross-dag-satisfied").is_empty()
    });
    let baseline = world.events_of("watcher", "cross-dag-satisfied")[0]["payload"]["last_seq"]
        .as_u64()
        .expect("a baseline");

    // The upstream moves: a person clears the action it was waiting on.
    world.run(&["attest", "moving", "approve"]).exited(0);

    world.until("the movement to be reported", |world| {
        !world.events_of("watcher", "upstream-modified").is_empty()
    });
    let modified = world.events_of("watcher", "upstream-modified");
    assert_eq!(modified[0]["payload"]["dependency"], "run:moving#build");
    assert_eq!(modified[0]["labels"]["node"], "ship");
    assert_eq!(modified[0]["payload"]["captured_last_seq"], json!(baseline));
    assert!(
        modified[0]["payload"]["observed_last_seq"]
            .as_u64()
            .expect("an observed position")
            > baseline,
        "the report does not say the upstream moved: {modified:?}"
    );

    world.release("late.go");
    world.until("the round to finish", |world| {
        !world.events_of("watcher", "round-finished").is_empty()
    });

    // Reported, not re-run: the consumer keeps the work it already did.
    let dispatched: Vec<Value> = world
        .events_of("watcher", "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "ship")
        .collect();
    assert_eq!(
        dispatched.len(),
        1,
        "the watch re-ran the consumer instead of reporting: {dispatched:?}"
    );
    // And it is reported once, not once per reconcile pass.
    assert_eq!(modified.len(), 1, "{modified:?}");
}

#[test]
fn the_recorded_position_survives_the_round_that_recorded_it() {
    let world = World::new("crossdag-durable");
    settle(
        &world,
        "steady",
        vec![agent("build", &[]), human("approve", &[])],
    );

    // The consumer fails, so it is carried into a second round still naming the
    // edge — and that second round is a different process from the one that
    // recorded where the upstream had got.
    world.script("ship.fail", "1");
    let run = settle(
        &world,
        "durable",
        vec![consumer("ship", "run:steady#build")],
    );
    let baseline = world.events_of(&run, "cross-dag-satisfied")[0]["payload"]["last_seq"]
        .as_u64()
        .expect("a baseline");

    // The upstream moves between the two rounds.
    world.run(&["attest", "steady", "approve"]).exited(0);

    world.run(&["round", "next", &run]).exited(0);
    world.run(&["round", "run", &run]);

    // A baseline held only in the round that captured it would be re-captured
    // here — at the upstream's *new* position — and the movement would never be
    // reported by anything.
    let modified = world.events_of(&run, "upstream-modified");
    assert_eq!(modified.len(), 1, "{modified:?}");
    assert_eq!(
        modified[0]["payload"]["captured_last_seq"],
        json!(baseline),
        "the second round measured from a position it captured itself: {modified:?}"
    );
    assert_eq!(
        world.events_of(&run, "cross-dag-satisfied").len(),
        1,
        "the edge recorded a second baseline"
    );
}

#[test]
fn a_malformed_cross_dag_reference_is_refused_by_name() {
    let world = World::new("crossdag-malformed");
    // `run:` without a node is not a graph node id either, so a loader that
    // only checked membership would refuse it as a missing dependency and send
    // the planner looking for a node they never wrote.
    let path = world.plan(
        "malformed",
        &plan_of("malformed", vec![consumer("ship", "run:produced")]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(crate::harness::REFUSED)
        .err_has("run:<run_id>#<node_id>");
}
