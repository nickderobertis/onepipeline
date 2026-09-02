//! Why a node the loop is not running is not running.
//!
//! `node-ready` says a node's dependencies settled and `node-dispatched` says
//! its dispatch started. Between them there used to be nothing at all — so a node
//! queued behind the operator's own other work, a node whose dependency had not
//! settled, a node a decision point was holding and a node waiting on a release
//! were four different answers that all rendered as the same empty span. These
//! journeys drive each of them and read the record back off a real run store.

use crate::harness::{agent, human, plan_of, World};
use serde_json::{json, Value};

fn held(world: &World, run: &str, node: &str) -> Vec<Value> {
    world
        .events_of(run, "node-held")
        .into_iter()
        .filter(|event| event["labels"]["node"] == node)
        .collect()
}

fn reasons(event: &Value) -> Vec<Value> {
    event["payload"]["reasons"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a hold carries no reasons: {event}"))
}

fn blocking(event: &Value) -> Vec<String> {
    reasons(event)
        .into_iter()
        .find(|reason| reason["kind"] == "dependencies")
        .map(|reason| {
            serde_json::from_value(reason["blocking"].clone()).expect("blocking is a list of ids")
        })
        .unwrap_or_else(|| panic!("no dependency reason on: {event}"))
}

fn plan_running(name: &str, concurrency: u64, nodes: Vec<Value>) -> Value {
    let mut plan = plan_of(name, nodes);
    plan["concurrency"] = json!(concurrency);
    plan
}

/// The brief's own shape: a node behind several running nodes that finish one at
/// a time names the shrinking set, and the last span ends in its dispatch.
///
/// Three successive spans out of one emission rule and no arithmetic per node:
/// the hold is re-stated whenever what is holding it changes, and what is holding
/// it is the dependencies that have not settled yet.
// llmlint: ignore-block[e2e_not_mocked] the layer under test is this crate's reconcile
// loop, driven as the compiled binary over a real run store; what the harness substitutes
// is `oneagentgraph`, a sibling behind its own subprocess boundary, so that a journey can
// hold a dispatch open at a chosen moment without paying for a model turn. Holding it open
// is the whole subject here — a hold is only observable while the node is not running.
#[test]
fn a_node_behind_several_dependencies_names_the_shrinking_set_and_then_dispatches() {
    let world = World::new("held-shrink");
    for node in ["one", "two", "three"] {
        world.script(&format!("{node}.wait"), "hold");
    }
    let plan = world.plan(
        "shrink",
        &plan_of(
            "shrink",
            vec![
                agent("one", &[]),
                agent("two", &[]),
                agent("three", &[]),
                agent("ship", &["one", "two", "three"]),
            ],
        ),
    );
    world.run(&["start", &plan, "--detach"]).exited(0);

    world.until("the first span to open", |world| {
        !held(world, "shrink", "ship").is_empty()
    });
    assert_eq!(
        blocking(&held(&world, "shrink", "ship")[0]),
        vec!["one", "two", "three"],
        "the first span does not name every dependency it is behind"
    );

    for (released, left) in [("one.go", vec!["two", "three"]), ("two.go", vec!["three"])] {
        let spans = held(&world, "shrink", "ship").len();
        world.release(released);
        world.until("the next span to open", |world| {
            held(world, "shrink", "ship").len() > spans
        });
        let spans = held(&world, "shrink", "ship");
        assert_eq!(
            blocking(spans.last().expect("a span")),
            left,
            "the span does not name what is left: {spans:?}"
        );
    }

    // And the last one ends in the dispatch, rather than in another span.
    world.release("three.go");
    world.until("the run to settle", |world| {
        world.run_file("shrink", "result.json").is_file()
    });
    let spans = held(&world, "shrink", "ship");
    assert_eq!(
        spans.iter().map(blocking).collect::<Vec<Vec<String>>>(),
        vec![
            vec!["one", "two", "three"],
            vec!["two", "three"],
            vec!["three"]
        ],
        "the shrinking set is not what the run recorded"
    );
    let unheld = world
        .events_of("shrink", "node-unheld")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "ship")
        .collect::<Vec<Value>>();
    assert_eq!(unheld.len(), 1, "{unheld:?}");
    assert_eq!(
        unheld[0]["payload"]["released"],
        json!([{ "kind": "dependencies", "blocking": ["three"] }]),
        "the hold cleared carrying something other than what was holding it"
    );
    let dispatched = world
        .events_of("shrink", "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "ship")
        .count();
    assert_eq!(dispatched, 1, "the shrinking set did not end in a dispatch");
} // llmlint: ignore-end[e2e_not_mocked]

/// A node the run's concurrency is holding says so once, and says it again only
/// when what is ahead of it changes.
///
/// The loop looks at the channel five times a second and its predecessor
/// reconciled forty times a second. Either would have written a record per look;
/// this asserts the count is the number of transitions.
// llmlint: ignore-block[e2e_not_mocked] the layer under test is this crate's reconcile
// loop, driven as the compiled binary over a real run store; what the harness substitutes
// is `oneagentgraph`, a sibling behind its own subprocess boundary, so that a journey can
// hold a dispatch open at a chosen moment without paying for a model turn. Holding it open
// is the whole subject here — a hold is only observable while the node is not running.
#[test]
fn a_node_the_concurrency_holds_is_reported_once_however_long_it_waits() {
    let world = World::new("held-concurrency");
    world.script("hog.wait", "hold");
    let plan = world.plan(
        "queued",
        &plan_running("queued", 1, vec![agent("hog", &[]), agent("queued", &[])]),
    );
    world.run(&["start", &plan, "--detach"]).exited(0);

    world.until("the queued node to be held", |world| {
        !held(world, "queued", "queued").is_empty()
    });
    assert_eq!(
        reasons(&held(&world, "queued", "queued")[0]),
        vec![json!({ "kind": "concurrency", "ahead": ["hog"], "limit": 1 })],
        "the hold does not name what is ahead of it or the limit it hit"
    );

    // Long enough that a record per pass would be hundreds, and a record per
    // look would be a dozen.
    std::thread::sleep(std::time::Duration::from_secs(3));
    let spans = held(&world, "queued", "queued");
    assert_eq!(
        spans.len(),
        1,
        "the hold was re-stated {} times while nothing about it changed",
        spans.len()
    );

    world.release("hog.go");
    world.until("the run to settle", |world| {
        world.run_file("queued", "result.json").is_file()
    });
    assert_eq!(
        held(&world, "queued", "queued").len(),
        1,
        "the hold was re-stated as it cleared"
    );
    let unheld = world
        .events_of("queued", "node-unheld")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "queued")
        .collect::<Vec<Value>>();
    assert_eq!(unheld.len(), 1, "{unheld:?}");
    assert_eq!(
        unheld[0]["payload"]["released"],
        json!([{ "kind": "concurrency", "ahead": ["hog"], "limit": 1 }])
    );
} // llmlint: ignore-end[e2e_not_mocked]

