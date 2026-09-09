//! Compatibility across the journal this change adds to, in both directions.
//!
//! Splitting `edit-committed` into two kinds and adding a field to the record is
//! a change to a **durable document** that outlives the build that wrote it: a
//! runs root holds journals from every build that ever ran on the host, and this
//! crate is not the only thing that reads one. So the property is held both ways
//! round, and neither direction is inferable from the other.
//!
//! * **A journal from before the change reads the same.** This build folds one
//!   and renders it, and the rendering is committed beside it — so a later change
//!   to how a pre-change record is read fails here rather than quietly changing
//!   what an old run says.
//! * **A reader from before the change reads a journal this build writes.** Such
//!   a reader knows exactly the record kinds the committed fixture carries and no
//!   others, so it is *modelled* by taking a journal this build wrote and
//!   removing every record of a kind that fixture does not carry — the new
//!   `command-accepted` included. What it reports has to be what it reported,
//!   which here means: the same as the whole journal says.
//!
//! [`FIXTURE`] is the immutable reference for what a preceding reader knows: the
//! journal of a real run of [`recorded_run`], carrying the record shapes the
//! build at `4db4e7e` — the commit this work forked from — wrote. The two things
//! this change adds to a record are the whole of what was put back, because they
//! are the whole of what it adds: the kind an accepted command that changes no
//! graph is journalled under, and the `operation_kinds` field. Host paths and
//! stream names are stood in for so the document is portable; nothing here reads
//! either. It is committed and **never regenerated** — a fixture edited to match
//! a newer build proves nothing about an older one, which is what
//! [`the_committed_fixture_is_a_journal_from_before_this_change`] holds it to.

use crate::harness::{agent, plan_of, World, REFUSED};

use serde_json::{json, Value};
use std::collections::BTreeSet;
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

    world.release("slow.go");
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

/// Every event kind one journal carries.
fn kinds_in(journal: &str) -> BTreeSet<String> {
    journal
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|event| event["kind"].as_str().map(str::to_string))
        .collect()
}

/// Lay one journal down as a run of its own under `root`, with the launch record
/// every view opens a run by.
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

/// A journal with every record of a kind `known` does not carry removed: what a
/// reader written against exactly those kinds sees of it.
fn as_a_preceding_reader_sees(journal: &str, known: &BTreeSet<String>) -> String {
    let mut kept = String::new();
    for line in journal.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value = serde_json::from_str(line).expect("a journal line is JSON");
        let kind = event["kind"].as_str().unwrap_or_default();
        if event["source"] == "pipeline" && !known.contains(kind) {
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
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
}

/// **Direction one.** This build reads a journal written before the change
/// exactly as it read one then: the fold that derives the run's state and the
/// views that render it say what the committed rendering says.
#[test]
fn this_build_reads_a_journal_from_before_the_change_as_it_did() {
    let world = World::new("compat-before");
    let root = world.fakes.join("before-root");
    run_of(&root, "before", FIXTURE);

    assert_eq!(
        views_of(&world, &root, "before"),
        RENDERED,
        "this build renders a pre-change journal differently than the committed reading of it"
    );
}

/// **Direction two.** A reader that knows only the kinds the fixture carries
/// still reports what it reported when it meets a journal this build writes —
/// the new kind for an accepted command that changed no graph included.
#[test]
fn a_reader_from_before_the_change_reads_a_journal_this_build_writes() {
    let world = World::new("compat-after");
    let run = recorded_run(&world, "compatafter");
    let journal = std::fs::read_to_string(world.run_file(&run, "events.jsonl"))
        .expect("the run's journal reads");

    // The journal really is one this build writes: it carries the new kind and
    // the new field, so what follows is about a reader meeting them.
    let kinds = kinds_in(&journal);
    assert!(
        kinds.contains("command-accepted") && journal.contains("operation_kinds"),
        "the run wrote no record this change added, so nothing here is tested: {kinds:?}"
    );

    // Laid down twice under one root: whole, and as a reader written against only
    // the fixture's kinds sees it. Rendered by the same binary, so what differs
    // is the journal and nothing else.
    let root = world.fakes.join("after-root");
    run_of(&root, "whole", &journal);
    run_of(
        &root,
        "preceding",
        &as_a_preceding_reader_sees(&journal, &kinds_in(FIXTURE)),
    );

    assert_eq!(
        views_of(&world, &root, "preceding").replace("preceding", "whole"),
        views_of(&world, &root, "whole"),
        "a reader that knows only the kinds a pre-change journal carries reports something \
         else when it meets a journal this build writes"
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
    world.release("slow.go");
}
