//! Compatibility across the journal this change adds to, in both directions.
//!
//! A runs root holds journals from every build that ever ran on the host, and
//! this crate is not the only thing that reads one — so neither direction is
//! inferable from the other and both are held:
//!
//! * **A journal from before the change reads the same.** This build folds one
//!   and renders it, and the rendering is committed beside it.
//! * **A reader from before the change reads a journal this build writes.**
//!   [`derived_by_a_preceding_reader`] is that reader, keying on exactly the
//!   record kinds the committed fixture carries and reading an `edit-committed`'s
//!   operations the way a consumer had to before `operation_kinds` existed. What
//!   it derives from a journal this build writes has to be what it derives from
//!   the fixture — the **same run**, so "what it reported" is a value in hand.
//!
//! Everything this build writes is stamped [`ENVELOPE_VERSION`], the fixture the
//! version before it, and both are in [`ENVELOPE_VERSIONS_READ`]. A relayed
//! envelope keeps its producer's own number in either journal, which is why the
//! assertions below ask the version only of this library's own records.
//!
//! [`FIXTURE`] is the immutable reference for what a preceding reader knows: the
//! journal of a real run of [`recorded_run`] at `4db4e7e`, the commit this work
//! forked from, with host paths and stream names stood in for. It is committed and
//! **never regenerated** — a fixture edited to match a newer build proves nothing
//! about an older one, which is what
//! [`the_committed_fixture_is_a_journal_from_before_this_change`] holds it to.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary against a real run store; what the journeys here assert is the record
// this crate writes and what a reader derives from it, neither of which the double
// produces. The scenario the double states is one a real sibling would need paid model
// turns to produce, and `dispatch.rs` is where the real `oneagentgraph` binary is driven
// instead. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{agent, plan_of, World, REFUSED};

use onepipeline::event::{ENVELOPE_VERSION, ENVELOPE_VERSIONS_READ};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A run's whole journal as the build before this change wrote it.
const FIXTURE: &str = include_str!("../golden/journal-before-command-accepted.jsonl");

/// What this build renders that journal as, committed beside it.
const RENDERED: &str = include_str!("../golden/journal-before-command-accepted.rendered");

/// The run the fixture holds, and the one every check here drives.
///
/// One of each thing the split touches: a command that changes no graph (the
/// `finding`, and the `complete` beside it), one that commits more than one
/// operation (the `add` naming a dependency), and one that commits exactly one
/// (the `amend`) — plus a settlement, so the views have a finished run to render
/// rather than a snapshot that would differ by a clock.
fn recorded_run(world: &World, name: &str) -> String {
    world.script("slow.wait", "hold");
    let path = world.plan(
        name,
        &plan_of(name, vec![agent("slow", &[]), agent("after", &["slow"])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("a node to be in flight", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });

    world
        .run_with_stdin(
            &["reply", name],
            &json!({"version": 2, "author": "monitor", "commands": [
                {"op": "finding", "id": "slow", "message": "the branch has no commits yet"}
            ]})
            .to_string(),
        )
        .exited(0);
    world
        .run_with_stdin(
            &["reply", name],
            &json!({"version": 2, "commands": [
                {"op": "add", "node": {"id": "extra", "persona": "engineer",
                                       "task": "## What\nextra", "deps": ["slow"]}},
                {"op": "amend", "id": "after", "text": "## What\nthe corrected criterion"},
            ]})
            .to_string(),
        )
        .exited(0);
    // The other command that changes no graph, so both of them cross the
    // compatibility boundary rather than one.
    world
        .run_with_stdin(
            &["reply", name],
            &json!({"version": 2, "commands": [
                {"op": "complete", "reason": "the publication is verified"}
            ]})
            .to_string(),
        )
        .exited(0);

    world.release("slow.go");
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

/// Every event kind one journal carries.
///
/// A line that is not an envelope carrying a kind fails here rather than being
/// passed over: both journals this reads are documents a check depends on, and a
/// fixture quietly edited into something unreadable would weaken every assertion
/// below without failing one.
fn kinds_in(journal: &str) -> BTreeSet<String> {
    records_of(journal)
        .iter()
        .map(|event| {
            event["kind"]
                .as_str()
                .unwrap_or_else(|| panic!("a journal record names its kind: {event}"))
                .to_string()
        })
        .collect()
}

/// The envelope versions **this library's own** records in one journal carry.
///
/// Only `pipeline` records: a relayed envelope keeps its producer's own number,
/// and asking this crate's version line of a sibling's record would be judging
/// that library for moving at its own pace.
fn own_versions_in(journal: &str) -> BTreeSet<u64> {
    records_of(journal)
        .iter()
        .filter(|event| event["source"] == "pipeline")
        .map(|event| {
            event["v"]
                .as_u64()
                .unwrap_or_else(|| panic!("a journal record names its envelope version: {event}"))
        })
        .collect()
}

/// The envelope versions everything **else** in one journal carries — the relayed
/// records, which keep their producer's own.
fn relayed_versions_in(journal: &str) -> BTreeSet<u64> {
    records_of(journal)
        .iter()
        .filter(|event| event["source"] != "pipeline")
        .map(|event| {
            event["v"]
                .as_u64()
                .unwrap_or_else(|| panic!("a journal record names its envelope version: {event}"))
        })
        .collect()
}

/// One journal's records, refusing a line that is not one.
fn records_of(journal: &str) -> Vec<Value> {
    journal
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|why| panic!("a journal line is an envelope: {why}: {line}"))
        })
        .collect()
}

