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
    agent, ended, human, plan_of, Run, World, NOTHING_DRIVING, REFUSED, SURFACE_WAITING,
    USAGE_ERROR, WATCH_ELAPSED,
};

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

/// The three classes this verb exists for — a graph edit, a surface being raised
/// and a node settling — each reach the watch as a line of its own, and the
/// machine-readable form carries the same three. The other five meaningful kinds
/// are driven by the journey below this one.
///
/// The graph edit is issued by the **monitor**, because an edit is emitted
/// whichever author issued it and the monitor's is the author a supervisor is
/// least expecting.
#[test]
fn an_edit_a_surface_and_a_settlement_each_reach_the_watch_as_a_line() {
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

    // The profile selection is `monitor`'s and shapes the event view exactly as
    // it does there: the shipped `planner` profile a watch reads through when it
    // is given neither is every pipeline-level event, so the same three survive
    // `--all` and the shipped `monitor` profile too. Given neither — and given no
    // bound either — the defaults carry a watch over a run that has finished.
    for selection in [
        vec!["watch", &run],
        vec!["watch", &run, "--all"],
        vec!["watch", &run, "--filter", "monitor"],
    ] {
        let again = world.run(&selection);
        agreed(&again, "settled", 0);
        let kinds = emitted(&again);
        for kind in ["edit-committed", "planner-surface-queued", "node-settled"] {
            assert!(
                kinds.iter().any(|emitted| emitted == kind),
                "{selection:?} lost `{kind}`, leaving {kinds:?}"
            );
        }
    }
}

/// The decision, completion and stop records a supervisor acts on reach the
/// watch as lines too — including the edit the reconciler refused, which is as
/// much a thing to act on as one that landed.
#[test]
fn the_decision_completion_and_stop_records_reach_the_watch_as_lines_too() {
    let world = World::new("watch-decisions");
    world.script("build.wait", "hold");
    let run = running(&world, "watchdecisions", vec![agent("build", &[])]);

    // llmlint: ignore-block[tests_mirror_real_usage] the durable queue is written
    // directly for the one reason `live_edit.rs` writes it directly: an edit the
    // *reconciler* refuses is the case a user cannot type, because the submission check
    // would reject this one first and `edit-rejected` — the record this journey is here
    // to see emitted — would never be written. Everything else below goes through the
    // binary's own verbs.
    std::fs::write(
        world.run_file(&run, "channel/commands.jsonl"),
        format!(
            "{}\n",
            json!({"id": 0, "commands": [{"op": "cancel", "id": "nowhere"}]})
        ),
    )
    .expect("the command is queued");
    // llmlint: ignore-end[tests_mirror_real_usage]
    world.until("the reconciler to refuse the edit", |world| {
        !world.events_of(&run, "edit-rejected").is_empty()
    });

    // A blocking surface begins holding the subtree that depends on the node it
    // names, and answering it releases exactly that subtree.
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
        r#"{{"kind":"blocker","message":"is this still wanted?","blocking":true,"node":"build"}}"#
    )
    .expect("the frame is written");
    stdin.flush().expect("flushed");
    world.until("the decision to begin holding the subtree", |world| {
        !world.events_of(&run, "decision-pending").is_empty()
    });
    world.run(&["next", &run]).exited(0);
    world
        .run_with_stdin(
            &["reply", &run],
            r#"{"completion":false,"reason":"carry on"}"#,
        )
        .exited(0);
    world.until("the decision to clear", |world| {
        !world.events_of(&run, "decision-cleared").is_empty()
    });
    drop(stdin);
    ended(serving);

    // The planner asks for completion, independently of any graph mutation.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({"version": 1, "commands": [
                {"op": "complete", "reason": "that is as far as this goes"}
            ]})
            .to_string(),
        )
        .exited(0);
    world.until("the completion request to be recorded", |world| {
        !world.events_of(&run, "completion-requested").is_empty()
    });

    world.run(&["stop", &run, "--force"]).exited(0);
    world.until("the stop to be recorded", |world| {
        !world.events_of(&run, "run-stopped").is_empty()
    });

    let watched = world.run(&["watch", &run, "--timeout", "30", "--tick-interval", "0"]);
    let kinds = emitted(&watched);
    for kind in [
        "edit-rejected",
        "decision-pending",
        "decision-cleared",
        "completion-requested",
        "run-stopped",
    ] {
        assert!(
            kinds.iter().any(|emitted| emitted == kind),
            "the watch calls `{kind}` meaningful and emitted none, only {kinds:?}"
        );
    }

    world.release("build.go");
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

