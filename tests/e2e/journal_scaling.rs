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
