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
fn a_surface_left_over_from_a_finished_round_is_discarded_rather_than_delivered() {
    let world = World::new("channel-stale");
    world.script("flaky.wait", "hold");
    world.script("flaky.fail", "1");
    let run = running(&world, "stale", vec![agent("flaky", &[])]);

    // Queued while round one is the round that is running.
    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "check-in",
            "--message",
            "from round one",
        ])
        .exited(0);
    world.release("flaky.go");
    world.until("round one to finish", |world| {
        !world.events_of(&run, "round-finished").is_empty()
    });

    // Round two, held open so the read below happens inside it.
    std::fs::remove_file(world.fakes.join("flaky.go")).expect("the rendezvous is re-armed");
    world.run(&["round", "next", &run]).exited(0);
    let mut second = world
        .cmd(&["round", "run", &run])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the round starts");
    world.until("round two to open", |world| {
        world.events_of(&run, "round-started").len() >= 2
    });

    // It describes work in a round that has finished. Delivering it would send
    // the planner to look at a node this round is not running, and the check-in
    // that replaces it describes the round that is.
    let read = world.run(&["next", &run]);
    read.exited(0);
    assert!(
        !read.stdout.contains("from round one"),
        "a surface from a finished round was delivered: {}",
        read.stdout
    );
    assert!(
        world.events_of(&run, "planner-surfaced").is_empty(),
        "a stale surface was consumed"
    );

    world.release("flaky.go");
    second.wait().expect("the round finishes");
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
    let run = running(&world, "paced", vec![agent("build", &[])]);
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

#[test]
fn the_channel_server_relays_a_boundary_frame_and_writes_back_the_verdict() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-serve");
    world.script("build.wait", "hold");
    let run = running(&world, "served", vec![agent("build", &[])]);

    // This is the orchestrator member's judge side: it reads the frame the
    // orchestrator emits at a round boundary, relays it to the planner, and
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

#[test]
fn a_human_action_is_attested_at_a_round_boundary_and_the_next_round_opens() {
    let world = World::new("channel-boundary-attest");
    world.script("driver.wait", "hold");
    let path = world.plan(
        "gatedrun",
        &plan_of(
            "gatedrun",
            vec![human("approve", &[]), agent("ship", &["approve"])],
        ),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.run(&["round", "run", "gatedrun"]).exited(1);

    // The round has settled: nothing is executing, and no later round can open
    // until a person's action is recorded. An attestation is not a graph edit,
    // so it is legal here — refusing it would strand the run.
    let result = world.run_json("gatedrun", "round-01/result.json");
    assert_eq!(result["state"], "waiting", "{result}");
    world
        .run_with_stdin(
            &["reply", "gatedrun"],
            r#"{"version":1,"commands":[{"op":"add","node":{"id":"late","persona":"e","task":"t"}}]}"#,
        )
        .exited(REFUSED)
        .err_has("no round executing");

    world.run(&["attest", "gatedrun", "approve"]).exited(0);
    assert_eq!(world.events_of("gatedrun", "human-attested").len(), 1);

    // With the action recorded, the dependent becomes eligible and the
    // transition opens the round that runs it.
    let transitioned = world.run(&["round", "next", "gatedrun"]);
    transitioned.exited(0).out_has("\"continuing\"");
    assert_eq!(transitioned.json()["next_round"], 2);

    world.run(&["round", "run", "gatedrun"]).exited(0);
    let second = world.run_json("gatedrun", "round-02/result.json");
    assert_eq!(second["state"], "complete", "{second}");
    let ids: Vec<&str> = second["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect();
    assert_eq!(ids, vec!["ship"], "the attested human was carried forward");
    world.release("driver.go");
}

#[test]
fn a_read_survives_a_pacemaker_it_could_not_reset_and_says_so() {
    let world = World::new("channel-reset-fails");
    world.script("build.wait", "hold");
    world.script("reset-timer.fail", "");
    let run = running(&world, "unresettable", vec![agent("build", &[])]);
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
