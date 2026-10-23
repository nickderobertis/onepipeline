//! A driver whose cost is its run's live state rather than its history, and member
//! liveness kept beside the journal rather than in it — through the compiled
//! binary against a real run store.
//!
//! No claim here is stated in seconds: each journey proves what happened — a record's
//! presence, an exact count, an unchanged prefix — and a timeout only guards a hang.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven here as
// the real compiled binary over the real run store it wrote; `harness.rs` carries the
// same suppression and the full rationale.

use std::path::{Path, PathBuf};

use crate::harness::{agent, epoch_millis, now_millis, plan_of, World};
use serde_json::{json, Value};

/// A world whose driver pauses where a journey arms it: the debug build's
/// file-based failpoints, under a directory of this world's own.
fn held_world(name: &str) -> (World, PathBuf) {
    let world = World::new(name);
    let seam = world.root.join("pass-seam");
    std::fs::create_dir_all(&seam).expect("the seam's directory");
    let world = world.with_env("ONEPIPELINE_REVIEW_FAILPOINT", &seam.to_string_lossy());
    (world, seam)
}

/// Hold the driver's next reconcile pass at the point it claims the run's queue.
fn hold_the_pass(world: &World, seam: &Path) {
    std::fs::write(seam.join("before-claim"), "").expect("the seam is armed");
    world.until("the driver's pass to be held", |_| {
        seam.join("before-claim.reached").is_file()
    });
}

/// Let the held pass go on.
fn release_the_pass(seam: &Path) {
    let _ = std::fs::remove_file(seam.join("before-claim"));
    std::fs::write(seam.join("before-claim.release"), "").expect("the pass is released");
}

/// One node held running, beating every 100 ms — and, where `works`, emitting a
/// `turn-activity` with every beat.
fn a_held_member(world: &World, node: &str, works: bool) {
    world.script(&format!("{node}.turn-open"), "");
    world.script(&format!("{node}.wait"), "hold");
    world.script(&format!("{node}.heartbeat"), "100");
    if works {
        world.script(&format!("{node}.activity-with-each-beat"), "");
    }
}

/// The members `liveness.json` holds, as written.
fn liveness(world: &World, run: &str) -> Vec<Value> {
    std::fs::read(world.run_file(run, "liveness.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|document| document["members"].as_array().cloned())
        .unwrap_or_default()
}

/// The latest heartbeat `liveness.json` holds for one node, in epoch milliseconds.
fn beat_of(world: &World, run: &str, node: &str) -> Option<u64> {
    liveness(world, run)
        .iter()
        .filter(|member| member["node"] == node)
        .filter_map(|member| member["last_heartbeat_at"].as_str())
        .map(epoch_millis)
        .max()
}

fn activity_of(world: &World, run: &str, node: &str) -> usize {
    world
        .events_of(run, "turn-activity")
        .iter()
        .filter(|event| event["labels"]["node"] == node)
        .count()
}

/// A member's worker events reach `events.jsonl` while the reconcile pass is held:
/// the relay that journals them does not wait for the loop.
#[test]
fn a_members_worker_events_are_journalled_while_the_reconcile_pass_is_held() {
    let (world, seam) = held_world("scaling-relay-held");
    let run = "relayheld";
    a_held_member(&world, "build", true);
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the member to be working", |world| {
        activity_of(world, run, "build") > 0
    });

    hold_the_pass(&world, &seam);
    let held_at = activity_of(&world, run, "build");
    world.until("five more of its records to be journalled", |world| {
        activity_of(world, run, "build") >= held_at + 5
    });
    assert!(
        !seam.join("before-claim.release").exists(),
        "the pass was released before the records were read"
    );

    release_the_pass(&seam);
    world.release("build.go");
}

