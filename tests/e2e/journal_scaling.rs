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

use crate::harness::{
    agent, counts, epoch_millis, now_millis, plan_of, reporting, World, LOOP_STATS_ENV,
};
use serde_json::{json, Value};

// llmlint: ignore-block[tests_mirror_real_usage] the criteria for this node require a journey that holds the reconcile pass by a failpoint the way the repository's existing journeys hold one; no verb holds a pass, and the hold is the debug build's own seam.
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
// llmlint: ignore-end[tests_mirror_real_usage]

/// The variable asking every process the binary starts to report where it read a
/// run's journal, one line per read; see `loopstats::JOURNAL_READS_ENV`.
const JOURNAL_READS_ENV: &str = "ONEPIPELINE_JOURNAL_READS";

/// A world whose every `onepipeline` process reports its journal reads into one
/// file of the world's own.
fn reporting_reads(name: &str) -> World {
    let world = World::new(name);
    let file = world.root.join("journal-reads.jsonl");
    world.with_env(JOURNAL_READS_ENV, &file.to_string_lossy())
}

/// One read of a run's journal, as the process that made it reported it.
#[derive(Debug, Clone, Copy)]
struct JournalRead {
    pid: u64,
    offset: u64,
    bytes: u64,
}

/// Every read of `run`'s journal reported since the report was last cleared.
fn journal_reads(world: &World, run: &str) -> Vec<JournalRead> {
    let journal = format!("{}{}events.jsonl", run, std::path::MAIN_SEPARATOR);
    std::fs::read_to_string(world.root.join("journal-reads.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a whole report line"))
        .filter(|read| {
            read["journal"]
                .as_str()
                .is_some_and(|path| path.ends_with(&journal))
        })
        .map(|read| JournalRead {
            pid: read["pid"].as_u64().expect("a pid"),
            offset: read["offset"].as_u64().expect("an offset"),
            bytes: read["bytes"].as_u64().expect("a length"),
        })
        .collect()
}

/// Start the read report afresh: what was read before this is not what a journey
/// asks about.
fn clear_journal_reads(world: &World) {
    let _ = std::fs::remove_file(world.root.join("journal-reads.jsonl"));
}

