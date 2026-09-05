//! Where a node's change is **now**, as the views report it.
//!
//! A run reported four of eleven nodes done while nine of the eleven had landed:
//! every settlement was an observation of the moment it published, and nothing
//! read it again. An adoption node was dispatched three times against work that
//! was already on its base. So the three places a landing is reported — the node
//! line, the run summary's count, and the status line — take a read when they
//! render, and these journeys hold what that read says and what it costs.
//!
//! Everything here is real. The run is driven through the binary, its nodes open
//! `onevcs` sessions and publish through them, and every landing on the base
//! afterwards is git this test performs on the repository it created: nothing is
//! published, merged, or released by anything outside the test, and no landing is
//! stated to the binary.
//!
//! # What a render cost when this was measured
//!
//! <!-- llmlint: ignore-block[comments_earn_their_place] this table is a deliverable
//! rather than an aside: the change was required to *state* what the read it adds costs a
//! supervisory look, and a figure nobody writes down is one nobody can weigh a later
//! regression against. It is deliberately not asserted — see the sentence under it — so
//! there is nowhere else it could live. The run prints the same three rows, so a reader
//! who doubts it re-reads it rather than trusting the snapshot. -->
//! Over the [`FIXTURE_NODES`]-node fixture below, on one developer host, each
//! figure averaged over [`REPETITIONS`] renders of that view — every render a
//! separate invocation of the binary, so the wall clock is the whole cost a
//! supervisor pays at a terminal rather than the render alone:
//!
//! | render | landing reads | lines printed | wall clock |
//! | --- | --- | --- | --- |
//! | `results` | 5 | 8 | 664 ms |
//! | `goals` (the run summary) | 4 | 4 | 500 ms |
//! | `status` | 4 | 4 | 497 ms |
//!
//! **No assertion below is a threshold on that time**: what a render costs is
//! held as *work* — see
//! [`a_render_asks_the_landing_read_once_per_node_it_prints_and_does_nothing_else_per_node`]
//! — because the work a render performs is a fact about this code while the
//! seconds it takes are a fact about the host's load at that instant.
//! <!-- llmlint: ignore-end[comments_earn_their_place] -->

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary and the sibling these journeys are about — `onevcs` — is the real library, over
// real git and a real origin on disk. `oneagentgraph` is substituted at its subprocess
// boundary so a journey states a dispatch outcome rather than paying for a model turn, and
// GitHub is substituted at `onevcs`'s own `ONEVCS_GH` override so a change request can be
// opened offline. `harness.rs` carries the same suppression and the full rationale.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::harness::{git, lifecycle, plan_of, World};

/// How many nodes the fixture run holds.
///
/// **Six**, driven as one run against one repository, and then landed — or not —
/// by this test:
///
/// | node | what its settlement recorded | what the test then did to its branch | reported |
/// | --- | --- | --- | --- |
/// | `landed-at-settlement` | landing `landed`, from the run's own close-out re-read | landed under a landing trailer, before that re-read | **landed** — and never read again |
/// | `merged-since` | landing `unlanded` | landed under a landing trailer | **landed** |
/// | `failed-but-landed` | `failed`, no landing at all | landed under a landing trailer | **landed** |
/// | `still-open` | landing `unlanded` | nothing; its change is still open | **not landed** |
/// | `landed-in-part` | landing `unlanded` | landed, then given a commit the landing does not carry | **not landed**, naming the landing |
/// | `merged-quietly` | landing `unlanded` | landed with nothing recording why | **undecidable** |
///
/// So of the six: **three landed** — one the run itself recorded and two a read
/// taken now decides — **two not landed**, and **one whose landing cannot be
/// decided**. The remaining answer, a read this host cannot make at all, is a
/// journey of its own below, because reaching it means taking the repository
/// away from under the run.
const FIXTURE_NODES: usize = 6;

/// The nodes the fixture's plan holds.
const NODES: [&str; FIXTURE_NODES] = [
    "landed-at-settlement",
    "merged-since",
    "failed-but-landed",
    "still-open",
    "landed-in-part",
    "merged-quietly",
];

/// The one node the run's own record says landed, which no render may read again.
const RECORDED_LANDED: &str = "landed-at-settlement";

/// How many times each render is performed where its cost is measured.
///
/// Three, because the figure reported is a mean and one reading of a process
/// start is mostly the host's page cache. Every *bound* is asserted of a single
/// render, so repeating changes nothing about what is enforced.
const REPETITIONS: usize = 3;