/// A profile that excludes a meaningful event suppresses it, exactly as it does
/// for `monitor`: what makes an event *meaningful* decides what a watch may
/// emit, and the profile decides what this reader is shown of it.
#[test]
fn a_profile_that_excludes_a_meaningful_event_keeps_it_off_the_watch() {
    let world = World::new("watch-filtered");
    world.script("build.wait", "hold");
    let run = running(&world, "watchfiltered", vec![agent("build", &[])]);
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
    world.release("build.go");
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });

    let shaped = world.run(&[
        "watch",
        &run,
        "--filter",
        r#"{"exclude": [{"kind": "planner-surface-queued"}]}"#,
        "--timeout",
        "30",
        "--tick-interval",
        "0",
    ]);
    agreed(&shaped, "settled", 0);
    let kinds = emitted(&shaped);
    assert!(
        kinds.iter().any(|kind| kind == "node-settled"),
        "the profile took an event it does not exclude: {kinds:?}"
    );
    assert!(
        !kinds.iter().any(|kind| kind == "planner-surface-queued"),
        "the profile excluded the surface and the watch emitted it anyway: {kinds:?}"
    );
    shaped.err_lacks("the base moved under us");
}

/// A run held on a **ready human action** is not a run held on a blocking
/// surface, and `--until surface` deliberately does not return on one.
///
/// The scope this verb was given, asserted rather than left to a reader of the
/// divergence record: `status` calls both a decision point, this returns on one
/// of them, and which one is the thing a caller has to know.
#[test]
fn a_run_waiting_on_a_person_is_not_a_blocking_surface_and_the_wait_runs_out() {
    let world = World::new("watch-human");
    let run = "watchhuman";
    // A held node beside the action, so the run is still being *driven* while it
    // waits on a person: a plan whose only node is a ready human action has no
    // driver left, which is the different answer this journey is not about.
    world.script("build.wait", "hold");
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), human("sign", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the human action to be waiting on a person", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["payload"]["status"] == json!("waiting"))
    });

    let watched = world.run(&["watch", run, "--timeout", "3", "--tick-interval", "1"]);
    agreed(&watched, "elapsed", WATCH_ELAPSED);
    // Nothing is unread either: a ready human action produces no surface, which
    // is exactly why a watch cannot report it as one.
    assert!(
        watched.stderr.contains("0 unread planner surfaces"),
        "a ready human action was counted as an unread surface:\n{}",
        watched.stderr
    );

    world.release("build.go");
}

