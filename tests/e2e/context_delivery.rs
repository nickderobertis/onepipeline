//! Where a `context` note lands: into the turn that is running now, or onto the
//! node's next dispatch.
//!
//! `context` exists to carry what the planner learned while the round ran to a
//! node that is *still running*, so the journeys here are about the one thing an
//! `edit-committed` cannot tell you on its own — whether the worker that was
//! already working read it. The one that proves live delivery drives it all the
//! way through: the note reaches a held dispatch, and the work that dispatch
//! leaves behind is the work the note asked for.
//!
//! `auto` is the default and therefore the compatibility promise: every
//! `context` edit written before delivery had modes is an `auto` one, and where
//! no controllable turn exists it must still behave exactly as it always did.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The double answers `interrupt` the way the real CLI does — exit 0 when
// a turn is open, its own `EXIT_NO_CONTROLLABLE_TURN` when there is none — and refuses the
// member names and blank redirections the real one refuses, through that library's own
// predicate. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{agent, plan_of, World, REFUSED};
use serde_json::{json, Value};

/// The note a planner writes at the moment it matters: a correction the worker
/// has to act on now, not after it has finished the wrong work.
const NOTE: &str = "the fixture moved to tests/data; stop editing src/old.rs";

fn envelope(commands: Value) -> String {
    json!({"version": 1, "commands": commands}).to_string()
}

/// Start a run whose one node is held open, and wait until its turn is something
/// an interrupt could reach.
///
/// The wait is on `turn-started` rather than on `node-dispatched`: a dispatch
/// that has started has a *process*, and what a live delivery addresses is a
/// turn, which the sibling announces separately. Waiting on the earlier fact
/// would race the address into the reconciler.
fn held(world: &World, name: &str, node: &str) -> String {
    // A turn that is open for as long as the dispatch is held, which is the
    // state a live delivery reaches — a dispatch that has merely started is a
    // different scenario, and the double keeps them apart.
    world.script(&format!("{node}.turn-open"), "");
    world.script(&format!("{node}.wait"), "hold");
    let path = world.plan(name, &plan_of(name, vec![agent(node, &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the held node's turn to open", |world| {
        world
            .events_of(name, "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == node)
    });
    name.to_string()
}

/// How the reconciler recorded the delivery of the one `context` edit committed.
fn delivery(world: &World, run: &str) -> Option<String> {
    world
        .events_of(run, "edit-committed")
        .iter()
        .filter(|event| event["payload"]["command"]["op"] == "context")
        .find_map(|event| {
            event["payload"]["operations"][0]["delivery"]
                .as_str()
                .map(str::to_string)
        })
}

/// The last thing the node's dispatch said it was doing.
fn activity(world: &World, run: &str, node: &str) -> Value {
    world
        .journal(run)
        .into_iter()
        .rfind(|event| {
            event["labels"]["node"] == node
                && event["source"] == "agentgraph"
                && event["kind"] == "turn-activity"
        })
        .expect("the node's dispatch reported what it was doing")
}

/// The journey the whole change exists for: a note written while the worker was
/// working reaches that worker, and the work it leaves behind is different
/// because of it.
#[test]
fn auto_delivers_into_the_running_turn_and_changes_what_that_worker_does() {
    let world = World::new("context-live");
    let run = held(&world, "livenote", "slow");

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "context", "id": "slow", "note": NOTE}])),
        )
        .exited(0)
        .out_has("\"applied\"");

    world.until("the note to be committed", |world| {
        delivery(world, &run).is_some()
    });
    assert_eq!(
        delivery(&world, &run).as_deref(),
        Some("live"),
        "a note aimed at a node with a turn in flight was deferred"
    );

    // The lever was pulled, and the sibling's own record of that reached the
    // merged store, stamped with the node it was about.
    let interrupted = world.events_of(&run, "turn-interrupted");
    assert_eq!(interrupted.len(), 1, "{interrupted:?}");
    assert_eq!(interrupted[0]["payload"]["delivered"], json!(true));
    assert_eq!(interrupted[0]["labels"]["node"], "slow");
    assert!(
        world.was_invoked("oneagentgraph", &["interrupt", "worker", "--input", NOTE]),
        "the note did not go through `oneagentgraph interrupt`: {:?}",
        world.invocations()
    );

    world.release("slow.go");
    world.until("the run to settle", |world| {
        !world.events_of(&run, "round-finished").is_empty()
    });

    // What the *running* turn did with it. The task prose it was dispatched
    // with cannot have carried the note — it was rendered before the note
    // existed — so the redirection is the only way it could have got there.
    let activity = activity(&world, &run, "slow");
    assert_eq!(
        activity["payload"]["redirected"], NOTE,
        "the running turn never saw the note: {activity}"
    );
    let task = activity["payload"]["task"].as_str().expect("task prose");
    assert!(
        !task.contains("## Planner context"),
        "a note delivered live was also rendered into the dispatch's prose: {task}"
    );

    // And the work it left behind is the work the note asked for, which is the
    // difference between a note that was accepted and one that landed.
    let did = std::fs::read_to_string(world.project.join("slow-redirected.md"))
        .expect("the redirected worker left its work behind");
    assert_eq!(did.trim(), NOTE);
}

