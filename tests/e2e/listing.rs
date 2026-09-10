//! The **listing** views: `runs`, `runs --mine`, and `status` given no run.
//!
//! These three answer out of each run's bounded summary document, and the rule
//! they are an instance of is that a **detail** read may fold a run's merged
//! event store and a **listing** may never. Before this, listing the runs one
//! session owns opened every run root under the runs root and folded every byte
//! every run on the host had ever recorded — 74.1 s to print one owned row over
//! 491 run roots holding 11 GB of journals, and the `--mine` filter ran after all
//! of it.
//!
//! So the journeys here are about what the process **did**, not about how long it
//! took: a run whose merged store is not on disk at all still lists, with every
//! fact it listed with. What each node of a run is doing is `status <RUN>`'s to
//! say, and that is a detail read which folds; `tests/e2e/views.rs` holds it.
//!
//! Everything is driven through the compiled binary. The only thing written by
//! hand is a run's own summary document, put into the three states a reader has
//! to meet — absent, stale, and at the schema a previous build wrote — each of
//! which is left by a *build* or a *crash* rather than by any verb.
//!
//! Two journeys hold the rule, and they hold different halves of it. The
//! portable one takes a run's store away and shows the output does not move,
//! which rules out a fold whose result is *used* — and would still pass a process
//! that opened the store and threw the bytes away. The other asks the **kernel**
//! what the process opened, over a root where every store is present and every
//! document current, and it is the one an ignored read fails: with an ignored
//! `journal::read` put on the listing path on purpose, the first journey passed
//! and this one failed naming the store it met.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary; `harness.rs` carries the same suppression and the full rationale. Every
// claim below is read off that binary's own stdout.

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] measured rather than
// assumed: every journey here but one drives a handful of runs, and the seven of them
// together run in about 35 seconds. The exception is the scale journey, which takes about
// 240 s and writes 10.7 GiB, because a bound about a host-sized runs root cannot be stated
// over a root that is not one. It sits in a binary that already holds three deliberately
// minute-long journeys in `loopcost.rs` and a whole module of them in `landing.rs`, on the
// same grounds those carry: what this exercises is `views`, `summary` and `ledger`, which
// any change under `src/` can move, so a project edged narrower than the crate would drop
// it out of `nx affected` for the very changes it exists to catch.

use serde_json::{json, Value};

use crate::harness::{agent, let_writeback_settle, lifecycle, plan_of, World};

use onepipeline::views::{RunPaths, SUMMARY_SCHEMA_VERSION};

const LISTINGS: [&[&str]; 3] = [&["runs"], &["runs", "--mine"], &["status"]];

