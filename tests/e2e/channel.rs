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

use crate::harness::{agent, ended, human, plan_of, World, NOTHING_DRIVING, REFUSED};
use serde_json::{json, Value};

/// Start a run detached and wait until it is executing.
fn running(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });
    name.to_string()
}

/// Wait for `ready`, failing at once — and with what it was handed — if the
/// observer member died instead.
///
/// A member that has died satisfies no wait this journey makes, so one that only
/// timed out would report the wait rather than the death that made it
/// impossible.
fn until_still_supervising(world: &World, what: &str, mut ready: impl FnMut(&World) -> bool) {
    world.until(what, |world| {
        let died = world
            .observer_supervision()
            .into_iter()
            .find(|record| !record["died"].is_null());
        assert!(
            died.is_none(),
            "the observer member died rather than {what}: its judge side was answered with {}",
            died.expect("a death")["died"]
        );
        ready(world)
    });
}

/// The same, with an observer graph attached.
///
/// Only the pacemaker journeys need one: the clock a surface resets belongs to a
/// member of that graph, and a run launched with `--dag-graph off` — the shipped
/// default — has no member to address.
///
/// The observer is **held** at its first instruction, which is what keeps the id
/// it answers to still. A double that is not held announces itself and exits at
/// once, the driver starts another in its place, and the launch record's
/// `graph_run` moves to the replacement — so a journey that reads that value and
/// then consumes a surface is addressing whichever observer the restart loop
/// happened to be on, and on a slow host it has read one and seen the reset use
/// the other. A real monitor member stays up for the run it watches, which is
/// also the only state in which resetting its clock means anything. Callers
/// release it with `observer.go`.
fn observed(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    world.script("observer.wait", "hold");
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&[
            "start",
            &path,
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
    // The premise of the assertion below, stated rather than assumed: one
    // observer, so the id read above is still the id a reset addresses. A
    // replacement started between the two would move `graph_run` on to it, and
    // the mismatch that follows reads as the crate having addressed the wrong
    // run rather than as this journey having read a superseded one.
    assert_eq!(
        world.observer_saw().len(),
        1,
        "the observer was replaced mid-journey, so the id read above is not the one addressed: {:?}",
        world.observer_saw()
    );
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
    world.release("observer.go");
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
    stdin.flush().expect("the frames flush");

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
    // Held open until here on purpose: the server is the reader waiting on every
    // one of those answers, and a stream closed before the render would have
    // said nobody was waiting on any of them.
    drop(stdin);
    ended(serving);
    world.release("build.go");
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the three journeys below
// cost 10.8s together and hold no two-party turn open, so the one separately-edged project
// here — `onepipeline-note-journeys`, edged on conversational cost — would put them where a
// change to `src/channel.rs` does not run them, which is the one change that must.
/// A surface whose server exited with the side that asked already gone stops
/// counting as one the planner is waiting on.
///
/// This is the observer member's own surface. Its conversation ends, the graph
/// tears down, and the `channel serve` that raised the question reaches the end
/// of its frame stream and exits — with nothing answered and nothing left that
/// could read an answer. Left standing, that entry sat in every status render
/// and every watch heartbeat as one unread planner update for an hour and a
/// half, degrading the one line a supervising manager is forbidden to filter.
///
/// Nothing is deleted to fix it: both texts are still there to read, and `next`
/// still hands them over saying which they are.
#[test]
fn a_surface_whose_server_exited_with_its_asker_gone_stops_counting_as_unread() {
    use std::io::Write;

    let world = World::new("channel-no-reader");
    world.script("build.wait", "hold");
    let run = running(&world, "noreader", vec![agent("build", &[])]);

    // The observer's judge side: the one question it stopped to ask, and one
    // report beside it. A one-second bound makes each wait its own synthesized
    // `continue`, so nothing here is ever answered.
    let mut command = world.cmd(&["channel", "serve", &run]);
    command
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let mut serving = command.spawn().expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    stdin
        .write_all(
            concat!(
                r#"{"kind":"planner-question","message":"Which base should build target?"}"#,
                "\n",
                r#"{"kind":"monitor","message":"the worker went quiet","blocking":false}"#,
                "\n",
            )
            .as_bytes(),
        )
        .expect("the frames write");
    stdin.flush().expect("the frames flush");
    world.until("both frames to reach the planner", |world| {
        world.events_of(&run, "planner-surface-queued").len() == 2
    });

    // While the member is still there, both are exactly what they look like:
    // unread updates, one of them a decision the run is held on.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("2 planner update(s) waiting");

    // The member's conversation ends. Its judge side reaches the end of the
    // frame stream and exits, having been answered nothing.
    drop(stdin);
    ended(serving);

    // Neither view counts them any more, and the run no longer says it is
    // waiting for a planner.
    for view in [vec!["runs"], vec!["status", &run]] {
        world
            .run(&view)
            .exited(0)
            .out_lacks("planner update(s) waiting")
            .out_lacks("waiting for planner");
    }
    // Said out loud rather than vanished, so an operator can still find them.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("2 planner update(s) nobody is waiting on");

    // The text is not lost: the queue still hands both over, and a reader can
    // tell what they are from the surface itself.
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("Which base should build target?");
    assert_eq!(read.json()["status"], "surface");
    assert_eq!(read.json()["surface"]["abandoned"], json!(true));

    // And the run's own record says what became of each, under its own id:
    // one line per surface saying it was abandoned, carrying the surface as it
    // then stood. The line is picked by what it says happened rather than by
    // the flag it carries, because every later line about the surface — the
    // claim above included — carries that flag too.
    let record = std::fs::read_to_string(world.run_file(&run, "channel/surfaces.jsonl"))
        .expect("the run recorded its surfaces");
    let abandoned: Vec<serde_json::Value> = record
        .lines()
        .map(|line| {
            // Every line of that record is one the run wrote, so a line that
            // does not parse is the defect this journey would otherwise skip
            // over on its way to a count that happened to come out right.
            serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|e| {
                panic!("the run wrote a surface record that is not JSON ({e}): {line}")
            })
        })
        .filter(|surface| surface["event"] == json!("abandoned"))
        .collect();
    assert_eq!(abandoned.len(), 2, "{record}");
    assert_eq!(abandoned[0]["id"], json!(0));
    assert!(abandoned[0]["abandoned"] == json!(true), "{record}");
    assert_eq!(abandoned[1]["id"], json!(1));
    assert!(abandoned[1]["abandoned"] == json!(true), "{record}");

    world.release("build.go");
}

/// A decision whose asker has gone releases the subtree it was holding, gives up
/// the pending slot, and takes its place behind everything somebody is still
/// waiting on.
///
/// The three halves the other two journeys leave out, and each is only true
/// through the running loop. A blocking surface a planner has *read* sits in the
/// pending slot rather than the queue, and it holds `ship` back by way of
/// `decisions_now`; when its asker goes, the slot has to be given up, the
/// decision has to clear inside the loop that is already running, and the node it
/// paused has to dispatch. Afterwards the surface is still there to read — and
/// still behind a live report queued after it, because nothing is waiting on it
/// and something is waiting on that.
#[test]
fn a_decision_nobody_is_waiting_on_releases_its_subtree_and_reads_last() {
    use std::io::Write;

    let world = World::new("channel-released");
    world.script("build.wait", "hold");
    world.script("ship.wait", "hold");
    let run = running(
        &world,
        "released",
        vec![agent("build", &[]), agent("ship", &["build"])],
    );

    // The observer's judge side stops to ask about `build`, which is what makes
    // the question hold everything downstream of it.
    let mut asking = world
        .cmd(&["channel", "serve", &run])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut asked = asking.stdin.take().expect("stdin is piped");
    writeln!(
        asked,
        r#"{{"kind":"blocker","message":"is this base still right?","node":"build"}}"#
    )
    .expect("the frame is written");
    asked.flush().expect("the frame flushes");
    world.until("the decision to begin holding the subtree", |world| {
        !world.events_of(&run, "decision-pending").is_empty()
    });

    // Read but not answered, which is the pending slot: the manager has the
    // text, and the run is still waiting for their ruling.
    world.run(&["next", &run]).exited(0);
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision")
        .out_lacks("planner update(s) waiting");

    // Its dependency settles, and `ship` is held by the decision rather than
    // dispatched — which is what a decision point is for.
    world.release("build.go");
    world.until("the node behind the decision to be held by it", |world| {
        world.events_of(&run, "node-held").iter().any(|event| {
            event["labels"]["node"] == "ship"
                && event["payload"]["reasons"]
                    .as_array()
                    .is_some_and(|reasons| reasons.iter().any(|held| held["kind"] == "decision"))
        })
    });
    assert!(
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .all(|event| event["labels"]["node"] != "ship"),
        "the held node ran while the decision was still outstanding: {:?}",
        world.kinds(&run)
    );

    // The member's conversation ends. Nobody is waiting for that ruling now.
    drop(asked);
    ended(asking);

    // The slot is given up, the decision clears inside the loop that is already
    // running, and the node it paused goes.
    world.until("the decision to clear", |world| {
        !world.events_of(&run, "decision-cleared").is_empty()
    });
    world.until("the node it was holding to dispatch", |world| {
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "ship")
    });
    // Said out loud from the slot it was delivered into, and not as a decision:
    // the run is not held on it, and its text is still in front of the manager
    // who was handed it.
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("waiting for planner")
        .out_has(
            "a planner update nobody is waiting on any more: blocker — is this base still right?",
        );

    // A live report queued after it goes first, though it is newer and holds
    // nothing: the older question is blocking and would have led the queue, and
    // it does not, because nobody is waiting on it.
    let mut reporting = world
        .cmd(&["channel", "serve", &run])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut reported = reporting.stdin.take().expect("stdin is piped");
    writeln!(
        reported,
        r#"{{"kind":"monitor","message":"the gate is green","blocking":false}}"#
    )
    .expect("the frame is written");
    reported.flush().expect("the frame flushes");
    world.until("the report to reach the planner", |world| {
        world.events_of(&run, "planner-surface-queued").len() == 2
    });

    let live = world.run(&["next", &run]);
    live.exited(0);
    assert_eq!(live.json()["surface"]["message"], "the gate is green");
    assert_eq!(live.json()["surface"]["abandoned"], serde_json::Value::Null);

    // And the question is not handed over a second time. It was delivered
    // before its asker went, and it stays in the slot it was delivered into —
    // both because a reader that already has it does not need it twice, and
    // because that slot is where a listener coming back for it looks. The run
    // still does not say it is waiting for a ruling nobody is owed.
    let after = world.run(&["next", &run]);
    after.exited(0);
    assert_eq!(after.json()["surface"], serde_json::Value::Null);
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("waiting for planner")
        .out_has(
            "a planner update nobody is waiting on any more: blocker — is this base still right?",
        );

    drop(reported);
    ended(reporting);
    world.release("ship.go");
}

/// A question stays answerable across its listener being replaced, twice, and
/// the verdict sent afterwards reaches the side that asked.
///
/// **This is the shape a dispatched agent's `ask-manager` wrapper actually
/// uses**, and the one the other journeys here leave out. That wrapper is not a
/// member holding a conversation open: it raises one blocking question through
/// one `channel serve`, and then waits for the verdict through a *succession* of
/// them, re-arming each time a listener exits with the question still open. Every
/// one of those exits is a frame stream that ended, and none of them is the asker
/// going anywhere — so a run that read the two as one fact took the question out
/// from under an agent that was still blocked on it, and the agent sat until it
/// was killed with nothing on either pipe. Nothing raised a surface and nothing
/// failed a node while that happened, which is why it is proven here rather than
/// left to a count.
///
/// Three real servers, each started and ended by this journey and none of them
/// signalled: the question outlives the first two and is answered through the
/// third.
#[test]
fn a_question_survives_its_listener_being_replaced_and_the_verdict_reaches_the_asker() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-rearm");
    world.script("build.wait", "hold");
    let run = running(&world, "rearmed", vec![agent("build", &[])]);
    let asker = "dispatch-that-is-still-blocked";

    // One listener of that asker: it raises the question, and its stream is a
    // pipe that is closed the moment the frame is written — which is what the
    // wrapper's own `printf | onepipeline channel serve` produces, and what
    // proves nothing about whether the agent behind it is still waiting.
    let listening = |frame: &str, window: &str| {
        let mut serving = world
            .cmd(&["channel", "serve", &run])
            .env(onepipeline::channel::ASKER_ENV, asker)
            .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", window)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the channel server starts");
        let mut stdin = serving.stdin.take().expect("stdin is piped");
        writeln!(stdin, "{frame}").expect("the frame is written");
        stdin.flush().expect("the frame flushes");
        drop(stdin);
        serving
    };

    // One re-arm: another listener of the same asker, saying only that somebody
    // is listening again. `nth` is how many have been sent, so the wait below is
    // for *this* one rather than for any of them.
    let rearm = |nth: usize| {
        let rearmed = listening(
            r#"{"kind":"planner-question","message":"a listener re-armed","blocking":false}"#,
            "1",
        );
        world.until(&format!("re-arm {nth} to reach the planner"), |world| {
            world
                .events_of(&run, "planner-surface-queued")
                .iter()
                .filter(|event| event["payload"]["message"] == "a listener re-armed")
                .count()
                >= nth
        });
        rearmed
    };

    // The question, and the listener that raised it going away without an
    // answer: a one-second window makes the wait its own synthesized `continue`,
    // so the listener ends exactly as the wrapper's does — having relayed
    // something that is not this question's answer.
    let asked = listening(
        r#"{"kind":"blocker","message":"is this base still right?","node":"build"}"#,
        "1",
    );
    world.until("the question to reach the planner", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });
    ended(asked);

    // One re-arm, and this one is the question **nobody has read yet**: it is in
    // the queue rather than in the slot, so what has to come back is its place in
    // the count a supervisor may not filter.
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("planner update(s) waiting")
        .out_has("1 planner update(s) nobody is waiting on");
    let rearmed = rearm(1);
    // Two, and the kinds say which: the re-arm's own note, and the question it
    // came back for — which is counted again, and is no longer one nobody is
    // waiting on.
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("nobody is waiting on")
        .out_has("2 planner update(s) waiting (1 blocker, 1 planner-question)");
    ended(rearmed);

    // The manager reads it, which is what puts a question where a verdict can
    // name it. Read here — after a listener has gone — because that is the
    // window the wrapper's re-arm falls in, and reading in it is what used to
    // consume the question into nothing.
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("is this base still right?");
    assert_eq!(read.json()["surface"]["abandoned"], json!(true));

    // And a re-arm against the question in the slot, which is the other half:
    // the asker is still there, so the question is still owed an answer, and a
    // verdict has a question to bind to again.
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("waiting for planner");
    let rearmed = rearm(2);
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision: blocker — is this base still right?");
    ended(rearmed);

    // A listener that takes the question back over and then stops **on its own
    // bound** leaves it exactly where it found it. That ending says only that
    // this process is done — the member is still there — so a session that has
    // just adopted an outstanding question must not turn round and give it up on
    // the way out.
    let mut bounded = world
        .cmd(&["channel", "serve", &run])
        .env(onepipeline::channel::ASKER_ENV, asker)
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .env("ONEPIPELINE_SERVE_SESSION_SECONDS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut armed = bounded.stdin.take().expect("stdin is piped");
    writeln!(
        armed,
        r#"{{"kind":"planner-question","message":"a bounded listener re-armed","blocking":false}}"#
    )
    .expect("the frame is written");
    armed.flush().expect("the frame flushes");
    world.until("the bounded re-arm to reach the planner", |world| {
        world
            .events_of(&run, "planner-surface-queued")
            .iter()
            .any(|event| event["payload"]["message"] == "a bounded listener re-armed")
    });
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision: blocker — is this base still right?");
    // Ended by its own bound with the stream still open in this journey's hand,
    // which is what makes the assertion after it about the ending rather than
    // about the stream.
    let reached = bounded.wait_with_output().expect("the session ends");
    assert!(reached.status.success(), "{reached:?}");
    assert!(
        String::from_utf8_lossy(&reached.stderr).contains("ONEPIPELINE_SERVE_SESSION_SECONDS"),
        "the session ended some other way than on its bound: {}",
        String::from_utf8_lossy(&reached.stderr)
    );
    drop(armed);
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision: blocker — is this base still right?");

    // The last listener is the one holding the wait when the manager finally
    // answers. The verdict names the question — and it reaches the asker, which
    // is the whole of what was lost.
    let mut waiting = listening(
        r#"{"kind":"planner-question","message":"a listener re-armed","blocking":false}"#,
        "60",
    );
    world.until("the last re-arm to reach the planner", |world| {
        world
            .events_of(&run, "planner-surface-queued")
            .iter()
            .filter(|event| event["payload"]["message"] == "a listener re-armed")
            .count()
            >= 3
    });
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision: blocker — is this base still right?");
    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"completion":false,"message":"yes, that base is still right","reason":"answered"}"#,
        )
        .exited(0);

    let stdout = waiting.stdout.take().expect("stdout is piped");
    let verdict = BufReader::new(stdout)
        .lines()
        .map_while(std::result::Result::ok)
        .find(|line| line.contains("completion"))
        .expect("the listener wrote a verdict back to its asker");
    assert!(
        verdict.contains("yes, that base is still right"),
        "the listener was handed something that is not the answer to its question: {verdict}"
    );

    // Answered, so the run is no longer waiting on it, and this time that is a
    // verdict rather than a listener exiting.
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("waiting for planner decision");
    ended(waiting);
    world.release("build.go");
}

