//! The read-only views. They render from the merged three-stream event store,
//! take no lock a writer needs, and never call a node running once the ledger
//! has recorded it settled.
//!
//! Ported from `test_monitor_e2e`, `test_monitor_run_plan_e2e`, `test_goals_e2e`, `test_run_views_by_id_e2e`, `test_live_dispatch_views_e2e`, and `test_telemetry_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. There is no alternative today: both sibling crates are at their own
// interface-only stage and refuse every invocation with exit 70. `harness.rs` carries the
// same suppression and the full rationale.

use crate::harness::{agent, human, lifecycle, plan_of, World};

fn settled(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path.to_string_lossy(), "--attach"]);
    world.until("the run to settle", |world| {
        !world.events_of(name, "round-finished").is_empty()
    });
    name.to_string()
}

#[test]
fn monitor_renders_all_three_streams_under_their_own_typed_ids() {
    let world = World::new("views-monitor");
    let run = settled(&world, "watched", vec![lifecycle("service", &[])]);

    let stream = world.run(&["monitor", &run]);
    stream.exited(0);
    // The first line is the contract, not a banner.
    assert!(
        stream.stdout.starts_with("Concise graph events;"),
        "{}",
        stream.stdout
    );
    stream.out_has("graph:service");
    stream.out_has("agent:");
    stream.out_has("vcs:");
    // A round transition has no node, so it has no graph id: it reaches the
    // reader as run state rather than as an event line, naming the run.
    stream.out_has("-- watched  round-01");
}