/// Envelope 204's cause: a `settle` of a node no dispatch is part of, queued while
/// the driver's pass is held, is claimed, applied and answered before the pass is
/// released — and the `reply` that queued it is answered.
#[test]
fn a_settle_queued_while_the_pass_is_held_is_answered_before_the_pass_is_released() {
    let (world, seam) = held_world("scaling-claim-held");
    let run = "claimheld";
    a_held_member(&world, "build", false);
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), agent("later", &["build"])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the member to be heartbeating", |world| {
        beat_of(world, run, "build").is_some()
    });

    hold_the_pass(&world, &seam);
    let replied = world.run_with_stdin(
        &["reply", run],
        &json!({"version": 2, "commands": [
            {"op": "settle", "id": "later", "outcome": "done",
             "evidence": "settled by hand while the driver's pass was held"},
        ]})
        .to_string(),
    );
    replied.exited(0);
    let receipt: Value = replied.json();
    assert_eq!(receipt["state"], json!("applied"), "{receipt}");
    let outcomes = std::fs::read_to_string(world.run_file(run, "channel/command-outcomes.jsonl"))
        .expect("the run's command outcomes");
    let answered: Vec<Value> = outcomes
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert!(
        answered
            .iter()
            .any(|outcome| outcome["applied"] == json!(true)),
        "the settle's outcome is not in the queue: {outcomes}"
    );
    assert!(
        world.events_of(run, "node-settled").iter().any(|event| {
            event["labels"]["node"] == "later" && event["payload"]["status"] == "done"
        }),
        "the settle was answered and not applied"
    );
    assert!(
        seam.join("before-claim.reached").is_file() && !seam.join("before-claim.release").exists(),
        "the pass was not held while the settle was answered"
    );

    release_the_pass(&seam);
    world.release("build.go");
}

/// A member held in an idle turn for at least twenty heartbeats puts none of them in
/// the journal: its entry in `liveness.json` advances to each beat's time instead,
/// and is gone once its dispatch settles.
#[test]
fn a_members_heartbeats_advance_liveness_beside_the_journal_and_never_enter_it() {
    let world = World::new("scaling-liveness");
    let run = "beside";
    a_held_member(&world, "build", false);
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);

    let mut seen: Vec<u64> = Vec::new();
    world.until("twenty heartbeats to advance the member's entry", |world| {
        if let Some(at) = beat_of(world, run, "build") {
            if seen.last() != Some(&at) {
                seen.push(at);
            }
        }
        seen.len() >= 20
    });
    assert!(
        seen.windows(2).all(|pair| pair[0] < pair[1]),
        "the entry did not advance beat by beat: {seen:?}"
    );
    let members = liveness(&world, run);
    assert_eq!(members.len(), 1, "{members:?}");
    assert_eq!(members[0]["member"], "worker", "{members:?}");
    assert!(
        members[0]["graph_run"]
            .as_str()
            .is_some_and(|run| !run.is_empty()),
        "{members:?}"
    );
    assert_eq!(
        world.events_of(run, "member-heartbeat").len(),
        0,
        "a heartbeat was journalled"
    );

    world.release("build.go");
    world.until("the dispatch to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });
    world.until("the member's entry to go with its dispatch", |world| {
        beat_of(world, run, "build").is_none()
    });
    assert_eq!(world.events_of(run, "member-heartbeat").len(), 0);
}

/// A member's liveness advances while the reconcile pass is held.
#[test]
fn liveness_advances_while_the_reconcile_pass_is_held() {
    let (world, seam) = held_world("scaling-liveness-held");
    let run = "livenessheld";
    a_held_member(&world, "build", false);
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the member to be heartbeating", |world| {
        beat_of(world, run, "build").is_some()
    });

    hold_the_pass(&world, &seam);
    let held_at = beat_of(&world, run, "build").expect("an entry");
    world.until("the entry to advance past the hold", |world| {
        beat_of(world, run, "build").is_some_and(|at| at > held_at + 500)
    });
    assert!(!seam.join("before-claim.release").exists());

    release_the_pass(&seam);
    world.release("build.go");
}

/// `status --json` with the quiet threshold at a second.
fn liveness_word(world: &World, run: &str) -> Value {
    let mut status = world.cmd(&["status", run, "--json"]);
    status.env(onepipeline::views::PARKED_AFTER_ENV, "1");
    world.run_on(status, "status --json").json()["liveness"].clone()
}

