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

use onepipeline::views::{Listing, RunPaths, RunSummary, SUMMARY_SCHEMA_VERSION};

/// Launch a run and wait for it to settle, as every journey in `views.rs` does.
fn settled(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

/// Where the binary left one run's state.
fn paths_of(world: &World, run: &str) -> RunPaths {
    RunPaths::under(&world.runs, run)
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
    let run = settled(
        &world,
        "recorded",
        vec![agent("build", &[]), agent("ship", &["build"])],
    );
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
    assert_eq!(served.node_counts.get("done"), Some(&2));
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
    assert_eq!(served.timing.settled_done, 2);

    // The same row, folded from nothing: the document is taken away, so the
    // reader has no choice but the reading every listing did before it existed.
    std::fs::remove_file(paths.summary()).expect("the document");
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

    // llmlint: ignore-block[tests_mirror_real_usage] no verb of this build leaves a run
    // root without the document its journal writer maintains — that is what makes it an
    // older build's run — so the only way to hold one is to take the document off the run
    // this build's own CLI recorded.
    std::fs::remove_file(paths.summary()).expect("the document");
    // llmlint: ignore-end[tests_mirror_real_usage]

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

/// A reader never sees a half-written summary.
///
/// One process lists the root continuously while **another** — the detached
/// driver the CLI launched — writes that run's summary again for every record it
/// appends. Every read has to answer either the state before that write or the
/// state after it: never a partial document, never a parse failure, never an
/// error. What makes that true is the write being a rename over the target
/// rather than a write through it, and this is what holds it true.
#[test]
fn a_listing_beside_a_live_writer_never_reads_a_half_written_summary() {
    let world = World::new("summary-concurrent");
    // Enough nodes that the driver is writing the document continuously for as
    // long as this reader runs against it.
    let nodes: Vec<serde_json::Value> = (0..12)
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
    std::fs::remove_file(paths.summary()).expect("the document");
    assert_eq!(served, RunSummary::of(&paths).expect("the run folds"));
    assert_eq!(served.node_counts.get("done"), Some(&12));
}