/// The compatibility promise, driven end to end: a node whose turn has no lever
/// takes the note exactly as `context` always delivered one — onto its next
/// dispatch, rendered as its own section of the task prose.
#[test]
fn auto_defers_to_the_next_dispatch_when_there_is_no_controllable_turn() {
    let world = World::new("context-deferred");
    // A member on a harness with no out-of-band turn control: it runs, and there
    // is nothing to redirect. It fails, so the round after it carries it.
    world.script("slow.no-lever", "");
    world.script("slow.fail", "1");
    let run = held(&world, "defernote", "slow");

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "context", "id": "slow", "note": NOTE}])),
        )
        .exited(0)
        .out_has("\"applied\"");

    world.until("the note to be committed", |world| {
        delivery(world, &run).is_some()
    });
    assert_eq!(
        delivery(&world, &run).as_deref(),
        Some("deferred"),
        "a note nothing could deliver was reported live"
    );
    // The lever was pulled and nothing happened, and the run's own record says
    // so rather than leaving the fall-through invisible.
    let interrupted = world.events_of(&run, "turn-interrupted");
    assert_eq!(interrupted.len(), 1, "{interrupted:?}");
    assert_eq!(interrupted[0]["payload"]["delivered"], json!(false));

    world.release("slow.go");
    world.until("the round to finish", |world| {
        !world.events_of(&run, "round-finished").is_empty()
    });
    // Nothing was redirected, so the turn did what it was originally given.
    assert_eq!(
        activity(&world, &run, "slow")["payload"]["redirected"],
        Value::Null
    );

    world.run(&["round", "next", &run]).exited(0);
    let next = world.run_json(&run, "round-02/plan.json");
    let carried = next["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .find(|node| node["id"] == "slow")
        .expect("the failed node is carried")
        .clone();
    assert_eq!(
        carried["context"], NOTE,
        "the deferred note is not on the node's next dispatch: {carried}"
    );
}

/// A planner who needs the correction *now* is told when it could not have it,
/// rather than being deferred without knowing.
#[test]
fn live_refuses_with_a_reason_when_the_note_cannot_reach_a_running_turn() {
    let world = World::new("context-live-refused");
    world.script("slow.no-lever", "");
    let run = held(&world, "liverefuse", "slow");

    // A turn that is running and has no lever: the interrupt is really pulled,
    // and its exit 3 is what the refusal is made of.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "context", "id": "slow", "note": NOTE, "deliver": "live"}])),
        )
        .exited(REFUSED)
        .err_has("no controllable turn in flight");

    // Nothing was committed, so the note is not quietly waiting on the next
    // dispatch either — a refused edit never reaches the graph.
    assert_eq!(delivery(&world, &run), None);
    let interrupted = world.events_of(&run, "turn-interrupted");
    assert_eq!(interrupted.len(), 1, "{interrupted:?}");
    assert_eq!(interrupted[0]["payload"]["delivered"], json!(false));

    world.release("slow.go");
}

/// A delivery that was attempted and *broke* is neither of the other two
/// answers, and neither mode hides it: a planner told `deferred` when the truth
/// is that the lever failed has been told something that is not so.
#[test]
fn a_delivery_that_fails_is_refused_under_every_mode_that_asks_for_one() {
    let world = World::new("context-lever-broken");
    world.script("interrupt.fail", "");
    let run = held(&world, "leverbroke", "slow");

    for mode in [json!("auto"), json!("live")] {
        let reply = world.run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "context", "id": "slow", "note": NOTE, "deliver": mode}])),
        );
        reply.exited(REFUSED);
        reply.err_has("delivering the note to node 'slow' failed");
        reply.err_has("the control socket refused");
    }
    assert_eq!(
        delivery(&world, &run),
        None,
        "a note whose delivery broke was committed anyway"
    );

    world.release("slow.go");
}

/// `next` is today's behaviour spelled out, and it does not reach for the lever
/// even when there is one to reach for.
#[test]
fn next_leaves_a_running_turn_alone_and_a_bad_mode_is_refused() {
    let world = World::new("context-next");
    let run = held(&world, "nextnote", "slow");

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "context", "id": "slow", "note": NOTE, "deliver": "next"}])),
        )
        .exited(0);
    world.until("the note to be committed", |world| {
        delivery(world, &run).is_some()
    });
    assert_eq!(delivery(&world, &run).as_deref(), Some("deferred"));
    assert!(
        !world.was_invoked("oneagentgraph", &["interrupt"]),
        "`next` reached for the lever anyway: {:?}",
        world.invocations()
    );

    // A mode outside the three is refused at the boundary, naming what it read.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(
                json!([{"op": "context", "id": "slow", "note": NOTE, "deliver": "eventually"}]),
            ),
        )
        .exited(REFUSED)
        .err_has("eventually");

    world.release("slow.go");
    world.until("the run to settle", |world| {
        !world.events_of(&run, "round-finished").is_empty()
    });
    assert_eq!(
        activity(&world, &run, "slow")["payload"]["redirected"],
        Value::Null,
        "`next` still reached the running turn"
    );
}