/// Two reasons at once are two entries of one record, and clearing one of them
/// leaves a record carrying only what still holds.
///
/// The decision entry names the reference and nothing else: what that decision
/// *is* — its kind, and the subtree it holds — stays on `decision-pending`, which
/// is the only account of it.
// llmlint: ignore-block[e2e_not_mocked] the layer under test is this crate's reconcile
// loop, driven as the compiled binary over a real run store; what the harness substitutes
// is `oneagentgraph`, a sibling behind its own subprocess boundary, so that a journey can
// hold a dispatch open at a chosen moment without paying for a model turn. Holding it open
// is the whole subject here — a hold is only observable while the node is not running.
#[test]
fn a_node_held_two_ways_at_once_keeps_the_reason_that_remains() {
    let world = World::new("held-both");
    world.script("build.wait", "hold");
    let plan = world.plan(
        "both",
        &plan_of(
            "both",
            vec![
                human("approve", &[]),
                agent("build", &[]),
                agent("ship", &["approve", "build"]),
            ],
        ),
    );
    world.run(&["start", &plan, "--detach"]).exited(0);

    world.until("both holds to be reported", |world| {
        held(world, "both", "ship")
            .last()
            .is_some_and(|event| reasons(event).len() == 2)
    });
    let both = reasons(held(&world, "both", "ship").last().expect("a hold"));
    assert_eq!(
        both,
        vec![
            json!({ "kind": "dependencies", "blocking": ["approve", "build"] }),
            json!({ "kind": "decision", "reference": "approve" }),
        ],
        "a node held two ways does not carry one entry per reason"
    );
    // The decision entry says which decision and nothing about what it is. The
    // record that owns that detail is beside it and still holds it.
    assert_eq!(
        both[1]
            .as_object()
            .expect("an entry")
            .keys()
            .collect::<Vec<&String>>(),
        vec!["kind", "reference"],
        "the hold copies the decision's own account of itself"
    );
    let pending = world.events_of("both", "decision-pending");
    assert_eq!(pending[0]["payload"]["kind"], json!("attestation"));
    assert_eq!(pending[0]["payload"]["unblocks"], json!(["ship"]));

    // The person takes the action. The decision clears; the dependency the held
    // dispatch is does not.
    let spans = held(&world, "both", "ship").len();
    world.run(&["attest", "both", "approve"]).exited(0);
    world.until("the remaining reason to be reported", |world| {
        held(world, "both", "ship").len() > spans
    });
    assert_eq!(
        reasons(held(&world, "both", "ship").last().expect("a hold")),
        vec![json!({ "kind": "dependencies", "blocking": ["build"] })],
        "clearing one reason did not leave a record carrying only the other"
    );

    world.release("build.go");
    world.until("the run to settle", |world| {
        world.run_file("both", "result.json").is_file()
    });
    assert_eq!(
        world
            .events_of("both", "node-unheld")
            .into_iter()
            .filter(|event| event["labels"]["node"] == "ship")
            .count(),
        1
    );
} // llmlint: ignore-end[e2e_not_mocked]

