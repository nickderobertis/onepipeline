//! Where a node's change is **now**, as the views report it.
//!
//! A run reported four of eleven nodes done while nine of the eleven had landed:
//! every settlement was an observation of the moment it published, and nothing
//! read it again. An adoption node was dispatched three times against work that
//! was already on its base. So the three places a landing is reported — the node
//! line, the run summary's count, and the status line — take a read when they
//! render, and these journeys hold what that read says and what it costs.
//!
//! Everything here is real: the repository, its origin, its branches, and every
//! landing on its base are git this test performs on disk. Nothing is published,
//! merged, or released by anything outside the test, and no landing is stated to
//! the binary — the run store records what each node's settlement observed, and
//! where the work is *now* is decided by `onevcs` off history.
//!
//! # What a render cost when this was measured
//!
//! Over the [`FIXTURE_NODES`]-node fixture below, on one developer host, each
//! figure averaged over [`REPETITIONS`] renders of that view — every render a
//! separate invocation of the binary, so the wall clock is the whole cost a
//! supervisor pays at a terminal and not the render alone:
//!
//! | render | landing reads | lines printed | wall clock |
//! | --- | --- | --- | --- |
//! | `results` | 4 | 6 | 165 ms |
//! | `goals` (the run summary) | 3 | 4 | 119 ms |
//! | `status` | 3 | 4 | 116 ms |
//!
//! A report rather than a bound, and the run that produced it prints the same
//! three rows. **No assertion below is a threshold on that time**: what a render
//! costs is held as *work* — see
//! [`a_render_asks_the_landing_read_once_per_node_it_prints_and_does_nothing_else_per_node`]
//! — because the work a render performs is a fact about this code while the
//! seconds it takes are a fact about the host's load at that instant.

use std::path::Path;

use serde_json::{json, Value};

use crate::harness::{git, World};

/// How many nodes the fixture run holds.
///
/// **Five**, and one of each answer there is:
///
/// | node | what its settlement recorded | where its branch is | reported |
/// | --- | --- | --- | --- |
/// | `landed-at-settlement` | `done`, landing `landed` | never landed on the base | **landed** — and never re-read |
/// | `merged-since` | `done`, landing `unlanded` | landed, under a landing trailer | **landed** |
/// | `still-open` | `done`, landing `unlanded` | pushed, not on the base | **not landed** |
/// | `merged-quietly` | `done`, landing `unlanded` | on the base, nothing records why | **undecidable** |
/// | `failed-but-landed` | `failed`, no landing at all | landed, under a landing trailer | **landed** |
///
/// So of the five: **three landed** — one the run itself observed and two a read
/// taken now decides — **one not landed**, and **one whose landing cannot be
/// decided**.
///
/// `landed-at-settlement`'s branch is deliberately one that has *not* landed:
/// its line reads `landed` only because the run recorded it, so a build that
/// re-read it anyway would report it as work nobody landed and every journey
/// below would say so.
const FIXTURE_NODES: usize = 5;

/// The nodes the fixture holds, in the order its plan names them.
const NODES: [&str; FIXTURE_NODES] = [
    "landed-at-settlement",
    "merged-since",
    "still-open",
    "merged-quietly",
    "failed-but-landed",
];

/// The one node the run's own settlement recorded as landed, which no render may
/// ask about again.
const RECORDED_LANDED: &str = "landed-at-settlement";

/// How many times each render is performed where its cost is measured.
///
/// Three, because the figure reported is a mean and one reading of a process
/// start is mostly the host's page cache. Every *bound* is asserted of a single
/// render, so repeating changes nothing about what is enforced.
const REPETITIONS: usize = 3;

/// The fixture run, and the world holding the repository its work is in.
struct Fixture {
    world: World,
    run: String,
}