/// A listener belonging to some *other* asker leaves an ended asker's question
/// exactly where it is.
///
/// The other direction, and the one that keeps the repair from being a
/// withdrawal of the fix it repairs. Taking a question back over is scoped to
/// the asker that raised it: were it scoped to the run, a question whose member
/// died would be resurrected — and would hold the subtree, and inflate the one
/// count a supervising manager may not filter — for as long as any unrelated
/// session happened to be serving that run, which is the defect the marking
/// exists to stop, arriving through another door.
#[test]
fn a_listener_of_another_asker_leaves_an_ended_askers_question_alone() {
    use std::io::Write;

    let world = World::new("channel-other-asker");
    world.script("build.wait", "hold");
    let run = running(&world, "otherasker", vec![agent("build", &[])]);

    let serving = |asker: &str, frame: &str| {
        let mut serving = world
            .cmd(&["channel", "serve", &run])
            .env(onepipeline::channel::ASKER_ENV, asker)
            .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the channel server starts");
        let mut stdin = serving.stdin.take().expect("stdin is piped");
        writeln!(stdin, "{frame}").expect("the frame is written");
        stdin.flush().expect("the frame flushes");
        drop(stdin);
        serving
    };

    // One asker's question, read by the manager and then left behind: this
    // asker's work is over, and nothing of it will come back.
    let asked = serving(
        "the-asker-that-ended",
        r#"{"kind":"blocker","message":"who owns this decision?","node":"build"}"#,
    );
    world.until("the question to reach the planner", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("who owns this decision?");
    // The surface says whose it is, which is the whole of what the scoping below
    // is decided on.
    assert_eq!(
        read.json()["surface"]["asker"],
        json!("the-asker-that-ended")
    );
    ended(asked);
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("waiting for planner")
        .out_has(
            "a planner update nobody is waiting on any more: blocker — who owns this decision?",
        );

    // A different asker serves the same run. It is a reader on this channel, and
    // it is still not the side that asked: the question stays where it is, the
    // run stays not-waiting, and the subtree stays released.
    let stranger = serving(
        "some-other-dispatch",
        r#"{"kind":"monitor","message":"unrelated work is green","blocking":false}"#,
    );
    world.until("the stranger's report to reach the planner", |world| {
        world.events_of(&run, "planner-surface-queued").len() == 2
    });
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("waiting for planner")
        .out_has(
            "a planner update nobody is waiting on any more: blocker — who owns this decision?",
        );
    ended(stranger);

    // And a question the stranger *is* waiting on takes the slot the abandoned
    // one was sitting in — a live question outranks one nobody is waiting on —
    // without taking its text down with it. The queue is the only place a reader
    // can still reach that text, so the displaced question is still handed over.
    let pressing = serving(
        "some-other-dispatch",
        r#"{"kind":"blocker","message":"whose call is the base?","node":"build"}"#,
    );
    world.until("the stranger's question to reach the planner", |world| {
        world.events_of(&run, "planner-surface-queued").len() == 3
    });
    let live = world.run(&["next", &run]);
    live.exited(0);
    assert_eq!(live.json()["surface"]["message"], "whose call is the base?");
    assert_eq!(live.json()["surface"]["abandoned"], serde_json::Value::Null);
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision: blocker — whose call is the base?");
    // Then what nobody is waiting on, in arrival order: the stranger's own
    // report, which was queued first, and behind it the question the live one
    // displaced out of the slot — still there, still saying what it is.
    let report = world.run(&["next", &run]);
    report.exited(0);
    assert_eq!(
        report.json()["surface"]["message"],
        "unrelated work is green"
    );
    let displaced = world.run(&["next", &run]);
    displaced.exited(0);
    assert_eq!(
        displaced.json()["surface"]["message"],
        "who owns this decision?"
    );
    assert_eq!(displaced.json()["surface"]["abandoned"], json!(true));
    ended(pressing);
    world.release("build.go");
}

/// The record of a hand-out says **when the text it carries was true**.
///
/// One queued surface handed out twice carries one instant and a second surface
/// carries its own, which is what lets a reader tell a drained backlog from a
/// condition that recurred — the reading a monitor got wrong off three records
/// it could not date. Divergence 66.
#[test]
fn a_delivered_surface_is_recorded_with_the_instant_it_was_queued() {
    use std::io::Write;

    let world = World::new("channel-surface-instant");
    world.script("build.wait", "hold");
    let run = running(&world, "surfaceinstant", vec![agent("build", &[])]);

    let serving = |asker: &str, frame: &str| {
        let mut serving = world
            .cmd(&["channel", "serve", &run])
            .env(onepipeline::channel::ASKER_ENV, asker)
            .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the channel server starts");
        let mut stdin = serving.stdin.take().expect("stdin is piped");
        writeln!(stdin, "{frame}").expect("the frame is written");
        stdin.flush().expect("the frame flushes");
        drop(stdin);
        serving
    };

    // One question, read once, and then left behind by the asker that raised it.
    let asked = serving(
        "the-asker-that-ended",
        r#"{"kind":"blocker","message":"who owns this decision?","node":"build"}"#,
    );
    world.until("the question to reach the planner", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });
    let first = world.run(&["next", &run]);
    first.exited(0).out_has("who owns this decision?");
    let queued_at = first.json()["surface"]["queued_at"].clone();
    assert!(queued_at.is_u64(), "{}", first.json());
    ended(asked);

    // A second question, from an asker that is waiting on it, which takes the
    // slot the first was sitting in and puts it back among the readable ones.
    let pressing = serving(
        "some-other-dispatch",
        r#"{"kind":"blocker","message":"whose call is the base?","node":"build"}"#,
    );
    world.until("the second question to reach the planner", |world| {
        world.events_of(&run, "planner-surface-queued").len() == 2
    });
    let second = world.run(&["next", &run]);
    second.exited(0);
    assert_eq!(
        second.json()["surface"]["message"],
        "whose call is the base?"
    );
    let second_queued_at = second.json()["surface"]["queued_at"].clone();

    // And the first, handed out a second time: the same queued surface, the same
    // text, and a second delivery of it.
    let again = world.run(&["next", &run]);
    again.exited(0);
    assert_eq!(
        again.json()["surface"]["message"],
        "who owns this decision?"
    );

    let handed = world.events_of(&run, "planner-surfaced");
    assert_eq!(handed.len(), 3, "{handed:?}");
    // Two hand-outs of one queued surface carry one instant, so a reader holding
    // both knows it is looking at one condition delivered twice rather than at a
    // condition that recurred.
    assert_eq!(handed[0]["payload"]["queued_at"], queued_at, "{handed:?}");
    assert_eq!(handed[2]["payload"]["queued_at"], queued_at, "{handed:?}");
    // And a separate surface carries its own, which is what makes the pair above
    // evidence of anything.
    assert_eq!(
        handed[1]["payload"]["queued_at"], second_queued_at,
        "{handed:?}"
    );
    assert_ne!(handed[1]["payload"]["queued_at"], queued_at, "{handed:?}");

    // The four fields the record already carried say exactly what the surface
    // handed over says, which is what they said before the fifth was added.
    let delivered = first.json();
    for field in ["kind", "message", "source", "blocking"] {
        assert_eq!(
            handed[0]["payload"][field], delivered["surface"][field],
            "the delivery record's `{field}` is no longer the delivered surface's"
        );
    }
    assert_eq!(handed[0]["payload"]["kind"], "blocker");
    assert_eq!(handed[0]["payload"]["message"], "who owns this decision?");
    assert_eq!(handed[0]["payload"]["blocking"], json!(true));

    ended(pressing);
    world.release("build.go");
}

