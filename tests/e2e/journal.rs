//! The journal is the run's authoritative record, and **the plan of record is
//! the graph the run is executing** — not the launch file, which every live edit
//! the reconciler committed is absent from.
//!
//! Ported from `test_journal_sequence_e2e` and `test_plan_of_record_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The scenario this journey states is one a real sibling would need paid
// model turns to produce, and `dispatch.rs` is where the real `oneagentgraph` binary is
// driven instead. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{agent, plan_of, World};
// The two journeys that assert a refusal are the ones a writer runs out of room in,
// which is `setrlimit(2)`, so both are `#[cfg(unix)]` and so is what only they reach
// for. Unconditionally imported, `REFUSED` is an unused import under `-D warnings` on
// Windows — a build failure this host's gate, which compiles the unix half, cannot see.
#[cfg(unix)]
use crate::harness::REFUSED;
use serde_json::{json, Value};

#[test]
fn a_runs_journal_is_one_ordered_merged_stream_with_per_stream_sequences() {
    let world = World::new("journal-sequence");
    let path = world.plan(
        "sequenced",
        &plan_of(
            "sequenced",
            vec![agent("first", &[]), agent("second", &["first"])],
        ),
    );
    // Detached, so the run really is written by more than one process: the
    // launcher records the launch and the driver it retains records everything
    // the loop does.
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the run to settle", |world| {
        world.run_file("sequenced", "result.json").is_file()
    });

    let events = world.journal("sequenced");
    assert!(events.len() > 4, "{events:?}");

    // Every envelope is version 1, timestamped in the one format, and says which
    // run it belongs to. A relayed one says so under this crate's own namespace:
    // the sibling's `run_id` is its *graph* run, a different identity that the
    // merged store keeps rather than overwrites.
    for event in &events {
        assert_eq!(event["v"], 1, "{event}");
        let ts = event["ts"].as_str().expect("a timestamp");
        assert_eq!(ts.len(), 24, "{ts} is not RFC 3339 millisecond UTC");
        assert!(ts.ends_with('Z'), "{ts} is not UTC");
        let labels = &event["labels"];
        if event["source"] == "pipeline" {
            assert_eq!(labels["run_id"], "sequenced", "{event}");
        } else {
            assert_eq!(labels["onepipeline.run_id"], "sequenced", "{event}");
            assert_ne!(
                labels["run_id"], "sequenced",
                "the sibling's own run id was overwritten: {event}"
            );
        }
    }

    // `seq` is monotonic **per stream** — that is what a consumer detects loss
    // with, and the run is written by more than one process: the launcher, and
    // the driver it retained. Every stream must be gapless from zero.
    let mut by_stream: std::collections::BTreeMap<String, Vec<u64>> = Default::default();
    for event in events.iter().filter(|event| event["source"] == "pipeline") {
        by_stream
            .entry(event["stream"].as_str().expect("a stream").to_string())
            .or_default()
            .push(event["seq"].as_u64().expect("a sequence"));
    }
    assert!(
        by_stream.len() > 1,
        "one process wrote the whole run: {by_stream:?}"
    );
    for (stream, mut seqs) in by_stream {
        let expected: Vec<u64> = (0..seqs.len() as u64).collect();
        seqs.sort_unstable();
        assert_eq!(seqs, expected, "stream {stream} has a gap");
    }

    // A relayed envelope keeps the sibling's own source and stream.
    assert!(
        events.iter().any(|event| event["source"] == "agentgraph"),
        "no dispatch was relayed into the merged store"
    );
}

