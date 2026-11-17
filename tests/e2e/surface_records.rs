//! What a run's journal says of each surface, read the way a browser view or an agent
//! reads a run's history: `planner-surface-queued`, `planner-surfaced` and
//! `planner-replied` joined by the surface's channel id and correlation, and labelled with
//! the node a surface is about.
//!
//! Every surface below is raised through the compiled binary's own verbs — `surface`,
//! `reply` carrying a finding, and `ask` — handed out by `next`, and answered by `reply`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as the real
// compiled binary against a real run store and the real linked `onemessagebus`;
// `harness.rs` carries the full rationale.

use serde_json::{json, Value};

use crate::ask::{asking, question_on, waited};
use crate::harness::{human, plan_of, World};

/// The one `kind` record of `run` whose payload's `field` is `value`.
fn record(world: &World, run: &str, kind: &str, field: &str, value: &Value) -> Value {
    let found: Vec<Value> = world
        .events_of(run, kind)
        .into_iter()
        .filter(|event| &event["payload"][field] == value)
        .collect();
    assert_eq!(found.len(), 1, "{kind} with {field} = {value}: {found:?}");
    found[0].clone()
}

/// A surface raised by `surface`, one raised about a node by a finding, and a question
/// `ask` raised about a node each leave one `planner-surface-queued` naming its channel id
/// and its correlation, labelled with the node where it is about one; `next` handing each
/// out leaves a `planner-surfaced` under the same id; and the reply to the question leaves a
/// `planner-replied` naming the question's correlation.
#[test]
fn each_surface_its_hand_out_and_its_reply_are_joined_by_its_id_and_correlation() {
    const RUN: &str = "surfacejoin";
    let world = World::new("surface-records");
    // A human node, so the attached launch returns awaiting the planner with nothing
    // driving the run, and there is a node for a surface to be about.
    let path = world.plan(RUN, &plan_of(RUN, vec![human("approve", &[])]));
    world
        .run(&["start", &path, "--attach"])
        .exited(0)
        .out_has("\"settlement\":\"awaiting-planner\"");

    let raised = world.run(&["surface", RUN, "--kind", "finding", "--message", "noted"]);
    raised.exited(0);
    let id = raised.json()["surface"].clone();
    let queued = record(
        &world,
        RUN,
        "planner-surface-queued",
        "message",
        &json!("noted"),
    );
    assert_eq!(queued["payload"]["surface"], id, "{queued}");
    assert_eq!(queued["payload"]["correlation"], Value::Null, "{queued}");
    assert!(queued["labels"].get("node").is_none(), "{queued}");

    world
        .run_with_stdin(
            &["reply", RUN],
            &json!({"version": 3, "commands": [
                {"op": "finding", "id": "approve", "message": "the approval has waited"}
            ]})
            .to_string(),
        )
        .exited(0);
    let finding = record(
        &world,
        RUN,
        "planner-surface-queued",
        "message",
        &json!("the approval has waited"),
    );
    assert_eq!(finding["labels"]["node"], "approve", "{finding}");
    assert_eq!(finding["payload"]["correlation"], Value::Null, "{finding}");
    assert!(finding["payload"]["surface"].is_u64(), "{finding}");
    assert_ne!(finding["payload"]["surface"], id, "two surfaces, one id");

    // Before this, a question that reached the channel left nothing in the run's history.
    let asked = asking(&world, RUN, &["--about", "approve", "Ship it today?"], &[]);
    let question = question_on(&world, RUN);
    let queued = record(
        &world,
        RUN,
        "planner-surface-queued",
        "kind",
        &json!("planner-question"),
    );
    assert_eq!(queued["payload"]["surface"], question["id"], "{queued}");
    assert_eq!(queued["payload"]["correlation"], question["correlation"]);
    assert_eq!(queued["labels"]["node"], "approve", "{queued}");
    assert_eq!(queued["payload"]["message"], "Ship it today?");
    assert_eq!(queued["payload"]["blocking"], true);

    // The question is handed out first because it blocks; `next` hands out the other two
    // after it, each under the id it was queued with.
    while !world.run(&["next", RUN]).json()["surface"].is_null() {}
    for queued in world.events_of(RUN, "planner-surface-queued") {
        let surfaced = record(
            &world,
            RUN,
            "planner-surfaced",
            "surface",
            &queued["payload"]["surface"],
        );
        assert_eq!(surfaced["payload"]["kind"], queued["payload"]["kind"]);
        assert_eq!(
            surfaced["payload"]["correlation"],
            queued["payload"]["correlation"]
        );
        assert_eq!(surfaced["labels"]["node"], queued["labels"]["node"]);
    }

    let correlation = question["correlation"].as_str().expect("a correlation");
    world
        .run_with_stdin(
            &["reply", RUN, "--correlation", correlation],
            &json!({"version": 2, "completion": false, "message": "yes"}).to_string(),
        )
        .exited(0);
    let answered = waited(asked);
    assert_eq!(answered.code, 0, "{}", answered.stderr);
    let replied = record(
        &world,
        RUN,
        "planner-replied",
        "correlation",
        &json!(correlation),
    );
    assert_eq!(replied["payload"]["message"], "yes", "{replied}");
}
