//! What a supervisor's intervention does to a live dispatch.
//!
//! A `cancel` is the lever a planner reaches for when a worker has gone wrong,
//! and for as long as it only raised a flag nobody read it was the most
//! expensive lie in this crate: a cancelled node kept committing for
//! three-quarters of an hour while every intervention appeared to land. So the
//! journeys here are about what *stops*, and they drive the whole of it — the
//! ask, what the worker did with it, the deadline, and the teardown.
//!
//! The requeue half is the same intervention seen from the other side. A cancel
//! parks the node and asks its dispatch to stop; until that dispatch settles it
//! still holds the node's workspace, so a requeue accepted in between returns
//! the node to a frontier it cannot leave.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The double answers `interrupt` the way the real CLI does — exit 0 when
// a turn is open, its own `EXIT_NO_CONTROLLABLE_TURN` when there is none — and the worker
// it acts out either takes the redirection or ignores it, which is the distinction these
// journeys are about. `dispatch.rs` drives the same cancellation through the *real*
// sibling. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{agent, plan_of, World, CANCEL_GRACE_ENV, REFUSED};
use serde_json::{json, Value};

/// A deadline short enough for a journey to wait through.
const SHORT_GRACE: &str = "1";

fn envelope(commands: Value) -> String {
    json!({"version": 1, "commands": commands}).to_string()
}

/// Start a run whose node is held open, and wait until its turn is something an
/// interrupt could reach.
///
/// `grace` is the deadline that run's driver carries, set on the launching
/// command so the driver it retains inherits it. The wait is on `turn-started`
/// rather than on `node-dispatched`: a dispatch that has started has a
/// *process*, and what a cancellation addresses is a turn.
fn held(world: &World, name: &str, grace: &str) -> String {
    world.script("slow.turn-open", "");
    world.script("slow.wait", "hold");
    let path = world.plan(name, &plan_of(name, vec![agent("slow", &[])]));
    let mut launch = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
    launch.env(CANCEL_GRACE_ENV, grace);
    world.run_on(launch, "start --detach").exited(0);
    world.until("the held node's turn to open", |world| {
        !world.events_of(name, "turn-started").is_empty()
    });
    name.to_string()
}

/// Cancel one node, and wait for the reconciler to commit the edit.
fn cancel(world: &World, run: &str, node: &str) {
    world
        .run_with_stdin(
            &["reply", run],
            &envelope(json!([{"op": "cancel", "id": node}])),
        )
        .exited(0);
    world.until("the cancel to be committed", |world| {
        world
            .events_of(run, "edit-committed")
            .iter()
            .any(|event| event["payload"]["command"]["op"] == "cancel")
    });
}

/// The planner surfaces of one kind this run raised.
fn surfaces(world: &World, run: &str, kind: &str) -> Vec<Value> {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == kind)
        .collect()
}

/// How one node settled, once it has.
fn settlement(world: &World, run: &str, node: &str) -> Value {
    world.until("the cancelled node to settle", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == node)
    });
    world
        .events_of(run, "node-settled")
        .into_iter()
        .find(|event| event["labels"]["node"] == node)
        .expect("the settlement was just seen")
}

/// The redirection the worker's running turn actually read, if it read one.
fn redirected(world: &World, run: &str, node: &str) -> Value {
    world
        .journal(run)
        .into_iter()
        .rfind(|event| {
            event["labels"]["node"] == node
                && event["source"] == "agentgraph"
                && event["kind"] == "turn-activity"
        })
        .map(|event| event["payload"]["redirected"].clone())
        .unwrap_or(Value::Null)
}