/// Build the fixture run: one real repository, five real branches, and a run
/// store recording what each node's settlement observed.
///
/// The run store is written rather than driven, because what these journeys are
/// about is a *later* reader of a settled run: a driver that produced the same
/// store would have to be held open while a person merged, which is the state
/// `tests/e2e/lifecycle.rs` already drives and is not what is under test here.
/// Everything the answer is read off — the branches, the base, and every landing
/// on it — is real git this function performs.
fn fixture(name: &str) -> Fixture {
    let world = World::new(name);
    let repository = world.repository("change-open", &[]);
    let checkout = repository.checkout.clone();

    // Five branches with real work on them, each pushed to the origin the
    // identity resolves to, so `onevcs` can find the branch and compare it.
    for node in NODES {
        let branch = branch_of(node);
        git(&world, &checkout, &["checkout", "-b", &branch, "main"]);
        std::fs::write(checkout.join(format!("{node}.txt")), format!("{node}\n"))
            .expect("the branch's work is written");
        git(&world, &checkout, &["add", "-A"]);
        git(
            &world,
            &checkout,
            &["commit", "-m", &format!("feat: {node}")],
        );
        git(&world, &checkout, &["push", "-u", "origin", &branch]);
        git(&world, &checkout, &["checkout", "main"]);
    }

    // And then the base takes two of them under a landing trailer, and a third
    // with nothing at all recording why. `Onevcs-Landed-Commit:` is the trailer
    // `onevcs` reads for a landing that opened no change request, and it names
    // the branch commit the landing carries — which is what a landing this host
    // performed would have left behind.
    for node in ["merged-since", "failed-but-landed"] {
        let branch = branch_of(node);
        let tip = git(&world, &checkout, &["rev-parse", &branch])
            .trim()
            .to_owned();
        merge(
            &world,
            &checkout,
            &branch,
            &format!("chore: land {node}\n\nOnevcs-Landed-Commit: {tip}\n"),
        );
    }
    merge(
        &world,
        &checkout,
        &branch_of("merged-quietly"),
        "chore: somebody else made this change\n",
    );
    git(&world, &checkout, &["push", "origin", "main"]);

    let run = "landings".to_owned();
    write_run_store(&world, &run);
    Fixture { world, run }
}

/// The branch one node's work is on.
fn branch_of(node: &str) -> String {
    format!("work/{node}")
}

/// Take one branch onto the base, with a message of the caller's choosing.
fn merge(world: &World, checkout: &Path, branch: &str, message: &str) {
    git(
        world,
        checkout,
        &["merge", "--no-ff", "-m", message, branch],
    );
}

