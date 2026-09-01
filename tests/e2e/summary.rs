//! The bounded listing: what a consumer reads instead of a run's whole journal.
//!
//! Every run here is launched and settled through the compiled binary, and every
//! summary is read through the crate's own public reader over the store that
//! binary left on disk — which is what the reader a consumer writes will do.
//! Nothing here folds a journal by hand or asserts on an internal.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary; `harness.rs` carries the same suppression and the full rationale. The
// reader under test here is this crate's own library surface, driven against the store the
// real binary wrote — there is no CLI verb for it, and asserting on the rendered `runs`
// output instead would be asserting on a different reader.

use crate::harness::{agent, plan_of, World};

use onepipeline::views::{Listing, Party, RunPaths, RunSummary, SUMMARY_SCHEMA_VERSION};

/// How many nodes the live-writer journey drives.
///
/// The reader has to run against the writer for long enough to be a race rather
/// than a coincidence, and every node past that is wall time the journey does
/// not need.
const NODES: u64 = 6;

/// Launch a run and wait for it to settle, as every journey in `views.rs` does.
fn settled(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

fn paths_of(world: &World, run: &str) -> RunPaths {
    RunPaths::under(&world.runs, run)
}

/// Take a run's summary document away, leaving the run recorded by a build that
/// never wrote one.
///
/// llmlint: ignore[tests_mirror_real_usage] no verb of this build removes the document its
/// journal writer maintains, and there is no interface that would: the run recorded by a
/// build predating it is the state under test, and taking the document off a run this
/// build's own CLI recorded is the only way to hold one. Every claim either side of this
/// is read through the public reader over the store the real binary wrote.
fn recorded_before_the_document(paths: &RunPaths) {
    std::fs::remove_file(paths.summary()).expect("the document");
}

/// A run driven through the CLI keeps a summary beside its result, and the row
/// it serves is the row a full fold produces.
///
/// **The check that keeps the two accounts from drifting**, and it fails when
/// either moves: the served row comes off the document the run's own journal
/// writer maintained as it recorded, and the row beside it comes off folding
/// that journal from nothing. One derivation runs over both, so any field that
/// starts being computed differently on one path shows up here as a difference.
#[test]
fn a_run_driven_through_the_cli_serves_the_row_its_own_fold_produces() {
    let world = World::new("summary-agrees");
    world.script("build.work", "the worker wrote this\n");
    // Six dispatches, because six is what makes the run's judge cost a number
    // that distinguishes reading it back from *nearly* reading it back: summed
    // one settlement at a time it is `0.12000000000000001`, which serde_json
    // writes out in full and — without `float_roundtrip` — parses back as
    // `0.12`. The equality below is what would catch that, and with two nodes it
    // would not have anything to catch.
    let nodes: Vec<serde_json::Value> = (0..NODES)
        .map(|nth| agent(&format!("step{nth:02}"), &[]))
        .collect();
    let run = settled(&world, "recorded", nodes);
    let paths = paths_of(&world, &run);
    assert!(
        paths.summary().is_file(),
        "the run's journal writer left no summary beside its result"
    );

    let served = RunSummary::of(&paths).expect("the run reads");
    assert_eq!(served.schema_version, SUMMARY_SCHEMA_VERSION);
    assert_eq!(served.run_id, run);
    // What the run actually was, read off the store the CLI wrote rather than
    // restated: the row and the journal beside it count the same records.
    assert_eq!(served.event_count as usize, world.journal(&run).len());
    assert_eq!(served.node_counts.get("done"), Some(&NODES));
    assert!(
        served.graph_complete,
        "a settled run is not complete: {served:?}"
    );
    assert!(!served.stop_recorded);
    // The launch record's own account, so a row's attribution needs no second
    // read of it.
    let launch = world.run_json(&run, "launch.json");
    assert_eq!(served.launcher, launch["launcher"].as_str().unwrap_or(""));
    assert_eq!(served.session, launch["session"].as_str().unwrap_or(""));
    assert_eq!(served.host.as_deref(), launch["host"].as_str());
    assert_eq!(served.started_at.as_deref(), launch["started_at"].as_str());
    // And the run's aggregate clock, so a consumer no longer starts a process
    // per listed row to get it.
    assert_eq!(served.timing.run_id, run);
    assert_eq!(served.timing.settled_done, NODES);
    // The cost read back off the document is the cost that was written, to the
    // last bit — see the note above the plan.
    assert_eq!(
        served.timing.usage[&Party::Judge].cost_usd,
        Some(0.12000000000000001),
        "the cost read back is not the cost the run recorded"
    );

    // The same row, folded from nothing: the document is taken away, so the
    // reader has no choice but the reading every listing did before it existed.
    recorded_before_the_document(&paths);
    let folded = RunSummary::of(&paths).expect("the run folds");
    assert_eq!(
        served, folded,
        "the row the writer maintained and the row a full fold produces differ"
    );
}

/// A run recorded by a build that never wrote a summary lists exactly as it
/// does today, and only more slowly.
///
/// This is what makes the landing non-breaking: the fallback is not a degraded
/// answer, it is the same answer.
#[test]
fn a_run_recorded_without_a_summary_lists_identically() {
    let world = World::new("summary-older-run");
    let run = settled(&world, "from-an-older-build", vec![agent("build", &[])]);
    let paths = paths_of(&world, &run);
    let maintained = RunSummary::of(&paths).expect("the run reads");

    recorded_before_the_document(&paths);

    let listing = Listing::of(&world.runs);
    assert_eq!(listing.root, world.runs);
    let row = listing
        .summaries
        .iter()
        .find(|row| row.run_id == run)
        .expect("the run is on the listing");
    assert_eq!(row, &maintained);
    assert!(
        listing.skipped.is_empty(),
        "a run with no summary was refused rather than folded: {:?}",
        listing.skipped
    );
    assert!(
        paths.summary().is_file(),
        "the fold was not cached, so every later listing folds again"
    );
}

/// A refused run root reaches the bounded listing on the same terms the folding
/// survey states, rather than being dropped where it is cheap to drop it.
///
/// The silent omission this whole seam exists to remove would come straight back
/// at the new surface otherwise: a host of thirty run roots reading as nothing
/// at all.
#[test]
fn the_bounded_listing_reports_the_roots_it_could_not_read() {
    let world = World::new("summary-skipped");
    let run = settled(&world, "readable", vec![agent("build", &[])]);
    // llmlint: ignore-block[tests_mirror_real_usage] a run root with no launch record is
    // the filesystem state a crash between the directory and its record leaves, and no
    // command makes one; the run beside it is launched and settled through the CLI.
    std::fs::create_dir_all(world.runs.join("half-written")).expect("a run root with no launch");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let listing = Listing::of(&world.runs);
    assert_eq!(
        listing
            .summaries
            .iter()
            .map(|row| row.run_id.as_str())
            .collect::<Vec<_>>(),
        vec![run.as_str()]
    );
    assert_eq!(listing.skipped.len(), 1, "{:?}", listing.skipped);
    assert!(listing.skipped[0].path.ends_with("half-written"));
    assert!(
        listing.skipped[0]
            .reason
            .contains("a run root records the launch that owns it"),
        "the refused root carries no reason of its own: {:?}",
        listing.skipped[0]
    );
}

/// A summary this build cannot read is a run that **folds**, not a run that
/// vanishes.
///
/// Two shapes of the same answer: a document at a schema version this build does
/// not write, and one a writer left half-finished. Neither is an error to a
/// reader — the store beside it is intact, and the reading every listing did
/// before this document existed is still there — so the run lists, and the row
/// it lists with is the row its own journal produces.
#[test]
fn a_summary_this_build_cannot_read_folds_rather_than_taking_the_run_away() {
    let world = World::new("summary-unreadable");
    let run = settled(&world, "readable", vec![agent("build", &[])]);
    let paths = paths_of(&world, &run);
    let whole = RunSummary::of(&paths).expect("the run reads");

    // llmlint: ignore-block[tests_mirror_real_usage] neither state has a verb: this build
    // writes exactly the version it reads — that is what makes the first one a *newer*
    // build's document — and the second is the file a process killed mid-write leaves, not
    // an output any interface produces. The run beside them is launched and settled through
    // the CLI, and every claim is read through the public reader over the store it wrote.
    let document = std::fs::read_to_string(paths.summary()).expect("the document");
    let mut later: serde_json::Value = serde_json::from_str(&document).expect("a summary");
    later["schema_version"] = serde_json::json!(SUMMARY_SCHEMA_VERSION + 1);
    std::fs::write(paths.summary(), later.to_string()).expect("a later build's document");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let listing = Listing::of(&world.runs);
    assert!(
        listing.skipped.is_empty(),
        "a document this build cannot read took the run away: {:?}",
        listing.skipped
    );
    assert_eq!(
        listing.summaries.iter().find(|row| row.run_id == run),
        Some(&whole),
        "a run whose document a newer build wrote did not fold to the row its journal says"
    );

    // llmlint: ignore-block[tests_mirror_real_usage] as above: the half-written file a
    // killed process leaves is not a state any verb of this build produces.
    std::fs::write(paths.summary(), &document[..document.len() / 2])
        .expect("a document a writer left half-finished");
    // llmlint: ignore-end[tests_mirror_real_usage]
    assert_eq!(RunSummary::of(&paths).expect("the run folds"), whole);

    // And a document that reads perfectly and is about **another run**: a run
    // root copied aside keeps the copy's own name and the original's document,
    // and a reader that served it would answer every question about the copy
    // with the original's row.
    // llmlint: ignore-block[tests_mirror_real_usage] copying a run root is something an
    // operator does to a directory on disk — to keep one, to move one between hosts — and
    // there is no verb for it here; the directory copied is the one this build's own CLI
    // wrote.
    let copy = world.runs.join("copied-aside");
    copy_tree(&world.runs.join(&run), &copy);
    std::fs::write(copy.join("summary.json"), &document).expect("the original's document");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let copied = RunSummary::of(&RunPaths::under(&world.runs, "copied-aside"))
        .expect("the copied run reads");
    assert_eq!(
        copied.run_id, "copied-aside",
        "a document about another run was served for this one"
    );
}

/// Copy a run root, as an operator keeping one aside does.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("the copy's directory");
    for entry in std::fs::read_dir(from).expect("the run root") {
        let entry = entry.expect("an entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("a copied file");
        }
    }
}