/// The journey the whole change exists for: a cancel reaches the running turn,
/// and the worker stops because of it.
///
/// Every link is asserted, because the defect was that all of them looked
/// present and none of them connected: the lever was pulled, the redirection
/// carried the instruction that makes a cooperative stop worth asking for, the
/// worker read it, it ended without being reaped, and the settlement says which
/// of the two happened.
#[test]
fn a_cancel_asks_the_running_turn_to_stop_and_the_worker_stops() {
    let world = World::new("cancel-stops");
    // A worker that takes the ask: the redirection an interrupt delivers ends
    // its turn, which is what a turn stopping on its own looks like.
    world.script("slow.stops-when-interrupted", "");
    let run = held(&world, "cancelstops", SHORT_GRACE);

    cancel(&world, &run, "slow");

    // The lever was pulled, through the sibling's own verb.
    world.until("the interrupt to be recorded", |world| {
        !world.events_of(&run, "turn-interrupted").is_empty()
    });
    let interrupted = world.events_of(&run, "turn-interrupted");
    assert_eq!(interrupted.len(), 1, "{interrupted:?}");
    assert_eq!(interrupted[0]["payload"]["delivered"], json!(true));
    assert_eq!(
        interrupted[0]["labels"]["node"], "slow",
        "the envelope is not stamped with the node it is about: {}",
        interrupted[0]
    );

    // It stopped on its own, so nothing was reaped — and the settlement says so,
    // which is the difference a supervisor acts on. Waited for before the
    // redirection is read: what the turn *did* with the ask is reported by that
    // turn, and it has not reported anything until it has ended.
    let settled = settlement(&world, &run, "slow");
    assert_eq!(settled["payload"]["status"], "cancelled", "{settled}");
    let detail = settled["payload"]["detail"]
        .as_str()
        .unwrap_or_else(|| panic!("a cancelled node says nothing about how it stopped: {settled}"));
    assert!(
        detail.contains("stopped after its turn was asked"),
        "the settlement does not say the dispatch stopped when it was asked: {detail}"
    );

    // And what the ask carried is what makes it worth more than killing: stop,
    // commit, and end the turn without starting anything else. Read off what
    // the turn reported it was doing, so this is the worker's own account of
    // having taken it rather than the run's account of having sent it.
    let asked = redirected(&world, &run, "slow");
    let asked = asked
        .as_str()
        .unwrap_or_else(|| panic!("the running turn never read a redirection: {asked}"));
    for instruction in [
        "Stop this task now",
        "Do not start any new work",
        "Commit anything",
        "end your turn",
    ] {
        assert!(
            asked.contains(instruction),
            "the cancellation's redirection does not ask the turn to {instruction:?}: {asked}"
        );
    }

    // Both transitions are the planner's to read, and only the first happened.
    let asked_for = surfaces(&world, &run, "dispatch-interrupted");
    assert_eq!(asked_for.len(), 1, "{asked_for:?}");
    assert!(
        asked_for[0]["payload"]["message"]
            .as_str()
            .is_some_and(|said| said.contains("the running turn took the redirection")),
        "the surface does not say what the delivery answered: {}",
        asked_for[0]
    );
    assert!(
        surfaces(&world, &run, "dispatch-killed").is_empty(),
        "a dispatch that stopped when it was asked was reported as one that had to be reaped"
    );
}

