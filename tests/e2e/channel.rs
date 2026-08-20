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

use crate::harness::{agent, human, plan_of, World, NOTHING_DRIVING, REFUSED};
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
    // On the **surface**, which is the claim. The event view beside it carries
    // the run's own `planner-surface-queued` for both check-ins, because both
    // were queued — being superseded is what happened to the surface, not
    // something the run un-records.
    let delivered = read.json()["surface"]["message"].clone();
    assert_eq!(
        delivered, "the second update",
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
    // On the surface, which is the claim: the event view beside it records that
    // both check-ins were queued, because both were.
    assert_eq!(read.json()["surface"]["message"], "second update");

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

/// The one line a supervisor is not allowed to filter out says *what* is
/// waiting, not only how much.
///
/// A blocking question is a run's only signal that it is held on a person, and
/// behind a pile of routine `monitor` updates a bare count rendered the two
/// identically. So the kinds ride the line, and the blocking one leads it.
#[test]
fn the_unread_line_names_the_kinds_waiting_so_a_question_is_not_buried() {
    use std::io::Write;

    let world = World::new("channel-unread-kinds");
    world.script("build.wait", "hold");
    let run = running(&world, "buried", vec![agent("build", &[])]);

    // An observer's judge side, raising what it saw: routine updates first, and
    // the one question it stopped to ask last — the order that buries it.
    let mut frames = String::new();
    for update in 0..5 {
        frames.push_str(&format!(
            "{{\"kind\":\"monitor\",\"message\":\"update {update}\",\"blocking\":false}}\n"
        ));
    }
    frames.push_str(
        "{\"kind\":\"planner-question\",\"message\":\"Which base should build target?\"}\n",
    );
    // The last of these carries a newline inside its kind. A kind is the
    // observer persona's own word, so it is a stranger's string on the one line
    // a supervisor may not filter out — and a second line spliced into that line
    // is how a run hides the question above it.
    for kind in ["edit-rejected", "quiet-worker", "check-in", "pro\\nposal"] {
        frames.push_str(&format!(
            "{{\"kind\":\"{kind}\",\"message\":\"one {kind}\",\"blocking\":false}}\n"
        ));
    }

    // The server waits for a verdict after every frame, and nothing here is
    // going to answer six of them: a one-second bound makes each frame's wait
    // its own synthesized `continue`, which is the timeout path this journey
    // rides rather than the question it is about.
    let mut command = world.cmd(&["channel", "serve", &run]);
    command
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let mut serving = command.spawn().expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    stdin
        .write_all(frames.as_bytes())
        .expect("the frames write");
    drop(stdin);
    serving.wait().expect("the channel server ends");

    world.until("every frame to reach the planner", |world| {
        world.events_of(&run, "planner-surface-queued").len() == 10
    });

    // The one question leads the parenthetical rather than sitting behind the
    // five updates that outnumber it; a queue of more kinds than a line can
    // carry says how many it left out rather than cutting them silently; and the
    // kind carrying a newline is rendered on the one line it belongs to.
    for view in [vec!["runs"], vec!["status", &run]] {
        let rendered = world.run(&view);
        rendered
            .exited(0)
            .out_has(
                "10 planner update(s) waiting (1 planner-question, 1 check-in, 1 edit-rejected, \
                 1 pro posal, and 2 other kind(s))",
            )
            .out_lacks("\nposal");
    }
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

/// A skip is not permanent when the work it was waiting for did in fact land.
///
/// The dependency failed here and its dependent was never asked — and stays
/// never asked, because the skip is re-derived from that failure on every pass.
/// Attesting the failed node is the statement that the work is there anyway, and
/// the run releases what it was holding inside the loop that was already going.
#[test]
fn attesting_a_failed_node_releases_the_dependents_it_had_skipped() {
    let world = World::new("channel-attest-failed");
    world.script("build.fail", "1");
    // A third branch, held open, so the loop is still running when the
    // attestation arrives: what is under test is the release *inside* it.
    world.script("hold.wait", "hold");
    let run = running(
        &world,
        "landed",
        vec![
            agent("build", &[]),
            agent("ship", &["build"]),
            agent("hold", &[]),
        ],
    );
    world.until("the dependency to fail", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "build")
    });

    world
        .run(&["results", &run])
        .exited(0)
        .out_has("never attempted; skipped by: build (failed)");
    assert!(
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .all(|event| event["labels"]["node"] != "ship"),
        "the skipped node ran before anything was attested: {:?}",
        world.kinds(&run)
    );

    // Nothing else in the vocabulary would do: the skip is re-derived from the
    // failure on every pass, so only saying the work landed releases it.
    world.run(&["attest", &run, "build"]).exited(0);
    // Once, like any other attestation, whichever settlement it was taken on.
    world
        .run(&["attest", &run, "build"])
        .exited(REFUSED)
        .err_has("already attested");
    world.until("the node it had skipped to run", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "ship")
    });
    let settled: Vec<_> = world
        .events_of(&run, "node-settled")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "ship")
        .collect();
    assert_eq!(settled[0]["payload"]["status"], "done", "{settled:?}");

    let attested = world.events_of(&run, "human-attested");
    assert_eq!(attested.len(), 1, "{attested:?}");
    assert_eq!(attested[0]["payload"]["ref"], "build");
    world.release("hold.go");
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("settled failed, attested as landed")
        .out_lacks("never attempted");
}

