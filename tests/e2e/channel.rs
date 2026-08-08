//! Ported from `test_channel_e2e`.
//!
//! The planner-facing channel: reading a surface, answering it, raising one, and
//! the pacemaker reset that consumption triggers. **Rendering is not reading** —
//! `monitor` shows a pending surface without consuming it, and `next` is the
//! only consumer.

use crate::harness::{agent, human, plan_of, World, REFUSED};

/// Start a run detached and wait until its first round is open.
fn running(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the run to open a round", |world| {
        !world.events_of(name, "round-started").is_empty()
    });
    name.to_string()
}

#[test]
fn a_surface_is_queued_sent_and_then_read_exactly_once() {
    let world = World::new("channel-once");
    world.script("build.wait", "hold");
    let run = running(&world, "surfaced", vec![agent("build", &[])]);

    world
        .run(&["surface", &run, "--kind", "check-in", "--message", "steady"])
        .exited(0)
        .out_has("\"queued\"");

    // Queuing records that a surface was *sent*. Consumption is a separate fact.
    assert_eq!(world.events_of(&run, "planner-surface-queued").len(), 1);
    assert!(world.events_of(&run, "planner-surfaced").is_empty());

    let read = world.run(&["next", &run]);
    read.exited(0).out_has("steady");
    assert_eq!(read.json()["status"], "surface");
    assert_eq!(world.events_of(&run, "planner-surfaced").len(), 1);

    // One reader, one surface: the queue does not hand it out twice.
    let again = world.run(&["next", &run]);
    again.exited(0);
    assert_eq!(again.json()["status"], "running");
    assert_eq!(again.json()["surface"], serde_json::Value::Null);

    world.release("build.go");
}

#[test]
fn consuming_a_surface_resets_the_check_in_pacemaker() {
    let world = World::new("channel-pacemaker");
    world.script("build.wait", "hold");
    let run = running(&world, "paced", vec![agent("build", &[])]);
    world
        .run(&["surface", &run, "--kind", "check-in", "--message", "steady"])
        .exited(0);

    assert!(
        !world.was_invoked("oneagentgraph", &["reset-timer"]),
        "queuing a surface reset the clock; only reading one does"
    );

    world.run(&["next", &run]).exited(0);
    assert!(
        world.was_invoked("oneagentgraph", &["reset-timer", &run, "check-in"]),
        "consumption did not reset the check-in pacemaker: {:?}",
        world.invocations()
    );
    world.release("build.go");
}

#[test]
fn rendering_a_surface_is_not_reading_it() {
    let world = World::new("channel-render");
    world.script("build.wait", "hold");
    let run = running(&world, "rendered", vec![agent("build", &[])]);
    world
        .run(&["surface", &run, "--kind", "check-in", "--message", "steady"])
        .exited(0);

    // The documented way to watch a run must not be the way to wedge it.
    world.run(&["monitor", &run]).exited(0);
    world.run(&["status", &run]).exited(0);
    assert!(world.events_of(&run, "planner-surfaced").is_empty());
    assert!(!world.was_invoked("oneagentgraph", &["reset-timer"]));

    world.run(&["next", &run]).exited(0).out_has("steady");
    world.release("build.go");
}

#[test]
fn the_next_check_in_replaces_the_queued_one_rather_than_being_blocked_by_it() {
    let world = World::new("channel-replace");
    world.script("build.wait", "hold");
    let run = running(&world, "fresh", vec![agent("build", &[])]);

    for message in ["first update", "second update"] {
        world
            .run(&["surface", &run, "--kind", "check-in", "--message", message])
            .exited(0);
    }
    // Being ignored makes the harness louder rather than quieter, and exactly
    // one check-in is ever pending — kept current, not kept still.
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("second update");
    assert!(!read.stdout.contains("first update"));

    let empty = world.run(&["next", &run]);
    assert_eq!(empty.json()["status"], "running");
    world.release("build.go");
}

#[test]
fn unread_surfaces_are_reported_separately_by_the_views_a_planner_reads() {
    let world = World::new("channel-unread");
    world.script("build.wait", "hold");
    let run = running(&world, "unread", vec![agent("build", &[])]);
    world
        .run(&["surface", &run, "--kind", "check-in", "--message", "steady"])
        .exited(0);

    // The state a planner who never attached is blind to: the row above says
    // only ACTIVE, and the delivery record is written on consumption.
    world
        .run(&["runs"])
        .exited(0)
        .out_has("planner update(s) waiting");
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("planner update(s) waiting");

    world.run(&["next", &run]).exited(0);
    let after = world.run(&["runs"]);
    assert!(
        !after.stdout.contains("planner update(s) waiting"),
        "a read surface is still reported unread:\n{}",
        after.stdout
    );
    world.release("build.go");
}