/// The command `args` runs, without reporting its reads: for the verbs that keep a
/// whole-journal read of their own (`transcript`, `results`, `monitor`).
fn unreported(world: &World, args: &[&str]) -> crate::harness::Run {
    let mut command = world.cmd(args);
    command.env_remove(JOURNAL_READS_ENV);
    world.run_on(command, &args.join(" "))
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

/// A `settle` of a node no dispatch is part of, queued while
/// the driver's pass is held, is claimed, applied and answered before the pass is
/// released — and the `reply` that queued it is answered. Released, the pass puts
/// the settlement on the run's board.
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
    world.until("the member's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
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
    world.until_store("the settlement to reach the board", |world| {
        on_the_board(world, &path, "later")
            .is_some_and(|item| item["onepipeline.settlement"]["status"] == "done")
    });
    world.release("build.go");
}

/// Hold one failpoint the driver reaches on each pass while armed, until it is reached.
fn hold_at(world: &World, seam: &Path, stage: &str, what: &str) {
    std::fs::write(seam.join(stage), "").expect("the seam is armed");
    world.until(what, |_| seam.join(format!("{stage}.reached")).is_file());
}

/// Let whatever is held at `stage` go on, and stop holding there.
fn release_at(seam: &Path, stage: &str) {
    let _ = std::fs::remove_file(seam.join(stage));
    std::fs::write(seam.join(format!("{stage}.release")), "").expect("the stage is released");
}

/// A `settle` the claimer is still applying when the loop's next pass begins is
/// folded by that pass all the same, and the node it readied starts without
/// anything else about the run moving.
///
/// The ordering is the one a slow host puts a reply in: the claimer takes the
/// envelope while a pass runs past its own claim, the pass ends, and the next one
/// begins before the claimer has finished applying it. A pass that took the
/// claimer's word for the apply at that moment folded a journal without it — and,
/// the queue having moved no further, never folded it again: the node the settle
/// readied stayed unstarted for as long as nothing else moved the run.
#[test]
fn a_settle_the_claimer_is_still_applying_as_a_pass_begins_is_folded_by_that_pass() {
    let (world, seam) = held_world("scaling-claim-overlap");
    let run = "claimoverlap";
    // Beating, so the loop passes while the member is held and has a pass to hold.
    a_held_member(&world, "build", false);
    let path = world.plan(
        run,
        &plan_of(
            run,
            vec![
                agent("build", &[]),
                agent("later", &["build"]),
                agent("after", &["later"]),
            ],
        ),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the held node to be dispatched", |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "build")
    });

    // The pass held past its own claim, so the claimer takes what is queued now —
    // and is held in turn, having claimed the settle and not yet applied it.
    std::fs::write(seam.join("claimer-applying"), "").expect("the seam is armed");
    hold_at(
        &world,
        &seam,
        "after-claim",
        "a pass to be held past its claim",
    );
    let settle = json!({"version": 2, "commands": [
        {"op": "settle", "id": "later", "outcome": "done",
         "evidence": "settled by hand while the claimer is slow to apply it"},
    ]})
    .to_string();
    std::thread::scope(|scope| {
        let replying = scope.spawn(|| world.run_with_stdin(&["reply", run], &settle));
        world.until("the claimer to be holding the settle", |_| {
            seam.join("claimer-applying.reached").is_file()
        });

        // The held pass ends and the next one begins, holding at its own claim: it
        // began while the claimer was still applying.
        std::fs::write(seam.join("before-claim"), "").expect("the seam is armed");
        release_at(&seam, "after-claim");
        world.until("the next pass to begin while the settle is applied", |_| {
            seam.join("before-claim.reached").is_file()
        });
        release_at(&seam, "claimer-applying");
        let replied = replying.join().expect("the reply thread");
        replied.exited(0);
        let receipt: Value = replied.json();
        assert_eq!(receipt["state"], json!("applied"), "{receipt}");
    });
    release_at(&seam, "before-claim");

    // Nothing else moves the run: `build` is still held, so the only thing that can
    // start `after` is the pass folding the settle the claimer applied.
    world.until("the node the settle readied to be dispatched", |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "after")
    });
    assert!(
        world
            .events_of(run, "node-settled")
            .iter()
            .all(|event| event["labels"]["node"] != "build"),
        "the held node settled, so something other than the settle moved the run"
    );
    world.release("build.go");
}

/// A pass claims the run's command queue only once the queue has moved: a claim
/// takes the queue's lock and reads its log from the cursor's offset, so a pass
/// claiming an unmoved queue would pay a locked read that finds nothing. Twenty
/// passes with nothing queued claim nothing; a reply is claimed and answered; and
/// twenty passes after it claim nothing again.
#[test]
fn a_pass_claims_the_command_queue_only_once_it_has_moved() {
    let world = World::new("scaling-queue-claims").with_env(LOOP_STATS_ENV, "1");
    let run = "unmoved";
    a_held_member(&world, "build", true);
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), agent("later", &["build"])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    reporting(&world, run);
    world.until("the member's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
    });
    // Twenty passes, the member's activity driving each, from a point past the
    // driver's opening claim.
    let twenty_passes = |world: &World| {
        let from = counts(world, run);
        world.until("twenty more passes", |world| {
            counts(world, run).passes >= from.passes + 20
        });
        counts(world, run).since(from)
    };
    twenty_passes(&world);
    let unmoved = twenty_passes(&world);
    assert_eq!(
        unmoved.queue_claims, 0,
        "passes over a queue nothing was added to claimed it: {unmoved:?}"
    );

    let before = counts(&world, run);
    let replied = world.run_with_stdin(
        &["reply", run],
        &json!({"version": 2, "commands": [
            {"op": "settle", "id": "later", "outcome": "done",
             "evidence": "settled by hand while its dependency runs"},
        ]})
        .to_string(),
    );
    replied.exited(0);
    let receipt: Value = replied.json();
    assert_eq!(receipt["state"], json!("applied"), "{receipt}");
    twenty_passes(&world);
    let moved = counts(&world, run).since(before);
    assert!(
        moved.queue_claims >= 1,
        "the reply was answered with no claim of the queue: {moved:?}"
    );
    let settled = twenty_passes(&world);
    assert_eq!(
        settled.queue_claims, 0,
        "passes after the reply was claimed claimed the queue again: {settled:?}"
    );
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