#[test]
fn a_line_this_build_cannot_read_is_skipped_rather_than_ending_the_read() {
    let world = World::new("journal-future");
    let path = world.plan("future", &plan_of("future", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);

    // llmlint: ignore-block[tests_mirror_real_usage] the case under test is a store this
    // build did not write: a record left by a *newer* onepipeline, whose envelope shape
    // this one cannot read. No command on this build's surface can produce one — a line a
    // sibling emits that this build cannot parse is skipped at the relay and never
    // reaches the store — so writing it is the only way to reach the reader's skip path,
    // and that path is what keeps one unreadable record from ending every view of the run.
    //
    // A record written by a newer schema still claims its sequence number, and
    // a reader skips it rather than failing the run it is observing.
    let journal = world.run_file("future", "events.jsonl");
    let mut text = std::fs::read_to_string(&journal).expect("the journal reads");
    text.push_str("{\"v\":99,\"from\":\"a newer build\"}\n");
    std::fs::write(&journal, text).expect("the journal is written");
    // llmlint: ignore-end[tests_mirror_real_usage]

    world.run(&["monitor", "future"]).exited(0);
    world.run(&["results", "future"]).exited(0);
    world.run(&["telemetry", "future"]).exited(0);

    // A driver is the one reader that cannot shrug: a record it cannot read
    // might have been an authoritative graph mutation — a `drop` that removed a
    // node it is about to dispatch — so it says so. It still drives, because
    // refusing would leave the run with nothing driving it.
    world
        .run(&["adopt", "future"])
        .exited(0)
        .err_has("cannot read")
        .err_has("missing a committed edit");
}

#[test]
fn a_dispatch_line_this_build_cannot_read_is_skipped_and_the_turn_after_it_is_not() {
    let world = World::new("journal-badline");
    // The sibling emits a line from a newer build *before* its real turn, so a
    // reader that stopped at the first unreadable one would lose the turn that
    // follows — and with it the node's own evidence.
    world.script("build.unreadable", "");
    let path = world.plan("badline", &plan_of("badline", vec![agent("build", &[])]));
    let started = world.run(&["start", &path.to_string_lossy(), "--attach"]);
    started.exited(0).err_has("skipped");

    assert!(
        world
            .journal("badline")
            .iter()
            .any(|event| event["source"] == "agentgraph" && event["labels"]["node"] == "build"),
        "the turn after the unreadable line never reached the merged store"
    );
    assert_eq!(
        world.run_json("badline", "result.json")["state"],
        "complete",
        "an unreadable sibling line failed the node it belonged to"
    );
}

/// The plan of record is what the loop is executing, not the file it launched.
#[test]
fn the_graph_of_record_is_the_one_the_loop_executed_not_the_launch_file() {
    let world = World::new("journal-record");
    world.script("flaky.wait", "hold");
    let path = world.plan(
        "record",
        &plan_of(
            "record",
            vec![agent("flaky", &[]), agent("after", &["flaky"])],
        ),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the node to be in flight", |world| {
        !world.events_of("record", "node-dispatched").is_empty()
    });

    world
        .run_with_stdin(
            &["reply", "record"],
            &json!({
                "version": 1,
                "commands": [{
                    "op": "retry",
                    "id": "flaky",
                    "node": {"id": "flaky-2", "persona": "engineer", "task": "## What\nagain"},
                }],
            })
            .to_string(),
        )
        .exited(0);
    world.release("flaky.go");
    world.until("the run to settle", |world| {
        world.run_file("record", "result.json").is_file()
    });

    // The plan file is never rewritten: it is what the run *started* with, and
    // the replacement is not in it.
    let launched: Vec<String> = world.run_json("record", "plan.json")["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .filter_map(|node| node["id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        launched,
        vec!["flaky".to_string(), "after".to_string()],
        "the launch file was rewritten"
    );

    // The graph the loop *executed* is the one that counts, and it carried the
    // replacement: the reconciler installed the edited graph immediately, so
    // `flaky-2` was dispatched without waiting for anything.
    let executed = world.run_json("record", "result.json");
    let status = |id: &str| {
        executed["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["id"] == id)
            .unwrap_or_else(|| panic!("{id} is missing from {executed}"))["status"]
            .clone()
    };
    assert_eq!(status("flaky-2"), "done");
    assert_eq!(status("after"), "done");
    // The superseded node left the graph with the same edit that replaced it —
    // left in, it would hold the run in `waiting` for work nothing will ever
    // dispatch again. What became of it is in the journal, where the edit is.
    assert!(
        executed["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .all(|node| node["id"] != "flaky"),
        "the superseded node is still in the graph: {executed}"
    );
    let superseded = world
        .events_of("record", "node-settled")
        .into_iter()
        .find(|event| event["labels"]["node"] == "flaky")
        .expect("the superseded node's own settlement is recorded");
    assert_eq!(superseded["payload"]["status"], "cancelled", "{superseded}");
    assert_eq!(executed["state"], "complete");
}

#[test]
fn the_runs_result_is_written_atomically_and_read_back_whole() {
    let world = World::new("journal-result");
    let path = world.plan("atomic", &plan_of("atomic", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);

    let result = world.run_json("atomic", "result.json");
    assert_eq!(result["run_id"], "atomic");
    assert_eq!(result["ok"], true);
    // One document for the whole run: there are no rounds to record separately.
    assert!(
        result.get("round").is_none(),
        "the result claims a round: {result}"
    );

    // No temporary survives the rename a reader could pick up instead.
    let leftovers: Vec<String> = std::fs::read_dir(world.run_file("atomic", ""))
        .expect("the run directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("tmp"))
        .collect();
    assert!(leftovers.is_empty(), "a temporary survived: {leftovers:?}");
}

/// One stream's records are read back in the order their producer wrote them,
/// whatever its clock said.
///
/// The producer here is the `oneagentgraph` stand-in, scripted to do what a real
/// one does when its host clock is corrected under it: keep counting its `seq`
/// forward while stamping a later record with an earlier reading. Everything
/// below that is real — the sibling's NDJSON crosses the launcher's own relay,
/// is merged by the merge under test, is written to the run's own store, and is
/// read back by `monitor`, which is where a person sees the order.
///
/// A dispatch reports its `turn-activity` before its `turn-completed`, always,
/// because that is the order the turn happened in. Sorted by the clock this
/// producer stamped, it reads the other way round.
#[test]
fn a_streams_own_order_survives_a_clock_that_disagrees_with_it() {
    let world = World::new("journal-clock");
    world.script("build.clock-stepped", "the turn's clock steps back");
    let path = world.plan("stepped", &plan_of("stepped", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);

    let rendered = world.run(&["monitor", "stepped", "--all"]);
    rendered.exited(0);
    let at = |kind: &str| {
        rendered
            .stdout
            .lines()
            .position(|line| line.contains(kind))
            .unwrap_or_else(|| panic!("`monitor` never rendered {kind}:\n{}", rendered.stdout))
    };
    assert!(
        at("turn-activity") < at("turn-completed"),
        "the merge reordered one stream against the sequence its producer stamped:\n{}",
        rendered.stdout
    );
}

/// Two records of one stream claiming one `seq` keep the order they arrived in.
///
/// Only a producer in error stamps one sequence twice, so there is nothing to be
/// *right* about here beyond being stable — which is exactly why it needs saying.
/// A store that shuffled these under a second reading would be a run whose record
/// changed when it was reread, and a reader comparing two readings of it would be
/// chasing a difference nobody made.
///
/// Read through `monitor`, like every other claim about the merged order.
#[test]
fn two_records_of_one_stream_claiming_one_sequence_keep_arriving_order() {
    let world = World::new("journal-dup-seq");
    world.script("build.duplicate-seq", "one seq, stamped twice");
    let path = world.plan("twice", &plan_of("twice", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);

    let rendered = world.run(&["monitor", "twice", "--all"]);
    rendered.exited(0);
    let at = |text: &str| {
        rendered
            .stdout
            .lines()
            .position(|line| line.contains(text))
            .unwrap_or_else(|| panic!("`monitor` never rendered {text}:\n{}", rendered.stdout))
    };
    assert!(
        at("the dispatch ran") < at("the dispatch ran again"),
        "the merge reordered two records that arrived under one sequence:\n{}",
        rendered.stdout
    );
}

/// A run whose journal is whole, and the run id it was written under.
///
/// Every journey below starts from a real store a real run wrote: the fragment
/// each of them is about is a *tail* on that store, and a torn record only
/// matters beside the records it is torn away from.
fn settled_run(name: &'static str) -> (World, String) {
    let world = World::new(name);
    let plan = world.plan(name, &plan_of(name, vec![agent("build", &[])]));
    world
        .run(&["start", &plan.to_string_lossy(), "--attach"])
        .exited(0)
        .settled();
    (world, name.to_string())
}

/// What a record a dying writer had got as far as looks like.
const FRAGMENT: &str =
    "{\"v\":1,\"ts\":\"2026-08-16T00:00:00.000Z\",\"stream\":\"dead-writer\",\"seq\":";

/// Leave the fragment a writer that died mid-record leaves behind.
///
/// The state on disk, written as the dead writer left it: bytes with no
/// terminator after them.
///
// llmlint: ignore-block[tests_mirror_real_usage] there is no user-facing path to this
// precondition and there must not be: the whole of the fix is that no append this build
// makes can leave a fragment, because one that fails takes its own bytes back off the
// file. The state the healing exists for is one *another* writer left — an older build,
// or a process the host killed between the two halves of a record — so a journey that
// waited for this binary to produce it would be waiting for the defect to come back.
// What is written here is bytes on disk, not a stand-in for anything in the crate, which
// every journey below drives as the compiled binary it is.
fn leave_a_fragment(journal: &std::path::Path, bytes: usize) -> String {
    use std::io::Write;
    let fragment = format!("{FRAGMENT}{}", "0".repeat(bytes));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(journal)
        .expect("the journal opens");
    file.write_all(fragment.as_bytes())
        .expect("the fragment is written");
    fragment
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A writer that runs out of room mid-record leaves the store on a boundary.
///
/// `write_all` loops on short writes, and a full disk answers the first
/// `write(2)` with a partial count: those bytes are in the file before the retry
/// returns the error, and nothing used to take them back out. What that left was
/// an unterminated fragment — and the next process to append glued its own whole
/// record onto the end of it, destroying a record nobody had lost yet.
#[cfg(unix)]
#[test]
fn a_writer_that_runs_out_of_room_leaves_the_journal_on_a_record_boundary() {
    let (world, run) = settled_run("journal-shortwrite");
    let journal = world.run_file(&run, "events.jsonl");
    let before = std::fs::read_to_string(&journal).expect("the journal reads");

    // Room for part of the next record and not for all of it: the record is
    // several kilobytes and the ceiling is sixty-four bytes above what the store
    // already holds.
    let ceiling = before.len() as u64 + 64;
    // Long enough that the record cannot fit in what is left, and short enough
    // that the channel's own small files — written before the journal is
    // appended to — stay well under a ceiling set from a store several
    // kilobytes long. The ceiling is on *every* file the process writes.
    let message = "x".repeat(300);
    let refused = world.run_with_file_ceiling(
        &["surface", &run, "--kind", "check-in", "--message", &message],
        ceiling,
    );
    refused.exited(REFUSED).err_has("events.jsonl");

    let after = std::fs::read_to_string(&journal).expect("the journal reads");
    assert!(
        after.ends_with('\n'),
        "the journal ends mid-record: {:?}",
        &after[after.len().saturating_sub(120)..]
    );
    assert_eq!(
        after, before,
        "the failed append left bytes behind rather than rolling them back"
    );
    for line in after.lines() {
        serde_json::from_str::<Value>(line)
            .unwrap_or_else(|e| panic!("a fragment reached the store: {e}: {line}"));
    }
}

/// A fragment found at append time is healed, and what it cost is reported.
///
/// Healing is not repair: the record the fragment was is gone, and a store that
/// quietly patched itself back to a boundary would be a run whose own account of
/// itself is wrong with nothing saying so. So the append says it on stderr, and
/// records it beside the store — where a read verb reports it to somebody who
/// was not watching the process that healed it.
#[test]
fn a_fragment_a_dead_writer_left_is_healed_and_the_loss_is_reported() {
    let (world, run) = settled_run("journal-heal");
    let journal = world.run_file(&run, "events.jsonl");
    let whole = std::fs::read_to_string(&journal).expect("the journal reads");
    let fragment = leave_a_fragment(&journal, 200);

    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "check-in",
            "--message",
            "the run is still going",
        ])
        .exited(0)
        .err_has("discarded a")
        .err_has("record fragment");

    let after = std::fs::read_to_string(&journal).expect("the journal reads");
    assert!(
        after.starts_with(&whole),
        "healing cut into the records the store already held"
    );
    assert!(
        !after.contains("dead-writer"),
        "the fragment is still in the store: {after}"
    );
    assert!(after.ends_with('\n'), "the store does not end on a record");
    assert_eq!(
        world
            .events_of(&run, "planner-surface-queued")
            .last()
            .expect("the surface reached the store")["payload"]["message"],
        json!("the run is still going"),
        "the record that healed the store is not in it"
    );

    // The loss outlives the process that healed it: a detached run writes its
    // stderr to a log nobody opens, and this is what a reader is shown instead.
    let recorded = read_torn_log(&world.run_file(&run, "events.jsonl.torn"));
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(
        recorded[0]["bytes"],
        json!(fragment.len()),
        "the loss was recorded as a size it was not: {recorded:?}"
    );
    assert_eq!(recorded[0]["offset"], json!(whole.len()));

    let status = world.run(&["status", &run]);
    status.exited(0).out_has("journal:").out_has(&format!(
        "1 fragment discarded at append: byte {} ({} bytes)",
        whole.len(),
        fragment.len()
    ));
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("this run's record of itself is incomplete");

    // The account of a loss is a file like any other, and a writer can die in
    // the middle of it too. It heals the same way — and says so on stderr rather
    // than into a log of its own, which is a recursion with no end.
    let losses = world.run_file(&run, "events.jsonl.torn");
    leave_a_fragment(&losses, 20);
    let healed_again = std::fs::read_to_string(&journal).expect("the journal reads");
    let second = leave_a_fragment(&journal, 60);
    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "check-in",
            "--message",
            "and going",
        ])
        .exited(0)
        .err_has("of the loss log itself");

    let recorded = read_torn_log(&losses);
    assert_eq!(
        recorded.len(),
        2,
        "the loss log lost a record of its own: {recorded:?}"
    );

    // A whole line of the loss log that is not one of its records — a hand
    // edit, or a build that wrote them differently — costs that one line and
    // not the account around it. A view that would not say what a run lost
    // because the record of a loss was unreadable would be the silence this
    // whole thing is about.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] nothing a user can type writes a line into this log — this build writes only its own records there — so the claim, which is about what a *reader* does with one it cannot read, has no other way to reach its precondition. What is written is a line of a file, not a stand-in for anything in the crate: the reader under test is the compiled binary, run below.
    std::fs::write(
        &losses,
        format!(
            "{}{{\"not\":\"a loss\"}}\n",
            std::fs::read_to_string(&losses).expect("the loss log reads")
        ),
    )
    .expect("the loss log is written");
    // llmlint: ignore-end[tests_mirror_real_usage]
    world.run(&["status", &run]).exited(0).out_has(&format!(
        "2 fragments discarded at append: byte {} ({} bytes), byte {} ({} bytes)",
        whole.len(),
        fragment.len(),
        healed_again.len(),
        second.len()
    ));
}

/// An append that heals a fragment and then fails still reports what it
/// discarded.
///
/// The two happen on one call and a host that has run out of room stays out of
/// room: the fragment is gone whether or not the record meant to replace it ever
/// reached the file. An appender that reported the loss only when its own write
/// succeeded would discard the evidence of a death on a full disk — the exact
/// conditions the death happened under.
#[cfg(unix)]
#[test]
fn an_append_that_heals_and_then_runs_out_of_room_still_reports_the_loss() {
    let (world, run) = settled_run("journal-heal-shortwrite");
    let journal = world.run_file(&run, "events.jsonl");
    let whole = std::fs::read_to_string(&journal).expect("the journal reads");
    let fragment = leave_a_fragment(&journal, 200);

    let ceiling = whole.len() as u64 + fragment.len() as u64 + 64;
    let refused = world.run_with_file_ceiling(
        &[
            "surface",
            &run,
            "--kind",
            "check-in",
            "--message",
            &"x".repeat(300),
        ],
        ceiling,
    );
    refused
        .exited(REFUSED)
        .err_has("discarded a")
        .err_has("events.jsonl");

    assert_eq!(
        std::fs::read_to_string(&journal).expect("the journal reads"),
        whole,
        "the store did not come back to the boundary the heal left it on"
    );
    let recorded = read_torn_log(&world.run_file(&run, "events.jsonl.torn"));
    assert_eq!(
        recorded.len(),
        1,
        "an append that failed after healing discarded the fragment and said nothing"
    );
    assert_eq!(recorded[0]["bytes"], json!(fragment.len()));
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("1 fragment discarded at append");
}

/// Two real appenders, one of which heals: neither loses the other's record.
///
/// The unsound fix this rules out is a lock taken on the healing path alone. A
/// lock one participant takes excludes nobody: the healer truncates back to the
/// last boundary it read, and a whole record another process appended in between
/// goes with the fragment — the very loss the healing is for. So every appender
/// takes the lock, on the same descriptor it appends through, and this journey
/// holds that lock from outside the binary to prove it: an appender that took no
/// lock would walk straight past a held one and heal the store under it.
///
/// Unix only, because the holder is `flock(2)`. The Windows half of that seam
/// excludes a second appender at `CreateFile` instead — see
/// `sys::open_locked_append` — and holding *that* from a test would prove the
/// share mode rather than the exclusion. Divergence 18 in the divergence record
/// is the standing note about which journeys this suite runs on one platform.
#[cfg(unix)]
#[test]
fn an_appender_waits_for_the_writer_ahead_of_it_rather_than_healing_under_it() {
    use std::os::unix::io::AsRawFd;

    let (world, run) = settled_run("journal-race");
    let journal = world.run_file(&run, "events.jsonl");
    let whole = std::fs::read_to_string(&journal).expect("the journal reads");
    leave_a_fragment(&journal, 400);

    // llmlint: ignore-block[tests_mirror_real_usage] the second writer here is a real
    // one, taking the store's own lock the way every appender does — that is the point:
    // an exclusion nobody outside the binary can take is one no journey can prove was
    // taken. There is no verb that holds a run's journal open for a while, and adding one
    // to test with would be a surface nobody asked for; what stands in is not a component
    // of the crate but a second participant in a protocol the host owns.
    let held = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .expect("the journal opens");
    // SAFETY: the descriptor is one this test owns for the whole of the block
    // below, and `flock` borrows no memory.
    assert_eq!(
        unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX) },
        0,
        "the journal could not be held: {}",
        std::io::Error::last_os_error()
    );
    // llmlint: ignore-end[tests_mirror_real_usage]

    const APPENDERS: usize = 8;
    let mut racing: Vec<_> = (0..APPENDERS)
        .map(|n| {
            let message = format!("appender {n} was here");
            world
                .cmd(&["surface", &run, "--kind", "check-in", "--message", &message])
                .spawn()
                .expect("the binary starts")
        })
        .collect();

    // None of them may touch the store while somebody else holds it. An appender
    // that healed here would truncate the fragment away under a writer that is
    // in the middle of appending, which is how a whole record is destroyed.
    std::thread::sleep(std::time::Duration::from_millis(500));
    for appender in &mut racing {
        assert!(
            appender
                .try_wait()
                .expect("the appender is waitable")
                .is_none(),
            "an appender wrote to a store another writer was holding"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&journal)
            .expect("the journal reads")
            .len(),
        whole.len() + 400 + FRAGMENT.len(),
        "the held store was written to anyway"
    );
    drop(held);

    for mut appender in racing {
        assert!(
            appender.wait().expect("the appender ran").success(),
            "an appender refused"
        );
    }

    let after = std::fs::read_to_string(&journal).expect("the journal reads");
    assert!(after.ends_with('\n'), "the store does not end on a record");
    for line in after.lines() {
        serde_json::from_str::<Value>(line)
            .unwrap_or_else(|e| panic!("a torn record reached the store: {e}: {line}"));
    }
    // Every record the store held before the race, and every record the race
    // added: the heal that ran in the middle of it took the fragment and
    // nothing else.
    for line in whole.lines() {
        assert!(
            after.contains(line),
            "a record the store held is gone: {line}"
        );
    }
    let messages: Vec<String> = world
        .events_of(&run, "planner-surface-queued")
        .iter()
        .filter_map(|event| event["payload"]["message"].as_str().map(str::to_string))
        .collect();
    for n in 0..APPENDERS {
        let mine = format!("appender {n} was here");
        assert!(
            messages.contains(&mine),
            "a racing appender's whole record was destroyed: {messages:?}"
        );
    }
    assert_eq!(
        read_torn_log(&world.run_file(&run, "events.jsonl.torn")).len(),
        1,
        "the fragment was healed more than once"
    );
}

/// A reader tells a record that stops early from one it merely cannot read.
///
/// They are different facts and they call for different things: a truncated
/// record is a loss this run really suffered, and a line from a newer build is
/// one this reader cannot interpret with nothing missing. Neither used to reach
/// a reader at all — `read` filtered both away, and the only hook was a bool on
/// the driver's stderr.
///
/// Two of each, because a view that could only say "1" of anything would be
/// counting nothing.
#[test]
fn a_read_verb_tells_a_truncated_record_from_one_it_cannot_read() {
    let (world, run) = settled_run("journal-classes");
    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "check-in",
            "--message",
            "the record after the fragment",
        ])
        .exited(0);
    let journal = world.run_file(&run, "events.jsonl");

    // llmlint: ignore-block[tests_mirror_real_usage] the store under test is one this
    // build cannot write any more: a whole record glued onto the fragment of a record a
    // dying writer left, which is what the append path now prevents and what a store an
    // older build wrote still holds. Both halves are real records this run produced — the
    // gluing is the measured on-disk shape reproduced, not a stand-in for anything in the
    // crate, which is driven here as the compiled binary it is.
    let text = std::fs::read_to_string(&journal).expect("the journal reads");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let last = lines.pop().expect("the store holds a record");
    let fragment = "{\"v\":1,\"ts\":\"2026-08-16T00:00:00.000Z\",\"stream\":\"dead-wri";
    lines.push(format!("{fragment}{last}"));
    let future = r#"{"v":99,"from":"a newer build"}"#;
    lines.push(future.to_string());
    lines.push(future.to_string());
    // The last line of all, with nothing after it: a record whose writer was
    // stopped between the record and its terminator and has not come back. Its
    // last byte is the first of a character it never finished, because a writer
    // stopped mid-record stops mid-*byte-sequence* as readily as between two —
    // and a reader that decoded the file whole would fail on that one byte and
    // hand back a store holding nothing at all.
    let tail = &fragment[..30];
    let mut store = format!("{}\n{tail}", lines.join("\n")).into_bytes();
    store.push(0xE2);
    std::fs::write(&journal, &store).expect("the store is written");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let offset = |upto: usize| -> u64 { lines[..upto].iter().map(|l| l.len() as u64 + 1).sum() };
    let glued = lines.len() - 3;
    let status = world.run(&["status", &run]);
    status
        .exited(0)
        .out_has(&format!(
            "2 truncated records: line {} at byte {} ({} bytes), line {} at byte {} ({} bytes)",
            glued + 1,
            offset(glued),
            fragment.len(),
            lines.len() + 1,
            offset(lines.len()),
            tail.len() + 1
        ))
        .out_has(&format!(
            "2 lines this build cannot read: line {} at byte {} ({} bytes), \
             line {} at byte {} ({} bytes)",
            lines.len() - 1,
            offset(lines.len() - 2),
            future.len(),
            lines.len(),
            offset(lines.len() - 1),
            future.len()
        ));

    // And the record the tear used to destroy is handed back: it is whole, it is
    // in the store, and the only thing wrong with it is what is in front of it.
    world
        .run(&["monitor", &run])
        .exited(0)
        .out_has("the record after the fragment");
}