/// The gate on divergence 36: its settlements, driven against a run holding a
/// node in every settlement this journey can reach.
///
/// Both directions, which is what makes it a gate rather than a demonstration —
/// one the entry names and the build refuses fails here, and so does one the
/// build takes that the entry does not name, because every settlement outside
/// the list is asserted refused rather than left unasked.
#[test]
fn attest_takes_exactly_the_settlements_the_divergence_record_names() {
    let record = std::fs::read_to_string(crate::harness::repo_file("docs/contract-divergences.md"))
        .expect("the divergence record reads");
    let entry = record
        .split("\n## ")
        .find(|entry| entry.starts_with("36."))
        .expect("the divergence record still carries entry 36");
    let block = entry
        .split("```json")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("entry 36 carries the json block this journey drives");
    let source: serde_json::Value = serde_json::from_str(block).expect("entry 36's block is JSON");
    assert_eq!(source["op"], "attest", "{source}");
    let settlements: Vec<String> = serde_json::from_value(source["settlements"].clone())
        .expect("entry 36 names the settlements it accepts");
    assert!(!settlements.is_empty(), "{source}");

    let world = World::new("channel-attest-source");
    world.script("wrong.fail", "1");
    world.script("running.wait", "hold");
    // Held so a `cancel` has a dispatch to stop: a node that settles first is a
    // node the planner can no longer idle.
    world.script("idle.wait", "hold");
    let run = running(
        &world,
        "sourced",
        vec![
            agent("running", &[]),
            agent("queued", &["running"]),
            agent("wrong", &[]),
            agent("after", &["wrong"]),
            agent("finished", &[]),
            human("approve", &[]),
            agent("later", &["approve"]),
            agent("idle", &[]),
        ],
    );
    world.until("the run to reach every settlement it can", |world| {
        let of = |kind: &str| -> Vec<String> {
            world
                .events_of(&run, kind)
                .iter()
                .filter_map(|event| event["labels"]["node"].as_str().map(str::to_string))
                .collect()
        };
        let (settled, dispatched) = (of("node-settled"), of("node-dispatched"));
        ["wrong", "finished", "approve"]
            .iter()
            .all(|node| settled.iter().any(|seen| seen == node))
            && dispatched.iter().any(|seen| seen == "idle")
    });
    // The planner's own idle, so a `parked` node is among the settlements below.
    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"version":1,"commands":[{"op":"cancel","id":"idle"}]}"#,
        )
        .exited(0);

    // Each node's settlement as a reader sees it, off the view that prints it.
    let nodes = [
        "running", "queued", "wrong", "after", "finished", "approve", "later", "idle",
    ];
    let settlement_of = |world: &World, node: &str| -> String {
        let results = world.run(&["results", &run]);
        results.exited(0);
        results
            .stdout
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                Some((words.next()?.to_string(), words.next()?.to_string()))
            })
            .find(|(id, _)| id == node)
            .map(|(_, settlement)| settlement)
            .unwrap_or_else(|| panic!("no settlement for {node} in:\n{}", results.stdout))
    };

    // Refused first, because a refusal changes nothing: every settlement the
    // entry does not name is answered with the two it does. This is the half
    // that fails when the build grows an acceptance nobody wrote down.
    let mut reached = std::collections::BTreeSet::new();
    for node in nodes {
        let settlement = settlement_of(&world, node);
        reached.insert(settlement.clone());
        if settlements.contains(&settlement) {
            continue;
        }
        let refused = world.run(&["attest", &run, node]);
        refused.exited(REFUSED);
        for accepted in &settlements {
            refused.err_has(accepted);
        }
    }
    // And not a vacuous sweep: the run really did hold a node in each of the
    // named settlements, and in several the entry says nothing about.
    for named in &settlements {
        assert!(
            reached.contains(named),
            "no node reached '{named}', so nothing here answered for it: {reached:?}"
        );
    }
    assert!(
        reached.len() >= settlements.len() + 4,
        "too few settlements to say what `attest` refuses: {reached:?}"
    );

    for node in nodes {
        if settlements.contains(&settlement_of(&world, node)) {
            world.run(&["attest", &run, node]).exited(0);
        }
    }
    world.release("running.go");
    world.release("idle.go");
}