/// A blocking surface whose server stopped while the side that asked stayed is
/// still there to be claimed and answered.
///
/// The discriminator is the *asker*, never the server. This is the ending that
/// separates the two: the server times out waiting for a verdict, reaches its
/// own `ONEPIPELINE_SERVE_SESSION_SECONDS` bound, and **exits by itself, exit
/// 0**, down the same path a stream that ended exits down — with the member's
/// frame stream still open in this journey's hand, because the member is still
/// working and still owed the answer.
///
/// Nothing here is killed, and that is the point: a server this journey killed
/// would never reach the decision under test at all, so the journey would pass
/// against a `serve` that withdrew on every exit. Reaching that decision and
/// having it come out the other way is the proof.
#[test]
fn a_blocking_surface_outlives_a_server_that_stopped_while_its_asker_stayed() {
    use std::io::Write;

    let world = World::new("channel-server-went");
    world.script("build.wait", "hold");
    let run = running(&world, "serverwent", vec![agent("build", &[])]);

    // One second for the verdict it will not get, and one second for the session
    // itself: the wait runs out first, and the bound is what ends the process
    // afterwards rather than anything this journey does to it.
    let mut command = world.cmd(&["channel", "serve", &run]);
    command
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .env("ONEPIPELINE_SERVE_SESSION_SECONDS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut serving = command.spawn().expect("the channel server starts");
    // Taken out of the child and held for the whole journey. It is the member's
    // end of the conversation: while this is alive the stream is open and the
    // asker has not gone, which is the entire premise being tested.
    let mut asking = serving.stdin.take().expect("stdin is piped");
    writeln!(
        asking,
        r#"{{"kind":"planner-question","message":"Which base should build target?"}}"#
    )
    .expect("the frame is written");
    asking.flush().expect("the frame flushes");
    world.until("the question to reach the planner", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });

    // Its own wait runs out, and then its own bound does, and it goes — cleanly,
    // and while still holding a stream nobody has closed.
    let went = serving
        .wait_with_output()
        .expect("the server ends on its own");
    assert!(
        went.status.success(),
        "the server did not end of its own accord: {went:?}"
    );
    let said = String::from_utf8_lossy(&went.stdout);
    assert!(
        said.contains("no planner reply") && said.contains("timed out"),
        "the server did not report its own timeout: {said}"
    );
    let why = String::from_utf8_lossy(&went.stderr);
    assert!(
        why.contains("ONEPIPELINE_SERVE_SESSION_SECONDS")
            && why.contains("stream still open")
            && why.contains("still waiting for an answer"),
        "the server did not say which of the two endings this was: {why}"
    );

    // Nothing was withdrawn. Both supervisory views still count it, and neither
    // reports it as one nobody is waiting on.
    for view in [vec!["runs"], vec!["status", &run]] {
        world
            .run(&view)
            .exited(0)
            .out_has("1 planner update(s) waiting")
            .out_lacks("nobody is waiting on");
    }
    // And the run's own record says nothing became of it, which is the direct
    // negative of the mark the other ending writes.
    let record = std::fs::read_to_string(world.run_file(&run, "channel/surfaces.jsonl"))
        .expect("the run recorded its surfaces");
    assert!(
        !record.contains("\"abandoned\""),
        "the run recorded an abandonment nobody asked for: {record}"
    );

    // Still claimable, still readable, and reading it puts the run back to
    // waiting for the ruling the member is still owed.
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("Which base should build target?");
    assert_eq!(read.json()["surface"]["abandoned"], serde_json::Value::Null);
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision");

    // And an answer sent to it afterwards is still accepted, which is what the
    // member that asked is waiting for.
    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"completion":false,"reason":"target main"}"#,
        )
        .exited(0)
        .out_has("\"delivered\"");
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("waiting for planner");

    // The member's end, closed only now that the journey is done with it.
    drop(asking);
    world.release("build.go");
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// A member that declares its work complete leaves nothing behind waiting on it
/// either, though its stream never ends.
///
/// The third way a session stops, and the one that reaches the same ending as a
/// stream that ended without looking anything like it: the member says
/// `completion: true` in a verdict this session carries back, its conversation
/// is over, and the server exits — with the frame stream still open in this
/// journey's hand, exactly as the bounded session leaves it. The stream is
/// therefore not what tells the two apart; whether the side that asked is still
/// there is, and here it is not.
///
/// A report it raised earlier and nobody read is the thing at stake: it would
/// otherwise sit in the unread count for the rest of the run with no reader for
/// its answer.
#[test]
fn a_member_that_declared_itself_complete_leaves_nothing_counted_as_unread() {
    use std::io::Write;

    let world = World::new("channel-completed");
    world.script("build.wait", "hold");
    let run = running(&world, "completed", vec![agent("build", &[])]);

    // Long enough that every wait below ends on the answer this journey sends
    // rather than on a bound: what stops this session is the member, not a clock.
    let mut command = world.cmd(&["channel", "serve", &run]);
    command
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let mut serving = command.spawn().expect("the channel server starts");
    let mut asking = serving.stdin.take().expect("stdin is piped");

    // One report, answered but never read: replying clears the wait, not the
    // queue, so it is still sitting there unread when the member finishes.
    writeln!(
        asking,
        r#"{{"kind":"monitor","message":"the worker went quiet","blocking":false}}"#
    )
    .expect("the report is written");
    asking.flush().expect("the report flushes");
    world.until("the report to reach the planner", |world| {
        world.events_of(&run, "planner-surface-queued").len() == 1
    });
    world
        .run_with_stdin(&["reply", &run], r#"{"completion":false,"reason":"noted"}"#)
        .exited(0);
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("1 planner update(s) waiting");

    // And now the member says it is done, which is what ends the session.
    writeln!(
        asking,
        r#"{{"kind":"monitor","message":"the worker is finished","blocking":false}}"#
    )
    .expect("the last frame is written");
    asking.flush().expect("the last frame flushes");
    world.until("the last frame to reach the planner", |world| {
        world.events_of(&run, "planner-surface-queued").len() == 2
    });
    world
        .run_with_stdin(&["reply", &run], r#"{"completion":true,"reason":"done"}"#)
        .exited(0);

    let went = serving
        .wait_with_output()
        .expect("the server ends on the member's own verdict");
    assert!(
        went.status.success(),
        "the server did not end on the completion it carried: {went:?}"
    );

    // Both are off the count nobody may filter, and both are still readable.
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("planner update(s) waiting")
        .out_has("2 planner update(s) nobody is waiting on");
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("the worker went quiet");
    assert_eq!(read.json()["surface"]["abandoned"], json!(true));

    // The stream is what the bounded session also leaves open, so it cannot be
    // what told the two endings apart — closed only now that this is done.
    drop(asking);
    world.release("build.go");
}

/// A member that has gone quiet does not hold a session past its bound.
///
/// The bound is a deadline, not something noticed between exchanges: a server
/// blocked on a stream nobody is writing to reaches it and stops anyway. That is
/// the difference between what the variable is named for and what a blocking
/// read on stdin would have made of it — an idle session outliving its own
/// bound for as long as the member stayed silent.
///
/// The stream stays open the whole time, so nothing is withdrawn; there is
/// simply nothing to withdraw, and the server says that rather than reporting a
/// count of none.
#[test]
fn a_quiet_stream_does_not_hold_a_session_past_its_bound() {
    let world = World::new("channel-quiet");
    world.script("build.wait", "hold");
    let run = running(&world, "quiet", vec![agent("build", &[])]);

    let mut command = world.cmd(&["channel", "serve", &run]);
    command
        .env("ONEPIPELINE_SERVE_SESSION_SECONDS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut serving = command.spawn().expect("the channel server starts");
    // Taken and held, and never written to: the member is there and has nothing
    // to say, which is the whole premise.
    let silent = serving.stdin.take().expect("stdin is piped");

    let went = serving
        .wait_with_output()
        .expect("the server ends on its own bound");
    assert!(
        went.status.success(),
        "the server did not end of its own accord: {went:?}"
    );
    let why = String::from_utf8_lossy(&went.stderr);
    assert!(
        why.contains("ONEPIPELINE_SERVE_SESSION_SECONDS")
            && why.contains("stream still open")
            && why.contains("it had raised nothing"),
        "the server did not say why it stopped on a quiet stream: {why}"
    );

    // Nothing was raised, so nothing is waiting and nothing was withdrawn.
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("planner update(s) waiting")
        .out_lacks("nobody is waiting on");

    drop(silent);
    world.release("build.go");
}
/// A surface queued while a reader was reading the channel survives that
/// reader's write-back of what it read.
///
/// The queue used to be one file, read, modified, and written back whole by
/// writer and reader alike with no lock, so a push landing inside a reader's
/// read-modify-write was overwritten by the reader's stale copy and gone for
/// good — with the queue file left saying `waiting: [], next_id: 0` beside a log
/// carrying the surface under id 0. That write-back is reproduced here at the
/// instant it landed, and the question is still counted, still handed over under
/// its id, and the next surface takes an id nothing has used.
#[test]
fn a_surface_queued_during_a_read_of_the_channel_survives_that_readers_write_back() {
    use std::io::Write;

    let world = World::new("channel-lost-update");
    world.script("seed.wait", "hold");
    let run = running(
        &world,
        "lostupdate",
        vec![agent("seed", &[]), agent("after", &["seed"])],
    );

    // The reader's read of the channel, made before the question exists: what
    // the manager's `next` read, and what it later wrote back.
    let read = world.run(&["next", &run]);
    read.exited(0);
    assert_eq!(read.json()["surface"], Value::Null);
    let queue = world.run_file(&run, "channel/queue.json");
    let stale = std::fs::read(&queue).expect("the reader left the queue it read");

    // The worker's blocking question, queued while that read is in flight.
    let mut serving = world
        .cmd(&["channel", "serve", &run])
        .env(onepipeline::channel::ASKER_ENV, "dispatch-seed")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"Which base should seed build on?","node":"seed"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the question to be queued", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });

    // The reader's write-back lands: its stale copy over the queue the question
    // was just written into. Renamed into place rather than written over, as the
    // reader's own atomic write was.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] the write-back is placed
    // because nothing user-facing can hold a reader between its read and its
    // write-back: the window is microseconds inside one `next`, and a journey
    // that raced real invocations against it would report the defect on the
    // runs it happened to hit. The bytes placed are exactly what that reader
    // wrote, taken from the reader itself, and everything before and after them
    // is driven through the CLI.
    // `surfaces_queued_while_the_channel_is_being_read_are_each_read_exactly_once`
    // is the concurrent journey over real invocations.
    let staged = queue.with_extension("staged");
    std::fs::write(&staged, &stale).expect("the stale copy is staged");
    std::fs::rename(&staged, &queue).expect("the stale copy lands");
    // llmlint: ignore-end[tests_mirror_real_usage]

    // The question is still there: counted by the supervisory views, holding
    // the subtree it named, and handed over under its own id.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("1 planner update(s) waiting");
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("Which base should seed build on?");
    assert_eq!(read.json()["status"], "surface");
    assert_eq!(read.json()["surface"]["id"], json!(0));
    assert_eq!(read.json()["surface"]["blocking"], json!(true));
    // Read is not answered: the run still awaits the verdict on it.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision: blocker — Which base should seed build on?");

    // And the id it was given is never handed out again: the next surface
    // takes the one after it rather than the one the stale copy said was free.
    let queued = world.run(&["surface", &run, "--kind", "finding", "--message", "noted"]);
    queued.exited(0);
    assert_eq!(queued.json()["surface"], json!(1));

    // The verdict names the question the reader was handed, and reaches the
    // worker that asked it.
    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"completion":false,"reason":"build on main"}"#,
        )
        .exited(0);
    let stdout = serving.stdout.take().expect("stdout is piped");
    let verdict = std::io::BufRead::lines(std::io::BufReader::new(stdout))
        .map_while(std::result::Result::ok)
        .find(|line| line.contains("reason"))
        .expect("the server wrote a verdict");
    assert!(verdict.contains("build on main"), "{verdict}");

    // The run's own record accounts for the question's whole life on its own:
    // queued, claimed, and answered, each under its id.
    let record = std::fs::read_to_string(world.run_file(&run, "channel/surfaces.jsonl"))
        .expect("the run recorded its surfaces");
    let events: Vec<(Value, Value)> = record
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap_or_else(|e| {
                panic!("the run wrote a surface record that is not JSON ({e}): {line}")
            })
        })
        .map(|line| (line["id"].clone(), line["event"].clone()))
        .collect();
    assert_eq!(
        events,
        vec![
            (json!(0), json!("queued")),
            (json!(0), json!("claimed")),
            (json!(1), json!("queued")),
            (json!(0), json!("answered")),
        ],
        "{record}"
    );

    drop(stdin);
    world.release("seed.go");
    ended(serving);
}

/// Surfaces queued by several writers while several readers read the channel
/// are each read exactly once, under distinct ids.
///
/// Real invocations, contending for real: every push is its own `surface`
/// process and every read its own `next`, with nothing between them but the
/// channel's own lock. What is asserted is the whole invariant the queue
/// promises — nothing queued is lost, nothing is delivered twice, and no id is
/// handed out twice. Sized to the invocations an ordinary journey here makes:
/// a dozen pushes and the reads that drain them, which is enough for the
/// writers to overlap each other and the readers.
#[test]
fn surfaces_queued_while_the_channel_is_being_read_are_each_read_exactly_once() {
    const WRITERS: usize = 3;
    const EACH: usize = 4;
    const READERS: usize = 2;

    let world = World::new("channel-contended");
    world.script("build.wait", "hold");
    let run = running(&world, "contended", vec![agent("build", &[])]);

    let writing = std::sync::atomic::AtomicBool::new(true);
    let read = std::sync::Mutex::new(Vec::<Value>::new());
    let queued = std::sync::Mutex::new(Vec::<(u64, String)>::new());
    std::thread::scope(|scope| {
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let (world, run, queued) = (&world, &run, &queued);
                scope.spawn(move || {
                    for n in 0..EACH {
                        let message = format!("writer {writer} finding {n}");
                        let pushed = world.run(&[
                            "surface",
                            run,
                            "--kind",
                            "finding",
                            "--message",
                            &message,
                        ]);
                        pushed.exited(0);
                        let id = pushed.json()["surface"].as_u64().expect("the surface's id");
                        queued.lock().expect("the list").push((id, message));
                    }
                })
            })
            .collect();
        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                let (world, run, read, writing) = (&world, &run, &read, &writing);
                scope.spawn(move || loop {
                    let next = world.run(&["next", run]);
                    next.exited(0);
                    let surface = next.json()["surface"].clone();
                    if !surface.is_null() {
                        read.lock().expect("the list").push(surface);
                        continue;
                    }
                    // A reader stops at the first empty read once every writer
                    // is done, so a surface the queue lost is a shortfall in
                    // what was read rather than a reader waiting for ever.
                    if !writing.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                })
            })
            .collect();
        // The writers are waited on by joining them, never by counting what
        // they queued: a writer that failed would leave that count short for
        // ever. And the readers are released **before** any writer's failure is
        // raised — a scope joins every thread it spawned on the way out, so a
        // panic here with the readers still looping on `writing` would wait on
        // them for ever and the failure would never be reported.
        let written: Vec<_> = writers.into_iter().map(|writer| writer.join()).collect();
        writing.store(false, std::sync::atomic::Ordering::SeqCst);
        let read_out: Vec<_> = readers.into_iter().map(|reader| reader.join()).collect();
        for writer in written {
            writer.expect("a writer finishes");
        }
        for reader in read_out {
            reader.expect("a reader finishes");
        }
    });

    let mut sent = queued.into_inner().expect("the list");
    sent.sort();
    let mut ids: Vec<u64> = sent.iter().map(|(id, _)| *id).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        (0..(WRITERS * EACH) as u64).collect::<Vec<_>>(),
        "an id was handed out twice or skipped: {sent:?}"
    );
    let mut got: Vec<(u64, String)> = read
        .into_inner()
        .expect("the list")
        .iter()
        .map(|surface| {
            (
                surface["id"].as_u64().expect("an id"),
                surface["message"].as_str().expect("a message").to_owned(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(got, sent, "a surface was lost or delivered twice");
    assert_eq!(
        world.events_of(&run, "planner-surfaced").len(),
        WRITERS * EACH,
        "the journal does not record one read per surface"
    );

    world.release("build.go");
}

/// A projection that is not a queue is rebuilt from the log, and a reader that
/// cannot write the rebuilt one back still answers.
///
/// A read that finds the projection unreadable or behind the log repairs it,
/// and the repair is a cache write — a run root the reader may not write into
/// is still a run whose record is intact, and a view that refused over it would
/// be the supervisory verb going dark on exactly the run it is asked about.
#[cfg(unix)]
#[test]
fn a_read_still_answers_from_the_log_when_it_cannot_write_the_projection_back() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new("channel-unwritable");
    world.script("build.wait", "hold");
    let run = running(&world, "unwritable", vec![agent("build", &[])]);
    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "finding",
            "--message",
            "still here",
        ])
        .exited(0);

    // The projection is not a queue any more — a write that died halfway —
    // and the directory it would be repaired into is one this reader may not
    // write.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] both halves of this state
    // are placed because nothing user-facing produces either: the projection is
    // written atomically, so only a writer dying between its temporary file and
    // its rename leaves a torn one, and a run root the reader may not write
    // into is the host's doing rather than the CLI's. What is under test is
    // driven through the CLI — whether `status` and `next` still answer for a
    // run whose record is intact.
    let queue = world.run_file(&run, "channel/queue.json");
    std::fs::write(&queue, b"{\"waiting\": [").expect("the projection is torn");
    let channel = queue.parent().expect("the channel directory").to_path_buf();
    let writable = std::fs::metadata(&channel)
        .expect("the channel directory")
        .permissions();
    std::fs::set_permissions(&channel, std::fs::Permissions::from_mode(0o555))
        .expect("the directory is made read-only");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let status = world.run(&["status", &run]);
    std::fs::set_permissions(&channel, writable).expect("the directory is writable again");
    status.exited(0).out_has("1 planner update(s) waiting");
    assert_eq!(
        std::fs::read(&queue).expect("the projection"),
        b"{\"waiting\": [",
        "the projection was written into a directory the reader may not write"
    );

    // Writable again, the next read repairs it and hands the surface over.
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("still here");
    let repaired: Value = serde_json::from_slice(&std::fs::read(&queue).expect("the projection"))
        .expect("the projection was repaired");
    assert_eq!(repaired["waiting"], json!([]), "{repaired}");

    world.release("build.go");
}

/// A run an older build left after the lost update — its queue saying nothing
/// was ever queued, beside a log carrying the question under id 0 — is brought
/// over with the question restored and the id never handed out again.
///
/// These are the files the observed failure left: `waiting: [], pending: null,
/// next_id: 0` while the log and the journal both carried the surface with id 0.
/// A projection with no stamp is an older build's, and that build logged a
/// surface only when it queued it, so the one thing its log can prove is a loss:
/// an id at or past the counter the projection holds is a write-back that was
/// overwritten. That question is what the supervising manager most needs back,
/// and it comes back through the same verbs that lost it.
#[test]
fn a_question_an_older_build_lost_from_its_queue_is_restored_from_its_log() {
    let world = World::new("channel-older-build");
    world.script("build.wait", "hold");
    let run = running(&world, "olderbuild", vec![agent("build", &[])]);

    // llmlint: ignore-block[tests_mirror_real_usage] the files are those of a
    // build this binary is not: it stamps every projection it writes and marks
    // every line it logs, so nothing it does can leave an unstamped projection
    // beside an unmarked log. What is under test is driven through the CLI —
    // whether the run answers for the question those files hold.
    let stale_read = concat!(
        r#"{"id":0,"kind":"blocker","message":"Which base should build target?","#,
        r#""source":"proposal","blocking":true,"queued_at":0,"asker":"dispatch-build"}"#,
    );
    std::fs::write(
        world.run_file(&run, "channel/surfaces.jsonl"),
        format!("{stale_read}\n"),
    )
    .expect("the older build's log is placed");
    let queue = world.run_file(&run, "channel/queue.json");
    let staged = queue.with_extension("staged");
    std::fs::write(&staged, r#"{"waiting":[],"pending":null,"next_id":0}"#)
        .expect("the older build's queue is staged");
    std::fs::rename(&staged, &queue).expect("the older build's queue is placed");
    // llmlint: ignore-end[tests_mirror_real_usage]

    // Counted, held on, and handed over under the id the log allocated.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("1 planner update(s) waiting");
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("Which base should build target?");
    assert_eq!(read.json()["surface"]["id"], json!(0));
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision: blocker — Which base should build target?");

    // And the id is not allocated a second time.
    let queued = world.run(&["surface", &run, "--kind", "finding", "--message", "noted"]);
    queued.exited(0);
    assert_eq!(queued.json()["surface"], json!(1));

    world
        .run_with_stdin(&["reply", &run], r#"{"completion":false,"reason":"main"}"#)
        .exited(0);
    world.release("build.go");
}

/// A projection whose claims moved under an intact stamp is rebuilt from the
/// log: the question it hid is still counted and handed over, and its id is
/// not handed out again.
///
/// A stamp matching the log's length used to be the whole of what a reader
/// checked, so a document with its waiting surfaces emptied and its counter
/// reset — by a rewrite, an editor, or a write that went wrong — was trusted
/// for good. Every writer now seals its claims and every reader checks the seal
/// from the document alone, so such a document reads as no document and the
/// whole log is folded.
#[test]
fn a_projection_whose_claims_moved_under_an_intact_stamp_is_rebuilt_from_the_log() {
    use std::io::Write;

    let world = World::new("channel-moved-claims");
    world.script("seed.wait", "hold");
    let run = running(&world, "movedclaims", vec![agent("seed", &[])]);

    let mut serving = world
        .cmd(&["channel", "serve", &run])
        .env(onepipeline::channel::ASKER_ENV, "dispatch-seed")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"Which base should seed build on?","node":"seed"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the question to be queued", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });

    // llmlint: ignore-block[tests_mirror_real_usage] the document is edited in
    // place because nothing this binary does moves a projection's claims under
    // its stamp — every write it makes seals what it stamps — so a rewrite that
    // did is one only another writer, an editor, or a failed write can leave.
    // The stamp is kept exactly as written, which is what a reader trusting the
    // stamp alone would take as current; everything before and after is driven
    // through the CLI.
    let queue = world.run_file(&run, "channel/queue.json");
    let mut document: Value =
        serde_json::from_slice(&std::fs::read(&queue).expect("the projection"))
            .expect("the projection is a document");
    assert!(
        document["accounted"]
            .as_u64()
            .is_some_and(|stamped| stamped > 0),
        "the projection carries no stamp to keep intact: {document}"
    );
    document["waiting"] = json!([]);
    document["pending"] = Value::Null;
    document["next_id"] = json!(0);
    let staged = queue.with_extension("staged");
    std::fs::write(
        &staged,
        serde_json::to_vec(&document).expect("the document"),
    )
    .expect("the moved document is staged");
    std::fs::rename(&staged, &queue).expect("the moved document lands");
    // llmlint: ignore-end[tests_mirror_real_usage]

    world
        .run(&["status", &run])
        .exited(0)
        .out_has("1 planner update(s) waiting");
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("Which base should seed build on?");
    assert_eq!(read.json()["surface"]["id"], json!(0));
    let queued = world.run(&["surface", &run, "--kind", "finding", "--message", "noted"]);
    queued.exited(0);
    assert_eq!(queued.json()["surface"], json!(1));

    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"completion":false,"reason":"build on main"}"#,
        )
        .exited(0);
    drop(stdin);
    world.release("seed.go");
    ended(serving);
}