/// A watch that cannot write what it has to say refuses, rather than going on
/// emitting into a descriptor nobody can take it.
///
/// Unix-only because `/dev/full` is: what it buys is a descriptor that accepts a
/// write and then fails it, which is the shape a closed pipe has and which no
/// portable spelling of this suite can produce on demand. The behaviour it holds
/// is not platform-specific — a watch is a blocking verb, and one that swallowed
/// a failed write would be reporting a run to nobody for as long as its timeout.
#[cfg(unix)]
#[test]
fn a_watch_whose_output_cannot_be_written_refuses_instead_of_going_on() {
    let world = World::new("watch-unwritable");
    world.script("build.wait", "hold");
    let run = running(&world, "watchunwritable", vec![agent("build", &[])]);

    for descriptor in ["stdout", "stderr"] {
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .expect("this host has /dev/full");
        let mut command = world.cmd(&["watch", &run, "--timeout", "600", "--tick-interval", "1"]);
        if descriptor == "stdout" {
            command.stdout(full).stderr(std::process::Stdio::piped());
        } else {
            command.stderr(full).stdout(std::process::Stdio::piped());
        }
        // The wait is ten minutes and the interval is one second, so a command
        // that returns at all returned because a write failed rather than
        // because it ran out of time.
        let finished = command.output().expect("the watch runs");
        assert_ne!(
            finished.status.code(),
            Some(0),
            "a watch whose {descriptor} could not be written reported success: {}{}",
            String::from_utf8_lossy(&finished.stdout),
            String::from_utf8_lossy(&finished.stderr)
        );
        if descriptor == "stdout" {
            // Standard error still works here, so the refusal is reportable and
            // says which descriptor failed.
            assert_eq!(finished.status.code(), Some(REFUSED));
            let said = String::from_utf8_lossy(&finished.stderr);
            assert!(
                said.contains("could not write to standard output"),
                "the refusal does not say which descriptor failed: {said}"
            );
        } else {
            // Nothing can be reported when the reporting descriptor is the dead
            // one, so what this holds is that the watch **stopped**: exactly the
            // one record it had written before the failed write, and no more.
            let written = String::from_utf8_lossy(&finished.stdout);
            assert_eq!(
                written
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count(),
                1,
                "the watch went on emitting past a write it could not make: {written}"
            );
        }
    }

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

/// What happens **while** the watch is already blocking reaches it as it happens.
///
/// This is the verb working rather than the verb reporting: every journey above
/// asks a run that has already done something, and a watch that only ever read
/// the past would pass all of them while telling a live supervisor nothing. So
/// this one starts the watch on a run where nothing has happened, waits for the
/// heartbeat that proves it is blocking with nothing to say, and only then
/// raises a surface and lets the node finish.
#[test]
fn what_happens_while_the_watch_is_blocking_reaches_it_as_it_happens() {
    use std::io::{BufRead, BufReader, Read};

    let world = World::new("watch-live");
    world.script("build.wait", "hold");
    let run = running(&world, "watchlive", vec![agent("build", &[])]);

    let mut watching = world
        .cmd(&["watch", &run, "--timeout", "600", "--tick-interval", "1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the watch starts");
    let mut records = BufReader::new(
        watching
            .stdout
            .take()
            .expect("the watch writes its machine form to standard output"),
    )
    .lines()
    .map(|line| {
        let line = line.expect("the watch's output reads");
        serde_json::from_str::<Value>(&line)
            .unwrap_or_else(|e| panic!("the watch wrote a line that is not JSON ({e}): {line}"))
    });

    // A heartbeat is only written after an interval in which nothing happened,
    // so reading one is what proves the watch is blocking rather than finished.
    let mut before = Vec::new();
    loop {
        let record = records
            .next()
            .expect("the watch keeps writing while it waits");
        let beat = record["watch"] == json!("heartbeat");
        before.push(record);
        if beat {
            break;
        }
    }
    assert!(
        before
            .iter()
            .all(|record| record["watch"] != json!("event")),
        "the watch emitted an event before anything had happened: {before:?}"
    );

    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "finding",
            "--message",
            "raised while the watch was already blocking",
        ])
        .exited(0);
    world.release("build.go");

    // Everything from here arrived after the watch was already waiting.
    let after: Vec<Value> = records.collect();
    let mut said = String::new();
    watching
        .stderr
        .take()
        .expect("the watch writes its human form to standard error")
        .read_to_string(&mut said)
        .expect("the human form reads");
    let ended = watching.wait().expect("the watch exits");

    let kinds: Vec<&str> = after
        .iter()
        .filter(|record| record["watch"] == json!("event"))
        .filter_map(|record| record["event"]["kind"].as_str())
        .collect();
    for kind in ["planner-surface-queued", "node-settled"] {
        assert!(
            kinds.contains(&kind),
            "`{kind}` happened while the watch was blocking and never reached it, \
             leaving {kinds:?}"
        );
    }
    assert!(
        said.contains("raised while the watch was already blocking"),
        "the human form beside it missed what arrived while it waited:\n{said}"
    );
    let last = after.last().expect("the watch says why it returned");
    assert_eq!(last["watch"], json!("return"), "{last}");
    assert_eq!(last["condition"], json!("settled"), "{last}");
    assert_eq!(ended.code(), Some(0), "{last}");
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

    // `--timeout 0` reads once and returns, which is the shape a caller uses to
    // take a cursor without waiting at all.
    let first = world.run(&["watch", &run, "--timeout", "0", "--tick-interval", "0"]);
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

/// A watch holds its cursor **before** a record whose writer has not finished it,
/// and emits that record once the writer has.
///
/// The tail behind the cursor is the one place a supervisor loses an event
/// outright rather than late: a watch that counted a half-written line as read
/// would move its cursor past it and never come back for it, and on a live run
/// the line being appended is the newest thing there is to say. Driven through
/// the binary because the cursor is the binary's output — what a caller resumes
/// from is the token printed on exit, not a value a function returned.
#[test]
fn a_watch_holds_at_a_record_its_writer_has_not_finished_and_emits_it_after() {
    let world = World::new("watch-torn");
    world.script("build.wait", "hold");
    let run = running(&world, "watchtorn", vec![agent("build", &[])]);
    world.release("build.go");
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });

    // llmlint: ignore-block[tests_mirror_real_usage] the journal is appended to
    // directly because the case under test is a *writer caught halfway through an
    // append*, which is a moment rather than a command: no verb of this binary can be
    // asked to leave half a record behind, and a driver reached that state by chance
    // would make the journey a race. The run has settled first, so this journey is the
    // only writer while it stands in for that moment, and everything it then asks is a
    // real `watch` invocation.
    let journal = world.run_file(&run, "events.jsonl");
    let settlement: Value = std::fs::read_to_string(&journal)
        .expect("the journal reads")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event["kind"] == json!("node-settled"))
        .expect("a settled run recorded a settlement");
    // A record of its own rather than a second copy of that one: its own stream
    // and a later timestamp, so what the resumed watch emits can only be this.
    let mut appended = settlement;
    appended["ts"] = json!("2099-01-01T00:00:00.000Z");
    appended["stream"] = json!("a-writer-caught-halfway");
    let record = serde_json::to_string(&appended).expect("the record renders");
    let (opening, rest) = record.split_at(record.len() / 2);

    let first = world.run(&["watch", &run, "--timeout", "0", "--tick-interval", "0"]);
    agreed(&first, "settled", 0);
    let cursor = returned(&first)["cursor"]
        .as_str()
        .expect("a watch prints a cursor on exit")
        .to_string();

    append(&journal, opening);
    let held = world.run(&[
        "watch",
        &run,
        "--timeout",
        "0",
        "--tick-interval",
        "0",
        "--cursor",
        &cursor,
    ]);
    agreed(&held, "settled", 0);
    assert!(
        emitted(&held).is_empty(),
        "a watch emitted a record whose writer had not finished it:\n{}",
        held.stdout
    );
    assert_eq!(
        returned(&held)["cursor"].as_str(),
        Some(cursor.as_str()),
        "a watch moved its cursor past a record it did not emit, so nothing will \
         ever emit that record:\n{}",
        held.stdout
    );

    append(&journal, &format!("{rest}\n"));
    // llmlint: ignore-end[tests_mirror_real_usage]
    let after = world.run(&[
        "watch",
        &run,
        "--timeout",
        "0",
        "--tick-interval",
        "0",
        "--cursor",
        &cursor,
    ]);
    agreed(&after, "settled", 0);
    assert_eq!(
        emitted(&after),
        vec!["node-settled".to_string()],
        "the finished record never reached the watch resuming from the cursor that \
         stopped before it:\n{}",
        after.stdout
    );
    assert_ne!(
        returned(&after)["cursor"].as_str(),
        Some(cursor.as_str()),
        "the cursor never moved past the record it finally emitted:\n{}",
        after.stdout
    );
}