#[test]
fn attesting_something_that_is_not_a_ready_human_action_is_refused_by_name() {
    let world = World::new("channel-attest-refuse");
    world.script("build.wait", "hold");
    let run = running(&world, "noaction", vec![agent("build", &[])]);

    // A running node is neither reference `attest` takes, and a name no node
    // has is neither either — and both refusals say what would have been
    // accepted rather than only what was not.
    for reference in ["build", "nowhere"] {
        world
            .run(&["attest", &run, reference])
            .exited(REFUSED)
            .err_has("not a ready, waiting human action")
            .err_has("nor a node that settled failed");
    }
    world.release("build.go");
}

#[test]
fn the_channel_server_relays_an_observer_frame_and_writes_back_the_verdict() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-serve");
    world.script("build.wait", "hold");
    let run = running(&world, "served", vec![agent("build", &[])]);

    // This is an observer member's judge side: it reads the frame that member
    // emits when it has something to raise, relays it to the planner, and
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

/// A live edit issued while the observer's side waits is the command path's, and
/// the wait is left standing.
///
/// The measured failure this prevents: an operator's correction, submitted
/// mid-supervision, was claimed off the reply queue by the observer member's
/// judge-side provider, which cannot read a graph edit as a supervisor ruling —
/// so the member died and the run went blind while somebody was watching it.
#[test]
fn a_commands_only_reply_reaches_the_command_path_while_the_observers_side_waits() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-reply-routing");
    world.script("build.wait", "hold");
    let run = running(&world, "routed", vec![agent("build", &[])]);

    let mut serving = world
        .cmd(&["channel", "serve", &run])
        // The wait itself is not what this journey is about: bounded well past
        // the handful of verbs below, so a slow machine answers the question
        // rather than the timeout answering it.
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"build is doing something odd; go on?","node":"build"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");

    // Read, so the question is outstanding and the observer member is between
    // turns with its judge side blocked on the answer.
    world.until("the frame to reach the planner", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });
    world.run(&["next", &run]).exited(0).out_has("go on?");
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision");

    // The operator corrects the run while it is being supervised. This is a live
    // edit and nothing else: there is no verdict in it for anybody to read.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({"version": 1, "commands": [
                {"op": "context", "id": "build", "note": "the scope changed", "deliver": "next"}
            ]})
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
    world.until("the edit to reach the graph", |world| {
        !world.events_of(&run, "edit-committed").is_empty()
    });

    // It answered nothing, because it said nothing: the question is still
    // outstanding, the subtree it holds is still held, and the observer's side
    // is still there.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision");
    assert!(
        world.events_of(&run, "decision-cleared").is_empty(),
        "a live edit cleared a decision it never answered: {:?}",
        world.kinds(&run)
    );
    assert!(
        serving
            .try_wait()
            .expect("the observer's side is readable")
            .is_none(),
        "the observer's side ended on a live edit it was never sent"
    );

    // The planner answers. That is what reaches the observer's conversation, and
    // it is the *first* thing that does: the edit never entered this queue.
    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"completion":false,"reason":"carry on"}"#,
        )
        .exited(0);
    let stdout = serving.stdout.take().expect("stdout is piped");
    let first = BufReader::new(stdout)
        .lines()
        .next()
        .expect("the server wrote a line")
        .expect("the line reads");
    assert!(
        !first.contains("commands"),
        "a graph edit was written back to the observer as a ruling: {first}"
    );
    assert!(
        first.contains("carry on"),
        "the observer's side was handed something other than the planner's verdict: {first}"
    );

    drop(stdin);
    world.release("build.go");
    let _ = serving.wait();
}

