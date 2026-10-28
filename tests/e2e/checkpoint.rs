//! The fold checkpoint, through the compiled binary against a real run store.
//!
//! No claim here is stated in seconds: a loaded host hands out time as it likes,
//! so each is stated as work, or as which records a reader consumed.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven here as
// the real compiled binary over the real run store it wrote; `harness.rs` carries the
// same suppression and the full rationale.

use std::time::Duration;

use crate::harness::{agent, counts, plan_of, reporting, World, LOOP_STATS_ENV};
use serde_json::{json, Value};

fn settled(world: &World, run: &str, nodes: Vec<Value>) {
    let plan = world.plan(run, &plan_of(run, nodes));
    world.run(&["start", &plan, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
}

/// A run of two nodes, settled, and read once so the document beside the store
/// accounts for as much of it as the merge order allows.
fn a_settled_run(name: &str) -> World {
    let world = World::new(name);
    settled(
        &world,
        "tail",
        vec![agent("build", &[]), agent("ship", &["build"])],
    );
    world.run(&["status", "tail"]).exited(0).out_has("2/2 done");
    assert!(
        accounted_for(&world) > 0,
        "a read of a settled run accounted for none of its store"
    );
    world
}

fn accounted_for(world: &World) -> usize {
    let document = world.run_json("tail", "checkpoint.json");
    usize::try_from(
        document["coverage"]["records"]
            .as_u64()
            .unwrap_or_else(|| panic!("no coverage in {document}")),
    )
    .expect("a record count")
}

/// How the run's summary line reads now.
///
/// The rendered state rather than one fragment of it, so "the same state" is a
/// comparison rather than a substring that happens to appear in both answers.
fn summary_line(world: &World) -> String {
    let read = world.run(&["status", "tail"]);
    read.exited(0);
    read.stdout
        .lines()
        .find(|line| line.contains(" done"))
        .unwrap_or_else(|| panic!("no summary line in:\n{}", read.stdout))
        .to_string()
}

/// What the run's summary line reads with no checkpoint there at all.
///
/// Whatever was there is put back, because the read this takes writes one of its
/// own: a control that left its own document behind would replace the one the
/// journey is about to make a claim about.
fn without_a_checkpoint(world: &World) -> String {
    let at = world.run_file("tail", "checkpoint.json");
    let held = std::fs::read(&at).ok();
    let _ = std::fs::remove_file(&at);
    let _ = std::fs::remove_dir_all(&at);
    let whole = summary_line(world);
    let _ = std::fs::remove_file(&at);
    if let Some(bytes) = held {
        std::fs::write(&at, bytes).expect("the document is put back");
    }
    whole
}

fn write_checkpoint(world: &World, document: &Value) {
    std::fs::write(
        world.run_file("tail", "checkpoint.json"),
        serde_json::to_vec_pretty(document).expect("a checkpoint serializes"),
    )
    .expect("the checkpoint is written");
}

fn parsed(document: &[u8]) -> Value {
    serde_json::from_slice(document).expect("the checkpoint parses")
}

/// What a marker *counts*, which is everything about it but its seal.
fn counted(coverage: &Value) -> Value {
    json!([
        coverage["bytes"],
        coverage["records"],
        coverage["at"],
        coverage["streams"]
    ])
}

/// A read through a usable checkpoint reports the state a full fold reports, over a
/// marker that accounts for most of the store.
///
/// The state is what this holds; *which records were folded to reach it* is held
/// where it can be counted — `Projected::took` in `src/checkpoint.rs`'s own
/// journeys, and the driver's `records_folded` below. It cannot be held here,
/// because the document seals over its own contents: a journey outside the crate
/// cannot plant a contradiction in one without the reader refusing it, which is the
/// property the seal exists for.
// llmlint: ignore-block[tests_mirror_real_usage] no verb removes the cache beside a run's
// journal, and what a marker accounts for is reported by no user-facing surface.
#[test]
fn a_read_through_a_usable_checkpoint_reports_what_a_full_fold_reports() {
    let world = a_settled_run("checkpoint-tail");

    let resumed = summary_line(&world);
    assert_eq!(
        resumed,
        without_a_checkpoint(&world),
        "a read through a usable checkpoint and a full fold report different states"
    );
    assert!(resumed.contains("2/2 done"), "{resumed}");

    // And what the next read will not fold, off the document this one wrote.
    let records = world.journal("tail").len();
    let accounted = accounted_for(&world);
    assert!(
        accounted * 2 > records,
        "a marker over a {records}-record store accounts for only {accounted}"
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// **A covered record rewritten in place is never read again**: the read reports
/// what the checkpoint folded, and only a reader with no checkpoint to resume from
/// reads the rewritten store.
///
/// The journal is append-only, and what a checkpoint covers is history no view,
/// pass or reply goes back over — on a long run it is hundreds of megabytes, and a
/// reader that re-read it to check the document paid the whole run per read. So
/// the discriminator runs the other way from a digest's: taking a covered
/// settlement out of the store leaves the document claiming two nodes done where
/// the store now folds to one, and a reader that resumed from the document — and
/// read nothing it covers — says `2/2 done`.
// llmlint: ignore-block[tests_mirror_real_usage] no verb rewrites a record inside a run's
// own journal or edits the cache beside it; what this holds is that the journal outranks
// the cache, which no user-facing surface reports and which shows otherwise only as a view
// serving state the store no longer holds.
#[test]
fn a_covered_record_changed_in_place_is_never_read_again() {
    let world = a_settled_run("checkpoint-rewritten");
    let accounted = accounted_for(&world);
    let marker = counted(&world.run_json("tail", "checkpoint.json")["coverage"]);
    assert!(
        summary_line(&world).contains("2/2 done"),
        "the store this journey is about does not read as settled to begin with"
    );

    let store = world.run_file("tail", "events.jsonl");
    let held = std::fs::read(&store).expect("the run's journal");
    let mut lines: Vec<Vec<u8>> = held
        .split_inclusive(|byte| *byte == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    let settlement = lines
        .iter()
        .take(accounted)
        .position(|line| {
            serde_json::from_slice::<Value>(line).is_ok_and(|event| event["kind"] == "node-settled")
        })
        .unwrap_or_else(|| {
            panic!("the marker accounts for {accounted} records and none of them settles a node")
        });
    let changed = lines[settlement].len() - 1;
    lines[settlement] = b"#".repeat(changed).into_iter().chain(*b"\n").collect();
    let mangled = lines.concat();
    assert_eq!(
        mangled.len(),
        held.len(),
        "this journey did not hold the store's own length"
    );
    std::fs::write(&store, &mangled).expect("the journal is rewritten");
    assert_eq!(
        counted(&world.run_json("tail", "checkpoint.json")["coverage"]),
        marker,
        "the marker's counts moved, so this journey is not about a rewrite they cannot see"
    );

    let read = summary_line(&world);
    assert!(
        read.contains("2/2 done"),
        "a read went back over the history its checkpoint covers: {read}"
    );
    let rebuilt = without_a_checkpoint(&world);
    assert!(
        rebuilt.contains("1/2 done"),
        "a read with no checkpoint did not fold the store as it now stands: {rebuilt}"
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// Every other state that makes a checkpoint unusable still reports what the journal
/// says, and leaves the run readable rather than refusing it.
///
/// A document a reader may not fold from must cost a fold and nothing else: the
/// failure this guards is a view that errors, or reports a run it cannot read as one
/// that is not there, over a cache nobody has to keep.
// llmlint: ignore-block[tests_mirror_real_usage] as above: no verb edits the cache beside a
// run's journal, and there is no interface that would.
#[test]
fn every_unusable_checkpoint_reports_what_the_journal_says() {
    let world = a_settled_run("checkpoint-unusable");
    let at = world.run_file("tail", "checkpoint.json");
    let usable = std::fs::read(&at).expect("the document this run carries");

    for (unusable, leave) in unusable_states() {
        // Back to the document each state is a departure from, so no state inherits
        // the one before it.
        let _ = std::fs::remove_file(&at);
        let _ = std::fs::remove_dir_all(&at);
        std::fs::write(&at, &usable).expect("the document is put back");
        leave(&world, &usable);
        let read = summary_line(&world);
        assert!(
            read.contains("2/2 done"),
            "a checkpoint {unusable} did not read as the journal says: {read}"
        );
    }
}

/// Every state that makes a checkpoint unusable. A list rather than a journey each,
/// because they assert one sentence and differ only in which byte is wrong.
type LeaveUnusable = fn(&World, &[u8]);

fn unusable_states() -> Vec<(&'static str, LeaveUnusable)> {
    vec![
        ("absent", |world, _| {
            std::fs::remove_file(world.run_file("tail", "checkpoint.json"))
                .expect("the checkpoint goes away");
        }),
        ("unreadable", |world, _| {
            std::fs::write(
                world.run_file("tail", "checkpoint.json"),
                b"{ this is not a checkpoint",
            )
            .expect("the checkpoint is mangled");
        }),
        ("at a version this build does not write", |world, usable| {
            let mut document = parsed(usable);
            let version = document["schema_version"].as_u64().unwrap_or(0);
            document["schema_version"] = json!(version + 1);
            write_checkpoint(world, &document);
        }),
        ("naming another run", |world, usable| {
            let mut document = parsed(usable);
            document["run_id"] = json!("somebody-elses-run");
            write_checkpoint(world, &document);
        }),
        // The digest on its own, with every count left exactly as the reader wrote
        // it: the state a rewritten covered prefix produces, reached from the
        // document's side instead of the store's.
        ("carrying a digest of other bytes", |world, usable| {
            let mut document = parsed(usable);
            document["coverage"]["digest"] = json!("ffffffffffffffffffffffffffffffff");
            write_checkpoint(world, &document);
        }),
        (
            "carrying a digest this build could not have written",
            |world, usable| {
                let mut document = parsed(usable);
                document["coverage"]["digest"] = json!("not a digest");
                write_checkpoint(world, &document);
            },
        ),
        (
            "claiming coverage the journal does not corroborate",
            |world, usable| {
                let mut document = parsed(usable);
                // Everything covered claimed as written a century after the records
                // the store holds in front of the marker.
                document["coverage"]["at"] =
                    json!({"ts": "2199-01-01T00:00:00.000Z", "stream": "zzzz"});
                document["coverage"]["bytes"] =
                    json!(document["coverage"]["bytes"].as_u64().unwrap_or(0) / 2);
                write_checkpoint(world, &document);
            },
        ),
        (
            "marking a byte that is not a record boundary",
            |world, usable| {
                let mut document = parsed(usable);
                let bytes = document["coverage"]["bytes"].as_u64().unwrap_or(0);
                document["coverage"]["bytes"] = json!(bytes - 1);
                write_checkpoint(world, &document);
            },
        ),
        (
            "marking more bytes than the journal holds",
            |world, usable| {
                let mut document = parsed(usable);
                let store = world.run_file("tail", "events.jsonl");
                let whole = std::fs::read(&store).expect("the run's journal");
                document["coverage"]["bytes"] = json!(whole.len() as u64 + 1);
                write_checkpoint(world, &document);
            },
        ),
        (
            "carrying a session token this crate refuses",
            |world, usable| {
                let mut document = parsed(usable);
                document["state"]["sessions"] =
                    json!({"build": {"token": "../somewhere-else", "branch": "work"}});
                write_checkpoint(world, &document);
            },
        ),
        (
            "carrying a park reason that says nothing",
            |world, usable| {
                let mut document = parsed(usable);
                document["state"]["parks"] = json!({"build": {"by": "planner", "reason": "   "}});
                write_checkpoint(world, &document);
            },
        ),
        (
            "carrying a session branch this crate refuses",
            |world, usable| {
                let mut document = parsed(usable);
                document["state"]["sessions"] =
                    json!({"build": {"token": "s-abc", "branch": "   "}});
                write_checkpoint(world, &document);
            },
        ),
        // A field this build does not know, which is what a *later* build's document
        // looks like from here once its version has been accepted by hand.
        (
            "carrying a field this build does not know",
            |world, usable| {
                let mut document = parsed(usable);
                document["what_a_later_build_added"] = json!("something");
                write_checkpoint(world, &document);
            },
        ),
        // A directory where the document goes: unreadable, and a write that cannot
        // land either — so this is the one state that also holds the write to being
        // best effort, which a reader may not fail over.
        ("a directory rather than a document", |world, _| {
            let at = world.run_file("tail", "checkpoint.json");
            let _ = std::fs::remove_file(&at);
            std::fs::create_dir(&at).expect("a directory where the document goes");
        }),
    ]
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// **The loop's own state and a view's agree** over a store the merge order
/// rearranges.
///
/// The loop folded the file as it was appended before this change and now folds it in
/// `journal::merge_order`, which is what lets one checkpoint serve both readers. The
/// rearrangement is *constructed* rather than waited for — a run's own store holds one
/// only when its appenders happen to interleave, and a journey whose subject arrives by
/// chance is one that sometimes proves nothing. So a relayed record stamped before the
/// records already in the store is appended by hand, and the view is then held to the
/// account the loop settled on.
// llmlint: ignore-block[tests_mirror_real_usage] the two accounts of one run are the
// binary's own outputs, read the way a consumer reads them. No verb appends another
// producer's record to a run's store after it has settled, and there is no interface that
// would — what it stands in for is a relay landing behind the loop, which is ordinary while
// a run is live and cannot be arranged once one is not.
#[test]
fn the_loop_and_a_view_agree_about_a_store_the_merge_order_rearranges() {
    let world = World::new("checkpoint-one-order");
    settled(
        &world,
        "tail",
        vec![agent("build", &[]), agent("ship", &["build"])],
    );

    // What the loop's own state settled on, before anything is added to the store.
    let settled_as: Vec<(String, String)> = world.run_json("tail", "result.json")["nodes"]
        .as_array()
        .expect("the result names its nodes")
        .iter()
        .map(|node| {
            (
                node["id"].as_str().unwrap_or_default().to_string(),
                node["status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(settled_as.len(), 2, "{settled_as:?}");

    // A record of another producer's stream, stamped before every record the store
    // already holds: the merge puts it first, and the file puts it last.
    let store = world.run_file("tail", "events.jsonl");
    let relayed = json!({
        "v": 1,
        "ts": "1999-01-01T00:00:00.000Z",
        "stream": "graph-behind",
        "seq": 0,
        "source": "agentgraph",
        "kind": "turn-activity",
        "labels": {"run_id": "tail", "node": "build"},
        "payload": {"tool": "Edit"},
        "artifacts": [],
    });
    let mut held = std::fs::read_to_string(&store).expect("the run's journal");
    held.push_str(&format!(
        "{relayed}
"
    ));
    std::fs::write(&store, held).expect("the journal is appended to");
    let after = world.journal("tail");
    let placed = |event: &Value| {
        (
            event["ts"].as_str().unwrap_or_default().to_string(),
            event["stream"].as_str().unwrap_or_default().to_string(),
        )
    };
    assert!(
        after
            .windows(2)
            .any(|pair| placed(&pair[1]) < placed(&pair[0])),
        "the store does not hold a record the merge moves in front of one appended \
         before it, so nothing here is about the order either reader folds in"
    );

    // And the view's account of that store, which has to be the loop's.
    let rendered = world.run(&["results", "tail"]);
    rendered.exited(0);
    for (node, status) in &settled_as {
        assert!(
            rendered.stdout.contains(node) && rendered.stdout.contains(status.as_str()),
            "the loop settled '{node}' as '{status}' and the view does not say so:\n{}",
            rendered.stdout
        );
    }
    assert!(summary_line(&world).contains("2/2 done"));
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// The reconcile loop folds far fewer records than a re-fold of the whole store per
/// change it recorded would have.
// llmlint: ignore-block[tests_mirror_real_usage] how many records a real driver folded out
// of a real run store is reported by no CLI output, and the cost it holds off shows only as
// a run that gets slower the longer it runs. The run, plan and dispatches are the real
// ones.
#[test]
fn the_reconcile_loop_folds_what_the_store_grew_by_rather_than_the_whole_journal() {
    let world = World::new("checkpoint-loop").with_env(LOOP_STATS_ENV, "1");
    // A chain rather than six independent nodes, so the run records its settlements
    // across passes as a real run does instead of bursting them into one instant. How
    // far the marker trails is decided by how far apart in time the records are, so a
    // burst is the one shape this claim would not be about.
    //
    // Built by carrying the node before rather than by naming it from an index: the
    // arithmetic form has nothing to subtract at the head of the chain.
    let ids: Vec<String> = (0..6).map(|nth| format!("step{nth:02}")).collect();
    let mut nodes: Vec<Value> = Vec::new();
    let mut before: Option<&str> = None;
    for id in &ids {
        nodes.push(agent(id, &before.map_or_else(Vec::new, |one| vec![one])));
        before = Some(id);
    }
    settled(&world, "loop", nodes);
    reporting(&world, "loop");

    let did = counts(&world, "loop");
    let records = world.journal("loop").len() as u64;
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

    // A margin rather than an absolute: how far the marker trails depends on how far
    // apart in time the records are, and fifty inside a few seconds is the hardest
    // shape for it — and the cheapest to fold anyway.
    assert!(
        did.records_folded * 2 < changes * records,
        "a driver folded {} records where a re-fold per change would have folded \
         {} — the saving this claims is not there: {did:?}",
        did.records_folded,
        changes * records
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// An attached launch folds nothing while its run records nothing.
///
/// The launch streams its run's records as they arrive, and it looked for them by
/// reading the whole run every twentieth of a second — each read folding the store past
/// its checkpoint — so what the driving process folded grew with how long its run took
/// rather than with what the run recorded. On a loaded Windows runner that was most of
/// what the journey above counted (run 36692817087). A journal that has not grown has
/// no line to stream, so it is not read again until it has.
// llmlint: ignore-block[tests_mirror_real_usage] how many records a real driver folded out
// of a real run store is reported by no CLI output, and a read that folds nothing new
// prints nothing either way. The launch, plan and dispatch are the real ones, attached as
// a person attaches.
#[test]
fn an_attached_launch_folds_nothing_while_its_run_records_nothing() {
    let world = World::new("checkpoint-attached-idle").with_env(LOOP_STATS_ENV, "1");
    world.script("hold.wait", "hold");
    let plan = world.plan("idle", &plan_of("idle", vec![agent("hold", &[])]));
    // Its settlement is the one thing read off it, and only once it has ended; what it
    // streams on the way is not this journey's, and a pipe nobody reads would hold it.
    let launch = world
        .cmd(&["start", &plan, "--attach"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the attached launch starts");
    world.until("the dispatch to start", |world| {
        !world.events_of("idle", "node-dispatched").is_empty()
    });
    reporting(&world, "idle");
    // The launch's own records are behind the window before it opens.
    onepipeline_testfakes::profile::thread::sleep(Duration::from_secs(2));

    let wrote = world.journal("idle").len();
    let before = counts(&world, "idle");
    onepipeline_testfakes::profile::thread::sleep(Duration::from_secs(3));
    let did = counts(&world, "idle").since(before);
    assert_eq!(
        world.journal("idle").len(),
        wrote,
        "the run recorded something inside the window this claim is about"
    );
    assert_eq!(
        did.records_folded, 0,
        "an attached launch folded records of a run that recorded none: {did:?}"
    );
    world.release("hold.go");
    let ended = launch.wait_with_output().expect("the attached launch ends");
    let said = String::from_utf8_lossy(&ended.stdout);
    assert!(
        ended.status.success() && said.contains("\"complete\""),
        "the attached launch did not settle its run complete ({}): {said}",
        ended.status
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// **A checkpoint an earlier build wrote, over a store wider than several rebuild
/// windows, is rebuilt to the whole store's fold, every record but the last instant
/// covered once.**
///
/// A run this host's drivers adopt carries the document the build before them wrote,
/// at a schema this one refuses, so adopting it rebuilds the fold from the journal's
/// first byte, a window at a time. The windows are what bound that rebuild's memory;
/// what has to hold across them is the fold: the state reads as a fold with no
/// document at all reads, and the marker the rebuild writes accounts for exactly the
/// records in front of the last instant — none dropped at a window's edge, none
/// counted twice — ending on the byte that instant's line starts at.
// llmlint: ignore-block[tests_mirror_real_usage] no verb writes an earlier build's
// document or lengthens a settled run's journal; a store this wide stands in for a long
// run's, which a journey cannot run in its time.
#[test]
fn an_earlier_builds_checkpoint_over_a_store_wider_than_its_windows_rebuilds_the_whole_fold() {
    let world = a_settled_run("checkpoint-wide-rebuild");
    let store = world.run_file("tail", "events.jsonl");
    let mut held = std::fs::read_to_string(&store).expect("the run's journal");
    // About five megabytes of another producer's records, each stamped after
    // everything the store holds, so the file's order is the merge's.
    let detail = "w".repeat(900);
    for seq in 0..5_500u64 {
        let record = json!({
            "v": 1,
            "ts": format!("2999-01-01T{:02}:{:02}:{:02}.{:03}Z",
                seq / 3_600_000 % 24, seq / 60_000 % 60, seq / 1_000 % 60, seq % 1_000),
            "stream": "graph-wide",
            "seq": seq,
            "source": "agentgraph",
            "kind": "turn-activity",
            "labels": {"run_id": "tail", "node": "build"},
            "payload": {"tool": "Edit", "detail": detail},
            "artifacts": [],
        });
        held.push_str(&format!("{record}\n"));
    }
    std::fs::write(&store, &held).expect("the journal is appended to");
    let last_line = held.trim_end_matches('\n').rsplit('\n').next().unwrap();
    let records = world.journal("tail").len();

    let at = world.run_file("tail", "checkpoint.json");
    let mut earlier = parsed(&std::fs::read(&at).expect("the run's checkpoint"));
    earlier["schema_version"] = json!(13);
    write_checkpoint(&world, &earlier);

    let rebuilt = summary_line(&world);
    assert!(rebuilt.contains("2/2 done"), "{rebuilt}");
    let written = parsed(&std::fs::read(&at).expect("the rebuild's checkpoint"));
    assert_ne!(
        written["schema_version"],
        json!(13),
        "the earlier document was kept"
    );
    assert_eq!(
        written["coverage"]["records"],
        json!(records - 1),
        "the rebuild did not cover every record but the last instant, once each"
    );
    assert_eq!(
        written["coverage"]["bytes"],
        json!(held.len() - last_line.len() - 1),
        "the marker does not end where the last instant's line starts"
    );
    assert_eq!(
        rebuilt,
        without_a_checkpoint(&world),
        "the rebuild and a fold with no document report different states"
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]