/// A push whose read of the surface log fails is refused, and records nothing.
///
/// Every mutation of the queue reads the log's tail under the log's lock and
/// then stamps the log's whole length as accounted for. A read that failed and
/// went on would append its record beside the ones it never folded and stamp
/// them accounted for — a question hidden for good by the write meant to record
/// one. So the push is refused where the log cannot be read: nothing is
/// appended, nothing is stamped, the journal does not say a surface was queued,
/// and the next push — with the log readable again — allocates the id the
/// refused one would have taken and folds every surface the log holds.
///
/// The failure is the one a disk gives, at the one read it has to fail at: the
/// binary is run under `strace`, with the surface log's second `read(2)` — the
/// first is the appender looking at the file's own tail — answered `EIO`. Only
/// reads of that file are touched; everything else the process does is real.
#[cfg(target_os = "linux")]
#[test]
fn a_push_whose_log_cannot_be_read_is_refused_and_records_nothing() {
    let world = World::new("channel-unreadable-log");
    world.script("build.wait", "hold");
    let run = running(&world, "unreadablelog", vec![agent("build", &[])]);
    world
        .run(&["surface", &run, "--kind", "finding", "--message", "first"])
        .exited(0);

    // llmlint: ignore-block[tests_mirror_real_usage] the projection is removed
    // because a push reads the log's tail only where the projection is behind
    // it, and nothing user-facing leaves it behind on purpose: a lost write to
    // the projection is the very state the log-derived queue exists to survive,
    // and the one shape of it a journey can place is the write never landing.
    // Everything under test is driven through the CLI.
    let queue = world.run_file(&run, "channel/queue.json");
    std::fs::remove_file(&queue).expect("the projection's write is lost");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let log = world.run_file(&run, "channel/surfaces.jsonl");
    let logged = std::fs::read(&log).expect("the log");
    let journalled = world.events_of(&run, "planner-surface-queued").len();

    let trace = world.root.join("unreadable-log.strace");
    let refused = under_strace(
        &world,
        &[
            "-P",
            &std::fs::canonicalize(&log)
                .expect("the log's real path")
                .to_string_lossy(),
            "-e",
            "inject=read:error=EIO:when=2",
        ],
        &trace,
        &["surface", &run, "--kind", "finding", "--message", "second"],
    );
    // The failure the command reports is the one that was induced: exactly one
    // read of the log was answered `EIO`, and the command named that file.
    let traced = std::fs::read_to_string(&trace).expect("the trace the tracer wrote");
    let injected: Vec<&str> = traced
        .lines()
        .filter(|l| l.contains("(INJECTED)"))
        .collect();
    assert_eq!(injected.len(), 1, "{traced}");
    assert!(injected[0].contains("read("), "{traced}");
    assert_eq!(
        refused.status.code(),
        Some(REFUSED),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("surfaces.jsonl") && stderr.contains("Input/output error"),
        "the refusal does not name the log and what the disk said: {stderr}"
    );
    assert_eq!(
        std::fs::read(&log).expect("the log"),
        logged,
        "a push that could not read the log still appended to it"
    );
    assert!(
        !queue.exists(),
        "a push that could not read the log still stamped a projection: {}",
        std::fs::read_to_string(&queue).unwrap_or_default()
    );
    assert_eq!(
        world.events_of(&run, "planner-surface-queued").len(),
        journalled,
        "the journal says a surface was queued that the log does not hold"
    );

    // Readable again, the next push takes the id the refused one would have,
    // and every surface the log holds is handed over under its own id.
    let queued = world.run(&["surface", &run, "--kind", "finding", "--message", "second"]);
    queued.exited(0);
    assert_eq!(queued.json()["surface"], json!(1));
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("2 planner update(s) waiting");
    let first = world.run(&["next", &run]);
    first.exited(0).out_has("first");
    assert_eq!(first.json()["surface"]["id"], json!(0));
    let second = world.run(&["next", &run]);
    second.exited(0).out_has("second");
    assert_eq!(second.json()["surface"]["id"], json!(1));

    world.release("build.go");
}

/// The binary under `strace`, with the tracer's own options in front of it.
///
/// The command is the one `World` composes — same binary, same environment —
/// with the tracer wrapped around it, so what is observed is the invocation a
/// user makes rather than a second one assembled here. Children are followed
/// and the trace goes to `into`.
///
/// It **refuses** rather than passes where the tracer will not run: a failure
/// nobody induced is not evidence of how the binary meets one, and this is the
/// one journey whose whole claim is about what the process did when the disk
/// failed under it.
#[cfg(target_os = "linux")]
fn under_strace(
    world: &World,
    tracing: &[&str],
    into: &std::path::Path,
    argv: &[&str],
) -> std::process::Output {
    let inner = world.cmd(argv);
    let mut traced = std::process::Command::new("strace");
    traced
        .arg("-f")
        .arg("-qq")
        .args(tracing)
        .arg("-o")
        .arg(into)
        .arg(inner.get_program())
        .args(inner.get_args())
        .stdin(std::process::Stdio::null());
    for (key, value) in inner.get_envs() {
        match value {
            Some(value) => traced.env(key, value),
            None => traced.env_remove(key),
        };
    }
    traced.output().unwrap_or_else(|error| {
        panic!(
            "this journey's whole claim is what the process did when its log could not be \
             read, and the tracer that makes it unreadable would not run: strace: {error}. \
             Install strace, or run the suite where ptrace is permitted — a journey that \
             cannot induce the failure is not a journey that observed the binary survive it."
        )
    })
}

/// A log line carrying an id nothing can follow does not make the next surface
/// take an id already in use.
///
/// The allocator hands out one past the highest id the log has queued, so an
/// id with no successor — the last one there is, which no writer here ever
/// allocates — is one the counter cannot move past. A fold that kept such a
/// record and left the counter *at* it would allocate that id to every surface
/// queued afterwards, for good: not one collision but all of them. So the
/// record is refused rather than folded, the counter stays where the last
/// usable id put it, and every later surface takes an id nothing has used. And
/// where the log has queued the id *before* the last, so that the last is the
/// one the allocator would hand out next, the push is refused rather than
/// made: nothing could follow it either.
#[test]
fn a_log_line_carrying_an_id_nothing_can_follow_does_not_make_a_later_surface_take_a_used_id() {
    let world = World::new("channel-last-id");
    world.script("build.wait", "hold");
    let run = running(&world, "lastid", vec![agent("build", &[])]);
    let first = world.run(&["surface", &run, "--kind", "finding", "--message", "first"]);
    first.exited(0);
    assert_eq!(first.json()["surface"], json!(0));

    // llmlint: ignore-block[tests_mirror_real_usage] the line is placed
    // because no writer here allocates this id: the allocator refuses it
    // below, so a line carrying it is a corrupt or hostile one, and what is
    // under test is that such a line cannot turn the allocator into one that
    // collides. Everything else is driven through the CLI.
    let log = world.run_file(&run, "channel/surfaces.jsonl");
    let line_under = |id: u64| {
        format!(
            r#"{{"event":"queued","id":{id},"kind":"finding","message":"placed under {id}","source":"proposal","blocking":false,"queued_at":0,"abandoned":false}}"#,
        )
    };
    let mut placed = std::fs::read_to_string(&log).expect("the log");
    placed.push_str(&line_under(u64::MAX));
    placed.push('\n');
    std::fs::write(&log, &placed).expect("the line is placed");
    // llmlint: ignore-end[tests_mirror_real_usage]

    // Two surfaces queued afterwards take two distinct ids, neither of which
    // the log has ever carried.
    let second = world.run(&["surface", &run, "--kind", "finding", "--message", "second"]);
    second.exited(0);
    assert_eq!(second.json()["surface"], json!(1));
    let third = world.run(&["surface", &run, "--kind", "finding", "--message", "third"]);
    third.exited(0);
    assert_eq!(third.json()["surface"], json!(2));
    let mut read = Vec::new();
    for _ in 0..3 {
        let next = world.run(&["next", &run]);
        next.exited(0);
        read.push((
            next.json()["surface"]["id"].as_u64().expect("an id"),
            next.json()["surface"]["message"]
                .as_str()
                .expect("a message")
                .to_owned(),
        ));
    }
    assert_eq!(
        read,
        vec![
            (0, "first".to_owned()),
            (1, "second".to_owned()),
            (2, "third".to_owned())
        ]
    );
    let none = world.run(&["next", &run]);
    none.exited(0);
    assert_eq!(
        none.json()["surface"],
        Value::Null,
        "the placed line was folded"
    );

    // And with the id before the last queued, the push that would hand out the
    // last is refused by name, recording nothing.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] placed for the reason the
    // block above gives: this id is one the log reaches after 2^64 - 1 pushes.
    let mut placed = std::fs::read_to_string(&log).expect("the log");
    placed.push_str(&line_under(u64::MAX - 1));
    placed.push('\n');
    std::fs::write(&log, &placed).expect("the line is placed");
    // llmlint: ignore-end[tests_mirror_real_usage]
    world
        .run(&["surface", &run, "--kind", "finding", "--message", "fourth"])
        .exited(REFUSED)
        .err_has("the channel has no id left to allocate")
        .err_has(&format!("{}, has already been queued", u64::MAX - 1));
    assert_eq!(
        std::fs::read_to_string(&log).expect("the log"),
        placed,
        "a refused push still appended to the log"
    );
    assert!(
        world
            .events_of(&run, "planner-surface-queued")
            .iter()
            .all(|event| event["payload"]["message"] != json!("fourth")),
        "the journal says a surface was queued that the log does not hold"
    );
    // What the log does hold under a usable id is still handed over.
    let last = world.run(&["next", &run]);
    last.exited(0).out_has("placed under 18446744073709551614");

    world.release("build.go");
}

/// A queue that records a name identifying nobody still hands over every surface
/// in it.
///
/// The one place this crate reads an asker leniently, and the reason it does. A
/// queue is read with `unwrap_or_default`, so a record this build refused would
/// not cost one field — it would read as **no queue at all**, and every question
/// in it would go missing from `status`, from `next`, and from the decisions the
/// run is held on. A blank name identifies nobody, which is what "no asker"
/// already means, so it is read as that and the surface around it is untouched.
///
/// The record is placed by hand because nothing this crate does can produce one:
/// every path that writes an asker checks it first. That is what makes this
/// worth a journey rather than a unit test — what is under test is not the
/// parse, it is whether a live run still answers for the queue holding it.
///
/// The queue placed carries no stamp, so this is also the journey for a
/// projection an older build wrote: it is taken as it stands rather than
/// rebuilt from a log that has no line for the surface it holds.
#[test]
fn a_queue_recording_a_name_that_identifies_nobody_still_hands_over_its_surfaces() {
    let world = World::new("channel-blank-record");
    world.script("build.wait", "hold");
    let run = running(&world, "blankrecord", vec![agent("build", &[])]);

    // llmlint: ignore-block[tests_mirror_real_usage] there is no user-facing route to
    // this state and that is the property under test: every path that writes an
    // asker checks it first, so a record naming nobody is one only another build,
    // a hand edit, or a partial write can leave. What the journey drives through
    // the CLI is what matters — whether a live run still answers for the queue
    // holding it — and the record it answers about has to be placed.
    //
    // Renamed into place rather than written over: the run's own loop is reading
    // this file while this happens, and a half-written one would read as the
    // empty queue this journey exists to prove it is not.
    let queue = world.run_file(&run, "channel/queue.json");
    let staged = queue.with_extension("staged");
    std::fs::write(
        &staged,
        concat!(
            r#"{"waiting":[{"id":0,"kind":"blocker","message":"is anyone there?","#,
            r#""source":"proposal","blocking":true,"queued_at":0,"asker":""}],"#,
            r#""pending":null,"next_id":1}"#,
        ),
    )
    .expect("the record is staged");
    std::fs::rename(&staged, &queue).expect("the record is placed");
    // llmlint: ignore-end[tests_mirror_real_usage]

    // The run answers for it exactly as it does for any question nobody has read.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("1 planner update(s) waiting");
    // And it is handed over whole, under nobody's name rather than under a name
    // that means nothing.
    let read = world.run(&["next", &run]);
    read.exited(0).out_has("is anyone there?");
    assert_eq!(read.json()["surface"]["asker"], serde_json::Value::Null);
    assert_eq!(read.json()["surface"]["abandoned"], serde_json::Value::Null);

    world.release("build.go");
}