/// The fixture run, the world it was driven in, and the branch each node left.
struct Fixture {
    world: World,
    run: String,
}

/// Drive the fixture run, then move the repository under it.
///
/// The run is real: six lifecycle nodes open `onevcs` sessions, work in them, and
/// publish under a `change-open` policy, so each settles with its change
/// unlanded and a person left to merge it. This test is that person, and it
/// merges with git.
///
/// The order is load-bearing. `landed-at-settlement` is landed **before** the run
/// is adopted, so the driver's own close-out re-read is what records it as
/// landed — which is the state a render must not spend a read on, and the only
/// way to reach it is to let a driver find it. Everything else is landed after
/// that, so nothing but a render has ever looked at it.
fn fixture(name: &str) -> Fixture {
    let world = World::new(name);
    let repository = world.repository("change-open", &[]);
    let checkout = repository.checkout.clone();
    for node in NODES {
        world.script(&format!("{node}.work"), &format!("the work of {node}\n"));
    }
    // A node the run settles `failed`, with the work its dispatch committed still
    // on the branch it handed back. Nothing about a failure says where that
    // branch is now, which is why a view asks about one.
    world.script("failed-but-landed.fail", "1");

    let run = name.to_owned();
    let plan = world.plan(
        &run,
        &plan_of(
            &run,
            NODES.iter().map(|node| lifecycle(node, &[])).collect(),
        ),
    );
    world.run(&["start", &plan, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    let branches = branches_of(&world, &run);
    // Waited for, because a run's result is written when its last node settles
    // while a session's publishing push reaches this checkout on the session's
    // own clock: one of the six was reliably still on its way. Then kept under a
    // ref of this test's own, so a sweep that reaps a spent session's branch
    // afterwards cannot take the work away mid-journey. Both are about the
    // fixture being there; neither is anything these journeys assert.
    world.until(
        "every branch the run published to reach the checkout",
        |world| {
            branches.values().all(|branch| {
                !git(world, &checkout, &["branch", "--list", branch])
                    .trim()
                    .is_empty()
            })
        },
    );
    for (node, branch) in &branches {
        git(&world, &checkout, &["branch", "-f", &kept(node), branch]);
    }

    // Landed before the run is adopted, which is what makes the driver's own
    // close-out re-read the thing that records it.
    land(&world, &checkout, RECORDED_LANDED, &branches, None);
    git(&world, &checkout, &["push", "origin", "main"]);
    world.run(&["adopt", &run]).settled();

    for node in ["merged-since", "failed-but-landed", "landed-in-part"] {
        land(&world, &checkout, node, &branches, None);
    }
    // Landed with nothing recording why, which is equally what somebody else
    // making the same change leaves behind — so it is undecidable rather than a
    // landing.
    land(
        &world,
        &checkout,
        "merged-quietly",
        &branches,
        Some("chore: somebody else made this change\n"),
    );
    git(&world, &checkout, &["push", "origin", "main"]);

    // A branch that landed and then went on, which is the ordinary shape a
    // retried dispatch leaves: the landing is real and there is still work to
    // publish, so it is not a landing *of this branch*. The commit goes on this
    // checkout's own copy, which is one of the copies `onevcs` asks — and the
    // copy still holding work is the one whose answer is true of the work.
    let branch = &branches["landed-in-part"];
    git(
        &world,
        &checkout,
        &["branch", "-f", branch, &kept("landed-in-part")],
    );
    git(&world, &checkout, &["checkout", branch]);

    std::fs::write(checkout.join("after-the-landing.txt"), "more work\n")
        .expect("the commit above the landing is written");
    git(&world, &checkout, &["add", "-A"]);
    git(
        &world,
        &checkout,
        &["commit", "-m", "feat: after the landing"],
    );
    git(&world, &checkout, &["checkout", "main"]);

    Fixture { world, run }
}

/// The branch each node's dispatch left behind, off the run's own result.
fn branches_of(world: &World, run: &str) -> BTreeMap<String, String> {
    let result = world.run_json(run, "result.json");
    let nodes = result["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("the run's result names its nodes: {result}"));
    let branches: BTreeMap<String, String> = nodes
        .iter()
        .filter_map(|node| {
            Some((
                node["id"].as_str()?.to_owned(),
                node["branch"].as_str()?.to_owned(),
            ))
        })
        .collect();
    for node in NODES {
        assert!(
            branches.contains_key(node),
            "{node} settled without naming the branch its work is on: {result}"
        );
    }
    branches
}

/// This test's own name for the commit one node's branch stood at when the run
/// settled, so a later sweep cannot take the work away mid-journey.
fn kept(node: &str) -> String {
    format!("kept/{node}")
}

/// Take one branch onto the base, the way a landing does.
///
/// The branch is a ref of this checkout, because that is where a session's
/// publishing push put it: a session clone takes its remote from the execution
/// checkout, so what a journey merges is the branch that push left here.
///
/// `message` is the whole commit message where a journey states one. The default
/// carries `Onevcs-Landed-Commit:` naming the branch commit the landing lands,
/// which is the trailer `onevcs` reads for a landing that opened no change
/// request — and the record a landing this host performed leaves behind.
fn land(
    world: &World,
    checkout: &Path,
    node: &str,
    branches: &BTreeMap<String, String>,
    message: Option<&str>,
) {
    let branch = &branches[node];
    // Back to where the run left it, in case a sweep has taken the name away
    // since; the commit is still here because [`kept`] is holding it.
    git(world, checkout, &["branch", "-f", branch, &kept(node)]);
    let tip = git(world, checkout, &["rev-parse", branch])
        .trim()
        .to_owned();
    let message = message
        .map(str::to_owned)
        .unwrap_or_else(|| format!("chore: land {branch}\n\nOnevcs-Landed-Commit: {tip}\n"));
    git(
        world,
        checkout,
        &["merge", "--no-ff", "-m", &message, branch],
    );
}

/// Every act one render recorded, as JSON lines.
fn acts(world: &World, path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "the render recorded nothing at {}: {error}\n{}",
            path.display(),
            world.dump()
        )
    });
    text.lines()
        .map(|line| serde_json::from_str(line).expect("a JSON act"))
        .collect()
}