/// The other half of the escalation: a worker that ignores the ask is reaped at
/// the deadline, and the two endings are told apart.
///
/// The deadline must not depend on the dispatch saying anything, which is why
/// this one is held open and silent from the moment it is asked: the loop's own
/// clock is what expires.
#[test]
fn a_dispatch_that_ignores_the_ask_is_killed_at_the_deadline() {
    let world = World::new("cancel-killed");
    // Nothing scripts a worker that stops, so the hold outlasts the ask.
    let run = held(&world, "cancelkilled", SHORT_GRACE);

    cancel(&world, &run, "slow");

    // It was asked first — a kill that skipped the ask would lose whatever the
    // turn had not committed for no reason.
    world.until("the interrupt to be recorded", |world| {
        !world.events_of(&run, "turn-interrupted").is_empty()
    });
    assert_eq!(
        surfaces(&world, &run, "dispatch-interrupted").len(),
        1,
        "the cancellation did not report what it asked for"
    );

    world.until("the deadline to expire", |world| {
        !surfaces(world, &run, "dispatch-killed").is_empty()
    });
    let killed = surfaces(&world, &run, "dispatch-killed");
    let message = killed[0]["payload"]["message"]
        .as_str()
        .expect("the surface says what happened");
    assert!(
        message.contains("was killed") && message.contains("reaped"),
        "the escalation does not say the dispatch was torn down: {message}"
    );
    assert_eq!(
        killed[0]["labels"]["node"], "slow",
        "the escalation is not raised against the node it ended: {}",
        killed[0]
    );

    let settled = settlement(&world, &run, "slow");
    assert_eq!(settled["payload"]["status"], "cancelled", "{settled}");
    let detail = settled["payload"]["detail"]
        .as_str()
        .unwrap_or_else(|| panic!("a reaped node says nothing about how it ended: {settled}"));
    assert!(
        detail.contains("had not exited") && detail.contains("killed"),
        "the settlement does not tell a reaped dispatch from one that stopped: {detail}"
    );
}

/// A member on a harness with no lever answers, rather than failing.
///
/// It is a fact about the harness — there is nothing to redirect — and the
/// cancellation must neither break on it nor pretend it landed. The deadline is
/// what stops the dispatch, exactly as it does for one that ignored the ask.
#[test]
fn a_member_with_no_lever_is_an_answer_and_the_deadline_still_applies() {
    let world = World::new("cancel-nolever");
    // A member running on a harness with no out-of-band control: its turn is
    // announced and there is nothing to reach into it with.
    world.script("slow.no-lever", "");
    let run = held(&world, "cancelnolever", SHORT_GRACE);

    cancel(&world, &run, "slow");

    world.until("the interrupt to be recorded", |world| {
        !world.events_of(&run, "turn-interrupted").is_empty()
    });
    let interrupted = world.events_of(&run, "turn-interrupted");
    assert_eq!(interrupted[0]["payload"]["delivered"], json!(false));
    assert!(
        interrupted[0]["payload"]["reason"].is_string(),
        "an interrupt that reached no turn carries no reason: {}",
        interrupted[0]
    );
    let asked_for = surfaces(&world, &run, "dispatch-interrupted");
    assert_eq!(asked_for.len(), 1, "{asked_for:?}");
    assert!(
        asked_for[0]["payload"]["message"]
            .as_str()
            .is_some_and(|said| said.contains("no turn to redirect")),
        "a member with no lever was not recorded as one: {}",
        asked_for[0]
    );

    // Not a failure, and not the end of it either: the deadline does the work.
    world.until("the deadline to expire", |world| {
        !surfaces(world, &run, "dispatch-killed").is_empty()
    });
    let settled = settlement(&world, &run, "slow");
    assert_eq!(settled["payload"]["status"], "cancelled", "{settled}");
}

/// A requeue issued while the node's own dispatch is still running is refused,
/// naming what to wait for.
///
/// This is the ordinary state right after a cancel — the node is parked and its
/// dispatch has not let go — and accepting it put a node back on the frontier
/// where it sat `ready` for forty minutes, waiting on the workspace its own
/// predecessor was holding, with nothing said about why.
#[test]
fn a_requeue_of_a_node_whose_dispatch_is_still_running_is_refused() {
    let world = World::new("requeue-inflight");
    // The default deadline: this journey is about what happens *while* the
    // dispatch is still there, so nothing may reap it out from under the edit.
    let run = held(&world, "requeuelive", "600");

    cancel(&world, &run, "slow");

    let refused = world.run_with_stdin(
        &["reply", &run],
        &envelope(json!([{"op": "requeue", "id": "slow"}])),
    );
    refused.exited(REFUSED);
    refused.err_has("still has a dispatch in flight");
    // Naming the dispatch, so a supervisor has something to look at rather than
    // a bare "not yet".
    refused.err_has("graph run");
    refused.err_has("wait for the node to settle");

    // Refused means nothing changed: the node is still parked, and no second
    // dispatch was started for it.
    assert_eq!(
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .filter(|event| event["labels"]["node"] == "slow")
            .count(),
        1,
        "a refused requeue dispatched the node anyway: {}",
        world.dump()
    );
    assert!(
        !world
            .events_of(&run, "edit-committed")
            .iter()
            .any(|event| event["payload"]["command"]["op"] == "requeue"),
        "a refused requeue was committed: {}",
        world.dump()
    );

    world.release("slow.go");
}