/// An asker this session cannot name is refused before it serves.
///
/// A blank value is not the absence it looks like: absent means this session
/// listens on its own, and blank would make it a session every other blank one
/// matches — so it would take over questions belonging to askers it has never
/// heard of, and hand their answers to the wrong side. Refused before the first
/// frame is read, so nothing is raised under a name that means nothing.
#[test]
fn an_asker_this_session_cannot_name_is_refused_before_it_serves() {
    let world = World::new("channel-blank-asker");
    world.script("build.wait", "hold");
    let run = running(&world, "blankasker", vec![agent("build", &[])]);

    for given in ["", "   "] {
        let mut command = world.cmd(&["channel", "serve", &run]);
        command.env(onepipeline::channel::ASKER_ENV, given);
        // Its stdin is closed, which is the frame stream ending — the one ending
        // that exits 0. So an exit 2 here is the refusal rather than the server
        // running out of input.
        world
            .run_on(command, "channel serve with a blank asker")
            .exited(2)
            .err_has("ONEPIPELINE_CHANNEL_ASKER is set to a blank value");
    }

    // And a value that is not text at all, which is the worse of the two because
    // it does not announce itself: read lossily, two environments that differ
    // collapse onto one string of replacement characters and two askers become
    // one. Unix-only, because that is where an environment value can hold bytes
    // no encoding claims.
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        let mut command = world.cmd(&["channel", "serve", &run]);
        command.env(
            onepipeline::channel::ASKER_ENV,
            std::ffi::OsStr::from_bytes(b"asker-\xff\xfe"),
        );
        world
            .run_on(command, "channel serve with an asker that is not text")
            .exited(2)
            .err_has("cannot read as text");
    }

    // Nothing was carried and nothing is waiting: the refusal happened before a
    // frame was read, so no surface was raised and then stranded.
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("planner update(s) waiting")
        .out_lacks("nobody is waiting on");

    world.release("build.go");
}

/// A session bound this host was given but cannot honour is refused before the
/// server carries anything.
///
/// `ONEPIPELINE_SERVE_SESSION_SECONDS` is external input at a trust boundary, so
/// a value that is not a whole number of seconds greater than zero fails loudly
/// rather than reading as the unset it is not. The moment matters as much as the
/// refusal: made after a frame had been carried, it would leave a question in
/// the queue raised by a session that then refused to stay for its answer.
///
/// Both spellings the fallback would have swallowed are here — a word, and the
/// zero that would have ended the session before it served anything — and beside
/// them the one that parses and still cannot be held: a bound further ahead than
/// this host's clock can name.
#[test]
fn a_session_bound_this_server_cannot_honour_is_refused_before_it_serves() {
    let world = World::new("channel-bad-bound");
    world.script("build.wait", "hold");
    let run = running(&world, "badbound", vec![agent("build", &[])]);

    for given in ["soon", "0"] {
        let mut command = world.cmd(&["channel", "serve", &run]);
        command.env("ONEPIPELINE_SERVE_SESSION_SECONDS", given);
        // Its stdin is closed, which is the frame stream ending — the one
        // ending that exits 0. So an exit 2 here is the refusal and cannot be
        // the server simply running out of input.
        world
            .run_on(command, &format!("channel serve with a bound of {given}"))
            .exited(2)
            .err_has("ONEPIPELINE_SERVE_SESSION_SECONDS is a whole number of seconds")
            .err_has(&format!("given '{given}'"));
    }

    // And the bound that is a whole number of seconds greater than zero and still
    // not one this host can hold: the sum is what every later comparison reads,
    // and `Instant` addition panics on an overflow rather than saturating, so the
    // arithmetic is refused where it is done instead of at the moment it is read.
    let mut furthest = world.cmd(&["channel", "serve", &run]);
    furthest.env("ONEPIPELINE_SERVE_SESSION_SECONDS", u64::MAX.to_string());
    world
        .run_on(furthest, "channel serve with a bound past this clock")
        .exited(2)
        .err_has("further ahead than this host's clock can name");

    // Nothing was carried and nothing is waiting: the refusals happened before
    // the server read a frame, so no surface was raised and then stranded.
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("planner update(s) waiting")
        .out_lacks("nobody is waiting on");

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

/// And a verdict that arrived beside graph edits is recorded exactly as one that
/// arrived alone.
///
/// The run's journal is its own account of what it was told and why it stopped
/// waiting, and which branch of the reply path an envelope took is not a fact
/// about the ruling. Recorded only for the commandless branch, the record went
/// missing for exactly the envelopes that did the most — and a completion
/// declared beside a command raised no completion request at all, so the one
/// event a supervisor watches for a run asking to finish was absent while the
/// receipt said the reply was applied.
#[test]
fn a_verdict_beside_commands_is_journalled_exactly_as_a_commandless_one_is() {
    let world = World::new("channel-verdict-journal");
    world.script("build.wait", "hold");
    let run = running(&world, "journalled", vec![agent("build", &[])]);

    // The commandless shape, which is what the both-halves shape is held to.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({"completion": false, "reason": "keep going"}).to_string(),
        )
        .exited(0);
    let alone = world.events_of(&run, "planner-replied");
    assert_eq!(alone.len(), 1, "{alone:?}");
    assert_eq!(alone[0]["payload"]["reason"], "keep going");
    assert_eq!(alone[0]["payload"]["completion"], json!(false));

    // The same verdict with a command riding along.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "completion": false,
                "reason": "keep going, with this in hand",
                "version": 2,
                "commands": [
                    {"op": "note", "id": "build", "addressee": "worker",
                     "text": "the fixture moved", "deliver": "next"}
                ],
            })
            .to_string(),
        )
        .exited(0);
    world.until("the verdict beside the edit to be recorded", |world| {
        world.events_of(&run, "planner-replied").len() >= 2
    });
    let beside = world.events_of(&run, "planner-replied");
    assert_eq!(beside.len(), 2, "{beside:?}");
    assert_eq!(
        beside[1]["payload"]["reason"],
        "keep going, with this in hand"
    );
    assert_eq!(beside[1]["payload"]["author"], "planner");
    // And the command half still reached the graph, so this is the record of an
    // envelope that did both rather than of one that was routed away from them.
    world.until("the edit to reach the graph", |world| {
        !world.events_of(&run, "edit-committed").is_empty()
    });

    // And the completion request a completion verdict raises, which is the event
    // a supervisor watches for a run asking to finish.
    assert!(
        world.events_of(&run, "completion-requested").is_empty(),
        "a run that never declared itself complete raised a completion request"
    );
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "completion": true,
                "reason": "publication verified",
                "version": 2,
                "commands": [
                    {"op": "note", "id": "build", "addressee": "worker",
                     "text": "wrapping up", "deliver": "next"}
                ],
            })
            .to_string(),
        )
        .exited(0);
    world.until("the completion request to be raised", |world| {
        !world.events_of(&run, "completion-requested").is_empty()
    });
    let requested = world.events_of(&run, "completion-requested");
    assert_eq!(requested.len(), 1, "{requested:?}");
    assert_eq!(requested[0]["payload"]["reason"], "publication verified");
    assert_eq!(world.events_of(&run, "planner-replied").len(), 3);
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
        r#"{"version":2,"commands":[{"op":"invented","id":"x"}]}"#,
        r#"{"version":2,"commands":[{"op":"drop","id":"build"}]}"#,
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
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file("settled", "result.json").is_file()
    });
    // The run's own ownership lock, which its driver releases as it goes: what a
    // settled run's views say about the driver is `SETTLED`, and deliberately
    // not `DRIVER DEAD` beside a prescription to replace it.
    world.until("the driver to release the run", |world| {
        !world.run_file("settled", "owner.lock").exists()
    });

    world
        .run_with_stdin(
            &["reply", "settled"],
            r#"{"completion":true,"reason":"done"}"#,
        )
        .exited(REFUSED)
        .err_has("has settled");
}

/// The same refusal for a settled run whose driver **reads as alive**, because
/// the refusal is a fact about the run and not about a process.
///
/// The journey above waits for the driver to exit, so the guard could be asked
/// of the liveness verdict and still pass it. That verdict is a different
/// question with a different answer at the same instant, and asking it delivered
/// replies into settled runs.
///
/// This one holds that window open without racing anything, in the state the
/// verdict is *designed* to give the benefit of the doubt to: a run driven from
/// **another host**. A pid means nothing across machines, so this host will not
/// call that driver dead however long ago it went — and the run has still
/// settled, which its own `SETTLED` row already said.
#[test]
fn a_settled_run_refuses_a_reply_however_alive_its_driver_still_looks() {
    const ELSEWHERE: &str = "a-host-this-is-not";
    let world = World::new("channel-settled-elsewhere");
    let path = world.plan("afar", &plan_of("afar", vec![agent("build", &[])]));
    let mut launch = world.cmd(&["start", &path, "--attach"]);
    launch.env("HOSTNAME", ELSEWHERE);
    world
        .run_on(launch, "start recorded on another host")
        .exited(0);
    assert!(
        world.run_file("afar", "result.json").is_file(),
        "the run never wrote its result: {:?}",
        world.kinds("afar")
    );

    // What this host makes of that driver, which is the reading the refusal used
    // to be decided by: nothing here can say it is gone.
    world.run(&["status", "afar"]).exited(0).out_has("SETTLED");

    world
        .run_with_stdin(&["reply", "afar"], r#"{"completion":true,"reason":"done"}"#)
        .exited(REFUSED)
        .err_has("has settled");

    // And nothing was queued for it, which is the half a `delivered` receipt was
    // read as: no verdict on the durable queue, and nothing recorded that a
    // planner replied.
    assert!(
        !world.run_file("afar", "channel/replies.jsonl").exists(),
        "a reply nothing will ever read was queued anyway"
    );
    assert!(
        world.events_of("afar", "planner-replied").is_empty(),
        "the run recorded a reply it refused: {:?}",
        world.kinds("afar")
    );
}

/// A run still awaiting an answer takes a reply, whatever its driver is doing
/// and whatever its graph has done.
///
/// The other side of the refusal above, and the reason it cannot simply be "this
/// run has finished": a blocking surface is the run *asking* for the reply, and
/// every node here has settled while the question about the run as a whole has
/// not. Its driver is gone — this is `unattended`, the state a run is left in
/// when the last node fails — so a guard reading either the graph or the process
/// would refuse the one reply somebody is waiting on.
#[test]
fn a_run_awaiting_an_answer_takes_a_reply_though_its_graph_has_settled() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-settled-awaiting");
    // The one node fails, so the graph converges with nothing ready and nothing
    // waiting on a person: settled by every reading except the question below.
    world.script("build.fail", "1");
    let path = world.plan("asked", &plan_of("asked", vec![agent("build", &[])]));
    world
        .run(&["start", &path, "--attach"])
        .exited(NOTHING_DRIVING)
        .out_has("\"settlement\":\"unattended\"");
    assert!(
        world.run_file("asked", "result.json").is_file(),
        "the run never wrote its result: {:?}",
        world.kinds("asked")
    );

    // A blocking question about the run, raised after all of that.
    let mut serving = world
        .cmd(&["channel", "serve", "asked"])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"the last node failed; what now?"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the question to reach the planner", |world| {
        !world
            .events_of("asked", "planner-surface-queued")
            .is_empty()
    });
    world.run(&["next", "asked"]).exited(0);

    world
        .run_with_stdin(
            &["reply", "asked"],
            r#"{"completion":false,"reason":"supersede it and try again"}"#,
        )
        .exited(0)
        .out_has("\"delivered\"");

    // Delivered means *reached the reader*: the verdict came out of the server
    // holding the question, which is what a receipt claims and what the settled
    // run above had none of.
    let stdout = serving.stdout.take().expect("stdout is piped");
    let verdict = BufReader::new(stdout)
        .lines()
        .map_while(std::result::Result::ok)
        .find(|line| line.contains("reason"))
        .expect("the server wrote a verdict");
    assert!(verdict.contains("supersede it and try again"), "{verdict}");

    drop(stdin);
    ended(serving);
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
            r#"{"version":2,"commands":[{"op":"cancel","id":"idle"}]}"#,
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
    ended(serving);
}