/// A node nothing stated is holding carries no record at all — not an empty
/// `reasons` array, and not a record saying it is held by nothing.
///
/// Two of those, and they are the two the seam deliberately leaves out: a node
/// that is dispatchable and merely awaiting the pass that starts it, and a human
/// action waiting on the person who has to take it. Neither is this loop
/// declining to run something.
// llmlint: ignore-block[e2e_not_mocked] the layer under test is this crate's reconcile
// loop, driven as the compiled binary over a real run store; what the harness substitutes
// is `oneagentgraph`, a sibling behind its own subprocess boundary, so that a journey can
// hold a dispatch open at a chosen moment without paying for a model turn. Holding it open
// is the whole subject here — a hold is only observable while the node is not running.
#[test]
fn a_dispatchable_node_and_a_waiting_human_action_are_held_by_nothing() {
    let world = World::new("held-nothing");
    let plan = world.plan(
        "free",
        &plan_of("free", vec![agent("solo", &[]), human("approve", &[])]),
    );
    world.run(&["start", &plan, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file("free", "result.json").is_file()
    });

    // The run did what it always did: the agent node ran, the human action is
    // waiting on a person.
    assert_eq!(
        world.run_json("free", "result.json")["state"],
        json!("waiting")
    );
    assert!(
        world
            .events_of("free", "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "solo"),
        "the dispatchable node was not dispatched"
    );
    assert_eq!(
        world.events_of("free", "node-held"),
        Vec::<Value>::new(),
        "a node nothing is holding was reported held"
    );
    assert_eq!(
        world.events_of("free", "node-unheld"),
        Vec::<Value>::new(),
        "a hold that never began was reported cleared"
    );
} // llmlint: ignore-end[e2e_not_mocked]

/// A hold outlives the driver that reported it.
///
/// A fresh driver folds the journal, so it knows what its predecessor was
/// holding: it does not restate a span already open, and it closes one that
/// cleared while nothing was driving — carrying the reasons the record it is
/// answering was written with, rather than reasons of its own.
// llmlint: ignore-block[e2e_not_mocked] the layer under test is this crate's reconcile
// loop, driven as the compiled binary over a real run store; what the harness substitutes
// is `oneagentgraph`, a sibling behind its own subprocess boundary, so that a journey can
// hold a dispatch open at a chosen moment without paying for a model turn. Holding it open
// is the whole subject here — a hold is only observable while the node is not running.
#[test]
fn a_hold_survives_the_driver_that_reported_it() {
    let world = World::new("held-adopted");
    let plan = world.plan(
        "adopted",
        &plan_of(
            "adopted",
            vec![human("approve", &[]), agent("ship", &["approve"])],
        ),
    );
    // Settles waiting on the person, with `ship` held behind them.
    world.run(&["start", &plan, "--attach"]).settled();
    let opened = held(&world, "adopted", "ship");
    assert_eq!(opened.len(), 1, "{opened:?}");
    let reported = opened[0]["payload"]["reasons"].clone();

    // A fresh driver over the same ledger, holding the same node for the same
    // reasons. It says nothing, because nothing changed.
    world.run(&["adopt", "adopted"]).settled();
    assert_eq!(
        held(&world, "adopted", "ship").len(),
        1,
        "an adopted driver restated a hold its predecessor had already reported"
    );
    assert_eq!(
        world.events_of("adopted", "node-unheld"),
        Vec::<Value>::new(),
        "an adopted driver released a hold that was still on"
    );

    // The person takes the action while nothing is driving, and the next driver
    // is the one that reports the release.
    world.run(&["attest", "adopted", "approve"]).exited(0);
    world.run(&["adopt", "adopted"]).settled();
    world.until("the run to settle", |world| {
        world.run_json("adopted", "result.json")["state"] == json!("complete")
    });
    let unheld = world
        .events_of("adopted", "node-unheld")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "ship")
        .collect::<Vec<Value>>();
    assert_eq!(unheld.len(), 1, "{unheld:?}");
    assert_eq!(
        unheld[0]["payload"]["released"], reported,
        "the release names something other than what the record it answers said"
    );
    assert_eq!(
        held(&world, "adopted", "ship").len(),
        1,
        "the hold was restated on its way out"
    );
} // llmlint: ignore-end[e2e_not_mocked]