/// The same requeue, once the dispatch has actually settled, still works.
///
/// The refusal above is about a dispatch that is still there, and nothing else:
/// a planner who waits gets the node back. Without this, "refuse a requeue" is
/// indistinguishable from "refuse requeues".
#[test]
fn a_requeue_of_a_parked_node_whose_dispatch_has_settled_is_applied() {
    let world = World::new("requeue-settled");
    // A second node held open throughout, so the loop is still running when the
    // requeue arrives: a graph whose every node has settled has no driver left
    // to pick one up.
    world.script("keep.wait", "hold");
    world.script("slow.turn-open", "");
    world.script("slow.wait", "hold");
    world.script("slow.stops-when-interrupted", "");
    let path = world.plan(
        "requeuesettled",
        &plan_of(
            "requeuesettled",
            vec![agent("slow", &[]), agent("keep", &[])],
        ),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the held node's turn to open", |world| {
        world
            .events_of("requeuesettled", "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == "slow")
    });
    let run = "requeuesettled".to_string();

    cancel(&world, &run, "slow");
    // The worker takes the ask and ends, so the dispatch settles on its own.
    settlement(&world, &run, "slow");

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "requeue", "id": "slow"}])),
        )
        .exited(0);
    world.until("the requeued node to be dispatched again", |world| {
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .filter(|event| event["labels"]["node"] == "slow")
            .count()
            >= 2
    });

    world.release("slow.go");
    world.release("keep.go");
}

/// A dispatch that has said **nothing** is cancelled on the loop's own clock.
///
/// This is the case that made the defect expensive. A dispatch nothing has heard
/// from has named no turn, so there is nothing to ask — and a deadline that
/// waited for the dispatch to speak before it started would never start at all.
/// It is also where the grace period is read: an unusable value falls back to
/// the default rather than to zero, which would turn every cooperative cancel
/// into an immediate kill.
#[test]
fn a_cancel_of_a_silent_dispatch_asks_nothing_and_still_carries_a_deadline() {
    let world = World::new("cancel-silent");
    // Both spellings of an unusable deadline, because they fail differently and
    // the fallback has to cover both: a literal zero, which taken at its word
    // would make every cooperative cancel an immediate kill and never wait for
    // the ask this change exists for, and a value that is not a number of
    // seconds at all. Each drives its own run; the node is named for its
    // scenario so the two holds are independent.
    for (node, grace) in [("zeroed", "0"), ("nonsense", "when-it-feels-like-it")] {
        // Held before it announces anything: no member, no turn, no address.
        world.script(&format!("{node}.wait"), "hold");
        let path = world.plan(node, &plan_of(node, vec![agent(node, &[])]));
        let mut launch = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
        launch.env(CANCEL_GRACE_ENV, grace);
        world.run_on(launch, "start --detach").exited(0);
        world.until("the node to be dispatched", |world| {
            !world.events_of(node, "node-dispatched").is_empty()
        });
        let run = node.to_string();

        cancel(&world, &run, node);

        let asked_for = world.surfaced(&run, "dispatch-interrupted");
        let message = asked_for["payload"]["message"]
            .as_str()
            .expect("the surface says what it did");
        assert!(
            message.contains("has named a turn to interrupt"),
            "a dispatch that had named no turn was reported as one that was asked: {message}"
        );
        assert!(
            message.contains("killed in 300s"),
            "the grace period {grace:?} was honoured instead of falling back to the \
             default: {message}"
        );
        assert!(
            !world.was_invoked("oneagentgraph", &["interrupt"]),
            "an interrupt was addressed at a turn nothing had named: {:?}",
            world.invocations()
        );

        // Released rather than left to the deadline: what this journey is about
        // is the ask and the clock, and waiting out the default would be waiting
        // on it.
        world.release(&format!("{node}.go"));
        let settled = settlement(&world, &run, node);
        assert_eq!(settled["payload"]["status"], "cancelled", "{settled}");
    }
}