/// The journal's newest record, in epoch milliseconds.
fn last_journalled(world: &World, run: &str) -> u64 {
    world
        .journal(run)
        .iter()
        .filter_map(|event| event["ts"].as_str())
        .map(epoch_millis)
        .max()
        .unwrap_or(0)
}

/// A live driver whose only activity is its members' heartbeats through a quiet turn
/// is not `PARKED` however long its journal has been quiet; one whose member does not
/// heartbeat is, as before.
#[test]
fn a_heartbeating_member_keeps_a_quiet_run_off_parked_and_a_silent_one_does_not() {
    let world = World::new("scaling-parked");
    a_held_member(&world, "beating", false);
    world.script("silent.wait", "hold");
    for run in ["beatingrun", "silentrun"] {
        let node = if run == "beatingrun" {
            "beating"
        } else {
            "silent"
        };
        let path = world.plan(run, &plan_of(run, vec![agent(node, &[])]));
        world.run(&["start", &path, "--detach"]).exited(0);
        world.until("its dispatch to be in flight", |world| {
            !world.events_of(run, "node-dispatched").is_empty()
        });
    }
    world.until(
        "the beating run's journal to be quiet past the threshold",
        |world| {
            now_millis().saturating_sub(last_journalled(world, "beatingrun")) > 2_000
                && beat_of(world, "beatingrun", "beating")
                    .is_some_and(|at| now_millis().saturating_sub(at) < 1_000)
        },
    );
    assert_ne!(liveness_word(&world, "beatingrun"), json!("PARKED"));
    world.until("the silent run to read parked", |world| {
        liveness_word(world, "silentrun") == json!("PARKED")
    });

    world.release("beating.go");
    world.release("silent.go");
}

/// A run whose only activity is its observer's heartbeats reads `PARKED` once the
/// quiet threshold passes — an observer's member is no dispatch's, and its beats are
/// neither journalled nor kept — and its observer verdict reads as it did.
#[test]
fn an_observers_heartbeats_alone_do_not_keep_a_run_off_parked() {
    let world = World::new("scaling-observer-parked");
    world.script("observer.wait", "hold");
    world.script("observer.heartbeat", "100");
    // A dispatch in flight that says nothing, so the driver stays with the run.
    world.script("quiet.wait", "hold");
    let run = "observedquiet";
    let path = world.plan(run, &plan_of(run, vec![agent("quiet", &[])]));
    let graph = world.shipped_dag_graph();
    let mut attached = world.cmd(&["start", &path, "--attach", "--dag-graph", &graph]);
    attached
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut driving = attached.spawn().expect("the attached launch starts");
    world.until("the observer to be watching the run", |world| {
        world.observer_saw().len() == 1
    });
    world.until("the dispatch to be in flight", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    world.until("the run to read parked", |world| {
        liveness_word(world, run) == json!("PARKED")
    });
    assert_eq!(world.events_of(run, "member-heartbeat").len(), 0);
    assert!(
        liveness(&world, run).is_empty(),
        "{:?}",
        liveness(&world, run)
    );
    let status = world.run(&["status", run]);
    status.exited(0);
    assert!(
        !status.stdout.contains("OBSERVER DEAD") && !status.stdout.contains("NOT RESTARTED"),
        "the observer's verdict moved: {}",
        status.stdout
    );

    world.release("quiet.go");
    world.release("observer.go");
    let _ = driving.wait();
}

/// An armed watch is not woken by a member's heartbeat, and is by a settlement.
#[test]
fn a_heartbeat_does_not_wake_a_watch_and_a_settlement_does() {
    let world = World::new("scaling-watch-wake");
    let run = "watchwake";
    a_held_member(&world, "build", false);
    // Held past the watch, so the settlement it wakes on is a node's and not the
    // run completing.
    world.script("keep.wait", "hold");
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), agent("keep", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the member to be heartbeating", |world| {
        beat_of(world, run, "build").is_some()
    });

    let quiet = world.run(&[
        "watch",
        run,
        "--until",
        "node-settled",
        "--timeout",
        "3",
        "--tick-interval",
        "0",
    ]);
    let ended: Value = quiet
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["watch"] == "return")
        .expect("the watch returned");
    assert_eq!(ended["condition"], "elapsed", "{ended}");

    let mut waiting = world.cmd(&[
        "watch",
        run,
        "--until",
        "node-settled",
        "--timeout",
        "120",
        "--tick-interval",
        "0",
    ]);
    waiting.stdout(std::process::Stdio::piped());
    let watching = waiting.spawn().expect("the watch starts");
    world.until("the watch to be armed", |world| {
        world.run_file(run, "watchers").is_dir()
            && std::fs::read_dir(world.run_file(run, "watchers"))
                .is_ok_and(|mut armed| armed.next().is_some())
    });
    world.release("build.go");
    let output = watching.wait_with_output().expect("the watch ends");
    let ended: Value = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["watch"] == "return")
        .expect("the watch returned");
    assert_eq!(ended["condition"], "node-settled", "{ended}");
    assert_eq!(ended["node"], "build", "{ended}");
    world.release("keep.go");
}

