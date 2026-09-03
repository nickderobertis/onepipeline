//! `onepipeline watch` — the bounded, resumable wait a supervisor writes their
//! wake loop around.
//!
//! Each journey below drives the compiled binary and asserts on its exit status,
//! on the lines it wrote to standard error, and on the NDJSON it wrote to
//! standard output beside them. The two forms are checked **against each other**
//! rather than one at a time: the failure this verb replaces is a watcher that
//! matched prose, and a machine-readable form that quietly said something else
//! would leave that failure exactly where it was.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes one *sibling* —
// `oneagentgraph` — at its subprocess boundary, and nothing inside the crate under test,
// which is driven here as a real compiled binary against a real run store. What the
// double buys is a dispatch this journey can hold open or fail on demand, which a real
// agent would need paid model turns to produce. `harness.rs` carries the same suppression
// and the full rationale.

use std::io::Write;

use serde_json::{json, Value};

use crate::harness::{
    agent, ended, plan_of, Run, World, NOTHING_DRIVING, REFUSED, SURFACE_WAITING, WATCH_ELAPSED,
};

/// Start a run detached and wait until it is executing.
fn running(world: &World, name: &str, nodes: Vec<Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });
    name.to_string()
}

/// Every NDJSON record the watch wrote to standard output, in order.
///
/// Parsed strictly: a line that is not JSON is the machine-readable form failing
/// at the one thing it promises, so it fails the journey rather than being
/// skipped into an assertion that then passes over what is left.
fn machine(watched: &Run) -> Vec<Value> {
    watched
        .stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| {
                panic!(
                    "`onepipeline {}` wrote a line to stdout that is not JSON ({e}): {line}",
                    watched.args
                )
            })
        })
        .collect()
}

/// The record a watch ends on.
fn returned(watched: &Run) -> Value {
    let records = machine(watched);
    let last = records
        .last()
        .unwrap_or_else(|| panic!("`onepipeline {}` wrote nothing at all", watched.args))
        .clone();
    assert_eq!(
        last["watch"],
        json!("return"),
        "the last record a watch wrote is not the one saying why it returned: {last}"
    );
    last
}

/// The event kinds a watch emitted, in the order it emitted them.
fn emitted(watched: &Run) -> Vec<String> {
    machine(watched)
        .into_iter()
        .filter(|record| record["watch"] == json!("event"))
        .map(|record| {
            record["event"]["kind"]
                .as_str()
                .unwrap_or_else(|| panic!("an emitted event carries no kind: {record}"))
                .to_string()
        })
        .collect()
}

/// Assert the exit status the binary returned is the one its own machine form
/// declared.
///
/// The whole promise of the verb is that a caller branches on the status without
/// reading anything, so a status that disagreed with the record beside it would
/// be two answers to one question.
fn agreed(watched: &Run, condition: &str, code: i32) {
    watched.exited(code);
    let last = returned(watched);
    assert_eq!(last["condition"], json!(condition), "{last}");
    assert_eq!(last["exit"], json!(code), "{last}");
    assert!(
        watched.stderr.contains(condition),
        "the human form beside the machine one does not say `{condition}`:\n{}",
        watched.stderr
    );
}