/// A lever that was pulled and *broke* is an answer too, and the deadline still
/// does the work.
///
/// The three answers an interrupt can give are not interchangeable — a delivery
/// that landed, a fact that there was no turn to land it in, and a lever that
/// failed — and only the first means the worker was told anything. None of them
/// may fail the cancellation, because the dispatch is running either way.
#[test]
fn a_lever_that_failed_does_not_fail_the_cancellation() {
    let world = World::new("cancel-lever-broken");
    world.script("interrupt.fail", "");
    let run = held(&world, "cancelbroke", SHORT_GRACE);

    cancel(&world, &run, "slow");

    let asked_for = world.surfaced(&run, "dispatch-interrupted");
    let message = asked_for["payload"]["message"]
        .as_str()
        .expect("the surface says what it did");
    assert!(
        message.contains("the lever failed"),
        "a delivery that broke was reported as one that landed or found no turn: {message}"
    );
    assert!(
        message.contains("the control socket refused"),
        "the failure does not carry what the lever said: {message}"
    );

    world.until("the deadline to expire", |world| {
        !surfaces(world, &run, "dispatch-killed").is_empty()
    });
    let settled = settlement(&world, &run, "slow");
    assert_eq!(settled["payload"]["status"], "cancelled", "{settled}");
}

/// Every member of the dispatch's graph run that has named a turn is asked.
///
/// A graph is a graph: several members work under one run, and a cancellation
/// that addressed only the last member it saw on the stream would leave the
/// others working. Their answers differ — one turn is live and the other is
/// over — and both are recorded, because "asked and there was nothing there" is
/// what tells a supervisor the ask reached everyone it could.
#[test]
fn every_member_that_has_named_a_turn_is_asked_to_stop() {
    let world = World::new("cancel-members");
    // A second member of the same graph run, whose turn is announced and not
    // controllable: two addresses, two different answers.
    world.script("slow.also-member", "reviewer");
    world.script("slow.stops-when-interrupted", "");
    let run = held(&world, "cancelmembers", SHORT_GRACE);
    world.until("the second member's turn to be announced", |world| {
        world
            .events_of(&run, "turn-started")
            .iter()
            .any(|event| event["labels"]["member"] == "reviewer")
    });

    cancel(&world, &run, "slow");

    let asked_for = world.surfaced(&run, "dispatch-interrupted");
    let message = asked_for["payload"]["message"]
        .as_str()
        .expect("the surface says what it did");
    assert!(
        message.contains("asked 2 turn(s)"),
        "only one member of a two-member dispatch was asked to stop: {message}"
    );
    assert!(
        message.contains("worker: the running turn took the redirection")
            && message.contains("reviewer: no turn to redirect"),
        "the surface does not carry what each member answered: {message}"
    );
    for member in ["worker", "reviewer"] {
        assert!(
            world.was_invoked("oneagentgraph", &["interrupt", member]),
            "member {member:?} was never asked to stop: {:?}",
            world.invocations()
        );
    }

    let settled = settlement(&world, &run, "slow");
    assert_eq!(settled["payload"]["status"], "cancelled", "{settled}");
}