#[test]
fn monitor_writes_nothing_and_consumes_nothing() {
    let world = World::new("views-readonly");
    world.script("build.wait", "hold");
    let path = world.plan("readonly", &plan_of("readonly", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("a node to be in flight", |world| {
        !world.events_of("readonly", "node-dispatched").is_empty()
    });

    let before = world.journal("readonly").len();
    for view in [
        vec!["monitor", "readonly"],
        vec!["results", "readonly"],
        vec!["status", "readonly"],
        vec!["goals", "readonly"],
        vec!["telemetry", "readonly"],
        vec!["runs"],
        vec!["host"],
    ] {
        world.run(&view).exited(0);
    }
    assert_eq!(
        world.journal("readonly").len(),
        before,
        "a read-only view wrote to the journal"
    );
    // And none of them took the lock the round holds.
    world.release("build.go");
}

#[test]
fn results_reports_each_nodes_own_evidence() {
    let world = World::new("views-results");
    world.script("failing.fail", "1");
    let run = settled(
        &world,
        "evidence",
        vec![
            agent("built", &[]),
            agent("failing", &[]),
            human("approve", &["built"]),
            agent("gated", &["approve"]),
        ],
    );

    let results = world.run(&["results", &run]);
    results
        .exited(0)
        .out_has("built")
        .out_has("done")
        .out_has("failing")
        .out_has("failed")
        .out_has("approve")
        .out_has("waiting")
        .out_has("action:")
        .out_has("unblocks: gated");
}

#[test]
fn goals_says_what_each_run_is_for_and_which_identities_it_holds() {
    let world = World::new("views-goals");
    let run = settled(&world, "purposeful", vec![lifecycle("service", &[])]);

    let goals = world.run(&["goals"]);
    goals
        .exited(0)
        .out_has("Deliver purposeful")
        .out_has("identities: owner/service");
    world
        .run(&["goals", &run])
        .exited(0)
        .out_has("Deliver purposeful");
}

#[test]
fn every_scoped_view_takes_the_run_id_the_launch_record_advertises() {
    let world = World::new("views-byid");
    let run = settled(&world, "addressed", vec![agent("build", &[])]);

    for view in ["monitor", "results", "status", "goals", "telemetry"] {
        world.run(&[view, &run]).exited(0).out_has(&run);
    }
    // Unscoped, the same views cover every run.
    for view in ["status", "goals", "telemetry"] {
        world.run(&[view]).exited(0).out_has(&run);
    }
}

#[test]
fn status_names_a_live_dispatch_and_flags_one_nothing_is_driving() {
    let world = World::new("views-live");
    world.script("build.wait", "hold");
    let path = world.plan("live", &plan_of("live", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the dispatch to be in flight", |world| {
        !world.events_of("live", "node-dispatched").is_empty()
    });

    // Before the dispatch has published anything, the ledger says it is running
    // and nothing is driving it — a positive claim, not a guess.
    world.run(&["status", "live"]).exited(0).out_has("UNDRIVEN");
    world.run(&["host"]).exited(0).out_has("build");

    world.release("build.go");
    world.until("the dispatch to publish", |world| {
        world
            .journal("live")
            .iter()
            .any(|event| event["source"] == "agentgraph")
    });
    world.until("the run to settle", |world| {
        !world.events_of("live", "round-finished").is_empty()
    });

    // Once it has settled, no view calls it running whatever else is true.
    let status = world.run(&["status", "live"]);
    status.exited(0);
    assert!(
        !status.stdout.contains("build: running"),
        "a settled node is still reported running:\n{}",
        status.stdout
    );
    world.run(&["host"]).exited(0).out_has("no live dispatches");
}

#[test]
fn status_carries_the_provider_health_block_from_the_sibling() {
    let world = World::new("views-health");
    let run = settled(&world, "healthy", vec![agent("build", &[])]);
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("providers: fake-provider");
}

#[test]
fn telemetry_buckets_sum_exactly_to_the_wall_clock() {
    let world = World::new("views-telemetry");
    let run = settled(
        &world,
        "timed",
        vec![agent("first", &[]), agent("second", &["first"])],
    );

    let telemetry = world.run(&["telemetry", &run]);
    telemetry.exited(0);
    let document = telemetry.json();
    let wall = document["wall_ms"].as_u64().expect("a wall clock");
    let summed: u64 = document["buckets"]
        .as_array()
        .expect("buckets")
        .iter()
        .map(|bucket| bucket["ms"].as_u64().expect("a bucket"))
        .sum();
    assert_eq!(summed, wall, "the buckets do not sum to WALL: {document}");
    assert_eq!(document["dispatches"], 2);
    assert_eq!(document["settled_done"], 2);

    let breakdown = world.run(&["telemetry", &run, "--breakdown"]);
    breakdown
        .exited(0)
        .out_has("WALL")
        .out_has("dispatching")
        .out_has("orchestrating");
}

#[test]
fn telemetry_counts_a_no_diff_node_without_counting_a_dispatch() {
    let world = World::new("views-nodiff");
    let run = settled(
        &world,
        "counted",
        vec![serde_json::json!({
            "id": "handoff",
            "task": "## What\nNothing changes.",
            "expects_no_diff": true,
        })],
    );

    let document = world.run(&["telemetry", &run]).json();
    assert_eq!(document["no_diff"], 1);
    assert_eq!(document["dispatches"], 0);
    let summed: u64 = document["buckets"]
        .as_array()
        .expect("buckets")
        .iter()
        .map(|bucket| bucket["ms"].as_u64().expect("a bucket"))
        .sum();
    assert_eq!(summed, document["wall_ms"].as_u64().expect("a wall clock"));
}

#[test]
fn runs_summarises_every_recorded_run_and_says_whose_it_is() {
    let world = World::new("views-runs");
    settled(&world, "alpha", vec![agent("build", &[])]);
    settled(&world, "beta", vec![agent("build", &[])]);

    let listing = world.run(&["runs"]);
    listing
        .exited(0)
        .out_has("alpha")
        .out_has("beta")
        .out_has("[mine]")
        .out_has("done");
}

#[test]
fn a_view_of_a_run_with_no_events_still_renders() {
    let world = World::new("views-empty");
    world.script("driver.wait", "hold");
    let path = world.plan("quiet", &plan_of("quiet", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

    for view in ["monitor", "results", "status", "goals", "telemetry"] {
        world.run(&[view, "quiet"]).exited(0);
    }
    world.release("driver.go");
}
