//! The read-only views. They render from the merged three-stream event store,
//! take no lock a writer needs, and never call a node running once the ledger
//! has recorded it settled.
//!
//! Ported from `test_monitor_e2e`, `test_monitor_run_plan_e2e`, `test_goals_e2e`, `test_run_views_by_id_e2e`, `test_live_dispatch_views_e2e`, and `test_telemetry_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The scenario this journey states is one a real sibling would need paid
// model turns to produce, and `dispatch.rs` is where the real `oneagentgraph` binary is
// driven instead. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{agent, human, lifecycle, plan_of, World};

/// How long a held publication phase is kept open, so its bucket is a real
/// duration on the clock rather than a bucket that merely exists.
const HELD: std::time::Duration = std::time::Duration::from_millis(400);

/// The floor a held stretch must clear once it has been measured. Below the
/// hold, because the two records bracketing it are written either side of the
/// rendezvous rather than exactly on it.
const FLOOR: u64 = 250;

fn settled(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
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

/// Every measured bucket, summed. An unmeasured one carries no `ms` at all,
/// which is the point: a zero there would read as a measurement.
fn measured(document: &serde_json::Value) -> u64 {
    document["buckets"]
        .as_array()
        .expect("buckets")
        .iter()
        .filter_map(|bucket| bucket.get("ms").and_then(serde_json::Value::as_u64))
        .sum()
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
    assert_eq!(
        measured(&document),
        wall,
        "the buckets do not sum to WALL: {document}"
    );
    assert_eq!(document["schema_version"], 2, "{document}");
    assert_eq!(document["dispatches"], 2);
    assert_eq!(document["settled_done"], 2);

    // Eight buckets, and the two nothing in this stack measures are served
    // absent rather than as a zero that reads as measured.
    let named: Vec<&str> = document["buckets"]
        .as_array()
        .expect("buckets")
        .iter()
        .map(|bucket| bucket["name"].as_str().expect("a bucket name"))
        .collect();
    assert_eq!(
        named,
        vec![
            "agent",
            "judge",
            "llmlint",
            "gate",
            "publication_wait",
            "lock_wait",
            "setup",
            "scheduling",
        ],
        "{document}"
    );
    for absent in ["judge", "llmlint"] {
        let bucket = document["buckets"]
            .as_array()
            .expect("buckets")
            .iter()
            .find(|bucket| bucket["name"] == absent)
            .expect("the bucket is still named");
        assert!(
            bucket.get("ms").is_none(),
            "{absent} reported a measured span nothing produces: {bucket}"
        );
    }

    let breakdown = world.run(&["telemetry", &run, "--breakdown"]);
    breakdown
        .exited(0)
        .out_has("WALL")
        .out_has("agent")
        .out_has("scheduling")
        .out_has("not measured");
}

/// The number a run is budgeted against, on a host whose routine failure mode
/// is quota exhaustion across five identities.
#[test]
fn telemetry_reports_what_each_party_spent() {
    let world = World::new("views-usage");
    let run = settled(&world, "spent", vec![agent("build", &[])]);

    let document = world.run(&["telemetry", &run]).json();
    let usage = &document["usage"];
    let total = &usage["total"];
    assert!(
        total["input"].as_u64().is_some_and(|tokens| tokens > 0),
        "the run reported no input tokens: {document}"
    );
    assert!(
        total["output"].as_u64().is_some_and(|tokens| tokens > 0),
        "the run reported no output tokens: {document}"
    );
    assert!(total["cost_usd"].as_f64().is_some(), "{document}");

    // The split between the sides of a two-party member is in the report it
    // settled with, which is where it is read from.
    assert!(
        usage["agent"]["input"].as_u64().is_some_and(|t| t > 0),
        "the agent side reported nothing: {document}"
    );
    assert!(
        usage["judge"]["input"].as_u64().is_some_and(|t| t > 0),
        "the judge side reported nothing: {document}"
    );
    // Nothing in this stack runs an LLM-lint pass, so it is absent rather than
    // present and zero.
    assert!(usage.get("llmlint").is_none(), "{document}");

    world
        .run(&["telemetry", &run, "--breakdown"])
        .exited(0)
        .out_has("usage agent")
        .out_has("usage total");
}

/// A gate run and a lock wait are the two stretches an operator most needs
/// answered apart from the agent's — and a lifecycle node spends real time in
/// both.
#[test]
fn telemetry_separates_gate_and_lock_time_from_agent_time() {
    let world = World::new("views-gatetime");
    // Both stretches are held open for a measurable span, so each is a real
    // duration on the clock rather than a bucket that merely exists.
    world.script("publish.hold", "hold");
    world.script("gate.hold", "hold");
    let path = world.plan("gated", &plan_of("gated", vec![lifecycle("service", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

    for (waited, release, held) in [
        ("lock-wait", "publish.go", "the lock wait"),
        ("gate-started", "gate.go", "the gate"),
    ] {
        world.until(&format!("{held} to start"), |world| {
            !world.events_of("gated", waited).is_empty()
        });
        let since = std::time::Instant::now();
        world.until(&format!("{held} to last a measurable stretch"), |_| {
            since.elapsed() >= HELD
        });
        world.release(release);
    }
    world.until("the run to settle", |world| {
        !world.events_of("gated", "round-finished").is_empty()
    });

    let document = world.run(&["telemetry", "gated"]).json();
    let span = |name: &str| {
        document["buckets"]
            .as_array()
            .expect("buckets")
            .iter()
            .find(|bucket| bucket["name"] == name)
            .unwrap_or_else(|| panic!("a {name} bucket"))
            .get("ms")
            .and_then(serde_json::Value::as_u64)
    };
    // Each held stretch is its own bucket, and neither is inside the agent's:
    // the whole point of the eight-way split.
    for name in ["lock_wait", "gate"] {
        let ms = span(name).unwrap_or_else(|| panic!("{name} is unmeasured: {document}"));
        assert!(
            ms >= FLOOR,
            "{name} measured {ms}ms of a stretch held for {}ms: {document}",
            HELD.as_millis()
        );
    }
    let agent = span("agent").expect("the agent bucket is measured");
    assert!(
        agent < span("gate").expect("gate") + span("lock_wait").expect("lock_wait"),
        "the held stretches were charged to the agent: {document}"
    );
    for name in ["publication_wait", "setup"] {
        assert!(
            span(name).is_some(),
            "{name} is unmeasured for a run that published: {document}"
        );
    }
    assert_eq!(
        measured(&document),
        document["wall_ms"].as_u64().expect("a wall clock"),
        "{document}"
    );
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
    assert_eq!(
        measured(&document),
        document["wall_ms"].as_u64().expect("a wall clock")
    );
    // A run that dispatched nothing spent nothing, and says so by absence.
    assert!(
        document["usage"].as_object().is_some_and(|u| u.is_empty()),
        "{document}"
    );
}

/// A dispatch whose retained report this host cannot reach.
///
/// The settlement still names where the report went, so the evidence is missing
/// rather than the result — and every view that would have read it says which
/// of the two it is meeting instead of reporting a dispatch that did nothing.
#[test]
fn evidence_this_host_cannot_read_is_reported_as_unread_rather_than_as_nothing() {
    let world = World::new("views-unread");
    world.script("report.missing", "");
    let run = settled(&world, "unread", vec![agent("build", &[])]);

    let transcript = world.run(&["transcript", &run]);
    transcript
        .exited(0)
        .out_has("unread  build")
        // The turn's tools are in the merged store and still render.
        .out_has("tool_call bash")
        // The words are not, and the line says so rather than omitting the
        // report it cannot read.
        .out_has("unreadable from this host");

    // Same fact in the telemetry: the member's total is on the wire, and the
    // split between its two sides was only ever in the report.
    let usage = world.run(&["telemetry", &run]).json()["usage"].clone();
    assert!(
        usage["total"]["input"].as_u64().is_some_and(|t| t > 0),
        "{usage}"
    );
    assert!(
        usage.get("agent").is_none() && usage.get("judge").is_none(),
        "an unreadable report was reported as a measured split: {usage}"
    );
}

/// A run whose driver has not dispatched anything yet has no transcript, and
/// says so rather than rendering an empty one.
#[test]
fn a_run_that_has_dispatched_nothing_says_it_has_no_transcript() {
    let world = World::new("views-notranscript");
    world.script("driver.wait", "hold");
    let path = world.plan("quiet", &plan_of("quiet", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

    world
        .run(&["transcript", "quiet"])
        .exited(0)
        .out_has("no dispatch has recorded a transcript");
    world.release("driver.go");
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
