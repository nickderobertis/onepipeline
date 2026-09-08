//! The fold checkpoint: what a reader resumes from instead of replaying a run's
//! whole journal.
//!
//! Both journeys drive the compiled binary against a real run store, and neither
//! is stated in seconds: what a supervisory read costs is **work**, and a loaded
//! host hands out time as it likes. So the claim "only the records the checkpoint
//! does not account for were folded" is held by *observing which records the
//! reader consumed* — one it accounts for is made unreadable, so a reader that
//! folded it again says something different — and the loop's half is held by the
//! driver's own count of the records it folded.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven here as
// the real compiled binary over the real run store it wrote; `harness.rs` carries the
// same suppression and the full rationale.

use crate::harness::{agent, counts, plan_of, reporting, World, LOOP_STATS_ENV};
use serde_json::Value;

/// Launch a run through the CLI and wait for its result.
fn settled(world: &World, run: &str, nodes: Vec<Value>) {
    let plan = world.plan(run, &plan_of(run, nodes));
    world.run(&["start", &plan, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
}

/// How many journal records the run's checkpoint accounts for.
fn accounted_for(world: &World, run: &str) -> usize {
    let document = world.run_json(run, "checkpoint.json");
    usize::try_from(
        document["coverage"]["records"]
            .as_u64()
            .unwrap_or_else(|| panic!("no coverage in {document}")),
    )
    .expect("a record count")
}

/// Make one record the checkpoint accounts for unreadable, **without moving a
/// byte after it**.
///
/// The one way to observe which records a reader consumed from outside the
/// process: the line stays exactly as long, so every offset past it and the
/// checkpoint's own byte marker are untouched, and what changes is only what a
/// reader that folds that line again can make of it. A reader resuming from the
/// checkpoint never looks at it and answers as before; one folding the whole
/// store skips it and answers differently.
///
/// Answers with the line it blanked, so a journey that found none fails saying
/// so rather than passing over a store it did not change.
fn blank_a_settlement_within(world: &World, run: &str, within: usize) -> usize {
    let path = world.run_file(run, "events.jsonl");
    let store = std::fs::read(&path).expect("the run's journal");
    let mut lines: Vec<Vec<u8>> = store
        .split_inclusive(|byte| *byte == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    let settlement = lines
        .iter()
        .take(within)
        .position(|line| {
            serde_json::from_slice::<Value>(line).is_ok_and(|event| event["kind"] == "node-settled")
        })
        .unwrap_or_else(|| {
            panic!("the checkpoint accounts for {within} records and none of them settles a node")
        });
    let blanked = lines[settlement].len() - 1;
    lines[settlement] = b"#".repeat(blanked).into_iter().chain([b'\n']).collect();
    std::fs::write(&path, lines.concat()).expect("the journal is rewritten");
    settlement
}

/// A read through a usable checkpoint folds only the records it does not account
/// for, and the same read without one folds them all.
///
/// The control is the same store read twice: the only difference between the two
/// answers is whether the checkpoint was there, which is what makes this a
/// comparison rather than an observation.
// llmlint: ignore-block[tests_mirror_real_usage] no verb blanks a record inside a run's
// own journal or removes the cache beside it, and there is no interface that would: what
// this journey holds is which records a reader *consumed*, which no user-facing surface
// reports and which shows up otherwise only as a read that costs more the longer the run
// has been going.
#[test]
fn a_read_through_a_usable_checkpoint_folds_only_what_it_does_not_account_for() {
    let world = World::new("checkpoint-tail");
    settled(
        &world,
        "tail",
        vec![agent("build", &[]), agent("ship", &["build"])],
    );

    // One read first, so the document beside the store accounts for as much of it
    // as the merge order allows — the driver's own last re-fold stopped at the
    // record it had just written.
    world.run(&["status", "tail"]).exited(0).out_has("2/2 done");
    let accounted = accounted_for(&world, "tail");
    assert!(
        accounted > 0,
        "a read of a settled run accounted for none of its store"
    );
    let blanked = blank_a_settlement_within(&world, "tail", accounted);

    world
        .run(&["status", "tail"])
        .exited(0)
        .out_has("2/2 done")
        .out_lacks("1/2 done");

    std::fs::remove_file(world.run_file("tail", "checkpoint.json"))
        .expect("the checkpoint goes away");
    world
        .run(&["status", "tail"])
        .exited(0)
        .out_has("1/2 done")
        .out_lacks("2/2 done");
    assert!(
        blanked < accounted,
        "the record this journey blanked was not one the checkpoint accounted for"
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// The reconcile loop folds about one record per record the run wrote, rather
/// than the whole journal per change it recorded.
///
/// Stated as a controlled comparison rather than as an absolute: the driver's own
/// count is held against what the tree before this change would have folded — a
/// full re-fold of the store on every state-changing record — over the same run.
// llmlint: ignore-block[tests_mirror_real_usage] what this compares is how many records a
// real driver folded out of a real run store, which no CLI output reports: the cost it
// holds off is invisible to every user-facing surface and shows only as a run that gets
// slower at everything the longer it has been running. The run, the plan and the
// dispatches are all the real ones.
#[test]
fn the_reconcile_loop_folds_what_the_store_grew_by_rather_than_the_whole_journal() {
    let world = World::new("checkpoint-loop").with_env(LOOP_STATS_ENV, "1");
    let nodes: Vec<Value> = (0..6)
        .map(|nth| agent(&format!("step{nth:02}"), &[]))
        .collect();
    settled(&world, "loop", nodes);
    reporting(&world, "loop");

    let did = counts(&world, "loop");
    let records = world.journal("loop").len() as u64;
    // Every record this loop wrote that changes what the graph is, which is what
    // it re-folded on before this change.
    let changes = world
        .journal("loop")
        .iter()
        .filter(|event| event["source"] == "pipeline")
        .filter(|event| {
            matches!(
                event["kind"].as_str().unwrap_or_default(),
                "node-settled" | "node-dispatched" | "edit-committed" | "release-adopted"
            )
        })
        .count() as u64;
    assert!(
        changes > 1,
        "the run recorded no changes to fold on: {did:?}"
    );

    // Held against what a re-fold per recorded change would have cost over the
    // same store, with a margin, rather than against an absolute: how far the
    // marker trails depends on how far apart in time the run's records are, and
    // this run records fifty of them inside a few seconds — which is the
    // *hardest* shape for it and the cheapest one to fold anyway. The run this
    // change is for recorded 22 MB over hours.
    assert!(
        did.records_folded * 2 < changes * records,
        "a driver folded {} records where a re-fold per change would have folded \
         {} — the saving this claims is not there: {did:?}",
        did.records_folded,
        changes * records
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]