/// A record whose terminator never landed is not a record yet.
///
/// The boundary case a short write stops on: the record itself is whole and
/// parses, and the newline after it never arrived. It is a write that stopped in
/// the middle — the append that writes it writes both in one call — so the next
/// append discards it, and a reader that had counted it as a record would have
/// handed back a record the store was about to say it lost.
#[test]
fn a_record_whose_terminator_never_landed_is_not_a_record_yet() {
    let (world, run) = settled_run("journal-unterminated");
    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "check-in",
            "--message",
            "the record with no newline after it",
        ])
        .exited(0);
    let journal = world.run_file(&run, "events.jsonl");

    // llmlint: ignore-block[tests_mirror_real_usage] the state is a write that stopped
    // between a record and its terminator, which is a thing only a dying writer produces
    // — no build carrying this fix leaves one, because an append that fails takes its own
    // bytes back off the file. The record itself is one this run really wrote; what the
    // journey does is take the newline off the end of the store, which is bytes on disk
    // and not a stand-in for anything in the crate.
    let text = std::fs::read_to_string(&journal).expect("the journal reads");
    let whole = text.trim_end_matches('\n');
    let last = whole.lines().next_back().expect("the store holds a record");
    let offset = whole.len() - last.len();
    std::fs::write(&journal, whole).expect("the store is written");
    // llmlint: ignore-end[tests_mirror_real_usage]

    world.run(&["status", &run]).exited(0).out_has(&format!(
        "1 truncated record: line {} at byte {offset} ({} bytes)",
        whole.lines().count(),
        last.len()
    ));
    // Not handed back as a record, either: it is not one until its writer says
    // so, and the store is about to discard it.
    world
        .run(&["monitor", &run])
        .exited(0)
        .out_lacks("the record with no newline after it");

    // And that is exactly what the next append does — it heals the store back to
    // its last boundary and reports the record as the loss it is.
    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "check-in",
            "--message",
            "the next one",
        ])
        .exited(0)
        .err_has("discarded a");
    let recorded = read_torn_log(&world.run_file(&run, "events.jsonl.torn"));
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0]["bytes"], json!(last.len()));
    assert_eq!(recorded[0]["offset"], json!(offset));
}