/// Append one relayed record straight to a run's journal, as a driver's relay
/// would have: a sibling's own stream and stamp.
fn relayed(world: &World, run: &str, kind: &str, node: &str, seq: u64, ts: &str) {
    let record = json!({
        "v": 2,
        "ts": ts,
        "stream": "older-graph-1",
        "seq": seq,
        "source": "agentgraph",
        "kind": kind,
        "labels": {"run_id": "older-graph", "node": node, "member": "worker"},
        "payload": {"kind": "tool_use", "name": "Bash", "detail": format!("relayed late {seq}")},
        "artifacts": [],
    });
    let mut line = record.to_string();
    line.push('\n');
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(world.run_file(run, "events.jsonl"))
        .and_then(|mut journal| journal.write_all(line.as_bytes()))
        .expect("appended to the journal");
}

/// The records of `kind` the journal holds past byte `from`, read leniently: what
/// is in front of `from` is overwritten in the journeys that ask, and no reader
/// here may read it.
fn past(world: &World, run: &str, from: usize, kind: &str) -> Vec<Value> {
    let store = std::fs::read(world.run_file(run, "events.jsonl")).expect("the journal");
    String::from_utf8_lossy(&store[from..])
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["kind"] == kind)
        .collect()
}

/// The byte the run's checkpoint covers the journal to.
fn covered(world: &World, run: &str) -> usize {
    usize::try_from(
        world.run_json(run, "checkpoint.json")["coverage"]["bytes"]
            .as_u64()
            .expect("a coverage"),
    )
    .expect("a length")
}

/// Overwrite everything in front of the checkpoint's marker with bytes that are no
/// journal record, and answer what was there.
fn overwrite_what_is_covered(world: &World, run: &str) -> usize {
    let at = covered(world, run);
    let store = world.run_file(run, "events.jsonl");
    let mut held = std::fs::read(&store).expect("the run's journal");
    held[..at].fill(b'#');
    std::fs::write(&store, &held).expect("the covered history is overwritten");
    at
}