/// A live edit the **reconciler** rejects is the command path's too.
///
/// The rejection comes back to the process that submitted it, not to the
/// channel: the question the observer's side asked is still unanswered, and a
/// refusal is no more a ruling than the edit was.
#[test]
fn a_rejected_commands_only_reply_leaves_the_observers_side_waiting() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-reply-rejected");
    // A turn that is open, beside a node that never dispatches: a `live` note
    // for the second passes the submission check and is refused by the
    // reconciler, which is the only way to be rejected from the durable queue.
    world.script("slow.turn-open", "");
    world.script("slow.wait", "hold");
    let path = world.plan(
        "refused",
        &plan_of(
            "refused",
            vec![agent("slow", &[]), agent("later", &["slow"])],
        ),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the held node's turn to open", |world| {
        !world.events_of("refused", "turn-started").is_empty()
    });

    let mut serving = world
        .cmd(&["channel", "serve", "refused"])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"slow has been at this a while; go on?","node":"slow"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the frame to reach the planner", |world| {
        !world
            .events_of("refused", "planner-surface-queued")
            .is_empty()
    });
    world.run(&["next", "refused"]).exited(0);

    world
        .run_with_stdin(
            &["reply", "refused"],
            &json!({"version": 1, "commands": [
                {"op": "context", "id": "later", "note": "start from the fixture", "deliver": "live"}
            ]})
            .to_string(),
        )
        .exited(REFUSED)
        .err_has("no controllable turn in flight");

    assert!(
        world.events_of("refused", "edit-committed").is_empty(),
        "a rejected edit reached the graph: {:?}",
        world.kinds("refused")
    );
    world
        .run(&["status", "refused"])
        .exited(0)
        .out_has("waiting for planner decision");
    assert!(
        serving
            .try_wait()
            .expect("the observer's side is readable")
            .is_none(),
        "the observer's side ended on an edit that was never even applied"
    );

    world
        .run_with_stdin(
            &["reply", "refused"],
            r#"{"completion":false,"reason":"go on without the note"}"#,
        )
        .exited(0);
    let stdout = serving.stdout.take().expect("stdout is piped");
    let first = BufReader::new(stdout)
        .lines()
        .next()
        .expect("the server wrote a line")
        .expect("the line reads");
    assert!(
        first.contains("go on without the note"),
        "the observer's side was handed something other than the planner's verdict: {first}"
    );

    drop(stdin);
    world.release("slow.go");
    let _ = serving.wait();
}

/// The same routing where the reply process is the one applying the edit.
///
/// Nothing is driving this run, so `reply` takes the ownership lock and
/// reconciles the edit itself rather than queuing it for a loop. Which of the
/// two applied it is an accident of what was running; the reader waiting for a
/// ruling must not be handed the edit either way.
#[test]
fn a_commands_only_reply_applied_under_the_lock_leaves_the_observers_side_waiting() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-reply-unlocked");
    let path = world.plan(
        "underlock",
        &plan_of("underlock", vec![human("approve", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);

    let mut serving = world
        .cmd(&["channel", "serve", "underlock"])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"is anyone going to approve this?","node":"approve"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the frame to reach the planner", |world| {
        !world
            .events_of("underlock", "planner-surface-queued")
            .is_empty()
    });
    world.run(&["next", "underlock"]).exited(0);

    world
        .run_with_stdin(
            &["reply", "underlock"],
            &json!({"version": 1, "commands": [
                {"op": "add", "node": {"id": "late", "persona": "engineer", "task": "## What\nsweep"}}
            ]})
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
    assert!(
        !world.events_of("underlock", "edit-committed").is_empty(),
        "the edit was not applied under the lock: {:?}",
        world.kinds("underlock")
    );
    world
        .run(&["status", "underlock"])
        .exited(0)
        .out_has("waiting for planner decision");
    assert!(
        serving
            .try_wait()
            .expect("the observer's side is readable")
            .is_none(),
        "the observer's side ended on a live edit it was never sent"
    );

    // The planner answers — and corrects the graph again in the same envelope,
    // which under the lock is one process doing both halves: the edit is applied
    // here and the verdict goes to the reader waiting for one.
    world
        .run_with_stdin(
            &["reply", "underlock"],
            &json!({
                "completion": false,
                "reason": "approve it yourself",
                "version": 1,
                "commands": [{"op": "drop", "id": "late", "dependents": "detach"}]
            })
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
    assert_eq!(
        world.events_of("underlock", "edit-committed").len(),
        2,
        "the commands half of the answering envelope was not applied: {:?}",
        world.kinds("underlock")
    );
    let stdout = serving.stdout.take().expect("stdout is piped");
    let first = BufReader::new(stdout)
        .lines()
        .next()
        .expect("the server wrote a line")
        .expect("the line reads");
    assert!(
        !first.contains("\"op\":\"add\""),
        "the commands-only edit applied under the lock was written back as a ruling: {first}"
    );
    assert!(
        first.contains("approve it yourself"),
        "the observer's side was handed something other than the planner's verdict: {first}"
    );

    drop(stdin);
    let _ = serving.wait();
}

/// A run carried across the upgrade: the live edit an older build already left
/// on the reply queue is passed over rather than handed out.
///
/// The routing keeps a commands-only envelope off this queue, but durable state
/// outlives the build that wrote it, and a run in flight when this landed can
/// have one sitting there already. It is skipped, the reader it was never for is
/// not ended by it, and the cursor does not move until that reader takes
/// something — the verdict behind it, exactly once.
#[test]
fn a_live_edit_an_older_build_left_on_the_reply_queue_is_passed_over() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-reply-legacy");
    world.script("build.wait", "hold");
    let run = running(&world, "upgraded", vec![agent("build", &[])]);

    // llmlint: ignore-block[tests_mirror_real_usage] this writes the durable reply queue
    // directly because the case under test is one no build in this tree can produce any
    // more: an envelope the *previous* build queued there before the routing existed.
    // Through the front door `reply` now routes it to the command path, which is the fix
    // — and would leave the reader this journey is about with nothing to skip.
    std::fs::write(
        world.run_file(&run, "channel/replies.jsonl"),
        format!(
            "{}\n",
            json!({
                "id": 0,
                "at": 1,
                "reply": {"version": 1, "commands": [
                    {"op": "context", "id": "build", "note": "from the old build"}
                ]}
            })
        ),
    )
    .expect("the older build's reply is queued");

    // llmlint: ignore-end[tests_mirror_real_usage]
    let mut serving = world
        .cmd(&["channel", "serve", &run])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"is the old note still right?","node":"build"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the frame to reach the planner", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });

    // Waiting, with an envelope it will not take sitting in front of it: the
    // cursor stays where it is, because only a claim moves it and this reader
    // has claimed nothing.
    assert!(
        !world.run_file(&run, "channel/replies-cursor.json").exists(),
        "a reader that took nothing advanced the cursor past a waiting envelope"
    );

    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"completion":false,"reason":"still right; carry on"}"#,
        )
        .exited(0);
    let stdout = serving.stdout.take().expect("stdout is piped");
    let first = BufReader::new(stdout)
        .lines()
        .next()
        .expect("the server wrote a line")
        .expect("the line reads");
    assert!(
        first.contains("still right; carry on"),
        "the reader was handed the older build's edit instead of the verdict: {first}"
    );

    // And the cursor moved over both: the one it took, and the one behind it
    // whose reader is the command queue, which holds its own copy.
    world.until("the cursor to be written", |world| {
        world.run_file(&run, "channel/replies-cursor.json").exists()
    });
    assert_eq!(
        world.run_json(&run, "channel/replies-cursor.json"),
        json!(2),
        "the reply the reader took is claimable a second time"
    );

    drop(stdin);
    world.release("build.go");
    let _ = serving.wait();
}