/// The nodes one render asked the landing read about, in the order it asked.
fn asked(acts: &[Value]) -> Vec<String> {
    acts.iter()
        .filter(|act| act["act"] == "landing-read")
        .map(|act| {
            act["node"]
                .as_str()
                .unwrap_or_else(|| panic!("a landing read outside any node's decision: {act}"))
                .to_owned()
        })
        .collect()
}

/// The `results` line for one node.
fn line_for(rendered: &str, node: &str) -> String {
    rendered
        .lines()
        .find(|line| line.trim_start().starts_with(node))
        .unwrap_or_else(|| panic!("{node} has no line in:\n{rendered}"))
        .to_owned()
}

/// Every answer a landing read gives reads back on the node line, as of the
/// moment the line is rendered.
///
/// The whole incident in one journey. Four of these nodes settled with their
/// change unlanded and nothing told the run otherwise; two of them are on the
/// base now, and a third is a node the run settled *failed* whose work somebody
/// landed anyway. Before this, every one of those lines carried the settlement's
/// dated claim, and a supervisor re-dispatched work that was already landed.
///
/// The two answers that are not landings are held just as hard, because what the
/// dated phrasing was protecting is that a read which cannot decide must not be
/// reported as a decision.
#[test]
fn every_answer_a_landing_read_gives_reads_back_on_the_node_line() {
    let Fixture { world, run } = fixture("landing-answers");
    let reads = world.root.join("results.reads");
    let results = world.run_recording_renders(&reads, &["results", &run]);
    results.exited(0);

    let trailer = "landed on its base — read now: a landing trailer on the base at ";
    let merged = line_for(&results.stdout, "merged-since");
    assert!(
        merged.contains(trailer) && !merged.contains("NOT landed"),
        "a change on its base is reported as one nobody landed: {merged}"
    );
    // A node that settled *failed*, with no landing recorded at all, whose work
    // the base has since taken. Nothing used to say anything about it.
    let failed = line_for(&results.stdout, "failed-but-landed");
    assert!(
        failed.contains(trailer),
        "a failed node whose work landed says nothing about it: {failed}"
    );
    // A change nobody landed still reads as one — and says so from a read rather
    // than from the settlement's own dated claim.
    let open = line_for(&results.stdout, "still-open");
    assert!(
        open.contains("NOT landed: read now, content comparison: the base does not carry"),
        "a change nobody landed is not reported as one: {open}"
    );
    // A landing the branch has gone past: there is work left to publish, so it is
    // not a landing of this branch — and the commit that *did* land is named
    // anyway, because a reader deciding what to do next needs both.
    let part = line_for(&results.stdout, "landed-in-part");
    assert!(
        part.contains("NOT landed: read now, a landing trailer on the base at ")
            && part.contains("commit(s) above it the landing did not carry"),
        "a branch that landed and went on is reported as though nothing landed: {part}"
    );
    // Undecidable, and reported as neither of the two answers nothing gave.
    let quiet = line_for(&results.stdout, "merged-quietly");
    assert!(
        quiet.contains(
            "landing UNDECIDED: read now, content comparison: the base already carries what \
             this branch changed, and nothing records why"
        ),
        "an undecidable landing is reported as a decision: {quiet}"
    );
    assert!(
        !quiet.contains("NOT landed") && !quiet.contains("landed on its base"),
        "an undecidable landing reads as one of the two answers nothing gave: {quiet}"
    );

    // And the landing the run itself recorded is reported from that record and
    // never asked about again: a base does not stop carrying work it has taken,
    // so a read there could only agree, at a cost.
    let recorded = line_for(&results.stdout, RECORDED_LANDED);
    assert!(
        recorded.contains("landed on its base — the run observed the change reach it"),
        "a landing the run observed is reported as something else: {recorded}"
    );
    assert!(
        !asked(&acts(&world, &reads)).contains(&RECORDED_LANDED.to_owned()),
        "a landing no later read can overturn was read again:\n{}",
        std::fs::read_to_string(&reads).unwrap_or_default()
    );
}