#[test]
fn a_legacy_verdict_is_accepted_and_recorded() {
    let world = World::new("channel-verdict");
    world.script("build.wait", "hold");
    let run = running(&world, "verdict", vec![agent("build", &[])]);

    let envelope =
        r#"{"completion":false,"message":"keep going","reason":"the graph is not complete"}"#;
    world
        .run_with_stdin(&["reply", &run], envelope)
        .exited(0)
        .out_has("\"delivered\"");
    assert_eq!(world.events_of(&run, "planner-replied").len(), 1);

    // A completion verdict is journalled for audit as well.
    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"completion":true,"reason":"publication verified"}"#,
        )
        .exited(0);
    let requested = world.events_of(&run, "completion-requested");
    assert_eq!(requested.len(), 1, "{requested:?}");
    assert_eq!(requested[0]["payload"]["reason"], "publication verified");
    world.release("build.go");
}

#[test]
fn a_reply_may_be_given_as_a_file_as_well_as_on_stdin() {
    let world = World::new("channel-file");
    world.script("build.wait", "hold");
    let run = running(&world, "filereply", vec![agent("build", &[])]);

    let path = world.root.join("reply.json");
    std::fs::write(
        &path,
        r#"{"completion":false,"message":"go on","reason":"why"}"#,
    )
    .expect("the reply is written");
    world
        .run(&["reply", &run, &path.to_string_lossy()])
        .exited(0)
        .out_has("\"delivered\"");
    world.release("build.go");
}

#[test]
fn a_malformed_reply_is_refused_rather_than_half_applied() {
    let world = World::new("channel-malformed");
    world.script("build.wait", "hold");
    let run = running(&world, "malformed", vec![agent("build", &[])]);

    for envelope in [
        "not json at all",
        r#"{"commands":[{"op":"attest","ref":"approve"}]}"#,
        r#"{"version":1,"commands":[{"op":"invented","id":"x"}]}"#,
        r#"{"version":1,"commands":[{"op":"drop","id":"build"}]}"#,
    ] {
        world
            .run_with_stdin(&["reply", &run], envelope)
            .exited(REFUSED);
    }
    world.release("build.go");
}

#[test]
fn a_settled_run_refuses_a_reply_nothing_will_ever_read() {
    let world = World::new("channel-settled");
    let path = world.plan("settled", &plan_of("settled", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the run to settle", |world| {
        !world.events_of("settled", "round-finished").is_empty()
    });
    world.until("the driver to exit", |world| {
        world
            .run(&["status", "settled"])
            .stdout
            .contains("DRIVER DEAD")
    });

    world
        .run_with_stdin(
            &["reply", "settled"],
            r#"{"completion":true,"reason":"done"}"#,
        )
        .exited(REFUSED)
        .err_has("has settled");
}

#[test]
fn attest_completes_a_ready_waiting_human_action() {
    let world = World::new("channel-attest");
    world.script("build.wait", "hold");
    let run = running(
        &world,
        "attested",
        vec![agent("build", &[]), human("approve", &[])],
    );
    world.until("the human action to be waiting", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "approve")
    });

    world.run(&["attest", &run, "approve"]).exited(0);
    world.until("the attestation to be committed", |world| {
        world
            .events_of(&run, "edit-committed")
            .iter()
            .any(|event| event["payload"]["command"]["op"] == "attest")
    });
    world.release("build.go");

    world.until("the run to settle", |world| {
        !world.events_of(&run, "round-finished").is_empty()
    });
    world.run(&["results", &run]).exited(0).out_has("approve");
}

#[test]
fn attesting_something_that_is_not_a_ready_human_action_is_refused_by_name() {
    let world = World::new("channel-attest-refuse");
    world.script("build.wait", "hold");
    let run = running(&world, "noaction", vec![agent("build", &[])]);

    world
        .run(&["attest", &run, "build"])
        .exited(REFUSED)
        .err_has("not a ready, waiting human action");
    world
        .run(&["attest", &run, "nowhere"])
        .exited(REFUSED)
        .err_has("not a ready, waiting human action");
    world.release("build.go");
}
