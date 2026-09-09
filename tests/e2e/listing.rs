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

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary; `harness.rs` carries the same suppression and the full rationale. Every
// claim below is read off that binary's own stdout.

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] measured rather than
// assumed: four of the five journeys here drive one run each and the module's cheap half
// runs in about 16 seconds, and the fifth — the scale journey — takes about 210 s and
// writes 10.7 GiB, because a bound about a host-sized runs root cannot be stated over a
// root that is not one. It sits in a binary that already holds three deliberately
// minute-long journeys in `loopcost.rs` and a whole module of them in `landing.rs`, on the
// same grounds those carry: what this exercises is `views`, `summary` and `ledger`, which
// any change under `src/` can move, so a project edged narrower than the crate would drop
// it out of `nx affected` for the very changes it exists to catch.

use serde_json::{json, Value};

use crate::harness::{agent, plan_of, World};

use onepipeline::views::{RunPaths, SUMMARY_SCHEMA_VERSION};

/// The three invocations this module is about, in the order a report states them.
const LISTINGS: [&[&str]; 3] = [&["runs"], &["runs", "--mine"], &["status"]];

/// Launch a run and wait for it to settle.
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

/// What the three listing views render, in one value.
fn listed(world: &World) -> Vec<String> {
    LISTINGS
        .iter()
        .map(|argv| {
            let rendered = world.run(argv);
            rendered.exited(0).stdout.clone()
        })
        .collect()
}

/// One run's summary document, as JSON.
fn document(paths: &RunPaths) -> Value {
    serde_json::from_str(&std::fs::read_to_string(paths.summary()).expect("the document"))
        .expect("a summary document")
}