/// The whole seam, through a **real observer member and its judge side**: a live
/// edit issued while that member is between turns leaves it supervising.
///
/// The other journeys drive `channel serve` directly, which proves the routing
/// but not the thing that broke: what died was a *member*, because the side
/// supervising it was handed a graph edit where a ruling belongs and a graph
/// edit supervises nothing. So this one runs the member the shipped dag-scope
/// graph declares, through the judge-side command provider that document names,
/// and the operator's correction arrives exactly where it used to kill it —
/// between the member's turns, while its judge side waits on the answer.
#[test]
fn a_live_edit_while_the_observer_member_is_between_turns_leaves_it_supervising() {
    let world = World::new("channel-observer-member");
    world.script("build.wait", "hold");
    // The node the operator's correction adds, held like the first: what proves
    // the correction reached the graph is the run *dispatching* it.
    world.script("sweep.wait", "hold");
    // Two supervision turns: the first is the one the live edit arrives during,
    // and the second is what proves supervision went on after it.
    world.script("observer.supervise", "2");
    let path = world.plan("watched", &plan_of("watched", vec![agent("build", &[])]));
    let mut start = world.cmd(&[
        "start",
        &path,
        "--detach",
        "--dag-graph",
        &world.shipped_dag_graph(),
    ]);
    // Inherited by the launched graph, and through it by the member's judge
    // side: a wait that expired would answer the member with a synthesized
    // verdict, and this journey would be reading that rather than the planner's.
    start.env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120");
    world
        .run_on(start, "start watched --detach --dag-graph")
        .exited(0);

    world.until("the observer member to raise its first turn", |world| {
        world
            .observer_supervision()
            .iter()
            .any(|record| record["turn"] == 1)
    });
    world.until("the member's question to reach the planner", |world| {
        world
            .events_of("watched", "planner-surface-queued")
            .iter()
            .any(|event| event["payload"]["kind"] == "monitor-question")
    });
    world
        .run(&["next", "watched"])
        .exited(0)
        .out_has("anything to correct?");

    // The envelope that used to end the member, arriving where it used to: on
    // the wait its judge side is sitting in.
    world
        .run_with_stdin(
            &["reply", "watched"],
            &json!({"version": 2, "commands": [
                {"op": "add", "node": {"id": "sweep", "persona": "engineer",
                                       "task": "## What\nsweep what build left"}}
            ]})
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
    // Not the record of the edit but its effect: the run reconciled the
    // correction and dispatched the work it added, which is a pass of the loop
    // and a dispatch of its own after the envelope reached the channel — and the
    // member's judge side polls that channel throughout.
    until_still_supervising(&world, "the added node to be dispatched", |world| {
        world
            .events_of("watched", "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "sweep")
    });
    let supervision = world.observer_supervision();
    assert!(
        supervision.iter().all(|record| record["ruling"].is_null()),
        "the member was ruled by something the planner never sent: {supervision:#?}"
    );

    // The planner rules *after* the correction rather than in the same instant.
    // The member's judge side reads this channel on a 50ms poll of its own, so a
    // correction and a ruling written inside one of those polls are read
    // together — and a journey that ruled that fast would be proving what a
    // batch does rather than where an unaccompanied live edit goes, which is the
    // case an operator correcting a run mid-supervision actually creates. The
    // correction is left on its own for several of those polls first; a member
    // that took it dies during this wait rather than after it.
    let ruling_is_due = std::time::Instant::now() + std::time::Duration::from_millis(500);
    until_still_supervising(&world, "the correction to sit unclaimed", |_| {
        std::time::Instant::now() >= ruling_is_due
    });

    // A ruling, so the turn the edit did not answer is answered and the member
    // has a second turn to still be alive for.
    world
        .run_with_stdin(
            &["reply", "watched"],
            r#"{"completion":false,"reason":"noted, carry on"}"#,
        )
        .exited(0);
    until_still_supervising(
        &world,
        "the member to be ruled and take another turn",
        |world| {
            world
                .observer_supervision()
                .iter()
                .any(|record| record["turn"] == 2)
        },
    );
    let ruled = world
        .observer_supervision()
        .into_iter()
        .find(|record| record["ruling"].is_string())
        .expect("the member was ruled");
    assert!(
        ruled["ruling"]
            .as_str()
            .is_some_and(|ruling| ruling.contains("noted, carry on")),
        "the member was ruled with something other than the planner's verdict: {ruled}"
    );
    until_still_supervising(&world, "the second turn to reach the planner", |world| {
        world
            .events_of("watched", "planner-surface-queued")
            .iter()
            .filter(|event| event["payload"]["kind"] == "monitor-question")
            .count()
            >= 2
    });

    world.run(&["next", "watched"]).exited(0);
    world
        .run_with_stdin(
            &["reply", "watched"],
            r#"{"completion":false,"reason":"nothing further"}"#,
        )
        .exited(0);
    until_still_supervising(&world, "the member to finish its second turn", |world| {
        world
            .observer_supervision()
            .iter()
            .filter(|record| record["ruling"].is_string())
            .count()
            >= 2
    });

    world.release("build.go");
    world.release("sweep.go");
}

/// Every shape a verdict is spelled in rules a supervised member.
///
/// Contract E names three fields — `completion`, `message`, `reason` — and any
/// one of them alone is a ruling. The side supervising a member decides what a
/// ruling is out of its own reading of that list, and this is the gate that
/// keeps the two readings together: a half this crate calls a verdict and the
/// supervising side does not would end the member the moment a planner used it,
/// and the run would go blind with nobody having changed anything visible.
#[test]
fn every_verdict_half_the_contract_names_rules_a_supervised_member() {
    let world = World::new("channel-verdict-halves");
    world.script("build.wait", "hold");
    world.script("observer.supervise", "3");
    let path = world.plan("halves", &plan_of("halves", vec![agent("build", &[])]));
    let mut start = world.cmd(&[
        "start",
        &path,
        "--detach",
        "--dag-graph",
        &world.shipped_dag_graph(),
    ]);
    start.env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120");
    world
        .run_on(start, "start halves --detach --dag-graph")
        .exited(0);

    for (turn, verdict) in [
        (1, json!({"completion": false})),
        (2, json!({"message": "keep going"})),
        (3, json!({"reason": "nothing to change"})),
    ] {
        until_still_supervising(&world, "the member to raise its turn", |world| {
            world
                .observer_supervision()
                .iter()
                .any(|record| record["turn"] == turn)
        });
        world
            .run_with_stdin(&["reply", "halves"], &verdict.to_string())
            .exited(0);
        until_still_supervising(&world, "the half to rule the member", |world| {
            world
                .observer_supervision()
                .iter()
                .filter(|record| record["ruling"].is_string())
                .count()
                >= turn as usize
        });
    }

    let rulings: Vec<String> = world
        .observer_supervision()
        .into_iter()
        .filter_map(|record| record["ruling"].as_str().map(str::to_string))
        .collect();
    assert_eq!(rulings.len(), 3, "{rulings:?}");
    for (half, ruling) in ["completion", "message", "reason"].iter().zip(&rulings) {
        assert!(
            ruling.contains(half),
            "the member was ruled by turn with something other than the `{half}` half: {ruling}"
        );
    }

    world.release("build.go");
}

/// A graph whose command judge names no command is refused by name, and the run
/// it was watching goes on without it.
///
/// The observer's judge side is a document an operator writes, so it is external
/// input like a plan or a reply. A command judge with nothing to run is refused
/// through `oneagentgraph`'s own validation of that document — not a second copy
/// of the rule — rather than the member being started on whatever an empty
/// command resolved to. And an observer that could not start is not a run that
/// stops: the run is left unwatched, which the launcher says out loud, and it
/// still executes, which is why it says it rather than failing.
#[test]
fn an_observer_graph_whose_judge_names_no_command_is_refused_and_the_run_goes_on() {
    use oneagentgraph::config::{JudgeSide, Member};

    let world = World::new("channel-observer-nocommand");
    world.script("observer.supervise", "1");
    let graphs = world.graphs();
    std::fs::create_dir_all(&graphs).expect("a directory for the graph configs");

    // The shipped document with its judge side emptied, rather than a graph
    // written out here: what this journey is about is that *one* field being
    // unusable, and a hand-written stand-in would drift into proving something
    // else about a document nobody ships.
    let mut config: oneagentgraph::config::GraphConfig = serde_norway::from_str(
        &std::fs::read_to_string(world.shipped_dag_graph()).expect("the shipped dag-scope graph"),
    )
    .expect("the shipped dag-scope graph parses");
    let mut emptied = 0;
    for member in config.members.values_mut() {
        if let Member::Onejudge(member) = member {
            if let JudgeSide::Command(command) = &mut member.judge {
                command.command.clear();
                emptied += 1;
            }
        }
    }
    assert_eq!(
        emptied, 1,
        "the shipped dag-scope graph no longer declares exactly one command judge"
    );
    let broken = graphs.join("dag-scope-nocommand.yaml");
    std::fs::write(
        &broken,
        serde_norway::to_string(&config).expect("the emptied graph serializes"),
    )
    .expect("the graph is written");

    let path = world.plan(
        "unwatched",
        &plan_of("unwatched", vec![agent("build", &[])]),
    );
    world
        .run(&[
            "start",
            &path,
            "--attach",
            "--dag-graph",
            &broken.display().to_string(),
        ])
        .exited(0)
        .err_has("has stopped watching");

    // Every record rather than a count of them: a driver starts another observer
    // when the one watching it stops, and a graph whose judge side cannot run
    // stops every time — so how many of these there are is the driver's bound
    // rather than anything about the refusal, and each one has to name it.
    let reported = world.observer_supervision();
    assert!(
        !reported.is_empty(),
        "the graph was never supervised at all"
    );
    assert!(
        reported.iter().all(|record| record["misconfigured"]
            .as_str()
            .is_some_and(|why| why.contains("needs a command to run"))),
        "the misconfiguration was not named: {reported:#?}"
    );
    assert!(
        reported.iter().all(|record| record["asked"].is_null()),
        "a graph that named no judge side supervised anyway: {reported:#?}"
    );

    assert!(
        !world.events_of("unwatched", "node-settled").is_empty(),
        "a misconfigured observer stopped the run it was watching: {:?}",
        world.kinds("unwatched")
    );
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
            &json!({"version": 2, "commands": [
                {"op": "note", "id": "build", "addressee": "worker",
                 "text": "the scope changed", "deliver": "next"}
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
    ended(serving);
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
    world.run(&["start", &path, "--detach"]).exited(0);
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
            &json!({"version": 2, "commands": [
                {"op": "note", "id": "later", "addressee": "worker",
                 "text": "start from the fixture", "deliver": "live", "persist": false}
            ]})
            .to_string(),
        )
        .exited(REFUSED)
        .err_has("composes it into no dispatch");

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
    ended(serving);
}

/// Both halves, where the reconciler refuses the edits: the ruling is still
/// delivered, and only the edits are reported refused.
///
/// The two halves answer to two different things — the verdict to a question a
/// reader is blocked on, the commands to the graph — so the fate of one is not
/// the fate of the other. A reader held until the reconciler happened to like
/// the edits riding alongside would be blocked by a refusal that was never about
/// it, which for an observer member's judge side is the same silence this whole
/// routing exists to end.
#[test]
fn a_rejected_reply_carrying_both_halves_still_delivers_its_verdict() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-reply-both-refused");
    // The same shape the commands-only refusal is built on: an open turn beside
    // a node that never dispatches, so a `live` note for the second is accepted
    // at submission and refused by the reconciler.
    world.script("slow.turn-open", "");
    world.script("slow.wait", "hold");
    let path = world.plan(
        "bothrefused",
        &plan_of(
            "bothrefused",
            vec![agent("slow", &[]), agent("later", &["slow"])],
        ),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the held node's turn to open", |world| {
        !world.events_of("bothrefused", "turn-started").is_empty()
    });

    let mut serving = world
        .cmd(&["channel", "serve", "bothrefused"])
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
            .events_of("bothrefused", "planner-surface-queued")
            .is_empty()
    });
    world.run(&["next", "bothrefused"]).exited(0);

    world
        .run_with_stdin(
            &["reply", "bothrefused"],
            &json!({
                "completion": false,
                "reason": "go on; the note was optional",
                "version": 2,
                "commands": [
                    {"op": "note", "id": "later", "addressee": "worker",
                     "text": "start from the fixture", "deliver": "live", "persist": false}
                ]
            })
            .to_string(),
        )
        .exited(REFUSED)
        .err_has("composes it into no dispatch");

    assert!(
        world.events_of("bothrefused", "edit-committed").is_empty(),
        "a rejected edit reached the graph: {:?}",
        world.kinds("bothrefused")
    );
    let stdout = serving.stdout.take().expect("stdout is piped");
    let verdict = BufReader::new(stdout)
        .lines()
        .next()
        .expect("the server wrote a line")
        .expect("the line reads");
    assert!(
        verdict.contains("go on; the note was optional"),
        "the verdict was withheld because the edits beside it were refused: {verdict}"
    );
    // And the run's own record says so, on this path as on the ones the edits
    // survived: a ruling that was delivered is a ruling that happened, whatever
    // the reconciler made of what rode beside it.
    let replied = world.events_of("bothrefused", "planner-replied");
    assert_eq!(replied.len(), 1, "{replied:?}");
    assert_eq!(
        replied[0]["payload"]["reason"],
        "go on; the note was optional"
    );
    world
        .run(&["status", "bothrefused"])
        .exited(0)
        .out_lacks("waiting for planner decision");

    drop(stdin);
    world.release("slow.go");
    ended(serving);
}