/// The views work is closed from count what a read taken now says, and count an
/// undecided landing apart from work nobody landed.
///
/// `status` otherwise reports only what is in flight, and the summary line is the
/// `n/n done` a planner reads to decide a run is finished — so these are the two
/// places the stale count actually cost dispatches.
#[test]
fn the_counting_views_report_what_a_read_taken_now_says() {
    let Fixture { world, run } = fixture("landing-counts");

    // `goals` and `runs` are the summary line; `status` carries it too, above its
    // own per-node lines.
    for view in [
        world.run(&["goals", &run]),
        world.run(&["runs"]),
        world.run(&["status", &run]),
    ] {
        view.exited(0);
        assert!(
            view.stdout.contains("2 not landed, 1 landing undecided"),
            "the count carries the settlement's answer rather than a read:\n{}",
            view.stdout
        );
    }
    let status = world.run(&["status", &run]);
    status.exited(0);
    status.out_has("2 node(s) have not landed: landed-in-part, still-open");
    status.out_has("1 node(s) whose landing this host could not decide: merged-quietly");
    // The three that landed are on neither line.
    for landed in [RECORDED_LANDED, "merged-since", "failed-but-landed"] {
        assert!(
            !status.stdout.contains(landed),
            "{landed} is on its base and is still reported as outstanding:\n{}",
            status.stdout
        );
    }
}