/// A hold reason a later build wrote is restated rather than taken as understood.
///
/// The fold keeps the producer's own words, so a driver reading a `node-held` a newer
/// build wrote meets a reason it has no variant for. Taking that hold as one it
/// understands would be the expensive mistake: the span would stay open and its release
/// would never be reported, because this driver could not recognise what cleared. So the
/// whole hold reads as one it does not know about and the driver states its own — a
/// duplicate span, which a reader can see, rather than a lost one, which nobody can.
// llmlint: ignore-block[tests_mirror_real_usage] the state under test is a record written
// by a build that is **not** this one, so by construction no interface this build exposes
// can produce it: a reason it had a variant for would not be the case under test. Writing
// the predecessor's line into the ledger is the simulation of the only real producer there
// is — another `onepipeline` — and everything downstream of it is real: the shipped binary
// adopts the run, folds that ledger and writes what it decides. This is the same forward
// compatibility `journal.rs` is built around, where a record from an unknown version is
// skipped and reported rather than refused.
// llmlint: ignore-block[e2e_not_mocked] the layer under test is this crate's reconcile
// loop, driven as the compiled binary over a real run store; what the harness substitutes
// is `oneagentgraph`, a sibling behind its own subprocess boundary, so that a journey can
// hold a dispatch open at a chosen moment without paying for a model turn. Holding it open
// is the whole subject here — a hold is only observable while the node is not running.
#[test]
fn a_hold_reason_written_by_a_later_build_is_restated_rather_than_assumed() {
    let world = World::new("held-unreadable");
    let plan = world.plan(
        "unreadable",
        &plan_of(
            "unreadable",
            vec![human("approve", &[]), agent("ship", &["approve"])],
        ),
    );
    world.run(&["start", &plan, "--attach"]).settled();
    let opened = held(&world, "unreadable", "ship");
    assert_eq!(opened.len(), 1, "{opened:?}");

    // The same record as a later build would have written it: the hold is on, and the
    // only reason it names is one this build has no variant for. Appended while nothing
    // is driving, so the next driver folds it as its predecessor's word.
    let mut future = opened[0].clone();
    future["seq"] = json!(9_000);
    future["payload"] = json!({ "reasons": [{ "kind": "lunar-phase", "phase": "waxing" }] });
    let events = world.run_file("unreadable", "events.jsonl");
    let mut written = std::fs::read_to_string(&events).expect("the journal is readable");
    written.push_str(&format!("{future}\n"));
    std::fs::write(&events, written).expect("a later build's record is appended");

    // A fresh driver over that ledger. It cannot read what is holding `ship`, so it
    // states the hold itself instead of trusting a span it could never close.
    world.run(&["adopt", "unreadable"]).settled();
    let after = held(&world, "unreadable", "ship");
    assert_eq!(
        after.len(),
        3,
        "a driver that could not read the standing hold neither restated it nor said why"
    );
    let restated = reasons(&after[2]);
    assert!(
        restated
            .iter()
            .all(|reason| reason["kind"] != json!("lunar-phase")),
        "the driver echoed a reason it cannot read back: {restated:?}"
    );
    assert_eq!(
        blocking(&after[2]),
        vec!["approve".to_owned()],
        "the hold it stated is not the one it can actually see"
    );
}
// llmlint: ignore-end[e2e_not_mocked]
// llmlint: ignore-end[tests_mirror_real_usage]
