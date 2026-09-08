//! The fold checkpoint, through the compiled binary against a real run store.
//!
//! No claim here is stated in seconds: a loaded host hands out time as it likes,
//! so each is stated as work, or as which records a reader consumed.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven here as
// the real compiled binary over the real run store it wrote; `harness.rs` carries the
// same suppression and the full rationale.

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

/// Give the document an account of its **covered** records that the journal does
/// not support, leaving the journal itself untouched.
///
/// The only way to observe from outside which records a reader consumed: the
/// covered settlement says `failed` in the document and `done` in the store, so a
/// reader that folded the covered records again overwrites it and one that folded
/// only the tail hands it back. Nothing about the journal changes, so this measures
/// what was read rather than saying anything about the journal's authority.
fn contradict_the_covered_records(world: &World) {
    let mut document = world.run_json("tail", "checkpoint.json");
    document["state"]["recorded"]["build"] = json!({"at": "failed"});
    write_checkpoint(world, &document);
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

/// A read through a usable checkpoint reports the state a full fold reports, and
/// gets there by folding only the records the checkpoint does not account for.
///
/// Two claims and two reads, because one read cannot carry both: the first holds
/// the state equal to a full fold's, and the second — over a document whose account
/// of its covered records the journal contradicts — shows those records were not
/// folded again. The journal is not touched by either.
// llmlint: ignore-block[tests_mirror_real_usage] no verb edits the cache beside a run's
// journal, and which records a reader *consumed* is reported by no user-facing surface.
#[test]
fn a_read_through_a_usable_checkpoint_folds_only_what_it_does_not_account_for() {
    let world = a_settled_run("checkpoint-tail");

    // The state, through a usable document and through no document at all.
    let resumed = summary_line(&world);
    assert_eq!(
        resumed,
        without_a_checkpoint(&world),
        "a read through a usable checkpoint and a full fold report different states"
    );
    assert!(resumed.contains("2/2 done"), "{resumed}");

    // And which records it consumed to get there.
    contradict_the_covered_records(&world);
    let tail_only = summary_line(&world);
    assert!(
        tail_only.contains("1/2 done"),
        "the records the marker accounts for were folded again: {tail_only}"
    );
    assert!(
        without_a_checkpoint(&world).contains("2/2 done"),
        "the control fold served an account only the document carried"
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// **A covered record rewritten in place at identical length rejects the
/// checkpoint**, and the read reports what a full fold of that store reports.
///
/// The modification no count can see: the byte count, the record count and both
/// ordering maxima are exactly what they were, so the digest of the covered bytes
/// is the only thing that refuses it.
///
/// **The rewrite is its own discriminator**, which is why nothing is planted in the
/// document here: taking a covered settlement out of the store leaves the honest
/// marker claiming two nodes done where the store now folds to one, so a reader
/// that accepted the document says `2/2 done` and one that refused it says
/// `1/2 done`. A planted account would have had to differ from *both*, and the
/// obvious one — the same node reported failed — reads identically to the store's
/// own answer, so it would have passed whether the digest was asked or not.
// llmlint: ignore-block[tests_mirror_real_usage] no verb rewrites a record inside a run's
// own journal or edits the cache beside it; what this holds is that the journal outranks
// the cache, which no user-facing surface reports and which shows otherwise only as a view
// serving state the store no longer holds.
#[test]
fn a_covered_record_changed_at_identical_length_rejects_the_checkpoint() {
    let world = a_settled_run("checkpoint-rewritten");
    let accounted = accounted_for(&world);
    let marker = world.run_json("tail", "checkpoint.json")["coverage"].clone();
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
        world.run_json("tail", "checkpoint.json")["coverage"],
        marker,
        "the marker moved, so this journey is not about a store whose marker did not"
    );

    let read = summary_line(&world);
    assert!(
        read.contains("1/2 done"),
        "a checkpoint over a rewritten prefix was folded from anyway: {read}"
    );
    assert_eq!(
        read,
        without_a_checkpoint(&world),
        "the state after a rewritten prefix is not the state that store folds to"
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// Every other state that makes a checkpoint unusable reports the journal's own
/// account rather than the document's.
///
/// The document contradicts its covered records throughout, so a state wrongly
/// accepted reads `1/2 done` where the journal's own answer reads `2/2 done`. The
/// journal is never touched.
// llmlint: ignore-block[tests_mirror_real_usage] as above: no verb edits the cache beside a
// run's journal, and there is no interface that would.
#[test]
fn every_unusable_checkpoint_reports_what_the_journal_says() {
    let world = a_settled_run("checkpoint-unusable");
    contradict_the_covered_records(&world);
    let at = world.run_file("tail", "checkpoint.json");
    let contradicting = std::fs::read(&at).expect("the contradicting document");
    assert!(
        summary_line(&world).contains("1/2 done"),
        "the document does not contradict its covered records, so nothing here is a test"
    );

    for (unusable, leave) in unusable_states() {
        // Back to the document each state is a departure from, so no state inherits
        // the one before it.
        let _ = std::fs::remove_file(&at);
        let _ = std::fs::remove_dir_all(&at);
        std::fs::write(&at, &contradicting).expect("the document is put back");
        leave(&world, &contradicting);
        let read = summary_line(&world);
        assert!(
            read.contains("2/2 done"),
            "a checkpoint {unusable} was folded from anyway: {read}"
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

/// The reconcile loop folds far fewer records than a re-fold of the whole store per
/// change it recorded would have.
// llmlint: ignore-block[tests_mirror_real_usage] how many records a real driver folded out
// of a real run store is reported by no CLI output, and the cost it holds off shows only as
// a run that gets slower the longer it runs. The run, plan and dispatches are the real
// ones.
#[test]
fn the_reconcile_loop_folds_what_the_store_grew_by_rather_than_the_whole_journal() {
    let world = World::new("checkpoint-loop").with_env(LOOP_STATS_ENV, "1");
    // A chain rather than six independent nodes, so the run records its
    // settlements across passes as a real run does instead of bursting them into
    // one instant. How far the marker trails is decided by how far apart in time
    // the records are, so a burst is the one shape this claim would not be about.
    let nodes: Vec<Value> = (0..6)
        .map(|nth| {
            let before = format!("step{:02}", nth - 1);
            let deps: Vec<&str> = if nth == 0 { vec![] } else { vec![&before] };
            agent(&format!("step{nth:02}"), &deps)
        })
        .collect();
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