/// A loss another build of this crate recorded is still reported.
///
/// The loss log is read line by line and a line this build cannot read is
/// dropped, so a key another build wrote used to cost exactly the thing the file
/// exists for: the loss goes unmentioned, and the run reads as one whose record
/// of itself is whole. A field it does not know is now ignored instead, and the
/// loss is on the view.
#[test]
fn a_loss_another_build_recorded_is_still_reported() {
    let (world, run) = settled_run("journal-newer-loss");
    let journal = world.run_file(&run, "events.jsonl");
    let whole = std::fs::read_to_string(&journal).expect("the journal reads");
    let fragment = leave_a_fragment(&journal, 120);
    world
        .run(&[
            "surface",
            &run,
            "--kind",
            "check-in",
            "--message",
            "the run is still going",
        ])
        .exited(0)
        .err_has("discarded a");

    // llmlint: ignore-block[tests_mirror_real_usage] no append this build makes writes a
    // key this build does not have, so the only way to hold a loss another build recorded
    // is to put the key on the one this build wrote. The loss itself is real — a fragment
    // healed by the running binary — and the claim below is read off the CLI.
    let losses = world.run_file(&run, "events.jsonl.torn");
    let recorded = read_torn_log(&losses);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    let mut written = recorded[0].clone();
    written["healed_by_build"] = json!("a build that came later");
    std::fs::write(&losses, format!("{written}\n")).expect("a loss another build recorded");
    // llmlint: ignore-end[tests_mirror_real_usage]

    world.run(&["status", &run]).exited(0).out_has(&format!(
        "1 fragment discarded at append: byte {} ({} bytes)",
        whole.len(),
        fragment.len()
    ));
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("this run's record of itself is incomplete");
}

/// The fragments an append healed out of one store, as it recorded them.
fn read_torn_log(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        })
        .unwrap_or_default()
}
