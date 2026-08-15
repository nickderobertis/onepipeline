//! The planner-facing channel: reading a surface, answering it, raising one, and
//! the pacemaker reset that consumption triggers. **Rendering is not reading** —
//! `monitor` shows a pending surface without consuming it, and `next` is the
//! only consumer.
//!
//! Ported from `test_channel_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The scenario this journey states is one a real sibling would need paid
// model turns to produce, and `dispatch.rs` is where the real `oneagentgraph` binary is
// driven instead. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{agent, human, plan_of, World, REFUSED};
use serde_json::json;

/// Start a run detached and wait until it is executing.
fn running(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });
    name.to_string()
}

/// The same, with an observer graph attached.
///
/// Only the pacemaker journeys need one: the clock a surface resets belongs to a
/// member of that graph, and a run launched with `--dag-graph off` — the shipped
/// default — has no member to address.
fn observed(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--detach",
            "--dag-graph",
            &world.shipped_dag_graph(),
        ])
        .exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
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

/// A surface outlives nothing: there is no round for it to be left over from.
///
/// Execution is one continuous run, so a queued surface stays consumable until
/// somebody reads it — which is the whole reason the round-scoped discard is
/// gone. What still replaces a surface is a *newer check-in*, at the queue
/// rather than at the read.
#[test]
fn a_surface_queued_before_a_node_settled_is_still_delivered_afterwards() {
    let world = World::new("channel-durable");
    world.script("flaky.wait", "hold");
    world.script("flaky.fail", "1");
    let run = running(&world, "durable", vec![agent("flaky", &[])]);

    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "check-in",
            "--message",
            "queued while it ran",
        ])
        .exited(0);
    world.release("flaky.go");
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });

    // Still there, and still the planner's to read: nothing about the node
    // settling makes what was said about it undeliverable.
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("queued while it ran");
    assert_eq!(world.events_of(&run, "planner-surfaced").len(), 1);
}

/// A newer check-in replaces the one nobody read, so being ignored makes the
/// harness louder rather than quieter.
#[test]
fn a_second_check_in_replaces_the_one_still_waiting_to_be_read() {
    let world = World::new("channel-supersede");
    world.script("build.wait", "hold");
    let run = running(&world, "superseded", vec![agent("build", &[])]);

    for message in ["the first update", "the second update"] {
        world
            .run(&["surface", &run, "--kind", "check-in", "--message", message])
            .exited(0);
    }

    let read = world.run(&["next", &run]);
    read.exited(0).out_has("the second update");
    assert!(
        !read.stdout.contains("the first update"),
        "a superseded check-in was delivered: {}",
        read.stdout
    );
    let again = world.run(&["next", &run]);
    assert_eq!(again.json()["status"], "running");
    world.release("build.go");
}