/// Every class a supervisor acts on reaches the watch as a line of its own — and
/// the machine-readable form carries the same three.
///
/// The graph edit is issued by the **monitor**, which is the author whose edits
/// went unnoticed on the run this verb comes from: four destructive ones matched
/// no wake condition, and what eventually surfaced them was the run dying.
#[test]
fn a_watch_emits_a_line_for_every_class_a_supervisor_acts_on() {
    let world = World::new("watch-classes");
    world.script("build.wait", "hold");
    let run = running(&world, "watchclasses", vec![agent("build", &[])]);

    // A graph edit, issued by the monitor rather than by the planner.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({"version": 1, "author": "monitor", "commands": [
                {"op": "context", "id": "build", "note": "the fixture moved", "deliver": "next"}
            ]})
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
    // A surface raised, before anybody has read it.
    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "finding",
            "--message",
            "the base moved under us",
        ])
        .exited(0);
    // And a node settling.
    world.release("build.go");
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });

    let watched = world.run(&["watch", &run, "--timeout", "60", "--tick-interval", "0"]);
    agreed(&watched, "settled", 0);

    for line in [
        "edit-committed",
        "planner-surface-queued",
        "node-settled",
        "the base moved under us",
    ] {
        assert!(
            watched.stderr.contains(line),
            "the watch emitted no line carrying {line:?}:\n{}",
            watched.stderr
        );
    }

    let kinds = emitted(&watched);
    for kind in ["edit-committed", "planner-surface-queued", "node-settled"] {
        assert!(
            kinds.iter().any(|emitted| emitted == kind),
            "the machine form carries no `{kind}`, only {kinds:?}"
        );
    }
    // The edit reaches the caller as the monitor's, which is the whole point of
    // emitting it: the author is on the record the watch handed over, so a
    // supervisor never has to go back to the store to find out whose it was.
    let edit = machine(&watched)
        .into_iter()
        .find(|record| record["event"]["kind"] == json!("edit-committed"))
        .expect("the edit was emitted");
    assert_eq!(
        edit["event"]["payload"]["author"],
        json!("monitor"),
        "{edit}"
    );
}

/// With nothing happening, the watch says so on the interval it was given — and
/// every heartbeat carries how many planner surfaces are unread and of which
/// kinds.
///
/// This is the failure the verb exists to end. A watch went quiet while
/// twenty-six updates queued and a question was asked three times, because the
/// loop around it filtered for event lines and there were none. Here there is
/// one unread surface and no events at all, and the count rides the line that is
/// written *because* nothing is happening.
#[test]
fn a_quiet_run_gets_heartbeats_that_count_what_is_unread_and_then_the_wait_elapses() {
    let world = World::new("watch-heartbeat");
    world.script("build.wait", "hold");
    let run = running(&world, "watchheartbeat", vec![agent("build", &[])]);
    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "finding",
            "--message",
            "nobody has read this",
        ])
        .exited(0);

    let watched = world.run(&["watch", &run, "--timeout", "4", "--tick-interval", "1"]);
    agreed(&watched, "elapsed", WATCH_ELAPSED);

    let beats: Vec<Value> = machine(&watched)
        .into_iter()
        .filter(|record| record["watch"] == json!("heartbeat"))
        .collect();
    assert!(
        !beats.is_empty(),
        "a four-second watch on a one-second interval said nothing while nothing \
         happened:\n{}",
        watched.stdout
    );
    for beat in &beats {
        assert_eq!(beat["unread"]["count"], json!(1), "{beat}");
        assert_eq!(
            beat["unread"]["kinds"][0]["kind"],
            json!("finding"),
            "{beat}"
        );
        assert_eq!(beat["unread"]["kinds"][0]["count"], json!(1), "{beat}");
    }
    assert!(
        watched
            .stderr
            .contains("1 unread planner surface(s): 1 finding"),
        "the human heartbeat does not state what is unread and of which kind:\n{}",
        watched.stderr
    );

    world.release("build.go");
}

/// A blocking surface returns a status of its own — and only when the watch was
/// asked to return on one.
#[test]
fn a_blocking_surface_returns_a_status_of_its_own_and_only_when_asked_for() {
    let world = World::new("watch-surface");
    world.script("build.wait", "hold");
    let run = running(&world, "watchsurface", vec![agent("build", &[])]);

    // Through the channel server, because that is the only author of a blocking
    // surface: `surface --kind finding` is a report and holds nothing back.
    let mut serving = world
        .cmd(&["channel", "serve", &run])
        // Nobody answers this one, and the server's own wait is not under test.
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"the gate refused; retry?","blocking":true,"node":"build"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the blocking surface to reach the planner", |world| {
        world
            .events_of(&run, "planner-surface-queued")
            .iter()
            .any(|event| event["payload"]["blocking"] == json!(true))
    });

    let watched = world.run(&["watch", &run, "--timeout", "30", "--tick-interval", "0"]);
    agreed(&watched, "surface-waiting", SURFACE_WAITING);
    let last = returned(&watched);
    assert_eq!(
        last["unread"]["kinds"][0]["kind"],
        json!("blocker"),
        "{last}"
    );
    assert!(
        watched
            .stderr
            .contains("1 unread planner surface(s): 1 blocker"),
        "the return line does not say what is waiting:\n{}",
        watched.stderr
    );

    // Asked to wait for the run instead, the same watch over the same state
    // reports the surface and goes on waiting.
    let waited = world.run(&[
        "watch",
        &run,
        "--timeout",
        "2",
        "--tick-interval",
        "1",
        "--until",
        "settled",
    ]);
    agreed(&waited, "elapsed", WATCH_ELAPSED);
    assert!(
        waited.stdout.contains("\"blocker\""),
        "the surface stopped being counted once it stopped ending the wait:\n{}",
        waited.stdout
    );

    // The server is blocked reading the pipe this test still holds; closing it is
    // what lets the frame stream end and the process exit.
    drop(stdin);
    ended(serving);
    world.release("build.go");
}

