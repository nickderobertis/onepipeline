//! The lever that binds a node's judge, driven through the real reply path.
//!
//! `amend` exists because `context` does not do this. A note steers the worker
//! for one dispatch and says of itself that it adds no acceptance criteria; a
//! manager who ruled mid-dispatch had nothing that reached the **judge**, and
//! that node's own judge overturned the ruling from a task that never mentioned
//! it. So the journeys here are about the one thing an accepted edit cannot tell
//! you on its own: what the dispatch was actually handed, on the dispatch after
//! the amendment and on every later one.
//!
//! What a node's dispatch is handed is one `--task`, and `oneagentgraph` gives
//! that one task to the worker and to the judge supervising it alike — which is
//! what `dispatch.rs`'s
//! `every_dag_scope_member_is_given_the_runs_description_and_its_own_job` proves
//! against the real sibling. So the task each dispatch recorded is where these
//! journeys read the bar from.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. What each dispatch was handed is read off the turn the double actually
// ran, never off anything this crate wrote down about it. `harness.rs` carries the same
// suppression and the full rationale.

use crate::harness::{agent, plan_of, World, REFUSED};
use serde_json::{json, Value};

/// The ruling a manager issues mid-dispatch, and the correction that replaces it.
const RULING: &str = "The four redundant comment lines are out of scope for this node: leave them.";
const CORRECTION: &str = "Restore the four comment lines after all; the reviewer asked for them.";

fn envelope(commands: Value) -> String {
    json!({"version": 1, "commands": commands}).to_string()
}

/// The task prose each of a node's dispatches was given, in dispatch order.
fn tasks_dispatched(world: &World, run: &str, node: &str) -> Vec<String> {
    world
        .journal(run)
        .into_iter()
        .filter(|event| {
            event["labels"]["node"] == node
                && event["source"] == "agentgraph"
                && event["kind"] == "turn-activity"
        })
        .filter_map(|event| event["payload"]["task"].as_str().map(str::to_string))
        .collect()
}

/// Start a run with a node held open, so an edit arrives while work is in
/// flight, beside a node that has not been dispatched at all.
fn held_beside_a_pending_node(world: &World, name: &str, extra: Vec<Value>) -> String {
    world.script("slow.turn-open", "");
    world.script("slow.wait", "hold");
    let mut nodes = vec![agent("slow", &[]), agent("later", &["slow"])];
    nodes.extend(extra);
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the held node's turn to open", |world| {
        !world.events_of(name, "turn-started").is_empty()
    });
    name.to_string()
}