/// Consumption resets the pacemaker, addressed by the **graph** run's id.
///
/// The two run ids on one run are the whole trap here. `oneagentgraph` mints an
/// id for the graph it starts, and its signals — a resettable schedule's clock
/// among them — answer only to that one; this crate's run id names a run that
/// library has never heard of. Handing over the wrong one is silent, because the
/// reset is best-effort by design, so the assertion names the id rather than
/// only the verb.
#[test]
fn consuming_a_surface_resets_the_check_in_pacemaker() {
    let world = World::new("channel-pacemaker");
    world.script("build.wait", "hold");
    let run = observed(&world, "paced", vec![agent("build", &[])]);
    world
        .run(&["surface", &run, "--kind", "check-in", "--message", "steady"])
        .exited(0);

    assert!(
        !world.was_invoked("oneagentgraph", &["reset-timer"]),
        "queuing a surface reset the clock; only reading one does"
    );

    let graph_run = world.run_json(&run, "launch.json")["graph_run"]
        .as_str()
        .expect("the launch record names the graph run driving this run")
        .to_string();
    assert_ne!(
        graph_run, run,
        "the two run ids are the same, so this journey could not tell them apart"
    );

    world.run(&["next", &run]).exited(0);
    // llmlint: ignore-block[tests_mirror_real_usage] the id a reset is addressed by never
    // appears on a product surface — `next` prints the surface whether or not the clock
    // restarted, deliberately — so the argv the double recorded is where that value exists.
    // `dispatch.rs` runs the same journey against the real sibling and asserts the outcome
    // where the sibling puts it; this is the half that names the argument.
    assert!(
        world.was_invoked("oneagentgraph", &["reset-timer", &graph_run, "check-in"]),
        "consumption did not reset the check-in pacemaker by the graph run's own id: {:?}",
        world.invocations()
    );
    assert!(
        !world.was_invoked("oneagentgraph", &["reset-timer", &run]),
        "the reset was addressed with this crate's run id, which names no graph run: {:?}",
        world.invocations()
    );
    // llmlint: ignore-end[tests_mirror_real_usage]
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
        world.run_file("settled", "result.json").is_file()
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
        world.run_file(&run, "result.json").is_file()
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

#[test]
fn the_channel_server_relays_a_boundary_frame_and_writes_back_the_verdict() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-serve");
    world.script("build.wait", "hold");
    let run = running(&world, "served", vec![agent("build", &[])]);

    // This is the orchestrator member's judge side: it reads the frame the
    // observer emits when it has something to raise, relays it to the planner, and
    // writes the answer back into the conversation.
    let mut serving = world
        .cmd(&["channel", "serve", &run])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");

    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"Node build failed its gate; retry?","blocking":true,"node":"build"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");

    world.until("the frame to reach the planner", |world| {
        world
            .events_of(&run, "planner-surface-queued")
            .iter()
            .any(|event| event["payload"]["kind"] == "blocker")
    });

    // A blocking surface is what `runs` and `status` report as awaiting a
    // decision once it is consumed.
    world
        .run(&["next", &run])
        .exited(0)
        .out_has("failed its gate");
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision");

    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"completion":true,"reason":"the run is finished"}"#,
        )
        .exited(0);

    // The verdict is written back on stdout, as the conversation's next turn.
    let stdout = serving.stdout.take().expect("stdout is piped");
    let verdict = BufReader::new(stdout)
        .lines()
        .map_while(std::result::Result::ok)
        .find(|line| line.contains("completion"))
        .expect("the server wrote a verdict");
    assert!(verdict.contains("true"), "{verdict}");
    assert!(verdict.contains("the run is finished"), "{verdict}");

    drop(stdin);
    world.release("build.go");
    let _ = serving.wait();
}

#[test]
fn the_channel_server_synthesizes_a_continuing_verdict_when_nobody_answers() {
    use std::io::Write;

    let world = World::new("channel-serve-timeout");
    world.script("build.wait", "hold");
    let run = running(&world, "unanswered", vec![agent("build", &[])]);

    let mut command = world.cmd(&["channel", "serve", &run]);
    command.env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1");
    let mut serving = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(stdin, r#"{{"kind":"blocker","message":"anyone there?"}}"#).expect("written");
    stdin.flush().expect("flushed");
    drop(stdin);

    let output = serving.wait_with_output().expect("the server exits");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Wedging the orchestrator on a planner who is away would be worse than
    // continuing, so the timeout is answered rather than left open.
    assert!(stdout.contains("\"completion\":false"), "{stdout}");
    assert!(stdout.contains("timed out"), "{stdout}");
    world.release("build.go");
}

#[test]
fn the_channel_server_refuses_a_frame_it_cannot_read() {
    use std::io::Write;

    let world = World::new("channel-serve-bad");
    world.script("build.wait", "hold");
    let run = running(&world, "badframe", vec![agent("build", &[])]);

    let mut serving = world
        .cmd(&["channel", "serve", &run])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(stdin, "this is not a frame").expect("written");
    stdin.flush().expect("flushed");
    drop(stdin);

    let output = serving.wait_with_output().expect("the server exits");
    assert_eq!(output.status.code(), Some(REFUSED));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("bad frame"),
        "{output:?}"
    );
    world.release("build.go");
}