/// Write the fixture's run store: a launch record and a journal recording what
/// each node settled as.
fn write_run_store(world: &World, run: &str) {
    let dir = world.runs.join(run);
    std::fs::create_dir_all(&dir).expect("the run directory");
    std::fs::write(
        dir.join("launch.json"),
        json!({
            "run_id": run,
            "session": world.session,
            "launcher": "claude-code",
            "started_at": "2026-01-01T00:00:00.000Z",
            "heartbeat_interval": 1_800,
        })
        .to_string(),
    )
    .expect("the launch record");

    let plan = json!({
        "schema_version": 1,
        "name": run,
        "goal": {"text": "land every node's work"},
        "concurrency": 4,
        "tasks": NODES
            .iter()
            .map(|node| json!({
                "id": node,
                "task": "## What\nland it\n",
                "repo": "service",
            }))
            .collect::<Vec<Value>>(),
    });

    let mut journal = String::new();
    let mut seq = 0;
    let mut append = |kind: &str, node: Option<&str>, payload: Value| {
        journal.push_str(
            &json!({
                "v": 1,
                "ts": "2026-01-01T00:00:00.000Z",
                "stream": "fixture",
                "seq": seq,
                "source": "pipeline",
                "kind": kind,
                "labels": {"run_id": run, "node": node},
                "payload": payload,
            })
            .to_string(),
        );
        journal.push('\n');
        seq += 1;
    };
    append("run-started", None, json!({"plan": plan}));
    for node in NODES {
        let branch = branch_of(node);
        let mut payload = json!({"status": "done", "branch": branch});
        match node {
            RECORDED_LANDED => payload["landing"] = json!("landed"),
            // No landing at all, and a branch it left behind: what a node whose
            // publication never ran records, and the shape that used to carry no
            // word about a landing onto any view.
            "failed-but-landed" => {
                payload["status"] = json!("failed");
                payload["outcome"] = json!("task-failed");
            }
            _ => {
                payload["landing"] = json!("unlanded");
                payload["change_url"] =
                    json!(format!("https://github.com/owner/service/pull/{node}"));
            }
        }
        append("node-settled", Some(node), payload);
    }
    std::fs::write(dir.join("events.jsonl"), journal).expect("the journal");
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

/// A node's change that reached its base since it settled reads back as landed
/// from every view a supervisor decides from.
///
/// The whole incident in one journey: the run's own record says `unlanded`,
/// nothing has told it otherwise, and the change is on the base. Before this,
/// every one of these lines reported the settlement's dated claim and a
/// supervisor re-dispatched work that was already landed.
#[test]
fn a_change_on_its_base_since_the_node_settled_reads_back_as_landed_from_every_view() {
    let Fixture { world, run } = fixture("landing-since");

    let results = world.run(&["results", &run]);
    results.exited(0);
    // Read now, and carrying the tier that decided it: "it landed" is exactly
    // the claim that used to be an inference and was wrong.
    let merged = line_for(&results.stdout, "merged-since");
    assert!(
        merged.contains("landed on its base — read now: a landing trailer on the base at "),
        "the node line reports the settlement's answer rather than a read: {merged}"
    );
    assert!(
        !merged.contains("NOT landed"),
        "a change on its base is reported as one nobody landed: {merged}"
    );
    // A node that settled *failed*, with no landing recorded at all, whose work
    // the base has since taken. Nothing used to say anything about it.
    let failed = line_for(&results.stdout, "failed-but-landed");
    assert!(
        failed.contains("landed on its base — read now: a landing trailer on the base at "),
        "a failed node whose work landed says nothing about it: {failed}"
    );

    // The two counting views, which are the ones work is closed from.
    for view in [
        world.run(&["goals", &run]),
        world.run(&["status", &run]),
        world.run(&["runs"]),
    ] {
        view.exited(0);
        assert!(
            view.stdout.contains("1 not landed"),
            "the count still carries the settlement's answer:\n{}",
            view.stdout
        );
        assert!(
            !view.stdout.contains("2 not landed"),
            "a change on its base is counted as work nobody landed:\n{}",
            view.stdout
        );
    }
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("1 node(s) have not landed: still-open")
        .out_lacks("merged-since");
}

/// A change the base does not carry reads back as not landed, and one nothing
/// can decide reads back as undecided — which is neither of the other two.
///
/// The property the dated phrasing was protecting, kept: a read that cannot
/// answer must not be reported as an answer. `merged-quietly`'s work is on the
/// base with nothing recording why, which is equally what somebody else making
/// the same change leaves behind — so it is reported as the open question it is.
#[test]
fn a_landing_that_cannot_be_decided_reads_back_as_undecided_rather_than_as_either_answer() {
    let Fixture { world, run } = fixture("landing-undecided");

    let results = world.run(&["results", &run]);
    results.exited(0);
    let open = line_for(&results.stdout, "still-open");
    assert!(
        open.contains("NOT landed: read now, content comparison: the base does not carry"),
        "a change nobody landed is not reported as one: {open}"
    );
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

    // And apart from the count of work nobody landed, in both counting views.
    world
        .run(&["goals", &run])
        .exited(0)
        .out_has("1 not landed, 1 landing undecided");
    world
        .run(&["status", &run])
        .exited(0)
        .out_has("1 node(s) whose landing this host could not decide: merged-quietly");
}

/// A landing the run itself observed is never read again.
///
/// The one answer a later read cannot overturn — a base does not stop carrying
/// work it has taken — so asking would spend a read to be told what the record
/// already says. The fixture makes that observable rather than merely counted:
/// this node's branch is one that never landed, so a build that re-read it would
/// report the run's own landed node as work nobody landed.
#[test]
fn a_landing_the_run_recorded_is_reported_from_the_record_and_never_read_again() {
    let Fixture { world, run } = fixture("landing-recorded");
    let reads = world.root.join("recorded.reads");

    let results = world.run_recording_renders(&reads, &["results", &run]);
    results.exited(0);
    let line = line_for(&results.stdout, RECORDED_LANDED);
    assert!(
        line.contains("landed on its base — the run observed the change reach it"),
        "a landing the run observed is reported as something else: {line}"
    );
    assert!(
        !asked(&acts(&world, &reads)).contains(&RECORDED_LANDED.to_owned()),
        "a landing no later read can overturn was read again:\n{}",
        std::fs::read_to_string(&reads).unwrap_or_default()
    );
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
        ("results", vec!["results", run.as_str()], 4),
        ("summary", vec!["goals", run.as_str()], 3),
        ("status", vec!["status", run.as_str()], 3),
    ] {
        // Each repetition records into a file of its own, so the acts asserted
        // on below are one render's rather than three renders' appended.
        let mut took = std::time::Duration::ZERO;
        let mut measured = None;
        for repetition in 0..REPETITIONS {
            let path = world.root.join(format!("{view}-{repetition}.reads"));
            let began = std::time::Instant::now();
            let run = world.run_recording_renders(&path, &argv);
            took += began.elapsed();
            run.exited(0);
            measured = Some((run, path));
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

/// The `results` line for one node.
fn line_for(rendered: &str, node: &str) -> String {
    rendered
        .lines()
        .find(|line| line.trim_start().starts_with(node))
        .unwrap_or_else(|| panic!("{node} has no line in:\n{rendered}"))
        .to_owned()
}