/// Lay one journal down as a run of its own under `root`, with the launch record
/// every view opens a run by.
///
// llmlint: ignore-block[tests_mirror_real_usage] a journal written by **another build** is
// the input here, and there is no invocation a user can type that produces one: this build
// stamps everything it writes in the one shape it accepts, so reaching the case through
// the CLI would mean editing a live run's store underneath it — which is the suite standing
// in for a producer rather than a replay of anything. `tests/replay.rs` states the same
// argument at length for the same reason, and everything the journeys below then assert is
// the real compiled binary reading a real store, which is exactly how it meets a journal a
// preceding build left in a runs root. What is written here is the two files a run *is* to
// a reader — its launch record and its journal — and nothing internal to either.
fn run_of(root: &Path, run: &str, journal: &str) {
    let dir = root.join(run);
    std::fs::create_dir_all(&dir).expect("a run directory");
    std::fs::write(
        dir.join("launch.json"),
        json!({
            "run_id": run,
            "plan": "plan.json",
            "launcher": "claude-code",
            "session": "session-compat",
            "pid": std::process::id(),
            "host": "compat",
            "started_at": "2026-09-01T04:00:00.000Z",
            "heartbeat_interval": 1_800,
        })
        .to_string(),
    )
    .expect("a launch record");
    std::fs::write(dir.join("events.jsonl"), journal).expect("a journal");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// What the two views say about a run laid down under its own root.
///
/// Both verbs, driven as a user drives them, because they are the two readers
/// the compatibility claim is about: the fold that derives a run's state, and the
/// rendering over it.
fn views_of(world: &World, root: &Path, run: &str) -> String {
    let read = |verb: &str| {
        let rendered = world
            .cmd(&[verb, run])
            // The root these journals were laid down under, in place of the
            // world's own: what is being read is the document, not a live run.
            .env("ONEPIPELINE_RUNS_DIR", root)
            .output()
            .expect("the view runs");
        assert!(
            rendered.status.success(),
            "`onepipeline {verb}` refused the run: {}",
            String::from_utf8_lossy(&rendered.stderr)
        );
        String::from_utf8_lossy(&rendered.stdout).into_owned()
    };
    format!("{}\n{}", read("status"), read("results"))
}

/// The record kinds a reader written before this change knows about.
///
/// Held to the fixture by
/// [`the_preceding_reader_knows_only_the_kinds_the_fixture_carries`], so it
/// cannot quietly learn a kind this change added.
const KNOWN_TO_A_PRECEDING_READER: [&str; 5] = [
    "edit-committed",
    "node-settled",
    "node-dispatched",
    "completion-requested",
    "planner-surface-queued",
];

/// What a reader written before this change derives from one run's journal.
///
/// Not a rendering and not a count of records: what such a reader is *for* is
/// the state it folds, which is why the split can be right even though the
/// number of `edit-committed` records moved. A consumer that counted them would
/// see one fewer; one that acts on them sees the same run, because the records
/// that moved carried nothing it acts on.
#[derive(Debug, Default, PartialEq, Eq)]
struct Derived {
    /// Every node a committed edit put into the graph, and the dependencies it
    /// arrived with.
    added: BTreeMap<String, Vec<String>>,
    /// Every dependency edge a committed edit added, as `from -> to`.
    edges: BTreeSet<(String, String)>,
    /// The amendment each node's task was last bound by.
    amendments: BTreeMap<String, String>,
    /// What each node settled as.
    settled: BTreeMap<String, String>,
    /// Which nodes were dispatched, and how many times.
    dispatched: BTreeMap<String, usize>,
    /// Every completion the run was asked for, with its reason.
    completions: Vec<String>,
    /// Every surface raised to the planner, as `kind: message`.
    surfaces: Vec<String>,
}

/// A reader written against exactly [`KNOWN_TO_A_PRECEDING_READER`] and nothing
/// else — a consumer downstream of this crate, as one looked before this change.
///
/// This file's own, and deliberately not the build under test: what the claim is
/// about is what *such a reader* reports, and asking this build would be asking
/// the thing the claim is about. It reads the operation list off `edit-committed`
/// the way a consumer written before `operation_kinds` existed had to — there was
/// no field to key on — so it is blind to everything this change added.
fn derived_by_a_preceding_reader(journal: &str) -> Derived {
    let mut read = Derived::default();
    for event in records_of(journal) {
        if event["source"] != "pipeline" {
            continue;
        }
        let kind = event["kind"].as_str().unwrap_or_default().to_string();
        if !KNOWN_TO_A_PRECEDING_READER.contains(&kind.as_str()) {
            continue;
        }
        let node = event["labels"]["node"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let payload = &event["payload"];
        match kind.as_str() {
            "edit-committed" => {
                for operation in payload["operations"].as_array().into_iter().flatten() {
                    match operation["kind"].as_str().unwrap_or_default() {
                        "node-added" => {
                            read.added.insert(
                                text(&operation["node"]["id"]),
                                operation["node"]["deps"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .map(text)
                                    .collect(),
                            );
                        }
                        "edge-added" => {
                            read.edges
                                .insert((text(&operation["from"]), text(&operation["to"])));
                        }
                        "task-amended" => {
                            read.amendments
                                .insert(text(&operation["node"]), text(&operation["text"]));
                        }
                        _ => {}
                    }
                }
            }
            "node-settled" => {
                read.settled.insert(node, text(&payload["status"]));
            }
            "node-dispatched" => *read.dispatched.entry(node).or_default() += 1,
            "completion-requested" => read.completions.push(text(&payload["reason"])),
            "planner-surface-queued" => read.surfaces.push(format!(
                "{}: {}",
                text(&payload["kind"]),
                text(&payload["message"])
            )),
            _ => {}
        }
    }
    read
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

/// The fixture is what it claims to be: a journal from **before** this change,
/// carrying neither the kind it adds nor the field it adds.
///
/// Without this the direction below could be satisfied by a fixture quietly
/// regenerated from a newer build, which would hold nothing at all.
#[test]
fn the_committed_fixture_is_a_journal_from_before_this_change() {
    let kinds = kinds_in(FIXTURE);
    assert!(
        !kinds.contains("command-accepted"),
        "the fixture carries the kind this change adds, so it is not from before it: {kinds:?}"
    );
    assert!(
        kinds.contains("edit-committed"),
        "the fixture carries no committed edit, so it says nothing about the split: {kinds:?}"
    );
    assert!(
        !FIXTURE.contains("operation_kinds"),
        "the fixture carries the field this change adds, so it is not from before it"
    );

    // And it says so in its own version line, which is the machine-readable half
    // of everything above: every record of this library's own in it is at the
    // version before the one this build writes, and that version is one this
    // build still reads.
    assert_eq!(
        own_versions_in(FIXTURE),
        BTreeSet::from([1]),
        "the fixture is not a version-1 journal, so it is not from before this change"
    );
    assert!(
        ENVELOPE_VERSION > 1 && ENVELOPE_VERSIONS_READ.contains(&1),
        "this build either writes the fixture's own version or no longer reads it, and \
         either way the fixture proves nothing about an older one"
    );
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] what the journeys
// below exercise is the fold in `src/projection.rs`, the rendering in `src/views.rs` and
// the record `src/engine.rs` and `src/driver.rs` write — which any change under `src/` can
// move — so a project edged narrower than the crate could not honestly run them. They are
// in the e2e binary because the property is about a **document** meeting a **build**, and
// only the compiled binary is the build.
/// **Direction one.** This build reads a journal written before the change
/// exactly as it read one then: the fold that derives the run's state and the
/// views that render it say what the committed rendering says.
#[test]
fn this_build_reads_a_journal_from_before_the_change_as_it_did() {
    let world = World::new("compat-before");
    let root = world.fakes.join("before-root");
    run_of(&root, "before", FIXTURE);

    // The journal being read is a **version 1** journal, which is the ordinary
    // contents of a runs root this build did not write into: the bump is a
    // statement about what a new record promises, not a line drawn under the old
    // ones.
    assert_eq!(own_versions_in(FIXTURE), BTreeSet::from([1]));

    assert_eq!(
        views_of(&world, &root, "before"),
        RENDERED,
        "this build renders a pre-change journal differently than the committed reading of it"
    );
}

/// The reader below is one written before this change: it knows exactly the
/// record kinds the committed fixture carries, and it cannot have learned the
/// kind this change adds.
#[test]
fn the_preceding_reader_knows_only_the_kinds_the_fixture_carries() {
    let carried = kinds_in(FIXTURE);
    for kind in KNOWN_TO_A_PRECEDING_READER {
        assert!(
            carried.contains(kind),
            "the reader keys on '{kind}', which the fixture does not carry, so it is not \
             a reader written against that journal"
        );
    }
    assert!(
        !KNOWN_TO_A_PRECEDING_READER.contains(&"command-accepted"),
        "the preceding reader has learned the kind this change adds"
    );
}

/// **Direction two.** A reader written against only the record kinds the fixture
/// carries still reports what it reported when it meets a journal this build
/// writes — the new kind for an accepted command that changed no graph included.
///
/// The two journals are the **same run**: the fixture is one recording of
/// [`recorded_run`] and this drives another. So "what it reported" is a value
/// this check has in hand rather than a claim, and the reader has to derive it
/// from both.
#[test]
fn a_reader_from_before_the_change_reads_a_journal_this_build_writes() {
    let world = World::new("compat-after");
    let run = recorded_run(&world, "compatafter");
    let journal = std::fs::read_to_string(world.run_file(&run, "events.jsonl"))
        .expect("the run's journal reads");

    // The journal really is one this build writes: it carries the new kind, the
    // new field, and the version that says both are there — so what follows is
    // about a reader meeting them.
    let kinds = kinds_in(&journal);
    assert!(
        kinds.contains("command-accepted") && journal.contains("operation_kinds"),
        "the run wrote no record this change added, so nothing here is tested: {kinds:?}"
    );
    assert_eq!(
        own_versions_in(&journal),
        BTreeSet::from([u64::from(ENVELOPE_VERSION)]),
        "this build wrote a record of its own at a version other than the one it declares"
    );
    // And the records it **relayed** keep their producer's own version, which is
    // what makes the two numbers in one journal a fact about who wrote each
    // envelope rather than a disagreement.
    assert_eq!(
        relayed_versions_in(&journal),
        BTreeSet::from([1]),
        "a relayed envelope was restamped with this crate's own version"
    );

    // The preceding reader meets all of that — the new kind included — and
    // derives the same run it derives from the version-1 fixture. Both journals
    // are the same scenario, so "what it reported" is a value in hand.
    assert_eq!(
        derived_by_a_preceding_reader(&journal),
        derived_by_a_preceding_reader(FIXTURE),
        "a reader written against only the kinds a pre-change journal carries derives a \
         different run when it meets a journal this build writes"
    );

    // And this build reads both, which is the promise the version line is for: a
    // runs root holding one journal of each renders both, and neither is refused.
    let root = world.fakes.join("both-versions-root");
    run_of(&root, "written-at-2", &journal);
    run_of(&root, "written-at-1", FIXTURE);
    assert_eq!(
        views_of(&world, &root, "written-at-2").replace("written-at-2", "run"),
        views_of(&world, &root, "written-at-1").replace("written-at-1", "run"),
        "the same run written at the two versions this build reads renders differently"
    );
}

/// The refusal path of the same envelope, so the atomicity this change lands is
/// held over a journal a preceding reader reads too: an envelope that refused
/// leaves nothing for either reader to disagree about.
#[test]
fn a_refused_envelope_leaves_the_same_nothing_for_either_reader() {
    let world = World::new("compat-refused");
    world.script("slow.turn-open", "");
    world.script("slow.wait", "hold");
    let path = world.plan(
        "compatrefused",
        &plan_of(
            "compatrefused",
            vec![agent("slow", &[]), agent("later", &["slow"])],
        ),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("a node to be in flight", |world| {
        !world
            .events_of("compatrefused", "node-dispatched")
            .is_empty()
    });

    world
        .run_with_stdin(
            &["reply", "compatrefused"],
            &json!({"version": 2, "commands": [
                {"op": "amend", "id": "later", "text": "## What\nnever applied"},
                {"op": "note", "id": "later", "addressee": "worker",
                 "text": "start from the fixture", "deliver": "live", "persist": false},
            ]})
            .to_string(),
        )
        .exited(REFUSED);

    let journal = std::fs::read_to_string(world.run_file("compatrefused", "events.jsonl"))
        .expect("the run's journal reads");
    let kinds = kinds_in(&journal);
    assert!(
        !kinds.contains("edit-committed") && !kinds.contains("command-accepted"),
        "a refused envelope left a committed record for a reader to find: {kinds:?}"
    );
    // And a reader written before the change derives nothing of it either: no
    // node added, no amendment bound.
    let read = derived_by_a_preceding_reader(&journal);
    assert!(
        read.added.is_empty() && read.amendments.is_empty(),
        "a refused envelope reached a preceding reader: {read:?}"
    );
    world.release("slow.go");
}

// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