/// The whole decision-point contract, end to end and with no driver but the
/// engine's own loop.
///
/// A `kind: human` node mid-graph holds **its own subtree** and nothing else:
/// the independent branch beside it runs to completion while the dependent one
/// waits. Clearing it with `attest` releases exactly that subtree, inside the
/// loop that was already running — nothing external drives the resumption.
#[test]
fn a_human_decision_holds_its_subtree_while_another_branch_runs_and_attest_resumes_it() {
    let world = World::new("channel-decision");
    // A third branch, held open, so the loop is still running when the
    // attestation arrives: what is under test is the resumption *inside* it.
    world.script("hold.wait", "hold");
    let path = world.plan(
        "decided",
        &plan_of(
            "decided",
            vec![
                agent("seed", &[]),
                human("approve", &["seed"]),
                agent("ship", &["approve"]),
                agent("probe", &[]),
                agent("report", &["probe"]),
                agent("hold", &[]),
            ],
        ),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

    // The decision is reported the moment it begins holding dependents back,
    // and it names what it holds.
    world.until("the decision to be reported", |world| {
        !world.events_of("decided", "decision-pending").is_empty()
    });
    let pending = world.events_of("decided", "decision-pending");
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0]["payload"]["reference"], "approve");
    assert_eq!(pending[0]["payload"]["kind"], "human");
    assert_eq!(pending[0]["payload"]["unblocks"], json!(["ship"]));

    // The independent branch runs to completion beside it. Nothing about a
    // decision on one branch reaches another.
    world.until("the independent branch to finish", |world| {
        world
            .events_of("decided", "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "report")
    });
    assert!(
        world
            .events_of("decided", "node-dispatched")
            .iter()
            .all(|event| event["labels"]["node"] != "ship"),
        "the paused subtree ran while its decision was outstanding: {:?}",
        world.kinds("decided")
    );

    // Cleared by the person who took the action, and released by the loop that
    // was already running: no `adopt`, no second driver, nothing else typed.
    world.run(&["attest", "decided", "approve"]).exited(0);
    world.until("the paused subtree to resume", |world| {
        world
            .events_of("decided", "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "ship")
    });
    let cleared = world.events_of("decided", "decision-cleared");
    assert_eq!(cleared.len(), 1, "{cleared:?}");
    assert_eq!(cleared[0]["payload"]["reference"], "approve");
    assert_eq!(cleared[0]["payload"]["released"], json!(["ship"]));

    world.release("hold.go");
    world.until("the run to settle", |world| {
        world.run_file("decided", "result.json").is_file()
    });
    assert_eq!(world.run_json("decided", "result.json")["state"], "complete");
}

/// An edit needs no live round, because there are none: a run nothing is
/// driving takes one under the ownership lock and applies it there.
#[test]
fn an_edit_to_a_run_nothing_is_driving_is_applied_rather_than_refused() {
    let world = World::new("channel-undriven-edit");
    let path = world.plan(
        "settledgraph",
        &plan_of("settledgraph", vec![human("approve", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);

    world
        .run_with_stdin(
            &["reply", "settledgraph"],
            r#"{"version":1,"commands":[{"op":"add","node":{"id":"late","persona":"e","task":"t"}}]}"#,
        )
        .exited(0)
        .out_has("\"applied\"");
    assert_eq!(world.events_of("settledgraph", "edit-committed").len(), 1);

    // And the adoption that picks the run back up dispatches what was added.
    world.run(&["attest", "settledgraph", "approve"]).exited(0);
    world.run(&["adopt", "settledgraph"]).exited(0);
    assert!(
        world
            .events_of("settledgraph", "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "late"),
        "the edit an undriven run took was never executed: {:?}",
        world.kinds("settledgraph")
    );
}

#[test]
fn a_read_survives_a_pacemaker_it_could_not_reset_and_says_so() {
    let world = World::new("channel-reset-fails");
    world.script("build.wait", "hold");
    world.script("reset-timer.fail", "");
    let run = observed(&world, "unresettable", vec![agent("build", &[])]);
    world
        .run(&["surface", &run, "--kind", "check-in", "--message", "steady"])
        .exited(0);

    // The planner has the surface either way, so a sibling that cannot reset
    // the clock is reported rather than allowed to fail the read.
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("steady");
    read.err_has("could not reset the check-in pacemaker");
    assert_eq!(world.events_of(&run, "planner-surfaced").len(), 1);
    world.release("build.go");
}

#[test]
fn the_channel_server_refuses_a_frame_missing_what_a_surface_needs() {
    use std::io::Write;

    let world = World::new("channel-frame-schema");
    world.script("build.wait", "hold");
    let run = running(&world, "strictframe", vec![agent("build", &[])]);

    // A frame is external input, so it has a schema: a missing `message` or an
    // unknown key is refused by name rather than defaulted into a surface the
    // planner then has to interpret.
    for frame in [
        r#"{"kind":"blocker"}"#,
        r#"{"message":"no kind"}"#,
        r#"{"kind":"blocker","message":"m","urgency":"high"}"#,
    ] {
        let mut serving = world
            .cmd(&["channel", "serve", &run])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the channel server starts");
        let mut stdin = serving.stdin.take().expect("stdin is piped");
        writeln!(stdin, "{frame}").expect("written");
        stdin.flush().expect("flushed");
        drop(stdin);

        let output = serving.wait_with_output().expect("the server exits");
        assert_eq!(output.status.code(), Some(REFUSED), "{frame} was accepted");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("bad frame"),
            "{frame}: {output:?}"
        );
    }
    assert!(
        world.events_of(&run, "planner-surface-queued").is_empty(),
        "a refused frame still reached the planner"
    );
    world.release("build.go");
}

/// The observer contract, end to end: the graph a `--dag-graph REF` launch
/// attaches watches the run and authors over the channel — and what it may
/// author is enforced.
///
/// Three things at once, because they are one journey: the graph really is
/// launched and really is only an observer, an op outside the monitor's
/// allowlist is refused with the reason, and an allowed one is applied *and*
/// surfaced to the planner, who owns the graph and did not ask for it.
#[test]
fn a_dag_graph_observes_while_the_monitors_edits_are_held_to_its_allowlist() {
    let world = World::new("channel-monitor");
    world.script("slow.wait", "hold");
    let path = world.plan(
        "watched",
        &plan_of("watched", vec![agent("slow", &[]), human("approve", &[])]),
    );
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--detach",
            "--dag-graph",
            &world.shipped_dag_graph(),
        ])
        .exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of("watched", "node-dispatched").is_empty()
    });

    // The graph was launched, and as an observer: it ran no engine verb, because
    // there is none to run.
    assert!(
        world.was_invoked("oneagentgraph", &["run"]),
        "the named dag-scope graph was never launched: {:?}",
        world.invocations()
    );
    assert!(
        !world.observer_saw().is_empty(),
        "the observer never read the run it was launched for"
    );

    // An op the monitor may not issue is refused by name, with the reason and
    // what to do instead — and nothing durable is written on its behalf.
    let refused = world.run_with_stdin(
        &["reply", "watched"],
        &json!({
            "version": 1,
            "author": "monitor",
            "commands": [{"op": "attest", "ref": "approve"}],
        })
        .to_string(),
    );
    refused
        .exited(REFUSED)
        .err_has("attest")
        .err_has("never by a watcher")
        .err_has("Surface it to the planner");
    assert!(
        world.events_of("watched", "human-attested").is_empty(),
        "a refused monitor edit still reached the run"
    );

    // An allowed one is applied, attributed, and surfaced to the planner
    // non-blocking: the monitor acted on its own judgement, so the planner
    // learns of it without having been asked to approve it first.
    world
        .run_with_stdin(
            &["reply", "watched"],
            &json!({
                "version": 1,
                "author": "monitor",
                "commands": [{
                    "op": "context",
                    "id": "slow",
                    "note": "the fixture moved to tests/data",
                    "deliver": "next",
                }],
            })
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");

    let committed = world
        .events_of("watched", "edit-committed")
        .into_iter()
        .next()
        .expect("the allowed edit was committed");
    assert_eq!(committed["payload"]["author"], "monitor", "{committed}");

    world.until("the planner to be told what the monitor did", |world| {
        world
            .events_of("watched", "planner-surface-queued")
            .iter()
            .any(|event| event["payload"]["kind"] == "monitor-edit")
    });
    let surfaced = world
        .events_of("watched", "planner-surface-queued")
        .into_iter()
        .find(|event| event["payload"]["kind"] == "monitor-edit")
        .expect("the monitor's edit was surfaced");
    assert_eq!(
        surfaced["payload"]["blocking"],
        json!(false),
        "a monitor's edit held the graph back to report itself: {surfaced}"
    );
    assert_eq!(surfaced["payload"]["source"], "monitor", "{surfaced}");

    world.release("slow.go");
}