/// Add to a run's journal exactly what a writer puts there: bytes on the end.
/// A cursor is a place in **one** run's journal, and one pasted against another
/// run is refused by name rather than resumed from.
///
/// This is the refusal neither store check can make. The token below is built to
/// pass both of them against the run it is aimed at: its byte is lifted from
/// *that* run's own journal, so it is in range and sits exactly on one of its
/// record boundaries. Only the run inside the token says it was printed somewhere
/// else — and without it this watch would resume, silently, from a position in a
/// different run that happens to be a number.
#[test]
fn a_cursor_from_another_run_is_refused_rather_than_resumed_from() {
    let world = World::new("watch-foreign-cursor");
    let mine = running(&world, "watchcursormine", vec![agent("build", &[])]);
    let theirs = running(&world, "watchcursortheirs", vec![agent("build", &[])]);
    for run in [&mine, &theirs] {
        world.until("the run to settle", |world| {
            world.run_file(run, "result.json").is_file()
        });
    }

    let watched = world.run(&["watch", &mine, "--timeout", "0", "--tick-interval", "0"]);
    agreed(&watched, "settled", 0);
    let cursor = returned(&watched)["cursor"]
        .as_str()
        .expect("a watch prints a cursor on exit")
        .to_string();
    assert!(
        cursor.starts_with(&format!("1:{mine}:")),
        "the cursor does not name the run it was printed for: {cursor}"
    );

    // A byte that is a real record boundary of the run this is about to be aimed
    // at, so the length and boundary checks both pass and the run is the only
    // thing left that can refuse it.
    let journal = std::fs::read_to_string(world.run_file(&theirs, "events.jsonl"))
        .expect("the journal reads");
    let boundary = journal
        .find('\n')
        .expect("the other run recorded at least one record")
        + 1;
    let foreign = format!("1:{mine}:{boundary}");

    world
        .run(&[
            "watch",
            &theirs,
            "--timeout",
            "0",
            "--tick-interval",
            "0",
            "--cursor",
            &foreign,
        ])
        .exited(REFUSED)
        // The refusal is *this* one, and not the length or boundary refusal
        // standing in for it: those two cannot see this token at all.
        .err_has("was printed by a watch of run")
        .err_has(&mine)
        .err_has(&theirs);

    // The same byte, named for the run it belongs to, is accepted — so what the
    // refusal above rejected is the run in the token and nothing else about it.
    let own = format!("1:{theirs}:{boundary}");
    let resumed = world.run(&[
        "watch",
        &theirs,
        "--timeout",
        "0",
        "--tick-interval",
        "0",
        "--cursor",
        &own,
    ]);
    agreed(&resumed, "settled", 0);
}