/// An envelope refused before it is routed is refused **whole**: neither half
/// reaches either reader.
///
/// Routing the halves apart makes this a question it never was: an envelope that
/// carries a perfectly good ruling can still be refused for what rides beside
/// it, and "deliver the half that was fine" is a tempting reading of the routing
/// that would be the wrong one. Validation is about the envelope, and an
/// envelope the submitter is told to fix is one it will send again — delivering
/// half of it first would answer a question with a ruling whose edits never
/// happened, and there is nothing the reader could do with that.
///
/// So the three refusals that precede routing — the envelope version, the
/// author's op allowlist, and the reconciler's own pre-queue validation — each
/// leave the pending surface standing and the reader still waiting, and the
/// submitter carries the refusal away alone.
///
/// The version is driven **both** ways it can be wrong: an envelope declaring
/// none, and one declaring the version this envelope used to be. The second is the
/// observable half of collapsing the two manager-note ops into one — that bump is
/// what a caller on the old shape meets first — and a check that only refused a
/// missing version would pass a build that still accepted the old one.
#[test]
fn a_reply_refused_before_routing_delivers_neither_half() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-reply-refused-whole");
    world.script("build.wait", "hold");
    let path = world.plan("whole", &plan_of("whole", vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);

    let mut serving = world
        .cmd(&["channel", "serve", "whole"])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"build has been at this a while; go on?","node":"build"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the frame to reach the planner", |world| {
        !world
            .events_of("whole", "planner-surface-queued")
            .is_empty()
    });
    world.run(&["next", "whole"]).exited(0);

    let ruling = "carry on regardless";
    for (refusal, envelope) in [
        (
            "an edit envelope requires version",
            json!({
                "completion": false, "reason": ruling,
                "commands": [
                    {"op": "note", "id": "build", "addressee": "worker", "text": "a note"}
                ]
            }),
        ),
        (
            "an edit envelope requires version",
            json!({
                "completion": false, "reason": ruling,
                // The version this envelope carried before the two manager-note
                // ops were collapsed into one, with an op that is otherwise
                // perfectly good: what is refused is the shape it is written in.
                "version": 1, "commands": [{"op": "cancel", "id": "build"}]
            }),
        ),
        (
            "not an op the monitor may issue",
            json!({
                "completion": false, "reason": ruling, "author": "monitor",
                "version": 2, "commands": [{"op": "complete", "reason": "looks done from here"}]
            }),
        ),
        (
            "nowhere",
            json!({
                "completion": false, "reason": ruling,
                "version": 2, "commands": [{"op": "cancel", "id": "nowhere"}]
            }),
        ),
    ] {
        world
            .run_with_stdin(&["reply", "whole"], &envelope.to_string())
            .exited(REFUSED)
            .err_has(refusal);
        assert!(
            world.events_of("whole", "edit-committed").is_empty(),
            "a refused envelope's edits reached the graph: {:?}",
            world.kinds("whole")
        );
        world
            .run(&["status", "whole"])
            .exited(0)
            .out_has("waiting for planner decision");
        assert!(
            serving
                .try_wait()
                .expect("the reader is readable")
                .is_none(),
            "the waiting reader was handed a half of an envelope refused as a whole"
        );
    }

    // The same ruling, in an envelope nothing refuses: what the reader takes is
    // this one, so none of the three above left a copy of it on the queue.
    world
        .run_with_stdin(
            &["reply", "whole"],
            &json!({"completion": false, "reason": "and now for real"}).to_string(),
        )
        .exited(0);
    let stdout = serving.stdout.take().expect("stdout is piped");
    let taken = BufReader::new(stdout)
        .lines()
        .next()
        .expect("the server wrote a line")
        .expect("the line reads");
    assert!(
        taken.contains("and now for real") && !taken.contains(ruling),
        "the reader was handed a ruling out of an envelope that was refused: {taken}"
    );

    drop(stdin);
    world.release("build.go");
    ended(serving);
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
    world.run(&["start", &path, "--attach"]).exited(0);

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
            &json!({"version": 2, "commands": [
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
                "version": 2,
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
    ended(serving);
}

/// Two rulings written inside one poll are two rulings, and the reader takes
/// them one per question.
///
/// Acceptance means delivery on this channel: a planner writes when it has
/// something to say and nothing has to be listening at that moment. So a second
/// verdict arriving while the first is still unread is not a correction of it —
/// it is the answer to the next question, and a reader that swept up the batch
/// and kept the newest would drop a ruling somebody was owed.
#[test]
fn two_verdicts_written_at_once_are_delivered_one_per_question() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-two-verdicts");
    world.script("build.wait", "hold");
    let run = running(&world, "queuedverdicts", vec![agent("build", &[])]);

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

    writeln!(stdin, r#"{{"kind":"blocker","message":"first question"}}"#).expect("written");
    stdin.flush().expect("flushed");
    world.until("the first question to reach the planner", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });

    // The second written before the first has been read, which is the only way
    // to have two unclaimed verdicts waiting at once.
    for reason in ["answering the first", "answering the second"] {
        world
            .run_with_stdin(
                &["reply", &run],
                &json!({"completion": false, "reason": reason}).to_string(),
            )
            .exited(0);
    }

    let first = written
        .next()
        .expect("the server wrote a line")
        .expect("the line reads");
    assert!(
        first.contains("answering the first"),
        "the first question was answered with a later ruling, losing the one it was owed: {first}"
    );

    // The second question is answered out of what is already on the queue: the
    // ruling nobody had read is still there for the reader that asks next.
    writeln!(stdin, r#"{{"kind":"blocker","message":"second question"}}"#).expect("written");
    stdin.flush().expect("flushed");
    let second = written
        .next()
        .expect("the server wrote a second line")
        .expect("the line reads");
    assert!(
        second.contains("answering the second"),
        "the ruling written while the first question was open never reached anybody: {second}"
    );

    drop(stdin);
    world.release("build.go");
    ended(serving);
}

/// Edits still queued when the wait runs out do not hold the verdict beside
/// them.
///
/// The two halves have two readers and two fates: the edits are durable and the
/// reconciler will reach them, which is what exit 1 says; the ruling answers a
/// question that was asked and answered, and the member waiting on it has no
/// stake in whether a reconcile pass has happened yet. Held back, it would be
/// lost outright — nothing queues it afterwards — and the run would go on
/// waiting for a decision its planner had already made.
#[test]
fn a_verdict_beside_edits_that_are_still_queued_is_delivered_anyway() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-queued-verdict");
    world.script("build.wait", "hold");
    let run = running(&world, "queuededit", vec![agent("build", &[])]);

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
        r#"{{"kind":"blocker","message":"is build worth continuing?","node":"build"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the question to reach the planner", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });
    world.run(&["next", &run]).exited(0);

    // llmlint: ignore-block[tests_mirror_real_usage] the reconciler's cursor is advanced
    // past this envelope on purpose, which is how a reader-starved command queue is
    // arranged: exit 1 exists for a reconciler that did not get to the edits in time, and
    // no invocation a planner can type guarantees that timing. `live_edit.rs` arranges the
    // commands-only half of this verdict the same way and for the same reason.
    std::fs::write(world.run_file(&run, "channel/commands-cursor.json"), "99")
        .expect("the cursor is advanced");
    let mut queued = world.cmd(&["reply", &run]);
    queued.env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1");

    // llmlint: ignore-end[tests_mirror_real_usage]
    let submitted = world.run_with_stdin_on(
        queued,
        &json!({
            "completion": false,
            "reason": "carry on while that lands",
            "version": 2,
            "commands": [{"op": "note", "id": "build", "addressee": "worker",
                          "text": "a note", "deliver": "next"}]
        })
        .to_string(),
    );
    // Exit 0: accepted, durable, and not reconciled yet is not a refusal, and
    // this verb's non-zero statuses are refusals to correct.
    submitted
        .exited(0)
        .out_has("\"queued\"")
        .err_has("has to drive the run");
    // Two fates, and the receipt names each: the ruling is gone to a reader and
    // the edits are still in the queue, which one word could only say one of.
    // Held to entry 64 here rather than only in the receipt journey, because this
    // is the one shape that reaches the record's `queued` words.
    let receipt = submitted.json();
    stated_by_entry_64(&receipt, "a verdict beside queued edits", true, true);
    assert_eq!(receipt["state"], "queued");
    assert_eq!(receipt["verdict"], "delivered");
    assert_eq!(receipt["commands"], "queued");
    // And the run's own record says a verdict was given, on this path as on the
    // ones where the edits landed.
    let replied = world.events_of(&run, "planner-replied");
    assert_eq!(replied.len(), 1, "{replied:?}");
    assert_eq!(replied[0]["payload"]["reason"], "carry on while that lands");

    assert!(
        world.events_of(&run, "edit-committed").is_empty(),
        "the edits this journey needs queued were reconciled: {:?}",
        world.kinds(&run)
    );
    let stdout = serving.stdout.take().expect("stdout is piped");
    let verdict = BufReader::new(stdout)
        .lines()
        .next()
        .expect("the server wrote a line")
        .expect("the line reads");
    assert!(
        verdict.contains("carry on while that lands"),
        "the verdict half was held back with the edits: {verdict}"
    );

    drop(stdin);
    world.release("build.go");
    ended(serving);
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
                    {"op": "note", "id": "build", "addressee": "worker",
                     "text": "from the old build", "deliver": "next"}
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
    ended(serving);
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
                "version": 2,
                "commands": [
                    {"op": "note", "id": "build", "addressee": "worker",
                     "text": "the fixture moved", "deliver": "next"}
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
    ended(serving);
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

        // Interleaved deliberately: whichever order the two readers reach the
        // queue in, every edit is still the reconciler's.
        for note in ["the scope changed", "and again"] {
            world
                .run_with_stdin(
                    &["reply", &run],
                    &json!({"version": 2, "commands": [
                        {"op": "note", "id": "build", "addressee": "worker",
                         "text": note, "deliver": "next"}
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
    ended(serving);
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
    world.run(&["start", &path, "--detach"]).exited(0);

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
    world.run(&["start", &path, "--attach"]).exited(0);

    world
        .run_with_stdin(
            &["reply", "settledgraph"],
            r#"{"version":2,"commands":[{"op":"add","node":{"id":"late","persona":"e","task":"t"}}]}"#,
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
    world.release("observer.go");
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
            &path,
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
            &json!({"version": 2, "author": "monitor", "commands": [command]}).to_string(),
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
    // a node added, that node parked and brought back, and finally the running
    // node superseded — which is the one that stops it, so it goes last. There
    // is no note among them: the one manager-note op may bind a criterion the
    // node's judge decides against, so an observer surfaces instead of sending.
    for command in [
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
                &json!({"version": 2, "author": "monitor", "commands": [command]}).to_string(),
            )
            .exited(0)
            .out_has("\"applied\"");
    }

    let committed = world.events_of("watched", "edit-committed");
    assert_eq!(committed.len(), 4, "{committed:?}");
    for edit in &committed {
        assert_eq!(edit["payload"]["author"], "monitor", "{edit}");
    }

    world.until("the planner to be told what the monitor did", |world| {
        world
            .events_of("watched", "planner-surface-queued")
            .iter()
            .filter(|event| event["payload"]["kind"] == "monitor-edit")
            .count()
            >= 4
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
    world.run(&["start", &path, "--detach"]).exited(0);
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
    ended(serving);
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
        .run(&["start", &path, "--attach"])
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
    ended(serving);
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

/// And it may not carry that declaration through by attaching a command to it.
///
/// The allowlist has to mean the same thing whatever else the envelope carries.
/// Asked only of a commandless verdict, it was bypassable by anyone who could
/// issue any op at all: `finding` is on the monitor's list, and an envelope
/// pairing one with `completion: true` walked the completion straight past the
/// guard. That is a privilege escalation rather than a reporting defect, so the
/// whole envelope is turned away — the ruling is not queued and the edit beside
/// it is never applied.
#[test]
fn a_monitor_cannot_declare_the_run_complete_by_attaching_a_command_to_the_verdict() {
    let world = World::new("channel-monitor-verdict-beside-edits");
    world.script("build.wait", "hold");
    let run = running(&world, "smuggled", vec![agent("build", &[])]);

    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "author": "monitor",
                "completion": true,
                "reason": "looks finished to me",
                "version": 2,
                "commands": [
                    {"op": "finding", "message": "the build looks done", "id": "build"}
                ],
            })
            .to_string(),
        )
        .exited(REFUSED)
        .err_has("not something the monitor may do")
        .err_has("Surface it to the planner");

    assert!(
        world.events_of(&run, "completion-requested").is_empty(),
        "the monitor declared the run complete beside a command: {:?}",
        world.kinds(&run)
    );
    assert!(
        world.events_of(&run, "planner-replied").is_empty(),
        "the refused verdict was journalled anyway: {:?}",
        world.kinds(&run)
    );
    // Half applied is the other half of the refusal: the command it rode in on
    // never reached the graph either.
    assert!(
        world.events_of(&run, "edit-committed").is_empty(),
        "the command beside the refused verdict was applied: {:?}",
        world.kinds(&run)
    );
    world.run(&["next", &run]).exited(0).out_lacks("looks done");

    // And the completion is what the envelope is refused for even where something
    // else in it is wrong too: asked before the version and before the ops, so a
    // monitor that mistypes an edit envelope is told the thing that matters
    // rather than being sent to fix the version and try the same escalation
    // again.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "author": "monitor",
                "completion": true,
                "reason": "looks finished to me",
                "commands": [{"op": "complete", "reason": "and here it is again"}],
            })
            .to_string(),
        )
        .exited(REFUSED)
        .err_has("not something the monitor may do")
        .err_lacks("requires version");

    // The same envelope from the planner is accepted, which is what makes the
    // refusal about the author rather than about the shape.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "completion": true,
                "reason": "the run is finished",
                "version": 2,
                "commands": [
                    {"op": "finding", "message": "the build looks done", "id": "build"}
                ],
            })
            .to_string(),
        )
        .exited(0);
    // A `finding` changes no graph, so it is journalled under the kind that says
    // so — the record is still the run's account of an accepted command.
    world.until("the planner's command to reach the record", |world| {
        !world.events_of(&run, "command-accepted").is_empty()
    });
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
    world.run(&["start", &path, "--attach"]).exited(0);

    world
        .run_with_stdin(
            &["reply", "undrivenmonitor"],
            &json!({
                "version": 2,
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

/// A message the shell would have eaten reaches the queue byte for byte, by
/// both body paths.
///
/// Every journey here builds argv directly, so what this proves is the half this
/// crate owns: the body arrives unaltered through the real verb and out of the
/// real reader. Divergence 38 records the half it removes.
#[test]
fn a_message_full_of_shell_metacharacters_reaches_the_queue_unchanged() {
    let world = World::new("channel-metachars");
    world.script("build.wait", "hold");
    let run = running(&world, "verbatim", vec![agent("build", &[])]);

    let body = "the gate ran `just check` and $(cat /etc/hostname) came back; \
                the log said a | b && c > d, with 'quotes' \"of both kinds\" and a $HOME";

    world
        .run_with_stdin(&["surface", &run, "--kind", "check-in"], body)
        .exited(0)
        .out_has("\"queued\"");
    let read = world.run(&["next", &run]);
    read.exited(0);
    assert_eq!(read.json()["surface"]["message"], body);

    // The file form is handed a trailing newline the queue must not keep.
    let path = world.root.join("body.txt");
    std::fs::write(&path, format!("{body}\n")).expect("the body is written");
    let path = path.to_string_lossy().into_owned();
    world
        .run(&["surface", &run, &path, "--kind", "check-in"])
        .exited(0);
    let again = world.run(&["next", &run]);
    again.exited(0);
    assert_eq!(again.json()["surface"]["message"], body);

    world.release("build.go");
}

/// A watcher says something through the envelope it already writes its edits in,
/// and the question it stopped to ask is read **before** the narration queued
/// ahead of it.
///
/// Both halves of the failure this is about, in one run: a monitor that surfaced
/// on every turn it took buried a blocking question behind fourteen restatements
/// of an intent to look, and the planner read it fifteen minutes late with the
/// whole frontier stopped behind it. So a finding is now a deliberate op — a turn
/// with nothing to report writes no command and queues nothing — and a blocking
/// surface is handed out first whatever it was queued behind.
#[test]
fn a_blocking_finding_is_read_before_the_narration_queued_ahead_of_it() {
    let world = World::new("channel-finding");
    world.script("build.wait", "hold");
    let run = running(
        &world,
        "ranked",
        vec![agent("build", &[]), agent("after", &["build"])],
    );

    // The pile a question used to be read out from behind.
    let narration: Vec<String> = (0..3).map(|n| format!("looking at {n}")).collect();
    for update in &narration {
        world
            .run_with_stdin(&["surface", &run, "--kind", "finding"], update)
            .exited(0);
    }

    // The monitor's question, raised through `commands` — the same envelope its
    // edits ride, which is what lets a turn with nothing to report say nothing.
    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"version":2,"author":"monitor","commands":[{"op":"finding",
               "message":"which base should build target?","blocking":true,"id":"build"}]}"#,
        )
        .exited(0);
    world.until("the finding to reach the planner's queue", |world| {
        world
            .events_of(&run, "planner-surface-queued")
            .iter()
            .any(|event| event["payload"]["blocking"] == json!(true))
    });

    // Blocking first. Three narrations were queued ahead of it and the reader
    // takes the question anyway, because it is the only one holding a subtree.
    let read = world.run(&["next", &run]);
    read.exited(0);
    let surface = read.json()["surface"].clone();
    assert_eq!(surface["message"], "which base should build target?");
    assert_eq!(surface["blocking"], json!(true));
    assert_eq!(surface["kind"], "finding");
    assert_eq!(surface["workstream"], "build");
    // Raised by the watcher, and recorded as the watcher's: a journal reader
    // tells a monitor's finding from a worker's proposal.
    assert_eq!(surface["source"], "monitor");

    // One thing said once. Every other monitor op additionally raises a "monitor
    // applied an edit" surface, and a finding is the op that has already spoken.
    assert!(
        world
            .events_of(&run, "planner-surface-queued")
            .iter()
            .all(|event| event["payload"]["kind"] != json!("monitor-edit")),
        "the finding was also reported as an edit the monitor made"
    );

    // The run is genuinely waiting on a decision, which is what holds `after`
    // back — and it stays waiting while the narration behind it is read, because
    // reading a report is not answering a question.
    assert!(!world.events_of(&run, "decision-pending").is_empty());
    let held = "waiting for planner decision: finding — which base should build target?";
    world.run(&["status", &run]).exited(0).out_has(held);
    for update in &narration {
        let next = world.run(&["next", &run]);
        next.exited(0);
        let narrated = next.json()["surface"].clone();
        assert_eq!(narrated["message"], update.as_str());
        // Typed at the verb rather than raised through the envelope, and
        // recorded as what it is: the kind the caller asked for, and advice as
        // its source — not the pacemaker's, which is the only other thing
        // `surface` can queue.
        assert_eq!(narrated["kind"], "finding");
        assert_eq!(narrated["source"], "proposal");
        assert_eq!(narrated["blocking"], json!(false));
        world
            .run(&["status", &run])
            .exited(0)
            .out_has(held)
            .out_lacks("waiting for planner reply");
    }

    world
        .run_with_stdin(&["reply", &run], r#"{"completion":false,"message":"main"}"#)
        .exited(0);
    world.until("the decision to clear", |world| {
        !world.events_of(&run, "decision-cleared").is_empty()
    });
    world.release("build.go");
}

/// A finding is validated like every other op — and the node it names is
/// optional, so the two answers a name can get are both here beside the
/// unnamed case that skips the question.
///
/// The node matters most when the finding is blocking, because the subtree it
/// holds is derived from the node it names — so a name the graph does not carry
/// would hold nothing back while still reading, in every planner view, as a
/// decision the run is waiting on.
#[test]
fn a_finding_is_placed_by_the_node_it_names_or_refused_by_it() {
    let world = World::new("channel-finding-refused");
    world.script("build.wait", "hold");
    let run = running(&world, "unplaceable", vec![agent("build", &[])]);

    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"version":2,"author":"monitor","commands":[{"op":"finding",
               "message":"the base moved","blocking":true,"id":"ghost"}]}"#,
        )
        .exited(REFUSED)
        .err_has("ghost")
        .err_has("build");
    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"version":2,"author":"monitor","commands":[{"op":"finding","message":"   "}]}"#,
        )
        .exited(REFUSED)
        .err_has("empty message");

    assert!(world.events_of(&run, "planner-surface-queued").is_empty());
    let empty = world.run(&["next", &run]);
    empty.exited(0);
    assert_eq!(empty.json()["status"], "running");

    // And a finding about the run rather than about any one node names none, so
    // there is no question for the graph to answer. It is queued with nothing to
    // place it against, which is what a whole-run observation is.
    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"version":2,"author":"monitor","commands":[{"op":"finding",
               "message":"nothing in this plan covers the migration the goal asks for"}]}"#,
        )
        .exited(0);
    world.until("the unplaced finding to reach the planner", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });
    let read = world.run(&["next", &run]);
    read.exited(0);
    let surface = read.json()["surface"].clone();
    assert_eq!(
        surface["message"],
        "nothing in this plan covers the migration the goal asks for"
    );
    assert_eq!(surface["kind"], "finding");
    assert_eq!(surface["blocking"], json!(false));
    assert_eq!(surface["workstream"], serde_json::Value::Null);
    world.release("build.go");
}