// llmlint: ignore-block[tests_mirror_real_usage] the criteria require synthesized run directories — a run in the shape v0.75.0 left, journalled heartbeats with no liveness file, relayed records stamped before covered history, and covered history overwritten with non-records — which no verb of this build writes, because this build no longer journals heartbeats or writes a schema-13 checkpoint.
/// Append one relayed record straight to a run's journal, as a driver's relay
/// would have: a sibling's own stream and stamp.
fn relayed(world: &World, run: &str, kind: &str, node: &str, seq: u64, ts: &str) {
    relayed_from(world, run, "older-graph", kind, node, seq, ts);
}

/// [`relayed`], from the graph run `graph`: its stream and its member's labels.
fn relayed_from(world: &World, run: &str, graph: &str, kind: &str, node: &str, seq: u64, ts: &str) {
    let record = json!({
        "v": 2,
        "ts": ts,
        "stream": format!("{graph}-1"),
        "seq": seq,
        "source": "agentgraph",
        "kind": kind,
        "labels": {"run_id": graph, "node": node, "member": "worker"},
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

/// Append one `member-heartbeat` to a run's journal, as a driver before this one
/// relayed every beat.
fn journal_a_beat(world: &World, run: &str, node: &str, seq: u64, ts: &str) {
    let mut beat = json!({
        "v": 2, "ts": ts, "stream": "older-graph-1", "seq": seq,
        "source": "agentgraph", "kind": "member-heartbeat",
        "labels": {"run_id": "older-graph", "node": node, "member": "worker"},
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

/// Rewrite a run's checkpoint as v0.75.0 wrote it: schema 13, no tally.
fn as_schema_13(world: &World, run: &str) {
    let mut older = world.run_json(run, "checkpoint.json");
    older["schema_version"] = json!(13);
    older.as_object_mut().expect("a document").remove("tally");
    std::fs::write(world.run_file(run, "checkpoint.json"), older.to_string())
        .expect("the older build's checkpoint");
}
// llmlint: ignore-end[tests_mirror_real_usage]

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
/// checkpoint written past the run's opening; answer the project its board is.
fn stopped_with(world: &World, run: &str, nodes: Vec<Value>, held: &str) -> String {
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
    path
}

/// The metadata the run's board holds for one node, where it holds an item for it.
fn on_the_board(world: &World, project: &str, node: &str) -> Option<Value> {
    world
        .store_tasks(project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .map(|task| task["item"]["metadata"].clone())
}

/// Every task a dispatch of `node` handed its graph, as the graph double recorded it.
fn tasks_handed(world: &World, node: &str) -> Vec<String> {
    let mine = format!("## What\nDo {node}.");
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .filter_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|arg| arg == "--task")?;
            args.get(at + 1)?.as_str().map(str::to_owned)
        })
        .filter(|task| task.starts_with(&mine))
        .collect()
}

/// **No pass, append, view or reply reaches what a checkpoint covers.** Every byte in
/// front of the marker is overwritten with bytes that are no record, and the tail
/// carries relayed worker records stamped earlier than covered pipeline records: the
/// adopting driver dispatches the ready node and journals its member's work, `reply`
/// is answered — a settle, and a note carried to a node's next dispatch, which that
/// dispatch is composed with — text `status` names the running node and no loss in
/// the store, `status --json` lists the nodes with their settlements, the board keeps
/// the settlement only the covered history recorded beside the one the reply made,
/// and an armed `watch` ends on a settlement.
#[test]
fn a_run_whose_covered_history_is_overwritten_is_driven_viewed_and_answered() {
    let world = reporting_reads("scaling-overwritten");
    let run = "overwritten";
    world.script("hold.wait", "hold");
    let project = stopped_with(
        &world,
        run,
        vec![
            agent("first", &[]),
            agent("hold", &["first"]),
            agent("next", &["hold"]),
            // Behind the held one, so nothing settles it before the reply does.
            agent("spare", &["next"]),
            // Behind it too, so the note the reply carries waits for its dispatch.
            agent("told", &["next"]),
        ],
        "hold",
    );
    world.release("hold.go");
    let at = overwrite_what_is_covered(&world, run);
    clear_journal_reads(&world);
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

    // Held quiet, beating and doing nothing more, so every event its line counts
    // is one the journey can name.
    a_held_member(&world, "next", false);
    // Held too, so the run cannot end before the journey takes the driver's reads:
    // the scratch handling a driver does as it lets go of an ended run reads the
    // whole journal once, and that is the run's ending, not a pass, append, view or
    // reply — the stop below ends it, after those reads are taken.
    world.script("told.wait", "hold");
    world.run(&["adopt", run, "--detach"]).exited(0);
    world.until(
        "the ready node to be dispatched and its turn open",
        |world| {
            past(world, run, at, "turn-started")
                .iter()
                .any(|record| record["labels"]["node"] == "next")
        },
    );
    let graph = past(&world, run, at, "turn-started")
        .into_iter()
        .find(|record| record["labels"]["node"] == "next")
        .and_then(|record| record["labels"]["run_id"].as_str().map(str::to_owned))
        .expect("the running node's graph run");

    // Worker events of the running node arrive in the tail stamped before every
    // record the checkpoint covers, and are folded for it all the same: its line
    // counts exactly them more, and names what the last of them was doing.
    let events_of_next = |world: &World| -> u64 {
        let text = world.run(&["status", run]);
        text.exited(0);
        text.stdout
            .lines()
            .find(|line| line.starts_with("  next: running for"))
            .and_then(|line| line.split(" event(s)").next())
            .and_then(|head| head.rsplit(['(', ' ']).next())
            .and_then(|count| count.parse().ok())
            .unwrap_or_else(|| panic!("no event count for the running node in:\n{}", text.stdout))
    };
    let before = events_of_next(&world);
    for seq in 0..3 {
        relayed_from(
            &world,
            run,
            &graph,
            "turn-activity",
            "next",
            seq,
            "2000-01-01T00:00:00.000Z",
        );
    }
    assert_eq!(
        events_of_next(&world),
        before + 3,
        "the earlier-stamped events were not folded for the running node"
    );
    let text = world.run(&["status", run]);
    assert!(
        text.stdout.contains("now Bash relayed late 2"),
        "the running node's line does not name the last of them:\n{}",
        text.stdout
    );

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
    let noted = world.run_with_stdin(
        &["reply", run],
        &json!({"version": 2, "commands": [
            {"op": "note", "id": "told", "addressee": "worker",
             "text": "the fixture moved", "deliver": "next"},
        ]})
        .to_string(),
    );
    noted.exited(0);
    let reading = world.run(&["status", run, "--json"]);
    reading.exited(0);
    assert_eq!(reading.json()["driven"], json!(true), "{}", reading.stdout);

    // Every line of the text is read off the checkpoint and what was appended past
    // it: a read of the whole store would meet the overwritten bytes, and say the
    // run's record of itself is incomplete.
    let text = world.run(&["status", run]);
    text.exited(0);
    assert!(
        text.stdout.contains("  next: running for"),
        "{}",
        text.stdout
    );
    assert!(
        !text.stdout.contains("record of itself is incomplete"),
        "{}",
        text.stdout
    );

    // The board keeps the settlement only the covered history holds beside the one
    // the reply made: a projection folded from the store's bytes would have lost it.
    world.until_store("the board to hold both settlements", |world| {
        ["first", "spare"].iter().all(|node| {
            on_the_board(world, &project, node)
                .is_some_and(|item| item["onepipeline.settlement"]["status"] == "done")
        })
    });

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
    // The carried note rides in as the task the noted node's dispatch hands its
    // graph, composed at its dispatch, after the note was carried.
    world.until("the noted node's graph to be handed its task", |world| {
        !tasks_handed(world, "told").is_empty()
    });
    let told = tasks_handed(&world, "told");
    assert!(
        told.iter().any(|task| task.contains("the fixture moved")),
        "the dispatch was not composed with the note carried to it: {told:?}"
    );

    // The reading lists what the run has not settled, each with its settlement,
    // once the driver that adopted it is stopped.
    // Unreported: what `stop` reads for a run's scratch and its run-end hook is the
    // run's ending, once, and not a pass, an append, a view or a reply — nor is what
    // the stopped driver reads as it ends, so its reads are those it made before.
    let driven = journal_reads(&world, run);
    unreported(&world, &["stop", run, "--force"]).exited(0);
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

    // And the running node's transcript shows their tool calls. Unreported, as the
    // whole read of its own it keeps is.
    let transcript = unreported(&world, &["transcript", run, "next"]);
    transcript.exited(0);
    for seq in 0..3 {
        assert!(
            transcript.stdout.contains(&format!("relayed late {seq}")),
            "the transcript does not show the earlier-stamped tool call {seq}:\n{}",
            transcript.stdout
        );
    }

    // Every read the binary reported making of the journal since the overwrite —
    // the adopting driver's passes, appends and attached render, every `reply`,
    // `status` and the `watch` — began at or past the byte the checkpoint covers.
    let before: std::collections::BTreeSet<u64> = driven.iter().map(|read| read.pid).collect();
    let mut reads = driven;
    reads.extend(
        journal_reads(&world, run)
            .into_iter()
            .filter(|read| !before.contains(&read.pid)),
    );
    let readers: std::collections::BTreeSet<u64> = reads.iter().map(|read| read.pid).collect();
    assert!(
        readers.len() >= 5,
        "fewer processes reported reading the journal than the driver and the verbs: {reads:?}"
    );
    let reached: Vec<&JournalRead> = reads
        .iter()
        .filter(|read| read.offset < at as u64)
        .collect();
    assert!(
        reached.is_empty(),
        "a read reached the {at} covered bytes: {reached:?}"
    );

    let store = std::fs::read(world.run_file(run, "events.jsonl")).expect("the journal");
    assert!(
        store[..at].iter().all(|byte| *byte == b'#'),
        "the covered history was rewritten"
    );
}

/// How many bytes of a journal's first `length` the reads took, counted with every
/// overlap: a rebuild that streams the journal once takes `length`, and one that read
/// it again, or went back over it, takes more.
fn read_within(reads: &[JournalRead], length: u64) -> u64 {
    reads
        .iter()
        .filter(|read| read.offset < length)
        .map(|read| (read.offset + read.bytes).min(length) - read.offset)
        .sum()
}

/// The run's journal length now.
fn journal_length(world: &World, run: &str) -> u64 {
    std::fs::metadata(world.run_file(run, "events.jsonl"))
        .expect("the journal")
        .len()
}

/// Every read a run can be given — with its checkpoint, with none, and with one an
/// earlier build wrote — recovers the same nodes, settlements and dispatch evidence,
/// and a rebuilt checkpoint carries this build's schema. Only the two that must
/// rebuild read what the checkpoint covers, and each, the driver's included, reads
/// the journal it rebuilds from once, as the binary reports its reads.
#[test]
fn a_run_rebuilds_its_checkpoint_where_it_must_and_reads_the_same() {
    let world = reporting_reads("scaling-rebuild");
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
    // `status --json` reported, and `transcript` — which keeps a whole read of its own
    // — not: what a rebuild reads is the first one's.
    let read = |world: &World| {
        let reading = world.run(&["status", run, "--json"]);
        reading.exited(0);
        let transcript = unreported(world, &["transcript", run, "worked"]);
        transcript.exited(0);
        (reading.json(), transcript.stdout.clone())
    };
    // Read and reported once rebuilt, the reads that rebuild in front of it.
    let rebuilt_once = |world: &World, case: &str, length: u64| {
        let reads = journal_reads(world, run);
        assert!(
            !reads.is_empty(),
            "{case}: no read of the journal was reported"
        );
        let took = read_within(&reads, length);
        assert!(
            took <= length,
            "{case}: the rebuild read {took} bytes of a {length}-byte journal: {reads:?}"
        );
    };
    clear_journal_reads(&world);
    let at = world.run_json(run, "checkpoint.json")["coverage"]["bytes"]
        .as_u64()
        .expect("a coverage");
    let (nodes, evidence) = read(&world);
    assert!(evidence.contains("tick 1"), "no tool call in:\n{evidence}");
    let behind: Vec<JournalRead> = journal_reads(&world, run)
        .into_iter()
        .filter(|read| read.offset < at)
        .collect();
    assert!(
        behind.is_empty(),
        "a read with a valid checkpoint went back over the {at} bytes it covers: {behind:?}"
    );

    // llmlint: ignore-block[tests_mirror_real_usage] the criterion's second case is a run with its checkpoint deleted, which the checkpoint is documented as safe to be; no verb deletes it.
    let checkpoint = world.run_file(run, "checkpoint.json");
    std::fs::remove_file(&checkpoint).expect("the checkpoint is deleted");
    // llmlint: ignore-end[tests_mirror_real_usage]
    clear_journal_reads(&world);
    let length = journal_length(&world, run);
    assert_eq!(read(&world), (nodes.clone(), evidence.clone()));
    assert_eq!(
        world.run_json(run, "checkpoint.json")["schema_version"],
        json!(15)
    );
    rebuilt_once(&world, "a deleted checkpoint", length);

    as_schema_13(&world, run);
    clear_journal_reads(&world);
    let length = journal_length(&world, run);
    assert_eq!(read(&world), (nodes.clone(), evidence.clone()));
    assert_eq!(
        world.run_json(run, "checkpoint.json")["schema_version"],
        json!(15)
    );
    rebuilt_once(&world, "a schema-13 checkpoint", length);

    // And the driver rebuilds where it must, and drives what is left.
    // llmlint: ignore[tests_mirror_real_usage] the criterion's second case is a run with its checkpoint deleted, which the checkpoint is documented as safe to be; no verb deletes it.
    std::fs::remove_file(&checkpoint).expect("the checkpoint is deleted again");
    clear_journal_reads(&world);
    let length = journal_length(&world, run);
    let dispatches_of_last = |world: &World| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .filter(|event| event["labels"]["node"] == "last")
            .count()
    };
    let dispatched_before = dispatches_of_last(&world);
    world.run(&["adopt", run, "--detach"]).exited(0);
    world.until("the driver to dispatch the last node again", |world| {
        dispatches_of_last(world) > dispatched_before
    });
    assert_eq!(
        world.run_json(run, "checkpoint.json")["schema_version"],
        json!(15)
    );
    // Every process the adoption started — the verb and the driver, its passes and its
    // attached render — together read the history the rebuild folded once: one read
    // from the journal's first byte, and every other read resuming past it. What they
    // each take again is the trailing instant the rebuilt checkpoint holds back, which
    // the next arrival can still sort in front of, and not history. Read while the
    // last node is still held, so the run cannot have ended: the scratch handling a
    // driver does as it lets go of an ended run reads the whole journal once, and
    // whether that lands before this read or after it is a race, not the rebuild.
    let reads = journal_reads(&world, run);
    world.release("last.go");
    world.until("the last node to settle", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "last")
    });
    let from_the_start: Vec<&JournalRead> = reads.iter().filter(|read| read.offset == 0).collect();
    assert_eq!(
        from_the_start.len(),
        1,
        "an adoption with no checkpoint read its journal from the start other than once: {reads:?}"
    );
    let resumed = reads
        .iter()
        .filter(|read| read.offset > 0)
        .map(|read| read.offset)
        .min()
        .expect("the driver read what was appended past the rebuild");
    assert!(
        resumed <= length && from_the_start[0].bytes <= length,
        "the rebuild read past the {length}-byte journal it found: {reads:?}"
    );
    assert_eq!(
        read_within(&reads, resumed),
        resumed,
        "an adoption with no checkpoint read the history it rebuilt from more than once: {reads:?}"
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
        journal_a_beat(&world, run, "worked", 1_000 + seq, &format_ms(ts + seq));
    }
    world.run(&["status", run, "--json"]).exited(0);
    as_schema_13(&world, run);
    // llmlint: ignore[tests_mirror_real_usage] the shape v0.75.0 left has no liveness file, which no verb of this build removes; the criterion is that run's adoption.
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

    // A liveness file this build cannot read says nothing, and the journal's
    // beats are read as before.
    // llmlint: ignore[tests_mirror_real_usage] an unreadable liveness file is one a later build would leave, which no verb of this one writes; the criterion is that a reader falls back to the journal.
    std::fs::write(
        world.run_file(run, "liveness.json"),
        json!({"schema_version": 2, "members": []}).to_string(),
    )
    .expect("an unreadable liveness file");
    journal_a_beat(&world, run, "quiet", 5_000, &format_ms(now_millis()));
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

    // Where both say something, the later wins: a liveness file older than the
    // journal's beat leaves the run parked, and one newer takes it off.
    let kept = |at: u64| {
        // llmlint: ignore[tests_mirror_real_usage] the file a driver writes is planted beside a journal an older driver wrote, the combination an adoption of that run meets; no verb writes both.
        std::fs::write(
            world.run_file(run, "liveness.json"),
            json!({"schema_version": 1, "members": [{"node": "quiet", "graph_run": "older-graph",
                   "member": "worker", "last_heartbeat_at": format_ms(at)}]})
            .to_string(),
        )
        .expect("a liveness file");
    };
    kept(now_millis() - 60_000);
    assert_eq!(liveness_word(&world, run), json!("PARKED"));
    kept(now_millis());
    assert_ne!(liveness_word(&world, run), json!("PARKED"));
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
    // It names the surface it returned on: the one left unread before it armed.
    assert!(
        ended["unread"]["kinds"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind["kind"] == "check-in")),
        "the return does not name the waiting surface: {ended}"
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

/// An envelope naming a dispatch in flight is not the claimer's: it is handed to
/// the loop, and so is everything claimed after it, which the loop answers in the
/// order they were submitted once its pass goes on. One the claimer may answer and
/// whose processing fails is answered refused, the failure its reason, while the
/// pass is still held.
#[test]
fn what_the_claimer_may_not_answer_waits_for_the_loop_in_order_and_a_failure_is_answered_refused() {
    let (world, seam) = held_world("scaling-claim-order");
    let run = "claimorder";
    a_held_member(&world, "build", false);
    let path = world.plan(
        run,
        &plan_of(
            run,
            vec![
                agent("build", &[]),
                agent("later", &["build"]),
                agent("other", &["build"]),
            ],
        ),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the member's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
    });
    hold_the_pass(&world, &seam);

    let queue = world.run_file(run, "channel/commands.jsonl");
    let next = std::fs::read_to_string(&queue)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count() as u64;
    // The failing seam names the envelope the claimer answers.
    // llmlint: ignore[tests_mirror_real_usage] the failing seam is the debug build's own, as `tests/note`'s journeys arm it; no verb makes an envelope's processing fail.
    std::fs::write(seam.join(format!("fail-envelope-{next}")), "").expect("the seam is armed");
    let failed = world.run_with_stdin(
        &["reply", run],
        &json!({"version": 2, "commands": [
            {"op": "settle", "id": "other", "outcome": "done", "evidence": "by hand"},
        ]})
        .to_string(),
    );
    assert!(
        failed.stderr.contains("failed while processing") || failed.stdout.contains("refused"),
        "{}\n{}",
        failed.stdout,
        failed.stderr
    );

    let mut amend = world.cmd(&["reply", run]);
    amend
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let mut amending = amend.spawn().expect("the amend is submitted");
    use std::io::Write;
    amending
        .stdin
        .take()
        .expect("a stdin")
        .write_all(
            json!({"version": 2, "commands": [{"op": "amend", "id": "build", "text": "one more thing"}]})
                .to_string()
                .as_bytes(),
        )
        .expect("written");
    world.until("the amend to be queued", |_| {
        std::fs::read_to_string(&queue)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count() as u64
            >= next + 2
    });
    let mut settle = world.cmd(&["reply", run]);
    settle
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let mut settling = settle.spawn().expect("the settle is submitted");
    settling
        .stdin
        .take()
        .expect("a stdin")
        .write_all(
            json!({"version": 2, "commands": [
                {"op": "settle", "id": "later", "outcome": "done", "evidence": "by hand"},
            ]})
            .to_string()
            .as_bytes(),
        )
        .expect("written");
    world.until("the settle to be queued and claimed", |_| {
        std::fs::read_to_string(&queue)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count() as u64
            >= next + 3
    });
    let outcomes = || -> Vec<Value> {
        std::fs::read_to_string(world.run_file(run, "channel/command-outcomes.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    };
    let answered = |id: u64| outcomes().into_iter().find(|outcome| outcome["id"] == id);
    let refused =
        answered(next).expect("the failing envelope was answered while the pass was held");
    assert_eq!(refused["applied"], json!(false), "{refused}");
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(
        answered(next + 1).is_none() && answered(next + 2).is_none(),
        "an envelope behind one naming a running dispatch was answered before the loop: {:?}",
        outcomes()
    );

    release_the_pass(&seam);
    world.until("both to be answered", |_| {
        answered(next + 1).is_some() && answered(next + 2).is_some()
    });
    let order: Vec<u64> = outcomes()
        .iter()
        .filter_map(|outcome| outcome["id"].as_u64())
        .filter(|id| *id > next)
        .collect();
    assert_eq!(order, vec![next + 1, next + 2], "answered out of order");
    let _ = amending.wait();
    let _ = settling.wait();
    world.release("build.go");
}

/// **What the claimer applied beside a pass is folded before the loop judges an
/// envelope in that same pass.** The loop holds one settle of `later`, a second
/// settle at another landing is applied beside the held pass, and a third, back at
/// the first landing, is queued beside a `finding` — which the claimer leaves to
/// the loop. Released, the loop answers it applied, a correction of the landing the
/// second stated: judged against a fold without the second, it was refused as a
/// settle of what the record already said.
#[test]
fn an_envelope_the_loop_judges_after_the_claimer_applied_one_is_judged_against_it() {
    let (world, seam) = held_world("scaling-claim-folded");
    let world = world.with_env(crate::harness::LOOP_STATS_ENV, "1");
    let run = "claimfolded";
    a_held_member(&world, "build", false);
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), agent("later", &["build"])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the member's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
    });
    let settle = |landing: &str| {
        json!({"op": "settle", "id": "later", "outcome": "done",
               "evidence": "the work landed by hand", "landing": landing})
    };
    let first = "1111111111111111111111111111111111111111";
    let second = "2222222222222222222222222222222222222222";
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [settle(first)]}).to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
    // Two passes begun since, so the loop's own fold holds the first whichever
    // thread applied it.
    crate::harness::reporting(&world, run);
    let passes = crate::harness::counts(&world, run).passes;
    world.until("the loop to fold the first settle", |world| {
        crate::harness::counts(world, run).passes >= passes + 2
    });

    hold_the_pass(&world, &seam);
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [settle(second)]}).to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
    let queue = world.run_file(run, "channel/commands.jsonl");
    let third = std::fs::read_to_string(&queue)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count() as u64;
    let mut again = world.cmd(&["reply", run]);
    again
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let mut replying = again.spawn().expect("the third settle is submitted");
    use std::io::Write;
    replying
        .stdin
        .take()
        .expect("a stdin")
        .write_all(
            json!({"version": 2, "commands": [
                settle(first),
                {"op": "finding", "message": "the landing was restated", "blocking": false},
            ]})
            .to_string()
            .as_bytes(),
        )
        .expect("written");
    let cursor = world.run_file(run, "channel/commands-cursor.json");
    world.until("the claimer to claim the third and hand it back", |_| {
        std::fs::read_to_string(&cursor)
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .is_some_and(|claimed| claimed > third)
    });

    release_the_pass(&seam);
    let replied = replying
        .wait_with_output()
        .expect("the third settle is answered");
    let receipt = String::from_utf8_lossy(&replied.stdout);
    assert!(
        receipt.contains("\"applied\""),
        "the loop judged the third settle against a fold without the second: {receipt}"
    );
    let settled = world
        .events_of(run, "node-settled")
        .iter()
        .filter(|event| event["labels"]["node"] == "later")
        .count();
    assert_eq!(settled, 3, "each of the three settles is one settlement");
    world.release("build.go");
}