/// A run nothing is driving returns the status this crate already assigns to
/// that condition, rather than a fresh one of the watch's own.
#[test]
fn a_watch_of_a_run_nothing_is_driving_returns_the_status_this_crate_already_assigns() {
    let world = World::new("watch-undriven");
    let run = "watchundriven";
    // The one node fails, so nothing is ready, nothing waits on a person, and no
    // surface blocks: the run is simply not being driven any more.
    world.script("build.fail", "1");
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the run to stop being driven", |world| {
        world.run(&["status", run]).stdout.contains("DRIVER DEAD")
    });

    let watched = world.run(&["watch", run, "--timeout", "30", "--tick-interval", "0"]);
    agreed(&watched, "nothing-driving", NOTHING_DRIVING);
    // A converged graph with a failed node in it is *not* a run that settled,
    // and exit 0 over it would be the false completion this crate exists to stop
    // reporting.
    assert_ne!(
        watched.code, 0,
        "a failed run was reported as a settled one"
    );
    assert!(
        emitted(&watched).iter().any(|kind| kind == "node-settled"),
        "the watch returned without ever emitting the settlement that ended the run: {}",
        watched.stdout
    );
}

/// A watch resumed from a cursor picks up where the one before it stopped, and
/// repeats nothing.
#[test]
fn a_resumed_watch_does_not_repeat_what_the_one_before_it_emitted() {
    let world = World::new("watch-cursor");
    world.script("build.wait", "hold");
    let run = running(&world, "watchcursor", vec![agent("build", &[])]);
    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "finding",
            "--message",
            "the first thing",
        ])
        .exited(0);

    let first = world.run(&["watch", &run, "--timeout", "2", "--tick-interval", "0"]);
    agreed(&first, "elapsed", WATCH_ELAPSED);
    assert!(
        first.stderr.contains("the first thing"),
        "the first watch never saw what it is being resumed past:\n{}",
        first.stderr
    );
    let cursor = returned(&first)["cursor"]
        .as_str()
        .expect("a watch prints a cursor on exit")
        .to_string();

    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "finding",
            "--message",
            "the second thing",
        ])
        .exited(0);

    let second = world.run(&[
        "watch",
        &run,
        "--timeout",
        "2",
        "--tick-interval",
        "0",
        "--cursor",
        &cursor,
    ]);
    agreed(&second, "elapsed", WATCH_ELAPSED);
    assert!(
        second.stderr.contains("the second thing"),
        "the resumed watch missed what happened after the cursor:\n{}",
        second.stderr
    );
    second.err_lacks("the first thing");
    assert_eq!(
        machine(&second)
            .into_iter()
            .filter(|record| record["watch"] == json!("event"))
            .count(),
        1,
        "the resumed watch re-emitted what the first one already had:\n{}",
        second.stdout
    );

    world.release("build.go");
}

/// A cursor is external input, and one this build cannot place is refused by
/// name rather than resumed from as though its digits meant a byte count.
#[test]
fn a_cursor_this_build_cannot_place_refuses_the_watch_before_it_blocks() {
    let world = World::new("watch-bad-cursor");
    world.script("build.wait", "hold");
    let run = running(&world, "watchbadcursor", vec![agent("build", &[])]);

    for token in ["nonsense", "2:0", "1:later"] {
        world
            .run(&["watch", &run, "--cursor", token, "--timeout", "600"])
            .exited(REFUSED)
            .err_has("is not a cursor this build reads");
    }

    world.release("build.go");
}