/// A verdict applied by `reply` itself, because nothing was driving the run, is
/// recorded and named exactly as one the reconciler applied.
///
/// Which process took the run's ownership lock is an accident of what else was
/// running, and it decides where the commands are compiled — not whether the
/// ruling beside them happened. A journal that recorded the verdict only where a
/// loop was alive would lose it for exactly the runs a manager reaches for by
/// hand.
#[test]
fn a_verdict_beside_commands_applied_with_nothing_driving_is_journalled_and_named() {
    let world = World::new("channel-undriven-verdict");
    let path = world.plan(
        "undrivenverdict",
        &plan_of("undrivenverdict", vec![human("approve", &[])]),
    );
    world.run(&["start", &path, "--attach"]).exited(0);

    let applied = world.run_with_stdin(
        &["reply", "undrivenverdict"],
        &json!({
            "completion": true,
            "reason": "the approval is all that is left",
            "version": 2,
            "commands": [{"op": "finding", "message": "waiting on a person", "id": "approve"}],
        })
        .to_string(),
    );
    applied.exited(0);
    let receipt = applied.json();
    // `0` because this process applied them itself: there was no queue to name.
    assert_eq!(receipt["reply"], json!(0));
    assert_eq!(receipt["state"], "applied");
    assert_eq!(receipt["verdict"], "delivered");
    assert_eq!(receipt["commands"], "applied");

    let replied = world.events_of("undrivenverdict", "planner-replied");
    assert_eq!(replied.len(), 1, "{replied:?}");
    assert_eq!(
        replied[0]["payload"]["reason"],
        "the approval is all that is left"
    );
    let requested = world.events_of("undrivenverdict", "completion-requested");
    assert_eq!(requested.len(), 1, "{requested:?}");
    assert_eq!(
        requested[0]["payload"]["reason"],
        "the approval is all that is left"
    );
    // And the command half was applied in the same process. A `complete` changes
    // no graph — its record is the `completion-requested` above — so it is
    // journalled under the kind that says an accepted command committed no graph
    // operation, which is still this process saying it applied it.
    assert!(
        !world
            .events_of("undrivenverdict", "command-accepted")
            .is_empty(),
        "the commands were not applied: {:?}",
        world.kinds("undrivenverdict")
    );
}

/// A finding raised while nothing is driving the run reaches the planner all the
/// same, applied by the `reply` that carried it.
///
/// Which side applies an edit is an accident of whether a driver happened to be
/// alive, and both sides raise what the op compiled to — otherwise a watcher's
/// finding would be silently swallowed by exactly the runs a planner is most
/// likely to be away from.
#[test]
fn a_finding_raised_while_nothing_drives_the_run_still_reaches_the_planner() {
    let world = World::new("channel-finding-undriven");
    let path = world.plan("parked", &plan_of("parked", vec![human("approve", &[])]));
    world.run(&["start", &path, "--attach"]).exited(0);

    world
        .run_with_stdin(
            &["reply", "parked"],
            r#"{"version":2,"commands":[{"op":"finding","message":"the approval has been waiting a while","id":"approve"}]}"#,
        )
        .exited(0)
        .out_has("\"applied\"");

    let read = world.run(&["next", "parked"]);
    read.exited(0);
    let surface = read.json()["surface"].clone();
    assert_eq!(surface["message"], "the approval has been waiting a while");
    assert_eq!(surface["kind"], "finding");
    assert_eq!(surface["workstream"], "approve");
    // The planner's own, so the record keeps it apart from a watcher's, and
    // non-blocking by default: an observation holds nothing back unless it says
    // so.
    assert_eq!(surface["source"], "proposal");
    assert_eq!(surface["blocking"], json!(false));
}

/// One envelope of each shape a reply can take, against the receipt entry 64 of
/// `docs/contract-divergences.md` states.
///
/// The manager who had sent two envelopes to one question could not tell from
/// either receipt which of them had answered it.
#[test]
fn the_reply_receipt_names_each_half_the_envelope_carried() {
    let world = World::new("channel-receipt-halves");
    world.script("build.wait", "hold");
    let run = running(&world, "receipted", vec![agent("build", &[])]);

    let note = |text: &str| {
        json!({"op": "note", "id": "build", "addressee": "worker",
               "text": text, "deliver": "next"})
    };
    // Each envelope with the halves it carries, which is what entry 64's presence
    // rules are read against.
    let shapes = [
        (
            "a verdict alone",
            json!({"completion": false, "reason": "carry on"}),
            true,
            false,
        ),
        (
            "commands alone",
            json!({"version": 2, "commands": [note("the fixture moved")]}),
            false,
            true,
        ),
        (
            "both halves",
            json!({
                "completion": false,
                "reason": "carry on, with this in hand",
                "version": 2,
                "commands": [note("and again")],
            }),
            true,
            true,
        ),
    ];

    for (shape, envelope, verdict, commands) in shapes {
        let answered = world.run_with_stdin(&["reply", &run], &envelope.to_string());
        answered.exited(0);
        let receipt = answered.json();
        stated_by_entry_64(&receipt, shape, verdict, commands);
        // And each half's word is its own, so an envelope that did both is not
        // read off one of them: the note landed, and the ruling did too.
        if commands {
            assert_eq!(receipt["commands"], "applied", "{shape}: {receipt}");
        }
        if verdict {
            assert_eq!(receipt["verdict"], "delivered", "{shape}: {receipt}");
        }
        assert_eq!(
            receipt["state"],
            if commands { "applied" } else { "delivered" },
            "{shape}: {receipt}"
        );
    }

    world.release("build.go");
}

/// Hold one receipt to entry 64 of `docs/contract-divergences.md`: every key it
/// answers is one the record names, every key that record names is present
/// exactly when the half it depends on was carried, and every word is one that
/// key may answer.
///
/// `live_edit.rs`'s `from_entry_57` reads its entry the same way.
fn stated_by_entry_64(receipt: &Value, shape: &str, verdict: bool, commands: bool) {
    let record = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
    )
    .expect("the divergence record reads");
    let entry = record
        .split("\n## ")
        .find(|entry| entry.starts_with("64."))
        .expect("the divergence record still carries entry 64");
    let block = entry
        .split("```json")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("entry 64 carries the json block this journey drives");
    let stated: serde_json::Map<String, Value> =
        serde_json::from_str(block).expect("entry 64's block is a JSON object");

    let answered = receipt
        .as_object()
        .expect("the receipt is one JSON object")
        .keys();
    for key in answered {
        assert!(
            stated.contains_key(key),
            "{shape}: the receipt answered '{key}', which entry 64 does not name: {receipt}"
        );
    }
    for (key, rule) in &stated {
        let present = rule["present"]
            .as_str()
            .expect("entry 64 states each key's presence rule");
        let carried = match present {
            "always" => true,
            "with a verdict half" => verdict,
            "with commands" => commands,
            other => panic!("entry 64 states a presence rule this journey cannot drive: {other}"),
        };
        let answer = receipt.get(key);
        assert_eq!(
            answer.is_some(),
            carried,
            "{shape}: '{key}' is stated present {present}, and the receipt says otherwise: \
             {receipt}"
        );
        let Some(answer) = answer else { continue };
        if let Some(words) = rule["values"].as_array() {
            assert!(
                words.contains(answer),
                "{shape}: '{key}' answered {answer}, which entry 64 does not state: {words:?}"
            );
        } else {
            assert!(answer.is_u64(), "{shape}: '{key}' answered {answer}");
        }
    }
}

/// And a reader that only knows the older answer still reads a correct result
/// from each of them.
///
/// `reply` and `state` keep their spelling and their meaning, and the two new
/// keys are added beside them rather than in place of anything — so a consumer
/// written before this change is unaffected, which is what lets the keys land
/// with no consumer to coordinate with.
#[test]
fn a_reader_of_the_older_receipt_still_reads_every_answer() {
    /// The receipt as it was before the two halves were named: exactly the two
    /// keys such a reader knew, and no tolerance for the ones it did not — serde
    /// ignores what it was not told about, which is the property under test.
    #[derive(serde::Deserialize)]
    struct OlderReceipt {
        reply: u64,
        state: String,
    }

    let world = World::new("channel-receipt-older-reader");
    world.script("build.wait", "hold");
    let run = running(&world, "olderreader", vec![agent("build", &[])]);

    let note = |text: &str| {
        json!({"op": "note", "id": "build", "addressee": "worker",
               "text": text, "deliver": "next"})
    };
    // Both readings of one answer: what the older shape takes out of it, and what
    // is actually on the wire. Every key the older reader knows has to read back
    // the same value, which is the whole of what "unaffected" means.
    let read = |world: &World, envelope: &serde_json::Value| {
        let answered = world.run_with_stdin(&["reply", &run], &envelope.to_string());
        answered.exited(0);
        let older: OlderReceipt = serde_json::from_str(answered.stdout.trim())
            .expect("the older reader reads the receipt");
        let whole = answered.json();
        assert_eq!(json!(older.reply), whole["reply"], "{whole}");
        assert_eq!(json!(older.state), whole["state"], "{whole}");
        older
    };

    let verdict_only = read(&world, &json!({"completion": false, "reason": "carry on"}));
    assert_eq!(verdict_only.state, "delivered");

    let commands_only = read(
        &world,
        &json!({"version": 2, "commands": [note("the fixture moved")]}),
    );
    assert_eq!(commands_only.state, "applied");

    let both = read(
        &world,
        &json!({
            "completion": false,
            "reason": "carry on, with this in hand",
            "version": 2,
            "commands": [note("and again")],
        }),
    );
    assert_eq!(both.state, "applied");

    world.release("build.go");
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] what this journey waits
// on is a `channel serve` session reaching its own one-second bound, which is the state the
// finding it proves is only visible in — an asker between listeners, its question still
// owed an answer — and there is no cheaper way to arrange it: the bound is the binary's own
// and a shorter one is not a unit this crate accepts. One second is what the two journeys
// it sits beside already wait, and it belongs in this file with the rest of the channel's
// routing, which any change under `src/` can move: a project edged narrower than the crate
// could not honestly run it, and edging one around a single journey would split the
// channel's behaviour across two projects to no reader's benefit.
/// A verdict is taken by whichever listener is polling when it lands, and never
/// by the question it names.
///
/// The journey entry 63 of `docs/contract-divergences.md` rests on, which is
/// where what it costs and what it is waiting on are stated. Two listeners and
/// one ruling: the question's own asker is between sessions, so the second
/// listener is the one polling, and it reads the ruling back while the run
/// reports the question answered.
///
/// It runs on top of entry 62's repair rather than around it: the question below
/// is in the slot and owed an answer when the ruling lands.
#[test]
fn a_verdict_is_taken_by_whichever_listener_polls_for_it_rather_than_by_the_question_it_names() {
    use std::io::{BufRead, BufReader, Write};

    let world = World::new("channel-verdict-stolen");
    world.script("build.wait", "hold");
    let run = running(&world, "stolen", vec![agent("build", &[])]);
    let worker = "the-dispatch-that-is-blocked";

    // The blocking question, raised by a listener that then reaches its **own
    // session bound** with the agent's stream still open. That ending withdraws
    // nothing — the member is still there and still owed an answer — so what it
    // leaves is the state the manager sees: a question the run is waiting on, an
    // agent still blocked on it, and no listener of that agent polling.
    let mut asking = world
        .cmd(&["channel", "serve", &run])
        .env(onepipeline::channel::ASKER_ENV, worker)
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .env("ONEPIPELINE_SERVE_SESSION_SECONDS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut blocked = asking.stdin.take().expect("stdin is piped");
    writeln!(
        blocked,
        r#"{{"kind":"blocker","message":"is this base still right?","node":"build"}}"#
    )
    .expect("the frame is written");
    blocked.flush().expect("the frame flushes");
    world.until("the question to reach the planner", |world| {
        !world.events_of(&run, "planner-surface-queued").is_empty()
    });
    let reached = asking.wait_with_output().expect("the session ends");
    assert!(reached.status.success(), "{reached:?}");
    assert!(
        String::from_utf8_lossy(&reached.stderr).contains("ONEPIPELINE_SERVE_SESSION_SECONDS"),
        "the listener ended some other way than on its bound: {}",
        String::from_utf8_lossy(&reached.stderr)
    );
    world.run(&["next", &run]).exited(0).out_has("is this base");
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("waiting for planner decision: blocker — is this base still right?");

    // And a listener of somebody else entirely — the monitor's own scoring
    // session, which raised a narration nobody is waiting on and is now in the
    // wait every frame is followed by.
    let mut scoring = world
        .cmd(&["channel", "serve", &run])
        .env(
            onepipeline::channel::ASKER_ENV,
            "the-monitors-scoring-session",
        )
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut narrating = scoring.stdin.take().expect("stdin is piped");
    writeln!(
        narrating,
        r#"{{"kind":"planner-question","message":"scoring the run","blocking":false}}"#
    )
    .expect("the frame is written");
    narrating.flush().expect("the frame flushes");
    world.until("the narration to reach the planner", |world| {
        world
            .events_of(&run, "planner-surface-queued")
            .iter()
            .any(|event| event["payload"]["message"] == "scoring the run")
    });

    // The manager answers the blocking question, and the receipt says delivered.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "completion": false,
                "message": "yes, that base is still right",
                "reason": "answered the blocker",
            })
            .to_string(),
        )
        .exited(0)
        .out_has("\"delivered\"");

    // The scoring session read it. Nothing addressed it there; it was simply the
    // reader that asked next.
    let stolen = BufReader::new(scoring.stdout.take().expect("stdout is piped"))
        .lines()
        .next()
        .expect("the scoring session wrote a line")
        .expect("the line reads");
    assert!(
        stolen.contains("yes, that base is still right"),
        "the scoring session read back something else: {stolen}"
    );

    // And every manager-visible indicator now says the question was answered:
    // the slot was cleared by the same call that queued the ruling.
    world
        .run(&["status", &run])
        .exited(0)
        .out_lacks("waiting for planner decision");

    // The blocked agent re-asks the identical question, and this time it is the
    // reader holding the wait — so the manager's second, identical copy is the
    // one that reaches it. That is the whole of the original observation.
    let mut reasking = world
        .cmd(&["channel", "serve", &run])
        .env(onepipeline::channel::ASKER_ENV, worker)
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "120")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut again = reasking.stdin.take().expect("stdin is piped");
    writeln!(
        again,
        r#"{{"kind":"blocker","message":"is this base still right?","node":"build"}}"#
    )
    .expect("the frame is written");
    again.flush().expect("the frame flushes");
    world.until("the question to be asked a second time", |world| {
        world
            .events_of(&run, "planner-surface-queued")
            .iter()
            .filter(|event| event["payload"]["message"] == "is this base still right?")
            .count()
            >= 2
    });
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({
                "completion": false,
                "message": "yes, that base is still right",
                "reason": "answered the blocker",
            })
            .to_string(),
        )
        .exited(0);
    let answered = BufReader::new(reasking.stdout.take().expect("stdout is piped"))
        .lines()
        .next()
        .expect("the re-asking session wrote a line")
        .expect("the line reads");
    assert!(
        answered.contains("yes, that base is still right"),
        "the resend did not reach the asker either: {answered}"
    );

    drop(blocked);
    drop(narrating);
    drop(again);
    world.release("build.go");
    ended(scoring);
    ended(reasking);
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