/// An envelope carrying both halves is delivered to both readers.
///
/// The edits are the reconciler's and the verdict is the pending surface's, and
/// a planner who corrects the graph *and* rules in one envelope gets both — the
/// routing splits the envelope by what is in it, and never drops a half.
#[test]
fn a_reply_carrying_both_halves_reaches_both_readers() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-reply-both");
    world.script("build.wait", "hold");
    let run = running(&world, "bothhalves", vec![agent("build", &[])]);

    let mut serving = world
        .cmd(&["channel", "serve", &run])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"build looks stuck; retry it?","node":"build"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the frame to reach the planner", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });
    world.run(&["next", &run]).exited(0);

    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "completion": false,
                "reason": "noted — carry on with the note in hand",
                "version": 1,
                "commands": [
                    {"op": "context", "id": "build", "note": "the fixture moved", "deliver": "next"}
                ]
            })
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");

    world.until("the edit to reach the graph", |world| {
        !world.events_of(&run, "edit-committed").is_empty()
    });
    world.until("the decision to be cleared", |world| {
        !world.events_of(&run, "decision-cleared").is_empty()
    });
    let stdout = serving.stdout.take().expect("stdout is piped");
    let written = BufReader::new(stdout)
        .lines()
        .next()
        .expect("the server wrote a line")
        .expect("the line reads");
    assert!(
        written.contains("with the note in hand"),
        "the verdict half never reached the observer: {written}"
    );

    drop(stdin);
    world.release("build.go");
    let _ = serving.wait();
}