/// A kind is a wire string that no library owns, so a **sibling's** event spelled
/// like one of this crate's own is not emitted as one.
///
/// The merged stream carries three libraries, and this verb's meaningful set is
/// this crate's vocabulary. An event from `oneagentgraph` that happens to spell
/// `node-settled` would, on a kind test alone, reach a supervisor as a node
/// settling — reporting as settled work that settled nothing.
#[test]
fn a_siblings_event_spelled_like_this_crates_own_is_not_emitted_as_one() {
    let world = World::new("watch-collision");
    world.script("build.wait", "hold");
    let run = running(&world, "watchcollision", vec![agent("build", &[])]);
    world.release("build.go");
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });

    // `--all` throughout: the shipped profile already rejects a sibling's records,
    // so under it this journey would pass whether or not the verb checked the
    // source. The widest selection is the one where the kind is all that is left
    // to tell the two libraries apart, which is the case under test.
    let first = world.run(&[
        "watch",
        &run,
        "--all",
        "--timeout",
        "0",
        "--tick-interval",
        "0",
    ]);
    agreed(&first, "settled", 0);
    let cursor = returned(&first)["cursor"]
        .as_str()
        .expect("a watch prints a cursor on exit")
        .to_string();

    // llmlint: ignore-block[tests_mirror_real_usage] the journal is appended to
    // directly because no verb of this binary can be asked to emit a *sibling's*
    // envelope under this crate's own kind: the relay writes a sibling's records with
    // that sibling's own vocabulary, and the collision under test is a spelling this
    // build must survive rather than one it can be made to produce. The run has
    // settled first, so this journey is the only writer, and the `watch` below is a
    // real invocation reading a real store.
    let settlement: Value = std::fs::read_to_string(world.run_file(&run, "events.jsonl"))
        .expect("the journal reads")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event["kind"] == json!("node-settled"))
        .expect("a settled run recorded a settlement");
    let mut collision = settlement;
    collision["ts"] = json!("2099-01-01T00:00:00.000Z");
    collision["stream"] = json!("a-sibling-that-spells-it-the-same-way");
    collision["source"] = json!("agentgraph");
    append(
        &world.run_file(&run, "events.jsonl"),
        &format!(
            "{}\n",
            serde_json::to_string(&collision).expect("the record renders")
        ),
    );
    // llmlint: ignore-end[tests_mirror_real_usage]

    let after = world.run(&[
        "watch",
        &run,
        "--all",
        "--timeout",
        "0",
        "--tick-interval",
        "0",
        "--cursor",
        &cursor,
    ]);
    agreed(&after, "settled", 0);
    assert!(
        emitted(&after).is_empty(),
        "a sibling's event reached the watch as one of this crate's own:\n{}",
        after.stdout
    );
    assert!(
        !after
            .stderr
            .contains("a-sibling-that-spells-it-the-same-way"),
        "the human form named a sibling's event as a settlement:\n{}",
        after.stderr
    );
}

