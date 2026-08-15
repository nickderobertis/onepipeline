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
use serde_json::json;

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
    // The superseded node stays in the executed graph, cancelled, so the run's
    // own record still names what was replaced.
    assert_eq!(status("flaky"), "cancelled");
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

    let rendered = world.run(&["monitor", "stepped"]);
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

    let rendered = world.run(&["monitor", "twice"]);
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