/// Both readers on the queue at once: neither loses a message and neither is
/// handed one twice.
///
/// Two rounds, because one proves only that the right envelope arrived. The
/// second proves the cursor moved over exactly what its reader took: a verdict
/// already read is never read again, and the live edits between the two rounds
/// are the reconciler's every time.
#[test]
fn the_two_readers_contend_for_the_channel_without_losing_or_repeating_a_reply() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-reply-contention");
    world.script("build.wait", "hold");
    let run = running(&world, "contended", vec![agent("build", &[])]);

    let mut serving = world
        .cmd(&["channel", "serve", &run])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    let stdout = serving.stdout.take().expect("stdout is piped");
    let mut written = BufReader::new(stdout).lines();

    let mut edits = 0;
    for (round, verdict) in [(1, "first ruling"), (2, "second ruling")] {
        writeln!(
            stdin,
            r#"{{"kind":"blocker","message":"round {round}: go on?","node":"build"}}"#
        )
        .expect("the frame is written");
        stdin.flush().expect("flushed");
        world.until("the frame to reach the planner", |world| {
            world.events_of(&run, "planner-surface-queued").len() >= round
        });
        world.run(&["next", &run]).exited(0);

        // Live edits and a ruling, contending for one queue. Every edit is the
        // reconciler's, whatever order they arrive in.
        for note in ["the scope changed", "and again"] {
            world
                .run_with_stdin(
                    &["reply", &run],
                    &json!({"version": 1, "commands": [
                        {"op": "context", "id": "build", "note": note, "deliver": "next"}
                    ]})
                    .to_string(),
                )
                .exited(0)
                .out_has("\"applied\"");
            edits += 1;
        }
        world
            .run_with_stdin(
                &["reply", &run],
                &json!({"completion": false, "reason": verdict}).to_string(),
            )
            .exited(0);

        // Every edit reached the graph exactly once, and this round's verdict —
        // and only this round's — reached the observer.
        world.until("the edits to reach the graph", |world| {
            world.events_of(&run, "edit-committed").len() >= edits
        });
        assert_eq!(
            world.events_of(&run, "edit-committed").len(),
            edits,
            "an edit was reconciled more than once: {:?}",
            world.events_of(&run, "edit-committed")
        );
        let line = written
            .next()
            .expect("the server wrote a line")
            .expect("the line reads");
        assert!(
            line.contains(verdict),
            "round {round} read back something other than its own verdict: {line}"
        );
        assert!(
            !line.contains("commands"),
            "a graph edit was written back to the observer as a ruling: {line}"
        );
    }

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
    assert_eq!(pending[0]["payload"]["kind"], "attestation");
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
    assert_eq!(
        world.run_json("decided", "result.json")["state"],
        "complete"
    );
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

    // Every op the monitor may not issue is refused by name, with the reason and
    // what to do instead — and nothing durable is written on any of their
    // behalf. All four, because an op granted by omission is the whole failure
    // this allowlist exists to prevent.
    for (op, command, said) in [
        (
            "attest",
            json!({"op": "attest", "ref": "approve"}),
            "never by a watcher",
        ),
        (
            "complete",
            json!({"op": "complete", "reason": "looks finished to me"}),
            "not an observation",
        ),
        (
            "drop",
            json!({"op": "drop", "id": "slow", "dependents": "detach"}),
            "decomposition decision the planner owns",
        ),
        (
            "reparent",
            json!({"op": "reparent", "id": "slow", "deps": []}),
            "decomposition decision the planner owns",
        ),
    ] {
        let refused = world.run_with_stdin(
            &["reply", "watched"],
            &json!({"version": 1, "author": "monitor", "commands": [command]}).to_string(),
        );
        refused
            .exited(REFUSED)
            .err_has(op)
            .err_has(said)
            .err_has("Surface it to the planner");
    }
    assert!(
        world.events_of("watched", "human-attested").is_empty(),
        "a refused monitor edit still reached the run"
    );
    assert!(
        world
            .events_of("watched", "completion-requested")
            .is_empty(),
        "a refused monitor edit still reached the run"
    );
    assert!(
        world.events_of("watched", "edit-committed").is_empty(),
        "a refused monitor edit still reached the graph: {:?}",
        world.kinds("watched")
    );

    // And every op it *may* issue is applied. In an order each one is legal in:
    // a note to the running node, a node added, that node parked and brought
    // back, and finally the running node superseded — which is the one that
    // stops it, so it goes last.
    for command in [
        json!({"op": "context", "id": "slow", "note": "the fixture moved", "deliver": "next"}),
        // Behind the held node, so it is still pending when it is parked: a
        // node that had already run is not a node `cancel` can idle.
        json!({"op": "add", "node": {"id": "extra", "persona": "engineer",
                                     "task": "## What\nsweep", "deps": ["slow"]}}),
        json!({"op": "cancel", "id": "extra"}),
        json!({"op": "requeue", "id": "extra"}),
        json!({"op": "retry", "id": "slow",
               "node": {"id": "slow-2", "persona": "engineer", "task": "## What\nagain"}}),
    ] {
        world
            .run_with_stdin(
                &["reply", "watched"],
                &json!({"version": 1, "author": "monitor", "commands": [command]}).to_string(),
            )
            .exited(0)
            .out_has("\"applied\"");
    }

    let committed = world.events_of("watched", "edit-committed");
    assert_eq!(committed.len(), 5, "{committed:?}");
    for edit in &committed {
        assert_eq!(edit["payload"]["author"], "monitor", "{edit}");
    }

    world.until("the planner to be told what the monitor did", |world| {
        world
            .events_of("watched", "planner-surface-queued")
            .iter()
            .filter(|event| event["payload"]["kind"] == "monitor-edit")
            .count()
            >= 5
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

/// The other decision point: a **blocking surface** an observer raised holds the
/// subtree that depends on the node it named, and answering it releases exactly
/// that subtree.
///
/// A `kind: human` node is structural — its dependents are blocked by the graph.
/// This one is not: the node the surface names has *settled*, and what holds its
/// dependents back is the unanswered question about it. Nothing else in the run
/// is touched.
#[test]
fn a_blocking_surface_holds_the_subtree_of_the_node_it_names_until_it_is_answered() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-surface-decision");
    // `seed` is held so the frame below lands before it settles: the loop starts
    // what became ready on the same pass it sees the settlement, so a surface
    // that arrived afterwards would be racing a dispatch that had already gone.
    world.script("seed.wait", "hold");
    world.script("keep.wait", "hold");
    let path = world.plan(
        "surfacegate",
        &plan_of(
            "surfacegate",
            vec![
                agent("seed", &[]),
                agent("after", &["seed"]),
                agent("keep", &[]),
            ],
        ),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the held node to be in flight", |world| {
        world
            .events_of("surfacegate", "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "seed")
    });

    // The observer raises a blocking question about `seed`.
    let mut serving = world
        .cmd(&["channel", "serve", "surfacegate"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"seed wrote something unexpected; go on?","node":"seed"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");

    world.until("the decision to be reported", |world| {
        !world
            .events_of("surfacegate", "decision-pending")
            .is_empty()
    });
    let pending = world.events_of("surfacegate", "decision-pending");
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0]["payload"]["kind"], "blocker");
    assert!(
        pending[0]["payload"]["reference"]
            .as_str()
            .is_some_and(|reference| reference.starts_with("surface:")),
        "a surface decision named something other than a surface: {pending:?}"
    );
    assert_eq!(pending[0]["payload"]["unblocks"], json!(["after"]));

    // The node it names settles — and its dependent does *not* go, because the
    // question about it has not been answered.
    world.release("seed.go");
    world.until("the named node to settle", |world| {
        world
            .events_of("surfacegate", "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "seed")
    });
    assert!(
        world
            .events_of("surfacegate", "node-dispatched")
            .iter()
            .all(|event| event["labels"]["node"] != "after"),
        "the held subtree ran while the question about it was outstanding: {:?}",
        world.kinds("surfacegate")
    );

    // Answered: read it, then reply. The subtree is released and nothing else
    // waited on it.
    world.run(&["next", "surfacegate"]).exited(0);
    world
        .run_with_stdin(
            &["reply", "surfacegate"],
            r#"{"completion":false,"reason":"go on"}"#,
        )
        .exited(0);
    world.until("the released subtree to settle", |world| {
        world
            .events_of("surfacegate", "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "after")
    });
    let cleared = world.events_of("surfacegate", "decision-cleared");
    assert_eq!(cleared.len(), 1, "{cleared:?}");
    assert_eq!(cleared[0]["payload"]["released"], json!(["after"]));

    // The verdict reached the observer's own conversation, which is what makes
    // this a channel rather than a one-way report.
    let stdout = serving.stdout.take().expect("stdout is piped");
    let verdict = BufReader::new(stdout)
        .lines()
        .map_while(std::result::Result::ok)
        .find(|line| line.contains("reason"))
        .expect("the server wrote a verdict");
    assert!(verdict.contains("go on"), "{verdict}");

    drop(stdin);
    world.release("keep.go");
    let _ = serving.wait();
}

/// A blocking surface that names no node holds no subtree — and is still what
/// the run is waiting on.
///
/// The other half of the decision contract: what a surface pauses is the
/// subtree of the node it named, so one that named none pauses nothing. It does
/// not therefore *cost* nothing: a run that cannot move with a question
/// outstanding is awaiting the planner, not abandoned, and the two send an
/// operator to different places.
#[test]
fn a_blocking_surface_naming_no_node_pauses_nothing_and_still_awaits_the_planner() {
    use std::io::Write;

    let world = World::new("channel-surface-runwide");
    // The one node fails, so the graph stops moving with nothing ready, nothing
    // waiting on a person, and — until the frame below — no question to answer.
    world.script("build.fail", "1");
    let path = world.plan("runwide", &plan_of("runwide", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(NOTHING_DRIVING)
        .out_has("\"settlement\":\"unattended\"");

    // A blocking question about the run rather than about any node in it.
    let mut serving = world
        .cmd(&["channel", "serve", "runwide"])
        // Nobody answers this one, and the server's own wait is not what is
        // under test: shortened so the journey is not the timeout.
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"the whole plan looks wrong; what now?"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the question to reach the planner", |world| {
        !world
            .events_of("runwide", "planner-surface-queued")
            .is_empty()
    });

    // The same run, driven again: it still cannot move, and now it says why.
    world
        .run(&["adopt", "runwide"])
        .exited(0)
        .out_has("\"settlement\":\"awaiting-planner\"");

    // And it held nothing back: the surface named no node, so its subtree is
    // empty and no dispatch was skipped on its account.
    let pending = world.events_of("runwide", "decision-pending");
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0]["payload"]["unblocks"], json!([]));

    drop(stdin);
    let _ = serving.wait();
}

/// A frame naming a node the run does not have is refused, not queued.
///
/// The node decides what a blocking frame holds back, so a name the graph does
/// not carry would raise a question about work nobody is doing — and hold
/// nothing while reading as something the run is waiting on.
#[test]
fn the_channel_server_refuses_a_frame_about_a_node_the_run_does_not_have() {
    use std::io::Write;

    let world = World::new("channel-frame-node");
    world.script("build.wait", "hold");
    let run = running(&world, "unknownnode", vec![agent("build", &[])]);

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
        r#"{{"kind":"blocker","message":"what about this?","node":"nowhere"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    drop(stdin);

    let output = serving.wait_with_output().expect("the server exits");
    assert_eq!(output.status.code(), Some(REFUSED), "{output:?}");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("nowhere"), "{said}");
    // And it names what the run does have, so the observer can correct itself.
    assert!(said.contains("build"), "{said}");
    assert!(
        world.events_of(&run, "planner-surface-queued").is_empty(),
        "a frame about a node nobody has still reached the planner"
    );
    world.release("build.go");
}