/// The journey the op exists for: a ruling reaches the task the node is judged
/// against, it is rendered where a ruling has to be, and a later ruling replaces
/// it rather than sitting beside it.
#[test]
fn an_amendment_binds_every_later_dispatch_and_a_second_one_replaces_it() {
    let world = World::new("amend-binds");
    // `later` has not been dispatched, so the amendment lands before its first
    // dispatch; `keep` stays in flight throughout, so the loop is still running
    // when the requeue below asks for a second dispatch of `later`.
    world.script("keep.wait", "hold");
    let run = held_beside_a_pending_node(&world, "amendbinds", vec![agent("keep", &[])]);
    world.script("later.wait", "hold");

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "later", "text": RULING}])),
        )
        .exited(0)
        .out_has("\"applied\"");
    world.until("the amendment to be committed", |world| {
        world
            .events_of(&run, "edit-committed")
            .iter()
            .any(|event| event["payload"]["command"]["op"] == "amend")
    });

    // It is journalled as an operation of its own, so replay reconstructs the
    // amended task without re-judging the amendment.
    let committed = world.events_of(&run, "edit-committed");
    let operations = committed
        .iter()
        .find(|event| event["payload"]["command"]["op"] == "amend")
        .map(|event| event["payload"]["operations"].clone())
        .expect("the amend was committed");
    assert_eq!(operations[0]["kind"], "task-amended", "{operations}");
    assert_eq!(operations[0]["text"], RULING, "{operations}");

    // A manager about to replace it can read what they are replacing, from
    // either view.
    for verb in ["status", "results"] {
        world
            .run(&[verb, &run])
            .exited(0)
            .out_has(RULING)
            .out_has("amend");
    }

    // The dependency settles, so the amended node dispatches — carrying the
    // ruling, in the one task its worker and its judge are both handed.
    world.release("slow.go");
    world.until("the amended node to be dispatched", |world| {
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "later")
    });

    // Parked and brought back, so the node is dispatched a second time: what a
    // note would have been consumed by, and what an amendment must survive.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "cancel", "id": "later"}])),
        )
        .exited(0);
    world.release("later.go");
    world.until("the parked node to settle", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "later")
    });
    let first = tasks_dispatched(&world, &run, "later");
    assert!(
        first[0].contains(RULING),
        "the ruling did not reach the node's dispatch: {}",
        first[0]
    );
    assert!(
        first[0].contains("## Amendment")
            && first[0].contains(
                "Where this section and the operational notes below disagree, this section wins."
            ),
        "the ruling reached the task without its authority: {}",
        first[0]
    );
    // And it is not disclaiming itself the way a carried note does — which is
    // the whole difference between the two levers.
    assert!(
        !first[0].contains("adds no acceptance criteria"),
        "the ruling was rendered as an observation: {}",
        first[0]
    );

    // The correction goes out before the node runs again, so what the second
    // dispatch is judged against is the ruling that replaced the first.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "later", "text": CORRECTION}])),
        )
        .exited(0)
        .out_has("\"applied\"");
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "requeue", "id": "later"}])),
        )
        .exited(0);
    world.until("the requeued node to be dispatched again", |world| {
        tasks_dispatched(world, &run, "later").len() >= 2
    });

    let both = tasks_dispatched(&world, &run, "later");
    assert!(
        both[1].contains(CORRECTION),
        "the replacing ruling did not reach the later dispatch: {}",
        both[1]
    );
    assert!(
        !both[1].contains(RULING),
        "the replaced ruling is still binding the judge beside its own correction: {}",
        both[1]
    );
    // And the view a manager reads says the same thing the dispatch was given.
    world
        .run(&["results", &run])
        .exited(0)
        .out_has(CORRECTION)
        .out_lacks(RULING);
    world.release("keep.go");
}

/// The three ways an amendment reaches nobody, each refused by the one it was —
/// and the graph left exactly as it was.
#[test]
fn an_amendment_nothing_will_read_is_refused_by_the_reason_it_was() {
    let world = World::new("amend-refused");
    let run = held_beside_a_pending_node(&world, "amendrefused", Vec::new());

    // A node the graph does not hold.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "nowhere", "text": RULING}])),
        )
        .exited(REFUSED)
        .err_has("no node 'nowhere'");

    // Blank text: a bar nobody can clear is refused rather than recorded as one.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "later", "text": "   \n"}])),
        )
        .exited(REFUSED)
        .err_has("cannot be blank");

    // A node that has settled done, which is the one nothing will ever read.
    world.release("slow.go");
    world.until("the first node to settle done", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "slow" && event["payload"]["status"] == "done")
    });
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "slow", "text": RULING}])),
        )
        .exited(REFUSED)
        .err_has("settled done");

    // Nothing was committed by any of the three, so no node is quietly carrying
    // a bar the reply said it refused.
    assert!(
        world
            .events_of(&run, "edit-committed")
            .iter()
            .all(|event| event["payload"]["command"]["op"] != "amend"),
        "a refused amendment reached the graph: {:?}",
        world.events_of(&run, "edit-committed")
    );
    world.run(&["results", &run]).exited(0).out_lacks(RULING);
}

/// An observer may not move a bar, and the refusal names the op it refused.
///
/// What a node is judged against is a decomposition decision the monitor's own
/// persona already reserves to the planner — and an observer that could move a
/// bar could resolve an ambiguity by editing rather than by escalating.
#[test]
fn a_monitor_is_refused_amend_by_name_and_told_what_to_do_instead() {
    let world = World::new("amend-monitor");
    let run = held_beside_a_pending_node(&world, "amendmonitor", Vec::new());

    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "version": 1,
                "author": "monitor",
                "commands": [{"op": "amend", "id": "later", "text": RULING}],
            })
            .to_string(),
        )
        .exited(REFUSED)
        .err_has("'amend' is not an op the monitor may issue")
        .err_has("Surface it to the planner instead");

    // The lever it *does* have goes through, so what is refused is the authority
    // rather than the author.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "version": 1,
                "author": "monitor",
                "commands": [{"op": "context", "id": "later", "note": "the fixture moved"}],
            })
            .to_string(),
        )
        .exited(0);
    world.release("slow.go");
}