fn append(journal: &std::path::Path, bytes: &str) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(journal)
        .expect("the journal is appendable")
        .write_all(bytes.as_bytes())
        .expect("the bytes are appended");
}

/// Everything a watch refuses, it refuses **before** it blocks.
///
/// A watch that waited out its whole timeout to report a mistyped profile or a
/// cursor from another run would be worse than the shell loop it replaces, so
/// each of these is given a bound long enough that a command which returned
/// after waiting could not be mistaken for one that refused.
#[test]
fn every_refusal_a_watch_makes_is_made_before_it_blocks() {
    let world = World::new("watch-refusals");
    world.script("build.wait", "hold");
    let run = running(&world, "watchrefusals", vec![agent("build", &[])]);

    // A run this host does not hold, and a profile this run does not have —
    // named the way `monitor` takes it.
    world
        .run(&["watch", "nosuchrun", "--timeout", "600"])
        .exited(REFUSED)
        .err_has("nosuchrun");
    world
        .run(&[
            "watch",
            &run,
            "--filter",
            "nosuchprofile",
            "--timeout",
            "600",
        ])
        .exited(REFUSED)
        .err_has("nosuchprofile");

    // A cursor is external input like any other. Tokens this build cannot read —
    // no separator, a version it does not speak, a byte that is not one, and the
    // pre-run spelling that named a byte without the run it was a byte of.
    for token in [
        "nonsense",
        "2:watchrefusals:0",
        "1:watchrefusals:later",
        "1:watchrefusals:-1",
        "1::0",
        "1:0",
    ] {
        world
            .run(&["watch", &run, "--cursor", token, "--timeout", "600"])
            .exited(REFUSED)
            .err_has("is not a cursor this build reads");
    }
    // Then the three a token can read as and still not be a place in this run:
    // another run's, one past the end of this store — which would otherwise read
    // as a run where nothing was happening — and one that fits but points into
    // the middle of a record rather than after one.
    world
        .run(&[
            "watch",
            &run,
            "--cursor",
            "1:someotherrun:0",
            "--timeout",
            "600",
        ])
        .exited(REFUSED)
        .err_has("was printed by a watch of run");
    world
        .run(&[
            "watch",
            &run,
            "--cursor",
            "1:watchrefusals:99999999",
            "--timeout",
            "600",
        ])
        .exited(REFUSED)
        .err_has("whose store holds");
    world
        .run(&[
            "watch",
            &run,
            "--cursor",
            "1:watchrefusals:3",
            "--timeout",
            "600",
        ])
        .exited(REFUSED)
        .err_has("inside a record");
    // And a wait no clock can reach is a value to refuse, not a process to abort.
    world
        .run(&["watch", &run, "--timeout", &u64::MAX.to_string()])
        .exited(REFUSED)
        .err_has("this host's clock");

    // The two clocks are on two verbs and neither takes the other's flag: this
    // stream's interval, and the pacemaker cadence a launch sets.
    world
        .run(&["watch", &run, "--heartbeat-interval", "5"])
        .exited(USAGE_ERROR)
        .err_has("--heartbeat-interval");
    let path = world.plan(
        "unstarted",
        &plan_of("unstarted", vec![agent("build", &[])]),
    );
    world
        .run(&["start", &path, "--tick-interval", "5"])
        .exited(USAGE_ERROR)
        .err_has("--tick-interval");
    // A condition the verb does not offer is refused naming the ones it does.
    world
        .run(&["watch", &run, "--until", "whenever", "--timeout", "600"])
        .exited(USAGE_ERROR)
        .err_has("surface");

    world.release("build.go");
}