/// A landing this host cannot read at all is reported as undecided, saying what
/// refused.
///
/// The answer that is neither of the other two and must never be collapsed into
/// them. Here the repository the run's work is in is gone from this host — the
/// state a checkout somebody deleted leaves, with the registry still naming it —
/// so `onevcs` refuses the question rather than answering it. A supervisor
/// meeting "this host could not decide it" has something to fix; one meeting
/// `NOT landed` would go and publish landed work again.
#[test]
fn a_landing_this_host_cannot_read_is_reported_as_undecided_saying_what_refused() {
    let world = World::new("landing-unreadable");
    let repository = world.repository("change-open", &[]);
    world.script("service.work", "the work nobody can find again\n");
    let run = "unreadable".to_owned();
    let plan = world.plan(&run, &plan_of(&run, vec![lifecycle("service", &[])]));
    world.run(&["start", &plan, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    // Answerable while the repository is there.
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("NOT landed: read now");

    // And then the checkout every publication fast-forwards is gone, which is
    // what the registry still names.
    std::fs::remove_dir_all(&repository.checkout).expect("the checkout is taken away");

    let results = world.run(&["results", &run]);
    results.exited(0).out_has("landing UNDECIDED: read now");
    results.out_has("this host could not decide it:");
    results.out_lacks("NOT landed");
    results.out_lacks("landed on its base");
    // The count says the same thing, apart from work nobody landed.
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("1 node(s) whose landing this host could not decide: service")
        .out_lacks("node(s) have not landed");
}

/// What a render costs is bounded as **work**, over the fixture above.
///
/// Every claim here counts acts and times nothing, so it gives the same verdict
/// on a loaded host as on an idle one — and a later change that puts the
/// per-node cost back fails here rather than reaching a supervisor.
///
/// Four bounds, one per way a render could cost more than it must:
///
/// 1. at most one landing read for each node the render reports on;
/// 2. no read at all for a node the run already recorded as landed;
/// 3. no read for a node the render does not report on;
/// 4. **nothing else per node** — no process this crate started, no read of the
///    run's ledger or journal, and so no walk of a base's history taken here and
///    no request over a network, both of which from this crate are a process.
#[test]
fn a_render_asks_the_landing_read_once_per_node_it_prints_and_does_nothing_else_per_node() {
    let Fixture { world, run } = fixture("landing-cost");

    // Measured beside the bound and asserted on by nothing: what a render costs
    // in seconds is a fact about this host's load at that instant, and a
    // threshold on it would fail correct work. It is printed so the figure in
    // this module's own header is a reading rather than a guess.
    let mut measured_cost = String::new();
    // One invocation before anything is timed, so what is measured is a render
    // rather than the first exec of a debug binary nobody has paged in.
    world.run(&["runs"]).exited(0);
    for (view, argv, expected) in [
        // `results` prints a line for every node; the counting views report on
        // the five whose publication answered a landing at all.
        ("results", vec!["results", run.as_str()], 5),
        ("summary", vec!["goals", run.as_str()], 4),
        ("status", vec!["status", run.as_str()], 4),
    ] {
        // Each repetition records into a file of its own, so the acts asserted
        // on below are one render's rather than three renders' appended.
        let mut took = std::time::Duration::ZERO;
        let mut measured = None;
        for repetition in 0..REPETITIONS {
            let path = world.root.join(format!("{view}-{repetition}.reads"));
            let began = std::time::Instant::now();
            let rendered = world.run_recording_renders(&path, &argv);
            took += began.elapsed();
            rendered.exited(0);
            measured = Some((rendered, path));
        }
        let (measured, path) = measured.expect("at least one render is measured");
        let acts = acts(&world, &path);
        let rendered: Vec<&Value> = acts.iter().filter(|act| act["view"] == view).collect();
        assert!(
            !rendered.is_empty(),
            "{view} recorded no render at all:\n{}",
            measured.stdout
        );

        let asked: Vec<String> = asked(
            &rendered
                .iter()
                .map(|act| (*act).clone())
                .collect::<Vec<Value>>(),
        );
        let mut once = asked.clone();
        once.sort();
        once.dedup();
        assert_eq!(
            once.len(),
            asked.len(),
            "{view} asked the landing read twice for one node: {asked:?}"
        );
        assert_eq!(
            asked.len(),
            expected,
            "{view} performed {} landing read(s), not {expected}: {asked:?}",
            asked.len()
        );
        assert!(
            !asked.contains(&RECORDED_LANDED.to_owned()),
            "{view} read a landing the run had already recorded: {asked:?}"
        );

        let reported: Vec<String> = rendered
            .iter()
            .filter(|act| act["act"] == "reported")
            .filter_map(|act| act["node"].as_str().map(str::to_owned))
            .collect();
        for node in &asked {
            assert!(
                reported.contains(node),
                "{view} read a landing for {node}, whose line it does not print: {reported:?}"
            );
        }
        assert!(
            asked.len() <= reported.len(),
            "{view} performed more reads than it has nodes to report on: {asked:?} over \
             {reported:?}"
        );

        // And nothing else per node: every act recorded inside a node's landing
        // decision is that node's one landing read.
        let per_node: Vec<&&Value> = rendered
            .iter()
            .filter(|act| act["act"] != "render" && act["act"] != "reported")
            .collect();
        for act in &per_node {
            assert_eq!(
                act["act"], "landing-read",
                "{view} did per-node work no landing read accounts for: {act}"
            );
        }
        assert_eq!(
            per_node.len(),
            asked.len(),
            "{view} performed per-node work beside its landing reads: {per_node:?}"
        );

        measured_cost.push_str(&format!(
            "  {view:<8} {} landing read(s), {} line(s) printed, {:?} per render\n",
            asked.len(),
            measured.stdout.lines().count(),
            took / u32::try_from(REPETITIONS).expect("a small count")
        ));
    }
    println!(
        "what one render of each view cost, averaged over {REPETITIONS} renders \
         each:\n{measured_cost}"
    );
}