fn statuses_of(reading: &Value) -> Vec<(String, String)> {
    let mut nodes: Vec<(String, String)> = reading["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("no nodes in {reading}"))
        .iter()
        .map(|node| {
            (
                node["id"].as_str().unwrap_or_default().to_string(),
                node["status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    nodes.sort();
    nodes
}

/// Leave a driven run stopped, with `held` dispatched and not settled, and its
/// checkpoint written past the run's opening.
fn stopped_with(world: &World, run: &str, nodes: Vec<Value>, held: &str) {
    let path = world.plan(run, &plan_of(run, nodes));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the held node to be dispatched", |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == held)
    });
    world.run(&["stop", run, "--force"]).exited(0);
    world.until("the stop to be recorded", |world| {
        !world.events_of(run, "run-stopped").is_empty()
    });
    world.run(&["status", run, "--json"]).exited(0);
    assert!(covered(world, run) > 0, "no checkpoint covers the run");
}

/// **No pass, append, view or reply reaches what a checkpoint covers.** Every byte in
/// front of the marker is overwritten with bytes that are no record, and the tail
/// carries relayed worker records stamped earlier than covered pipeline records: the
/// adopting driver dispatches the ready node and journals its member's work, `reply`
/// is answered, `status --json` lists the nodes with their settlements, and an armed
/// `watch` ends on a settlement.
#[test]
fn a_run_whose_covered_history_is_overwritten_is_driven_viewed_and_answered() {
    let world = World::new("scaling-overwritten");
    let run = "overwritten";
    world.script("hold.wait", "hold");
    stopped_with(
        &world,
        run,
        vec![
            agent("first", &[]),
            agent("hold", &["first"]),
            agent("next", &["hold"]),
            // Behind the held one, so nothing settles it before the reply does.
            agent("spare", &["next"]),
        ],
        "hold",
    );
    world.release("hold.go");
    let at = overwrite_what_is_covered(&world, run);
    for seq in 0..3 {
        relayed(
            &world,
            run,
            "turn-activity",
            "first",
            seq,
            "2000-01-01T00:00:00.000Z",
        );
    }

    a_held_member(&world, "next", true);
    world.run(&["adopt", run, "--detach"]).exited(0);
    world.until("the ready node to be dispatched and working", |world| {
        past(world, run, at, "turn-activity")
            .iter()
            .any(|record| record["labels"]["node"] == "next")
    });

    let replied = world.run_with_stdin(
        &["reply", run],
        &json!({"version": 2, "commands": [
            {"op": "settle", "id": "spare", "outcome": "done", "evidence": "settled by hand"},
        ]})
        .to_string(),
    );
    replied.exited(0);
    assert_eq!(replied.json()["state"], json!("applied"));

    assert!(
        past(&world, run, at, "node-settled")
            .iter()
            .any(|record| record["labels"]["node"] == "spare"
                && record["payload"]["status"] == "done"),
        "the settle was answered and not applied"
    );
    let reading = world.run(&["status", run, "--json"]);
    reading.exited(0);
    assert_eq!(reading.json()["driven"], json!(true), "{}", reading.stdout);

    let mut waiting = world.cmd(&[
        "watch",
        run,
        "--until",
        "node-settled",
        "--timeout",
        "120",
        "--tick-interval",
        "0",
    ]);
    waiting.stdout(std::process::Stdio::piped());
    let watching = waiting.spawn().expect("the watch starts");
    world.until("the watch to be armed", |world| {
        std::fs::read_dir(world.run_file(run, "watchers"))
            .is_ok_and(|mut armed| armed.next().is_some())
    });
    world.release("next.go");
    let output = watching.wait_with_output().expect("the watch ends");
    let ended: Value = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["watch"] == "return")
        .expect("the watch returned");
    assert!(
        ["node-settled", "settled"].contains(&ended["condition"].as_str().unwrap_or_default()),
        "{ended}"
    );

    // The reading lists what the run has not settled, each with its settlement,
    // once the driver that adopted it is stopped.
    world.run(&["stop", run, "--force"]).exited(0);
    world.until("the adopted driver's stop to be recorded", |world| {
        !past(world, run, at, "run-stopped").is_empty()
    });
    let reading = world.run(&["status", run, "--json"]);
    reading.exited(0);
    let unsettled = statuses_of(&reading.json()["ending"]);
    assert!(
        !unsettled
            .iter()
            .any(|(node, _)| node == "first" || node == "spare"),
        "a node settled done is listed unsettled: {unsettled:?}"
    );

    let store = std::fs::read(world.run_file(run, "events.jsonl")).expect("the journal");
    assert!(
        store[..at].iter().all(|byte| *byte == b'#'),
        "the covered history was rewritten"
    );
}

/// Every read a run can be given — with its checkpoint, with none, and with one an
/// earlier build wrote — recovers the same nodes, settlements and dispatch evidence,
/// and a rebuilt checkpoint carries this build's schema.
#[test]
fn a_run_rebuilds_its_checkpoint_where_it_must_and_reads_the_same() {
    let world = World::new("scaling-rebuild");
    let run = "rebuilt";
    a_held_member(&world, "worked", true);
    world.script("broke.fail", "");
    world.script("dropped.turn-open", "");
    world.script("dropped.wait", "hold");
    world.script("dropped.stops-when-interrupted", "");
    world.script("last.wait", "hold");
    let path = world.plan(
        run,
        &plan_of(
            run,
            vec![
                agent("worked", &[]),
                agent("broke", &[]),
                agent("dropped", &[]),
                agent("last", &["worked"]),
            ],
        ),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the worker to have worked", |world| {
        activity_of(world, run, "worked") >= 3
    });
    world.release("worked.go");
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "cancel", "id": "dropped"}]}).to_string(),
        )
        .exited(0);
    world.until("every node but the last to settle", |world| {
        ["worked", "broke", "dropped"].iter().all(|node| {
            world
                .events_of(run, "node-settled")
                .iter()
                .any(|event| event["labels"]["node"] == *node)
        }) && world
            .events_of(run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "last")
    });
    world.run(&["stop", run, "--force"]).exited(0);
    world.until("the stop to be recorded", |world| {
        !world.events_of(run, "run-stopped").is_empty()
    });

    let settled: Vec<(String, String)> = world
        .events_of(run, "node-settled")
        .iter()
        .map(|event| {
            (
                event["labels"]["node"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                event["payload"]["status"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    for (node, status) in [
        ("worked", "done"),
        ("broke", "failed"),
        ("dropped", "cancelled"),
    ] {
        assert!(
            settled.contains(&(node.to_string(), status.to_string())),
            "{node} did not settle {status}: {settled:?}"
        );
    }
    let read = |world: &World| {
        let reading = world.run(&["status", run, "--json"]);
        reading.exited(0);
        let transcript = world.run(&["transcript", run, "worked"]);
        transcript.exited(0);
        (reading.json(), transcript.stdout.clone())
    };
    let (nodes, evidence) = read(&world);
    assert!(evidence.contains("tick 1"), "no tool call in:\n{evidence}");

    let checkpoint = world.run_file(run, "checkpoint.json");
    std::fs::remove_file(&checkpoint).expect("the checkpoint is deleted");
    assert_eq!(read(&world), (nodes.clone(), evidence.clone()));
    assert_eq!(
        world.run_json(run, "checkpoint.json")["schema_version"],
        json!(14)
    );

    let mut earlier = world.run_json(run, "checkpoint.json");
    earlier["schema_version"] = json!(13);
    earlier.as_object_mut().expect("a document").remove("tally");
    std::fs::write(&checkpoint, earlier.to_string()).expect("an earlier build's checkpoint");
    assert_eq!(read(&world), (nodes.clone(), evidence.clone()));
    assert_eq!(
        world.run_json(run, "checkpoint.json")["schema_version"],
        json!(14)
    );

    // And the driver rebuilds where it must, and drives what is left.
    std::fs::remove_file(&checkpoint).expect("the checkpoint is deleted again");
    world.run(&["adopt", run, "--detach"]).exited(0);
    world.release("last.go");
    world.until("the last node to settle", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "last")
    });
    assert_eq!(
        world.run_json(run, "checkpoint.json")["schema_version"],
        json!(14)
    );
    let (_, evidence_after) = read(&world);
    assert_eq!(evidence_after, evidence);
}

/// A run in the shape v0.75.0 leaves — its member heartbeats in the journal, no
/// `liveness.json`, a schema-13 checkpoint and a retry's `edit-committed` — is
/// adopted with its journal's existing bytes untouched, and `status`, `results`,
/// `monitor` and `transcript` report its nodes, settlements and a dispatch's tool
/// calls and output as they did before.
#[test]
fn a_run_in_the_shape_an_older_build_left_is_adopted_with_its_history_intact() {
    let world = World::new("scaling-older");
    let run = "older";
    world.script("flaky.fail", "");
    a_held_member(&world, "worked", true);
    world.script("last.wait", "hold");
    let path = world.plan(
        run,
        &plan_of(
            run,
            vec![
                agent("flaky", &[]),
                agent("worked", &[]),
                agent("last", &["worked"]),
            ],
        ),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("flaky to fail", |world| {
        world.events_of(run, "node-settled").iter().any(|event| {
            event["labels"]["node"] == "flaky" && event["payload"]["status"] == "failed"
        })
    });
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "retry", "id": "flaky", "node": {
                "id": "flaky-2", "persona": "engineer",
                "task": "## What\nDo flaky again.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- flaky-2 is done."
            }}]})
            .to_string(),
        )
        .exited(0);
    world.until("the worker to have worked", |world| {
        activity_of(world, run, "worked") >= 3
    });
    world.release("worked.go");
    world.until(
        "the retry and the worker to settle and the last node to start",
        |world| {
            let settled = world.events_of(run, "node-settled");
            ["flaky-2", "worked"].iter().all(|node| {
                settled.iter().any(|event| {
                    event["labels"]["node"] == *node && event["payload"]["status"] == "done"
                })
            }) && world
                .events_of(run, "node-dispatched")
                .iter()
                .any(|event| event["labels"]["node"] == "last")
        },
    );
    world.run(&["stop", run, "--force"]).exited(0);
    world.until("the stop to be recorded", |world| {
        !world.events_of(run, "run-stopped").is_empty()
    });
    // What an older driver journalled for every beat, and the checkpoint it wrote.
    let ts = now_millis();
    for seq in 0..5 {
        let mut beat = json!({
            "v": 2, "ts": format_ms(ts + seq), "stream": "older-graph-1", "seq": 1_000 + seq,
            "source": "agentgraph", "kind": "member-heartbeat",
            "labels": {"run_id": "older-graph", "node": "worked", "member": "worker"},
            "payload": {}, "artifacts": [],
        })
        .to_string();
        beat.push('\n');
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(world.run_file(run, "events.jsonl"))
            .and_then(|mut journal| journal.write_all(beat.as_bytes()))
            .expect("a beat is journalled");
    }
    world.run(&["status", run, "--json"]).exited(0);
    let mut older = world.run_json(run, "checkpoint.json");
    older["schema_version"] = json!(13);
    older.as_object_mut().expect("a document").remove("tally");
    std::fs::write(world.run_file(run, "checkpoint.json"), older.to_string())
        .expect("the older build's checkpoint");
    let _ = std::fs::remove_file(world.run_file(run, "liveness.json"));
    assert!(!world.events_of(run, "edit-committed").is_empty());

    let reads = |world: &World| -> Vec<String> {
        [
            vec!["status", run],
            vec!["results", run],
            vec!["monitor", run],
            vec!["transcript", run, "worked"],
        ]
        .iter()
        .map(|args| {
            let read = world.run(args);
            read.exited(0);
            read.stdout.clone()
        })
        .collect()
    };
    let before = reads(&world);
    let history = std::fs::read(world.run_file(run, "events.jsonl")).expect("the journal");

    world.run(&["adopt", run, "--detach"]).exited(0);
    world.release("last.go");
    world.until("the last node to settle", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "last")
    });
    let after_bytes = std::fs::read(world.run_file(run, "events.jsonl")).expect("the journal");
    assert_eq!(
        &after_bytes[..history.len()],
        &history[..],
        "the journal's existing bytes moved"
    );

    let after = reads(&world);
    // `status`: the same graph, and no record of it lost.
    let of_three = |status: &str| {
        status
            .lines()
            .next()
            .is_some_and(|line| line.contains("/3 done"))
    };
    assert!(
        of_three(&before[0]) && of_three(&after[0]),
        "{}\n---\n{}",
        before[0],
        after[0]
    );
    assert!(!after[0].contains("cannot read"), "{}", after[0]);
    // `results`: each node the older build settled reads as it did.
    for node in ["flaky", "flaky-2", "worked"] {
        let line = |text: &str| {
            text.lines()
                .find(|line| line.split_whitespace().next() == Some(node))
                .map(str::to_string)
        };
        assert!(
            line(&before[1]).is_some(),
            "{node} is missing from:\n{}",
            before[1]
        );
        assert_eq!(line(&after[1]), line(&before[1]), "{node}");
    }
    // `monitor`: every line it showed before is still shown.
    for line in before[2].lines().filter(|line| !line.starts_with("--")) {
        assert!(after[2].contains(line), "monitor lost:\n{line}");
    }
    assert_eq!(after[3], before[3], "the dispatch's transcript moved");
    assert!(before[3].contains("tick 1"), "{}", before[3]);
}