/// Put a document back, exactly as given.
// llmlint: ignore-block[tests_mirror_real_usage] no verb writes this document — its run's
// own journal writer maintains it — and the three states below are left by a build or a
// crash rather than by an interface: a build that never wrote one, a writer killed between
// its append and the document beside it, and a build whose schema predates this one. The
// document put back is always the one this build's own writer wrote, edited only where the
// state under test is the edit.
fn put_back(paths: &RunPaths, document: &Value) {
    std::fs::write(paths.summary(), document.to_string()).expect("the document");
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
    // llmlint: ignore[tests_mirror_real_usage] the reason is on `put_back`: no verb removes
    // the document its run's journal writer maintains, and a build that predates the
    // document is the state under test.
    std::fs::remove_file(paths.summary()).expect("the document");
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
    let mut stale = written.clone();
    stale["journal_len"] = json!(1);
    put_back(&paths, &stale);
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
    let mut older = written.clone();
    older["schema_version"] = json!(1);
    for gone in ["parked", "judge_rejected", "landings"] {
        older.as_object_mut().expect("a document").remove(gone);
    }
    put_back(&paths, &older);

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
const SCALED_OWNED: usize = 3;

/// What each of the three renders may take over the unmultiplied root.
///
/// **A bound rather than a ratio**, because it is what a fold cannot meet: the
/// same question over a comparable root took 74.1 s on the host this was measured
/// on. It is generous against what the listing actually costs, deliberately — a
/// threshold on wall clock is a fact about the host's load as much as about the
/// code, and this one is set where only a return to folding could cross it.
const LISTING_BOUND: std::time::Duration = std::time::Duration::from_secs(5);

/// What the same render may take once those journals hold ten times as much.
///
/// Twice its own median over the unmultiplied root. Whatever the host was doing
/// is in both figures, so this compares the listing with itself rather than with
/// a clock.
const GROWTH_BOUND: u32 = 2;

/// How many renders each median is taken over, after one warm-up.
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

    let about = std::fs::metadata(paths.journal()).expect("the journal");
    let modified = about
        .modified()
        .expect("a modification time")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("an instant past the epoch")
        .as_millis();
    let mut summary = document(paths);
    summary["journal_len"] = json!(about.len());
    summary["journal_mtime_ms"] = json!(u64::try_from(modified).expect("a millisecond count"));
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

/// The median of [`TIMED`] renders of one invocation, after one warm-up.
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

/// Over a host-sized runs root, each of the three renders in seconds — and
/// tenfold the journal bytes barely moves any of them.
///
/// The two halves are one claim. The **bound** says the listing is fast over a
/// root a fold could not survive; the **ratio** says why, by moving the one input
/// a fold is linear in and watching the clock stay where it was. Neither is a
/// figure about this host: 74.1 s was measured for `runs --mine` over a
/// comparable root before this, and the bound is set an order of magnitude below
/// that so only a return to folding can cross it.
#[test]
fn a_host_sized_runs_root_lists_in_seconds_and_ten_times_the_journal_bytes_barely_moves_it() {
    let world = World::new("listing-scale");
    // A run whose graph did not complete, so every cloned row is reported as
    // being driven and goes on to ask whether anything is watching it — the most
    // expensive row this listing renders, four hundred times over.
    world.script("build.fail", "1");
    let template = paths_of(
        &world,
        &settled(&world, "template", vec![agent("build", &[])]),
    );
    let filler = std::fs::read(template.journal()).expect("the template's own records");
    assert!(!filler.is_empty(), "the template run recorded nothing");

    let per_run = SCALED_JOURNAL_BYTES / SCALED_RUNS as u64;
    let stranger = "another-planner";
    let mut assembled: Vec<RunPaths> = Vec::new();
    for nth in 0..SCALED_RUNS {
        let owner = if nth < SCALED_OWNED {
            world.session.as_str()
        } else {
            stranger
        };
        let paths = cloned_run(&template, &world.runs, &format!("scaled-{nth:04}"), owner);
        journal_of(&paths, &filler, per_run);
        assembled.push(paths);
    }
    // llmlint: ignore[tests_mirror_real_usage] the template is not one of the four hundred
    // this measures, and leaving it in would make the owned count off by one.
    std::fs::remove_dir_all(&template.dir).expect("the template run root");

    let held = |assembled: &[RunPaths]| -> u64 {
        assembled
            .iter()
            .map(|paths| std::fs::metadata(paths.journal()).map_or(0, |about| about.len()))
            .sum()
    };
    let bytes = held(&assembled);
    assert!(
        bytes >= SCALED_JOURNAL_BYTES,
        "the root holds {bytes} journal byte(s), short of the {SCALED_JOURNAL_BYTES} this \
         measures over"
    );

    // Exactly the runs this session owns, out of the four hundred on the root.
    assert_eq!(
        rendered(&world, &["runs", "--mine"]).lines().count(),
        SCALED_OWNED,
        "--mine over {SCALED_RUNS} run roots rendered something other than the {SCALED_OWNED} \
         this session owns"
    );
    assert_eq!(rendered(&world, &["runs"]).lines().count(), SCALED_RUNS);

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
    for paths in &assembled {
        // Ten times **this journal's own** length rather than ten times the
        // even share, so the total that comes out is ten times the total that
        // went in rather than ten times what was aimed at.
        let held = std::fs::metadata(paths.journal())
            .expect("the journal")
            .len();
        journal_of(paths, &filler, held * JOURNAL_MULTIPLE);
    }
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

    for (argv, was) in LISTINGS.iter().zip(before) {
        let took = median(&world, argv);
        assert!(
            took <= was * GROWTH_BOUND,
            "`onepipeline {}` took {took:?} over {grown} journal byte(s) against {was:?} over \
             {bytes} — {JOURNAL_MULTIPLE} times the bytes moved it past the {GROWTH_BOUND}x a \
             bounded read is held to",
            argv.join(" ")
        );
        println!(
            "  {:<12} {was:?} over {bytes} journal byte(s), {took:?} over {grown}",
            argv.join(" ")
        );
    }
}