/// A summary the store has since moved past is **refolded**, not served.
///
/// Staged the way it happens: a writer appends a record and does not get as far
/// as writing the document beside it — a driver killed between the two — so the
/// document on disk describes a store shorter than the one there now. The
/// journal's own recorded length is what says so.
#[test]
fn a_summary_the_store_has_moved_past_is_refolded_rather_than_served() {
    let world = World::new("summary-stale");
    let run = settled(&world, "moved-on", vec![agent("build", &[])]);
    let paths = paths_of(&world, &run);
    let before = std::fs::read_to_string(paths.summary()).expect("the document");
    let served = RunSummary::of(&paths).expect("the run reads");
    assert!(!served.stop_recorded);

    // A record the CLI really appends, so the store genuinely moves on.
    world
        .run(&["stop", &run, "--force"])
        .exited(0)
        .out_has(r#""stopped":true"#);

    // llmlint: ignore-block[tests_mirror_real_usage] what this stages is a writer that
    // appended and died before writing the document beside it, which is a killed process
    // rather than an interface — and the document put back is the one this build's own
    // writer wrote a moment earlier, not one invented here.
    std::fs::write(paths.summary(), &before).expect("the document that writer left");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let refolded = RunSummary::of(&paths).expect("the run reads");
    assert!(
        refolded.stop_recorded,
        "a document the store had moved past was served: {refolded:?}"
    );
    assert_eq!(refolded.event_count, served.event_count + 1);
    // And what was served is what a fold says, so the refold is the whole answer
    // rather than a patch on a stale one.
    recorded_before_the_document(&paths);
    assert_eq!(refolded, RunSummary::of(&paths).expect("the run folds"));

    // And the half a length cannot see: a store rewritten to its own size — the
    // shape a heal of a torn tail leaves — moves only its modification time, and
    // that is enough to say the document no longer describes it.
    // llmlint: ignore-block[tests_mirror_real_usage] a store healed back to a record
    // boundary by an append that met a dead writer's fragment is left by a *crash*, not by
    // an interface; what is written back here is the store's own bytes, unchanged.
    let store = std::fs::read(paths.journal()).expect("the store");
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    std::fs::write(paths.journal(), &store).expect("a store rewritten to its own length");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let after = RunSummary::of(&paths).expect("the run reads");
    let stamped = std::fs::metadata(paths.journal())
        .and_then(|about| about.modified())
        .expect("the store's modification time")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a time after the epoch")
        .as_millis() as u64;
    assert_eq!(after.journal_len, refolded.journal_len);
    assert_eq!(
        after.journal_mtime_ms, stamped,
        "a document written against a store that has since been rewritten to its \
         own length was served"
    );
}

/// A reader never sees a half-written summary.
///
/// One process lists the root continuously while **another** — the detached
/// driver the CLI launched — writes that run's summary again for every record it
/// appends. Every read has to answer either the state before that write or the
/// state after it: never a partial document, never a parse failure, never an
/// error. What makes that true is the write being a rename over the target
/// rather than a write through it, and this is what holds it true.
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] this journey costs 7.5s and
// is the *third* fastest of the six in this file — the same tier, the same binary, and the
// same real `onepipeline` subprocess every other journey under `tests/e2e/` drives. The
// deadline below is a failure bound rather than a cost: the run settles in a couple of
// seconds and the loop leaves the moment it does. The separately-edged project this
// repository does have, `onepipeline-note-journeys`, is edged on *conversational* cost —
// each of its journeys holds a two-party turn open — which is a different thing from wall
// time, and moving a seven-second journey behind that edge would put it where a change to
// this file does not run it.
#[test]
fn a_listing_beside_a_live_writer_never_reads_a_half_written_summary() {
    let world = World::new("summary-concurrent");
    // Enough nodes that the driver is writing the document continuously for as
    // long as this reader runs against it, and no more: what the journey needs
    // is the two processes overlapping, which a handful of dispatches already
    // gives it.
    let nodes: Vec<serde_json::Value> = (0..NODES)
        .map(|nth| agent(&format!("step{nth:02}"), &[]))
        .collect();
    let path = world.plan("live", &plan_of("live", nodes));
    world.run(&["start", &path, "--detach"]).exited(0);

    let paths = paths_of(&world, "live");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut reads = 0u32;
    let mut counted = 0u64;
    let mut settled = false;
    while std::time::Instant::now() < deadline {
        let listing = Listing::of(&world.runs);
        assert!(
            listing.skipped.is_empty(),
            "a run being written was refused rather than read: {:?}",
            listing.skipped
        );
        let row = listing
            .summaries
            .iter()
            .find(|row| row.run_id == "live")
            .unwrap_or_else(|| panic!("the live run left the listing: {listing:?}"));
        assert_eq!(row.schema_version, SUMMARY_SCHEMA_VERSION);
        // The store is append-only, so every answer is either the state this
        // reader last saw or one past it. A read that came back with less than
        // it had already been told is a document caught half-written.
        assert!(
            row.event_count >= counted,
            "a read went backwards, from {counted} records to {}",
            row.event_count
        );
        counted = row.event_count;
        reads += 1;
        if world.run_file("live", "result.json").is_file() {
            settled = true;
            break;
        }
    }
    assert!(settled, "the run never settled; {reads} reads made");
    assert!(
        reads > 20,
        "only {reads} reads ran against the writer, which is not a race"
    );
    // And the run that was being written all along ends where its own fold says.
    let served = RunSummary::of(&paths).expect("the run reads");
    recorded_before_the_document(&paths);
    assert_eq!(served, RunSummary::of(&paths).expect("the run folds"));
    assert_eq!(served.node_counts.get("done"), Some(&NODES));
}