/// Milliseconds since the epoch as the journal stamps them.
fn format_ms(millis: u64) -> String {
    let seconds = millis / 1_000;
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60,
        millis % 1_000
    )
}

/// A run in the shape v0.75.0 leaves — member heartbeats in its journal and no
/// `liveness.json` — reads `alive … ago` and `PARKED` off those records as before.
#[test]
fn journalled_heartbeats_with_no_liveness_file_read_as_they_did() {
    let world = World::new("scaling-journalled-beats");
    let run = "journalledbeats";
    world.script("quiet.turn-open", "");
    world.script("quiet.wait", "hold");
    let path = world.plan(run, &plan_of(run, vec![agent("quiet", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the quiet member's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
    });
    world.until("the journal to be quiet past the threshold", |world| {
        now_millis().saturating_sub(last_journalled(world, run)) > 2_000
    });
    assert_eq!(liveness_word(&world, run), json!("PARKED"));
    assert!(liveness(&world, run).is_empty());

    let beat = json!({
        "v": 2, "ts": format_ms(now_millis()), "stream": "older-graph-1", "seq": 5_000,
        "source": "agentgraph", "kind": "member-heartbeat",
        "labels": {"run_id": "older-graph", "node": "quiet", "member": "worker"},
        "payload": {}, "artifacts": [],
    });
    let mut line = beat.to_string();
    line.push('\n');
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(world.run_file(run, "events.jsonl"))
        .and_then(|mut journal| journal.write_all(line.as_bytes()))
        .expect("a beat is journalled");
    assert_ne!(liveness_word(&world, run), json!("PARKED"));
    let status = world.run(&["status", run]);
    status.exited(0);
    let quiet = status
        .stdout
        .lines()
        .find(|line| line.trim_start().starts_with("quiet: running"))
        .unwrap_or_else(|| panic!("no line for quiet:\n{}", status.stdout));
    assert!(quiet.contains("alive "), "{quiet}");
    world.until("the journalled beat to age past the threshold", |world| {
        liveness_word(world, run) == json!("PARKED")
    });
    world.release("quiet.go");
}

/// A watch given no cursor, and an attach's first render, start where the run's
/// checkpoint stops — the history in front of it is overwritten with bytes that are
/// no record, and neither reports them — and the watch still returns at once on a
/// planner surface left unread before it armed.
#[test]
fn a_watch_and_an_attach_start_where_the_checkpoint_stops() {
    let world = World::new("scaling-start");
    let run = "startpoint";
    world.script("hold.wait", "hold");
    stopped_with(
        &world,
        run,
        vec![
            agent("first", &[]),
            agent("hold", &["first"]),
            agent("after", &["hold"]),
        ],
        "hold",
    );
    world.release("hold.go");
    world
        .run(&[
            "surface",
            run,
            "--kind",
            "check-in",
            "--message",
            "still there?",
        ])
        .exited(0);
    world.run(&["status", run, "--json"]).exited(0);
    let at = overwrite_what_is_covered(&world, run);
    assert!(at > 0);

    let watched = world.run(&["watch", run, "--timeout", "30", "--tick-interval", "0"]);
    let ended: Value = watched
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["watch"] == "return")
        .unwrap_or_else(|| panic!("no return in {}", watched.stdout));
    assert_eq!(ended["condition"], "surface-waiting", "{ended}");
    assert!(
        ended["unread"]["count"]
            .as_u64()
            .is_some_and(|count| count >= 1),
        "{ended}"
    );
    let printed: Vec<Value> = watched
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["watch"] == "event")
        .collect();
    assert!(
        printed
            .iter()
            .all(|record| record["event"]["labels"]["node"] != "first"),
        "the watch printed history its checkpoint covers: {printed:?}"
    );

    let attached = world.run(&["adopt", run, "--attach"]);
    attached.exited(0);
    assert!(
        !attached.stderr.contains("graph:first"),
        "the attach rendered history its checkpoint covers:\n{}",
        attached.stderr
    );
    assert!(
        attached.stderr.contains("graph:after"),
        "{}",
        attached.stderr
    );
}