/// The monitor may not declare the run finished, in a verdict any more than in
/// an op.
///
/// The legacy verdict says what `complete` says, in a field rather than in a
/// command list — so an allowlist that guarded only the ops would let a
/// commandless reply walk straight past it.
#[test]
fn a_monitor_cannot_declare_the_run_complete_with_a_commandless_verdict() {
    let world = World::new("channel-monitor-verdict");
    world.script("build.wait", "hold");
    let run = running(&world, "verdict", vec![agent("build", &[])]);

    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "author": "monitor",
                "completion": true,
                "reason": "looks finished to me",
            })
            .to_string(),
        )
        .exited(REFUSED)
        .err_has("not something the monitor may do")
        .err_has("Surface it to the planner");
    assert!(
        world.events_of(&run, "completion-requested").is_empty(),
        "the monitor declared the run complete: {:?}",
        world.kinds(&run)
    );

    // The planner's own verdict is unaffected, which is what makes the refusal
    // about the author rather than about the field.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({"completion": true, "reason": "the run is finished"}).to_string(),
        )
        .exited(0);
    assert_eq!(world.events_of(&run, "completion-requested").len(), 1);
    world.release("build.go");
}

/// An edit the monitor applies to a run nothing is driving is surfaced to the
/// planner exactly as one applied by the loop is.
///
/// Which of the two applied it is an accident of whether anything was driving
/// the run; the planner owns the graph either way, and learning about the edit
/// is not something they should have to be lucky to do.
#[test]
fn a_monitor_edit_applied_with_nothing_driving_is_still_surfaced_to_the_planner() {
    let world = World::new("channel-monitor-undriven");
    let path = world.plan(
        "undrivenmonitor",
        &plan_of("undrivenmonitor", vec![human("approve", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);

    world
        .run_with_stdin(
            &["reply", "undrivenmonitor"],
            &json!({
                "version": 1,
                "author": "monitor",
                "commands": [{"op": "add", "node": {"id": "sweep", "persona": "engineer",
                                                    "task": "## What\nsweep"}}],
            })
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");

    let committed = world
        .events_of("undrivenmonitor", "edit-committed")
        .into_iter()
        .next()
        .expect("the edit was applied");
    assert_eq!(committed["payload"]["author"], "monitor", "{committed}");
    let surfaced = world
        .events_of("undrivenmonitor", "planner-surface-queued")
        .into_iter()
        .find(|event| event["payload"]["kind"] == "monitor-edit")
        .expect("the monitor's edit was surfaced to the planner");
    assert_eq!(surfaced["payload"]["blocking"], json!(false), "{surfaced}");
    assert_eq!(surfaced["payload"]["source"], "monitor", "{surfaced}");
}