fn settled(world: &World, name: &str, nodes: Vec<Value>) -> String {
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

fn listed(world: &World) -> Vec<String> {
    LISTINGS
        .iter()
        .map(|argv| {
            let rendered = world.run(argv);
            rendered.exited(0).stdout.clone()
        })
        .collect()
}

fn document(paths: &RunPaths) -> Value {
    serde_json::from_str(&std::fs::read_to_string(paths.summary()).expect("the document"))
        .expect("a summary document")
}

// llmlint: ignore-block[tests_mirror_real_usage] no verb writes this document — its run's
// own journal writer maintains it — and the three states below are left by a build or a
// crash rather than by an interface: a build that never wrote one, a writer killed between
// its append and the document beside it, and a build whose schema predates this one. The
// document put back is always the one this build's own writer wrote, edited only where the
// state under test is the edit.
fn put_back(paths: &RunPaths, document: &Value) {
    std::fs::write(paths.summary(), document.to_string()).expect("the document");
}

/// A journal's length and modification time, in the millisecond the crate stamps
/// a summary document with.
fn stamp_of(paths: &RunPaths) -> (u64, u64) {
    let about = std::fs::metadata(paths.journal()).expect("the run's merged store");
    let modified = about
        .modified()
        .expect("a modification time")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("an instant past the epoch")
        .as_millis();
    (
        about.len(),
        u64::try_from(modified).expect("a millisecond count"),
    )
}

/// Put something at the store's path that **cannot be read as a file**, leaving the
/// document current about it.
///
/// A directory, because it is the one form of "unreadable" every platform agrees on
/// and no privilege overrides: `open` refuses it everywhere, and `stat` still
/// answers, so the length and modification time the document is stamped with are
/// real readings of what is at that path. The document is re-stamped afterwards for
/// exactly that reason — it is this build's own document carrying the stamp this
/// build's own writer records for what the path now holds.
fn store_unreadable(paths: &RunPaths) {
    std::fs::remove_file(paths.journal()).expect("the run's merged store");
    std::fs::create_dir(paths.journal()).expect("something unreadable at the store's path");
    let (len, modified) = stamp_of(paths);
    let mut summary = document(paths);
    summary["journal_len"] = json!(len);
    summary["journal_mtime_ms"] = json!(modified);
    put_back(paths, &summary);
}

/// Take the run's merged event store away, and stamp its document for a store
/// that is not there.
///
/// `(0, 0)` is what this build stamps for a run whose journal it cannot stat,
/// which is a real state — a run root written before its first record — so a
/// document carrying it is **current** rather than stale. What that buys the
/// journey is the sharpest possible statement of the rule: the store is gone, and
/// anything still rendering read something else.
fn store_taken_away(paths: &RunPaths) {
    let mut summary = document(paths);
    summary["journal_len"] = json!(0);
    summary["journal_mtime_ms"] = json!(0);
    put_back(paths, &summary);
    std::fs::remove_file(paths.journal()).expect("the run's merged store");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A run whose merged event store is **not on disk** lists exactly as it did
/// with it.
///
/// The whole of "a listing never folds", established from what the process did
/// rather than from its wall clock: the bytes a fold would have read are not
/// there to read, and all three views print what they printed before. The
/// discriminator is on the same output — `results` over the same run *is* a
/// detail read, and it changes.
#[test]
fn the_listing_views_render_a_run_whose_merged_store_is_not_there() {
    let world = World::new("listing-storeless");
    world.script("build.work", "the worker wrote this\n");
    let run = settled(
        &world,
        "recorded",
        vec![agent("build", &[]), agent("docs", &["build"])],
    );
    let paths = paths_of(&world, &run);

    let before = listed(&world);
    for rendered in &before {
        assert!(
            rendered.contains(&run) && rendered.contains("2/2 done"),
            "the listing does not report the run it was asked about: {rendered}"
        );
    }
    let folded = world.run(&["results", &run]);
    let folded_before = folded.exited(0).stdout.clone();

    store_taken_away(&paths);
    assert!(
        !paths.journal().exists(),
        "the store this journey is about is still there"
    );

    assert_eq!(
        listed(&world),
        before,
        "a listing rendered differently once the store it must not read was gone"
    );
    // And the store really was where those facts had been coming from: the view
    // that folds no longer has them.
    let folded = world.run(&["results", &run]);
    let folded_after = folded.exited(0).stdout.clone();
    assert_ne!(
        folded_after, folded_before,
        "the detail read was unchanged by the store going away, so the listing \
         proved nothing: {folded_after}"
    );
}

/// Every listing renders a run whose merged store **cannot be read at all**,
/// exactly as it renders one whose store is readable — with the run's own document
/// current, and current *about that store*.
///
/// This is the direct proof of the rule, and it does not depend on a clock. A
/// journey that measures how long a listing takes over a bigger journal is asking a
/// stopwatch whether the journal was read; this asks the filesystem, which answers
/// outright. A listing that folded would open the store and get an error where its
/// records used to be, and would render a run with nothing in it; a listing that
/// reads the stored summary cannot tell the difference and renders what it rendered
/// before. There is no host speed, no ratio and no threshold in that.
///
/// **Unreadable, rather than absent**, which is what makes it sharper than
/// [`the_listing_views_render_a_run_whose_merged_store_is_not_there`] beside it:
/// removing the store changes what the document has to stamp — `(0, 0)`, the
/// reading for a run with no journal — while this leaves the stamp a real length
/// and a real modification time that the document still matches. So the row is
/// served on the ordinary path, the one every live run takes, rather than on the
/// path for a run whose store is missing.
///
/// The store is made unreadable by putting a **directory** where the records go: a
/// path `stat` still answers for, and that no process on any platform can read as a
/// file — not a privileged one, which is what a mode of `000` cannot promise. The
/// discriminator is on the same output: `results` over the same run *is* a detail
/// read, and it loses exactly what the listings keep.
#[test]
fn the_listing_views_render_a_run_whose_merged_store_cannot_be_read() {
    let world = World::new("listing-unreadable");
    world.script("build.work", "the worker wrote this\n");
    let run = settled(
        &world,
        "unreadable",
        vec![agent("build", &[]), agent("docs", &["build"])],
    );
    let paths = paths_of(&world, &run);

    let before = listed(&world);
    for rendered in &before {
        assert!(
            rendered.contains(&run) && rendered.contains("2/2 done"),
            "the listing does not report the run it was asked about: {rendered}"
        );
    }
    let folded_before = world.run(&["results", &run]).exited(0).stdout.clone();

    store_unreadable(&paths);
    assert!(
        std::fs::read(paths.journal()).is_err(),
        "the store this journey is about can still be read, so nothing below is a claim"
    );
    let document = document(&paths);
    assert_eq!(
        (
            document["journal_len"].as_u64(),
            document["journal_mtime_ms"].as_u64()
        ),
        {
            let (len, modified) = stamp_of(&paths);
            (Some(len), Some(modified))
        },
        "the document is not current about what is at the store's path, so this run \
         would be refolded and the journey would be about the fallback instead"
    );

    assert_eq!(
        listed(&world),
        before,
        "a listing rendered differently once the store it must not read could not be read"
    );
    // And the store really was where those facts would have come from: the view that
    // folds cannot render them any more.
    let folded_after = world.run(&["results", &run]).exited(0).stdout.clone();
    assert_ne!(
        folded_after, folded_before,
        "the detail read was unchanged by the store becoming unreadable, so the listings \
         proved nothing: {folded_after}"
    );
}

/// A run with **no** document, and one **stale** against its journal, render as a
/// run whose document is current does — and each leaves a current one behind.
#[test]
fn a_missing_document_and_a_stale_one_list_as_a_current_one_does() {
    let world = World::new("listing-refold");
    world.script("build.work", "the worker wrote this\n");
    let run = settled(&world, "refolded", vec![agent("build", &[])]);
    let paths = paths_of(&world, &run);

    let current = listed(&world);
    let written = document(&paths);

    // A run recorded by a build that never wrote one.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] no verb removes the document its run's
    // journal writer maintains, and none could: a build that never wrote one is the state
    // under test, and it is reached by taking away what this build wrote. Every claim after
    // it is read off the compiled binary's own stdout.
    std::fs::remove_file(paths.summary()).expect("the document");
    // llmlint: ignore-end[tests_mirror_real_usage]
    assert_eq!(
        listed(&world),
        current,
        "a run with no document listed differently from one with a current document"
    );
    assert!(
        paths.summary().is_file(),
        "the fold was not cached, so the next listing pays for it again"
    );
    assert_eq!(
        document(&paths),
        written,
        "the fold is not the row it caches"
    );

    // And one whose journal has moved past it: the length the writer recorded is
    // not the length the file holds.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] what this stages is a writer that
    // appended and died before writing the document beside it — a killed process rather
    // than an interface, and the state `RunSummary::of`'s staleness check exists for. The
    // document is this build's own with one recorded length moved, which is exactly what
    // that writer would have left.
    let mut stale = written.clone();
    stale["journal_len"] = json!(1);
    put_back(&paths, &stale);
    // llmlint: ignore-end[tests_mirror_real_usage]
    assert_eq!(
        listed(&world),
        current,
        "a stale document was served instead of refolded"
    );
    assert_eq!(
        document(&paths),
        written,
        "the stale document was left on disk for the next reader to refuse again"
    );
}

/// A document written by the build **before** this one is refused, refolded once,
/// and left behind at the version this build reads.
///
/// The fallback the schema version exists for. A schema 1 document carries none
/// of the three fields a row is now rendered from — which nodes are parked, which
/// a judge turned down, and each node's landing inputs — so reading it as one of
/// this build's would report every one of them as an absence nobody recorded.
#[test]
fn a_document_at_the_schema_before_this_build_is_refused_refolded_and_left_current() {
    let world = World::new("listing-older-schema");
    world.script("build.work", "the worker wrote this\n");
    let run = settled(&world, "older", vec![agent("build", &[])]);
    let paths = paths_of(&world, &run);

    let current = listed(&world);
    let written = document(&paths);
    assert_eq!(written["schema_version"], json!(SUMMARY_SCHEMA_VERSION));

    // The document as the previous build wrote it: at its own version, and
    // without the three keys it never had.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] the state under test is *a previous
    // build's output*, and no interface of this build produces one: the writer that
    // maintains this document only ever writes the schema it reads, so the only way to hold
    // a run recorded by the build before this one is to write what that build wrote. It is
    // written from this build's own document rather than invented — the version it carried,
    // less exactly the keys it did not have — and every claim after it is read off the
    // compiled binary's own stdout. `tests/golden/run-summary-v1.json` is the same document
    // pinned, and `src/summary.rs` holds the reader's refusal of it.
    let mut older = written.clone();
    older["schema_version"] = json!(1);
    for gone in ["parked", "judge_rejected", "landings"] {
        older.as_object_mut().expect("a document").remove(gone);
    }
    put_back(&paths, &older);
    // llmlint: ignore-end[tests_mirror_real_usage]

    assert_eq!(
        listed(&world),
        current,
        "a run whose document this build refuses did not refold to the same row"
    );
    let after = document(&paths);
    assert_eq!(
        after["schema_version"],
        json!(SUMMARY_SCHEMA_VERSION),
        "the refused document was left for the next listing to refuse again: {after}"
    );
    assert_eq!(after, written, "the refold is not the row it replaced");

    // And the next listing reads it without a fold: take the store away, and the
    // row is still there.
    store_taken_away(&paths);
    assert_eq!(
        listed(&world),
        current,
        "the document the refold left behind was not read on its own"
    );
}

/// The listing reports a driver this host can prove is gone, and one that is
/// merely quiet, under the two different words for them.
///
/// Both verdicts are read from the **host** as the row renders — neither is on
/// the document — and the pair is what a stored `ACTIVE` would have collapsed:
/// `PARKED` is a live pid with nothing happening, and `DRIVER DEAD` is a run
/// nothing holds. `tests/e2e/driver.rs` holds the same distinction on the detail
/// read.
#[test]
fn the_listing_tells_a_driver_this_host_can_prove_is_gone_from_one_merely_quiet() {
    let world = World::new("listing-liveness");
    // A run whose graph did **not** complete, because a completed one is
    // `SETTLED` whatever became of its driver — which is the distinction
    // `Standing::word` draws and `tests/e2e/views.rs` holds. This one lost a
    // node, so the word on its row is the liveness verdict.
    world.script("build.fail", "1");
    let run = settled(&world, "stopped", vec![agent("build", &[])]);
    world
        .run(&["stop", &run, "--force"])
        .exited(0)
        .out_has(r#""stopped":true"#);

    for argv in LISTINGS {
        let rendered = world.run(argv);
        rendered.exited(0).out_has(&run).out_has("DRIVER DEAD");
    }

    // The other one, on a run whose driver is alive and has stopped writing: the
    // dispatch is held open, so the pid proves ownership and not progress.
    let quiet = World::new("listing-parked").with_env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    quiet.script("build.wait", "hold");
    let path = quiet.plan("quiet", &plan_of("quiet", vec![agent("build", &[])]));
    quiet.run(&["start", &path, "--detach"]).exited(0);
    quiet.until("the listing to report the run parked", |world| {
        world.run(&["runs"]).stdout.contains("PARKED")
    });
    for argv in LISTINGS {
        let rendered = quiet.run(argv);
        rendered.exited(0).out_has("quiet").out_has("PARKED");
        assert!(
            !rendered.stdout.contains("DRIVER DEAD"),
            "a live driver that stopped writing was reported dead: {}",
            rendered.stdout
        );
        assert!(
            rendered.stdout.contains("adopt"),
            "a parked run was not told the way back: {}",
            rendered.stdout
        );
    }
    quiet.release("build.go");
}

/// `--mine` renders exactly the runs the reader's session owns, and the marker
/// and label say whose every row is.
///
/// The filter is the reason this whole path is bounded: it ran *after* every run
/// on the host had been folded, so asking for the handful a session owns cost
/// more than asking for all of them.
#[test]
fn mine_renders_the_runs_this_session_owns_and_says_whose_the_rest_are() {
    let world = World::new("listing-mine");
    // Named so that neither id is a substring of the other, nor of the
    // `[mine]` label the rows carry.
    let ours = settled(&world, "launched-here", vec![agent("build", &[])]);
    let stranger = world.as_session("another-planner");
    let theirs = settled(&stranger, "launched-elsewhere", vec![agent("build", &[])]);

    let all = world.run(&["runs"]);
    all.exited(0)
        .out_has(&ours)
        .out_has(&theirs)
        .out_has("[mine]");
    assert!(
        all.stdout.contains(&format!("* {ours}")),
        "the run this session owns carries no ownership marker: {}",
        all.stdout
    );

    let owned = world.run(&["runs", "--mine"]);
    owned.exited(0).out_has(&ours);
    assert!(
        !owned.stdout.contains(&theirs),
        "--mine rendered a run this session does not own: {}",
        owned.stdout
    );

    // And from the other side, so the marker is about the reader rather than
    // about the run: the same two runs, read by the session that owns the other.
    let owned = stranger.run(&["runs", "--mine"]);
    owned.exited(0).out_has(&theirs);
    assert!(
        !owned.stdout.contains(&ours),
        "--mine rendered a run this session does not own: {}",
        owned.stdout
    );
}

/// How many run roots the scale journey assembles.
///
/// Four hundred, because that is the shape the bound is about: a supervisory
/// host accumulates run roots and never sheds them, and the one measured when
/// this was written held 491.
const SCALED_RUNS: usize = 400;

/// How many bytes of journal those roots hold together at the first measurement.
///
/// One gibibyte, spread evenly. The host this was written on held 11 GB across
/// 491 roots; a gibibyte is the smallest total at which a fold is unmistakably
/// the thing being paid for rather than process start.
const SCALED_JOURNAL_BYTES: u64 = 1 << 30;

/// What the second measurement multiplies that total by, with the run count held
/// fixed.
///
/// The two measurements together are the claim: the first says the listing is
/// fast, and the second says what it is fast *because of* — a tenfold change in
/// the one input a fold is linear in barely moves it.
const JOURNAL_MULTIPLE: u64 = 10;

/// How many of those runs the reading session owns.
///
/// **A handful**, because that is the shape the cost was wrong about: the filter
/// ran after every run on the host had been folded, so asking for three rows out
/// of four hundred cost more than asking for all four hundred.
const SCALED_OWNED: usize = 3;

/// The tally every row cloned from the **bulk** template renders.
///
/// Asserted rather than assumed, and this is the assertion the whole bound rests
/// on. A fixture can go degenerate — a clone that carried no node state would
/// render `0/0 done`, and every other assertion in the journey would pass and
/// pass *faster*, reporting a bound met over rows with nothing in them. That is
/// not hypothetical: it cost this work an hour over a hand-built root whose
/// journals were named wrong, where the rows read `0/0 done` and the folding
/// build looked like 0.29 s against the 271.7 s it really took.
const BULK_TALLY: &str = "2/3 done";

/// The tally every row cloned from the **owned** template renders.
///
/// Two nodes, one of which published a change: the other half of the same guard.
const OWNED_TALLY: &str = "1/2 done";

/// The clauses a row carrying a landing to decide can end with.
///
/// Either is a **verdict reached as the row rendered** — the base does not carry
/// the change, or nothing this host can read decides it — and a row that carried
/// no landing at all would carry neither. Both are admitted because which one it
/// is depends on whether the branch the template published is still resolvable
/// when the row renders, and that is the repository's business rather than this
/// journey's claim.
const LANDING_VERDICTS: [&str; 2] = ["not landed", "landing undecided"];

/// What each of the three renders may take over the unmultiplied root.
///
/// **A bound rather than a ratio**, because it is what a fold cannot meet: the
/// same question over a comparable root took 74.1 s on the host this was measured
/// on. It is generous against what the listing actually costs, deliberately — a
/// threshold on wall clock is a fact about the host's load as much as about the
/// code, and this one is set where only a return to folding could cross it.
const LISTING_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

/// A **regression guard**, and deliberately not the evidence for anything.
///
/// Both medians are tens of milliseconds, so the ratio between them measures the
/// host as much as the code: on unchanged code these journeys have given ratios
/// from 0.42x to 2.72x, the grown fixture sometimes the faster of the two. Five
/// clears the worst of that noise and still fails a fold, which is three to four
/// orders of magnitude — `runs --mine` over a comparable root took 74.1 s before
/// this work. A 300 ms floor stood here instead and made the bar 600 ms at these
/// medians, which a tenfold regression walks through; ruled 2026-09-10.
///
/// What proves the journal goes unread is no clock at all:
/// [`the_listing_views_render_a_run_whose_merged_store_cannot_be_read`] and its
/// counterpart over the verb render over a store that cannot be read.
const GROWTH_GUARD: u32 = 5;

/// How many renders each median is taken over, after one warm-up.
///
/// The warm-up is discarded because what it measures is mostly a debug binary
/// nobody has paged in, and a median rather than a mean because the outlier this
/// has to survive is another test on the same host, not a slow render.
const TIMED: usize = 5;

/// One run root cloned from a real one, under a new id and a new owner.
///
/// Everything but the journal is the template's own file, because everything but
/// the journal is what a listing reads: the launch record and the summary
/// document are this build's own, written by a real run driven through the
/// binary, and only the two facts that must name *this* run rather than the one
/// it was copied from are rewritten.
// llmlint: ignore-block[tests_mirror_real_usage] four hundred run roots is a *host*, not a
// command: no verb makes one and the only honest way to hold the shape is to assemble it.
// What is assembled is this build's own output — one real run driven through the compiled
// binary, cloned — rather than documents invented here.
fn cloned_run(template: &RunPaths, root: &std::path::Path, id: &str, session: &str) -> RunPaths {
    let paths = RunPaths::under(root, id);
    std::fs::create_dir_all(&paths.dir).expect("a run root");
    let journal = template
        .journal()
        .file_name()
        .expect("the journal has a name")
        .to_owned();
    for entry in std::fs::read_dir(&template.dir).expect("the template run") {
        let entry = entry.expect("an entry of the template run");
        if !entry.file_type().expect("its kind").is_file() || entry.file_name() == journal {
            continue;
        }
        std::fs::copy(entry.path(), paths.dir.join(entry.file_name())).expect("a copied file");
    }
    for path in [paths.launch(), paths.summary()] {
        let mut held: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("a copied document"))
                .expect("a document");
        held["run_id"] = json!(id);
        held["session"] = json!(session);
        // A pid means nothing across machines, so a run recorded elsewhere reads
        // as the live work it is — which is the **expensive** row to render, the
        // one that goes on to ask whether anything is watching the run.
        held["host"] = json!("another-host");
        std::fs::write(&path, held.to_string()).expect("the document");
    }
    paths
}

/// Grow one run's journal to `bytes` and stamp its document for the store that
/// leaves.
///
/// The filler is the template run's **own records**, repeated: real journal
/// lines, so a fold of one of these would do the work a fold of a real run does.
/// The stamp is written after the bytes, so what the document claims is what the
/// file holds — which is what makes it *current*, and the state the bound is
/// about.
fn journal_of(paths: &RunPaths, filler: &[u8], bytes: u64) {
    use std::io::Write;
    let mut file = std::io::BufWriter::new(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.journal())
            .expect("the run's journal"),
    );
    let mut held = std::fs::metadata(paths.journal()).map_or(0, |about| about.len());
    while held < bytes {
        file.write_all(filler).expect("journal records");
        held += filler.len() as u64;
    }
    file.flush().expect("journal records");
    // Forced to the device before this returns, rather than left as dirty pages
    // for the kernel to write back **while the next measurement runs**. Ten
    // gibibytes of writeback competing with the reads being timed is the host's
    // clock, not the listing's, and it moved a 27 ms median to 88 ms without the
    // binary doing anything differently.
    file.get_ref().sync_all().expect("journal records on disk");
    drop(file);

    let (len, modified) = stamp_of(paths);
    let mut summary = document(paths);
    summary["journal_len"] = json!(len);
    summary["journal_mtime_ms"] = json!(modified);
    put_back(paths, &summary);
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// One invocation's stdout, taken **without** the harness's own look at the
/// world.
///
/// `World::run` captures a dump of every run root beside the output, which is
/// exactly the read this journey is about the binary no longer making — over four
/// hundred roots it is the harness that would be reading the gigabyte, and every
/// figure below would be its.
fn rendered(world: &World, argv: &[&str]) -> String {
    let out = world.cmd(argv).output().expect("the binary runs");
    assert!(
        out.status.success(),
        "`onepipeline {}` exited {:?}: {}",
        argv.join(" "),
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn median(world: &World, argv: &[&str]) -> std::time::Duration {
    let mut took: Vec<std::time::Duration> = Vec::with_capacity(TIMED);
    for nth in 0..=TIMED {
        let began = std::time::Instant::now();
        let out = world.cmd(argv).output().expect("the binary runs");
        let elapsed = began.elapsed();
        assert!(
            out.status.success(),
            "`onepipeline {}` exited {:?}: {}",
            argv.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        // The first is the warm-up: what it measures is mostly a debug binary
        // nobody has paged in.
        if nth > 0 {
            took.push(elapsed);
        }
    }
    took.sort_unstable();
    println!("  {:<16} {took:?}", argv.join(" "));
    took[TIMED / 2]
}

/// Over a host-sized runs root, each of the three renders in seconds.
///
/// **The bound is the claim**: 74.1 s was measured for `runs --mine` over a
/// comparable root before this work, and the bound is set an order of magnitude
/// below that, so only a return to folding can cross it. Both measurements are held
/// to it, over the unmultiplied root and over ten times the journal bytes.
///
/// The ratio between those two is kept as [`GROWTH_GUARD`] and is **not** evidence
/// that the journal goes unread; that constant says why, and
/// [`the_listing_views_render_a_run_whose_merged_store_cannot_be_read`] is where the
/// property is actually proven — over a store that cannot be read at all, which no
/// clock enters into.
///
/// **What the rows are is asserted, not assumed.** A clock over a degenerate
/// fixture is the one way this journey could report green while measuring
/// nothing, so before it times anything it reads the rendered rows back and holds
/// them to the shape the bound is set against: every row carries a real node
/// tally folded out of a real graph, and the rows `--mine` renders — the
/// invocation the whole 74.1 s figure was about — each carry a landing **decided
/// as the row rendered**, which is the per-row work a listing keeps paying. See
/// [`BULK_TALLY`] for what that guard is for.
#[test]
fn a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it() {
    let world = World::new("listing-scale");
    // A repository, so the owned template's node can publish a change and leave
    // it open: `change-open` is the policy that settles a node `done` with its
    // change unlanded and a person left to merge it, which is what puts a landing
    // on the row for the render to decide.
    let repository = world.repository("change-open", &[]);
    world.script("build.fail", "1");
    // The published node has to leave a diff, or there is nothing to publish and
    // the settlement records no landing at all — which is a fixture that renders
    // a tally and no verdict, and is what the guard below caught the first time
    // it ran.
    world.script("publish.work", "the change this run published\n");

    // Two templates, because the rows have two jobs. The bulk one is a graph that
    // did not complete — so every row cloned from it is reported as being driven
    // and goes on to ask whether anything is watching the run — and the owned one
    // adds a published change, so the rows `--mine` renders each cost a landing
    // read as they render. All four hundred rows would be the second if the
    // repository could be asked four hundred times inside a bound; it cannot, and
    // a fixture whose clock was dominated by a read both the old path and the new
    // one pay identically would be a bound about the wrong thing.
    let bulk = paths_of(
        &world,
        &settled(
            &world,
            "bulk",
            vec![agent("build", &[]), agent("docs", &[]), agent("ship", &[])],
        ),
    );
    let owned = paths_of(
        &world,
        &settled(
            &world,
            "owned",
            vec![agent("build", &[]), lifecycle("publish", &[])],
        ),
    );
    // Each template's records, read **before** it leaves the root and carried
    // with it. A clone's journal is grown out of its own template's records
    // rather than out of one template's for every root, because a fixture whose
    // store describes one graph and whose document describes another is two
    // fixtures: the folding build reads the store and this one reads the
    // document, and a figure comparing them would not be the same work done
    // twice. Paired with the template it came from so the two cannot be handed
    // out separately.
    let records = |template: &RunPaths| {
        let filler = std::fs::read(template.journal()).expect("the template's own records");
        assert!(
            !filler.is_empty(),
            "the {} template recorded nothing",
            template.run
        );
        filler
    };
    let bulk_template = (&bulk, records(&bulk));
    let owned_template = (&owned, records(&owned));
    // The templates are the shape the rows will be, before a single one is
    // cloned: a fixture that never had the tally cannot clone one.
    for (template, tally) in [(&bulk, BULK_TALLY), (&owned, OWNED_TALLY)] {
        let row = rendered(&world, &["runs"]);
        assert!(
            row.lines()
                .any(|line| line.contains(&template.run) && line.contains(tally)),
            "the {} template does not render {tally}, so no row cloned from it will: {row}",
            template.run
        );
    }

    let per_run = SCALED_JOURNAL_BYTES / SCALED_RUNS as u64;
    let stranger = "another-planner";
    let mut assembled: Vec<(RunPaths, &[u8])> = Vec::new();
    for nth in 0..SCALED_RUNS {
        // The rows this session owns are the ones carrying a landing, so the
        // invocation the cost was worst on is the one whose every row does the
        // per-row work.
        let ((template, filler), owner) = if nth < SCALED_OWNED {
            (&owned_template, world.session.as_str())
        } else {
            (&bulk_template, stranger)
        };
        let paths = cloned_run(template, &world.runs, &format!("scaled-{nth:04}"), owner);
        journal_of(&paths, filler, per_run);
        assembled.push((paths, filler));
    }
    // llmlint: ignore-block[tests_mirror_real_usage] the templates are the journey's own
    // scaffolding rather than two of the four hundred roots it measures, and leaving them on
    // the root would make both the run count and the owned count wrong. No verb sweeps a run
    // root, and one that did would be a different journey. The checkout below is a **host**
    // state rather than a record: a machine that no longer holds the repository a run
    // published to is what a run recorded elsewhere looks like from here, and it is the
    // state `Stands::Undecided` exists for.
    for template in [&bulk, &owned] {
        std::fs::remove_dir_all(&template.dir).expect("the template run root");
    }
    // The repository the owned rows published to, gone from this host.
    //
    // The rows keep their landing and every render still decides it — which is
    // the per-row work this journey has to be measuring rows that do. What
    // changes is the *answer*: a host that cannot reach the repository says so
    // rather than claiming the change did or did not land, and it says so in
    // about a millisecond. Left in place, three real repository walks are the
    // whole of the clock — 215 ms of a 275 ms render — and they go cold when ten
    // gibibytes are written beside them, so the ratio below would be a reading of
    // this host's page cache rather than of the one input a fold is linear in.
    std::fs::remove_dir_all(&repository.checkout).expect("the repository's checkout");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let held = |assembled: &[(RunPaths, &[u8])]| -> u64 {
        assembled
            .iter()
            .map(|(paths, _)| std::fs::metadata(paths.journal()).map_or(0, |about| about.len()))
            .sum()
    };
    let bytes = held(&assembled);
    assert!(
        bytes >= SCALED_JOURNAL_BYTES,
        "the root holds {bytes} journal byte(s), short of the {SCALED_JOURNAL_BYTES} this \
         measures over"
    );

    let owned_rows = rendered(&world, &["runs", "--mine"]);
    assert_eq!(
        owned_rows.lines().count(),
        SCALED_OWNED,
        "--mine over {SCALED_RUNS} run roots rendered something other than the {SCALED_OWNED} \
         this session owns"
    );
    for row in owned_rows.lines() {
        assert!(
            row.contains(OWNED_TALLY),
            "an owned row does not carry the node tally the bound is set against, so this \
             fixture is not the shape it measures: {row}"
        );
        assert!(
            LANDING_VERDICTS.iter().any(|verdict| row.contains(verdict)),
            "an owned row carries no landing verdict, so nothing on this root costs the \
             per-row read a listing keeps paying — one of {LANDING_VERDICTS:?} was expected: \
             {row}"
        );
    }

    // Said out loud, because the whole bound below is a clock over these rows and
    // a reader who cannot see what they are cannot weigh it.
    println!(
        "  the rows this bound is measured over:\n    {}\n    {}",
        owned_rows.lines().next().unwrap_or("(no owned row)"),
        rendered(&world, &["runs"])
            .lines()
            .find(|row| row.contains(BULK_TALLY))
            .unwrap_or("(no bulk row)")
    );

    // And every row on the root, owned or not, is a row folded out of a real
    // graph rather than an empty one.
    let all_rows = rendered(&world, &["runs"]);
    assert_eq!(all_rows.lines().count(), SCALED_RUNS);
    for row in all_rows.lines() {
        assert!(
            row.contains(BULK_TALLY) || row.contains(OWNED_TALLY),
            "a row carries neither tally, so the fixture has gone degenerate and the bound \
             below would be a clock over nothing: {row}"
        );
        assert!(
            !row.contains("0/0 done"),
            "a row reports an empty graph, which is what a clone that stopped carrying node \
             state looks like: {row}"
        );
    }

    // And the store under each root describes the **same graph** its document
    // does, asked of this build's own folding path.
    //
    // `status <RUN>` is a detail read: it folds the run's merged store and says
    // what that fold makes of it, where `status` given no run reads the bounded
    // document beside it. So one run rendered both ways is the two builds this
    // journey's figures compare — the folding one and this one — put over the
    // same root, and a line that matches is the statement that they are reading
    // one graph. Without it the clock below is honest about its own build and
    // says nothing about what it is being compared against: a clone carrying one
    // template's document and another's records renders `1/2 done` here and
    // `2/3 done` there, both green, neither the same work.
    let listing = rendered(&world, &["status"]);
    for nth in [0, SCALED_OWNED] {
        let run = format!("scaled-{nth:04}");
        let folded = rendered(&world, &["status", &run]);
        let folded = folded.lines().next().expect("the run's own line");
        let read = listing
            .lines()
            .find(|line| line.starts_with(&run))
            .expect("the listing's line for the same run");
        assert_eq!(
            folded, read,
            "{run} reads as one run to a fold of its store and as another to the document              beside it, so the two builds this journey times are not reading one graph"
        );
    }

    // Before the first measurement, so both are taken over a disk that has finished
    // with the fixture rather than one still writing it: what is being compared is
    // the render, and the ten gibibytes written below would otherwise be in the
    // second figure and not the first.
    let_writeback_settle();
    binary_in_cache();

    let mut before = Vec::new();
    for argv in LISTINGS {
        let took = median(&world, argv);
        assert!(
            took < LISTING_BOUND,
            "`onepipeline {}` took {took:?} over {SCALED_RUNS} run roots holding {bytes} \
             journal byte(s), past the {LISTING_BOUND:?} a listing is held to",
            argv.join(" ")
        );
        before.push(took);
    }

    // The same root, with the one input a fold is linear in multiplied by ten and
    // the run count held exactly where it was.
    for (paths, filler) in &assembled {
        // Ten times **this journal's own** length rather than ten times the
        // even share, so the total that comes out is ten times the total that
        // went in rather than ten times what was aimed at. Grown out of the
        // records this root's own documents describe, for the reason they were
        // paired above.
        let held = std::fs::metadata(paths.journal())
            .expect("the journal")
            .len();
        journal_of(paths, filler, held * JOURNAL_MULTIPLE);
    }
    // The documents the listing reads, back in the cache the first measurement
    // found them in. Writing ten gibibytes evicts six hundred kilobytes of small
    // files, and a median taken over cold documents is a reading of the host's
    // page cache rather than of the listing: what the ratio below is about is the
    // one input a fold is linear in, and the cache state either measurement
    // happens to start in is not it.
    for (paths, _) in &assembled {
        for document in [paths.summary(), paths.launch()] {
            let _ = std::fs::read(document);
        }
    }
    // And again, for the write that just happened — this is the one the ratio is
    // about, and `fsync` per file only puts that file's pages on the device.
    let_writeback_settle();
    let grown = held(&assembled);
    assert!(
        grown >= bytes * JOURNAL_MULTIPLE,
        "the grown root holds {grown} journal byte(s), short of {JOURNAL_MULTIPLE} times the \
         {bytes} it held"
    );
    assert_eq!(
        rendered(&world, &["runs"]).lines().count(),
        SCALED_RUNS,
        "the run count did not stay where it was"
    );

    binary_in_cache();
    for (argv, was) in LISTINGS.iter().zip(before) {
        let took = median(&world, argv);
        assert!(
            took <= was * GROWTH_GUARD,
            "`onepipeline {}` took {took:?} over {grown} journal byte(s) against {was:?} over \
             {bytes} — {JOURNAL_MULTIPLE} times the bytes moved it past the {GROWTH_GUARD}x \
             regression guard a bounded read is kept under",
            argv.join(" ")
        );
        // And the grown root is held to the same absolute bound the unmultiplied
        // one is, which is the statement that does not depend on a ratio at all: a
        // fold of ten gibibytes cannot come in under five seconds.
        assert!(
            took < LISTING_BOUND,
            "`onepipeline {}` took {took:?} over {grown} journal byte(s), past the \
             {LISTING_BOUND:?} a listing is held to",
            argv.join(" ")
        );
        println!(
            "  {:<12} {was:?} over {bytes} journal byte(s), {took:?} over {grown}",
            argv.join(" ")
        );
    }
}

/// `status` given no run renders the run-level lines and **leaves the per-node
/// block to `status <RUN>`**.
///
/// The one thing this verb stopped printing without a run named, said out loud:
/// a per-node block for every run on a host is a fold of every store on it,
/// which is the cost the listing exists to remove. It is not lost — it is where
/// a detail read is, and the same invocation with the run named still prints it.
#[test]
fn status_given_no_run_leaves_the_per_node_block_to_status_given_one() {
    let world = World::new("listing-detail");
    // A dispatch held open, so the run has a node the detail read has something
    // to say about: `status <RUN>` ages a running node and names what it is
    // doing.
    world.script("build.wait", "hold");
    let path = world.plan("detailed", &plan_of("detailed", vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the node to be dispatched", |world| {
        !world.events_of("detailed", "node-dispatched").is_empty()
    });

    let detail = world.run(&["status", "detailed"]);
    detail
        .exited(0)
        .out_has("detailed")
        .out_has("build: running for");

    let listed = world.run(&["status"]);
    listed.exited(0).out_has("detailed").out_has("0/1 done");
    assert!(
        !listed.stdout.contains("build: running for"),
        "`status` given no run printed a per-node line, which is the fold this \
         path must not make: {}",
        listed.stdout
    );
    // And the run-level line itself is the same line either way, which is what
    // makes the split a move rather than a loss.
    let header = |rendered: &str| {
        rendered
            .lines()
            .find(|line| line.starts_with("detailed"))
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(
        header(&listed.stdout),
        header(&detail.stdout),
        "the two reads disagree about the run itself"
    );
    world.release("build.go");
}

/// Every path the process **opened**, as the kernel recorded it.
///
/// The command is the one `World` composes — same binary, same environment —
/// with the tracer wrapped around it, so what is observed is the invocation a
/// user makes rather than a second one assembled here.
///
/// It **refuses** rather than passes where the tracer will not run: an
/// observation nobody made is not an observation of nothing, and this is the one
/// journey whose whole claim is about what the process did.
#[cfg(target_os = "linux")]
fn opened_by(world: &World, argv: &[&str], into: &std::path::Path) -> Vec<String> {
    let inner = world.cmd(argv);
    let mut traced = std::process::Command::new("strace");
    traced
        // Children too: a listing that shelled out to read a store would have
        // read it just the same.
        .arg("-f")
        .arg("-qq")
        .arg("-e")
        .arg("trace=openat,open")
        .arg("-o")
        .arg(into)
        .arg(inner.get_program())
        .args(inner.get_args())
        .stdin(std::process::Stdio::null());
    for (key, value) in inner.get_envs() {
        match value {
            Some(value) => traced.env(key, value),
            None => traced.env_remove(key),
        };
    }
    let observed = traced.output().unwrap_or_else(|error| {
        panic!(
            "this journey's whole claim is what the process opened, and the tracer would \
             not run: strace: {error}. Install strace, or run the suite where ptrace is \
             permitted — a journey that cannot observe is not a journey that observed \
             nothing."
        )
    });
    assert!(
        observed.status.success(),
        "`strace onepipeline {}` exited {:?}: {}",
        argv.join(" "),
        observed.status.code(),
        String::from_utf8_lossy(&observed.stderr)
    );
    let trace = std::fs::read_to_string(into).expect("the trace the tracer wrote");
    // Each line names its path in the first quoted field, and a line carrying no
    // path is a resumption or a signal rather than an open.
    let opened: Vec<String> = trace
        .lines()
        .filter_map(|line| line.split_once('"'))
        .filter_map(|(_, rest)| rest.split_once('"'))
        .map(|(path, _)| path.to_owned())
        .collect();
    assert!(
        !opened.is_empty(),
        "the tracer recorded no open at all, so it observed nothing: {trace}"
    );
    opened
}

/// The three listing commands open **no run's merged event store**, over a root
/// where every store is present and every document is current.
///
/// What this holds that
/// [`the_listing_views_render_a_run_whose_merged_store_is_not_there`] cannot.
/// That journey takes the store away and shows the output does not move, which
/// rules out a fold whose result is *used* — and it would still pass a process
/// that opened the store and threw the bytes away. On a host holding eleven
/// gigabytes of journals, throwing them away is the whole cost. So this one asks
/// the **kernel** what the process opened, over the state a listing meets on a
/// live host: the stores are all there, and nothing about them is stale.
///
/// The **positive control** is what makes the silence a measurement: `results`
/// over one of the same runs is a detail read, is traced the same way in the same
/// journey, and does open the store. A tracer that saw nothing at all would pass
/// the first half and fail the second.
///
/// **Linux, and deliberately not skipped anywhere.** There is no portable way to
/// ask another process what it opened, so the observation is made on the platform
/// this crate's deterministic tier and coverage floor are measured on, where it
/// runs every time and refuses if it cannot; every other platform holds the same
/// rule through the store-removal journey above, which is weaker and portable.
#[cfg(target_os = "linux")]
#[test]
fn no_listing_command_opens_a_run_store_that_is_there() {
    let world = World::new("listing-traced");
    world.script("build.work", "the worker wrote this\n");
    let recorded: Vec<String> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|name| settled(&world, name, vec![agent("build", &[])]))
        .collect();

    // One listing first, so every document is current: a stale one is refolded,
    // and this journey would then be about a fold rather than about its absence.
    listed(&world);
    for run in &recorded {
        let paths = paths_of(&world, run);
        assert!(
            paths.journal().is_file(),
            "{run}'s merged store is not there, which is the other journey's premise"
        );
        let document = document(&paths);
        let (len, modified) = stamp_of(&paths);
        assert_eq!(
            (
                document["journal_len"].as_u64(),
                document["journal_mtime_ms"].as_u64()
            ),
            (Some(len), Some(modified)),
            "{run}'s document is stale against its journal, so a fold here would be \
             correct and this journey would be about nothing"
        );
    }

    for (nth, argv) in LISTINGS.iter().enumerate() {
        let opened = opened_by(&world, argv, &world.root.join(format!("trace-{nth}.txt")));
        for run in &recorded {
            let paths = paths_of(&world, run);
            let summary = paths.summary().display().to_string();
            assert!(
                opened.contains(&summary),
                "`onepipeline {}` never opened {run}'s summary document, so this trace is \
                 not of the read under test: {opened:?}",
                argv.join(" ")
            );
            let journal = paths.journal().display().to_string();
            assert!(
                !opened.contains(&journal),
                "`onepipeline {}` opened {journal}: a listing may not read a run's merged \
                 event store",
                argv.join(" ")
            );
        }
        // And nothing named like one, however it was reached — a store opened
        // through a relative path or another run's root is the same read.
        assert!(
            !opened
                .iter()
                .any(|path| std::path::Path::new(path).file_name()
                    == std::path::Path::new(&paths_of(&world, &recorded[0]).journal()).file_name()),
            "`onepipeline {}` opened a run's merged event store: {opened:?}",
            argv.join(" ")
        );
    }

    // The control. `results` folds by design, so it opens exactly what the three
    // above must not — which is what says the tracer was watching this binary's
    // opens rather than recording an empty room.
    let paths = paths_of(&world, &recorded[0]);
    let opened = opened_by(
        &world,
        &["results", &recorded[0]],
        &world.root.join("trace-detail.txt"),
    );
    let journal = paths.journal().display().to_string();
    assert!(
        opened.contains(&journal),
        "`results` did not open the store it folds, so nothing above was observed: {opened:?}"
    );
}

/// Put the binary back in the page cache before a measurement.
///
/// Measurement hygiene, and stated as no more than that: both measurements in a
/// journey read the binary first, so neither begins from a cache state the other
/// did not have. **What that is worth is unestablished** — no run has been taken
/// with this and without it, all else equal — and nothing this suite asserts rests
/// on it. It is here because a quarter-gigabyte binary and a ten-gibibyte fixture
/// share one page cache, and a measurement that begins by not knowing which of the
/// two is resident is one nobody can read.
fn binary_in_cache() {
    let read = std::fs::read(crate::harness::binary()).expect("the binary under test");
    assert!(!read.is_empty(), "the binary under test is empty");
}
