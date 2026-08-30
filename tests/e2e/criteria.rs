//! A settling node's criteria, read against the branch it settled on.
//!
//! Where a criterion names a literal value in a named file, the file is on the
//! node's own branch and the comparison costs one read. These journeys drive
//! that against a **real** branch: a real `onevcs` session over a real git
//! origin on disk, with a real dispatch writing a real file into the worktree,
//! and the answer read back off the run's own record and the manager's own
//! queue — never off an internal.
//!
//! What is proven here is as much what the check *declines* to say as what it
//! says. A criterion it cannot parse is silence; a file it cannot read is a
//! third answer of its own; and a mismatch never changes what the node settled
//! on. See `docs/contract-divergences.md` entry 47.

// llmlint: ignore-file[e2e_not_mocked] the same substitution every journey in this suite
// makes and no other: `oneagentgraph` stands in at its subprocess boundary so a dispatch
// states what it wrote instead of paying for a model turn, and GitHub stands in at
// `onevcs`'s own `ONEVCS_GH` override. The crate under test is the compiled binary, and
// the branch these journeys read is a real git branch in a real session worktree — which
// is the whole point of them. `harness.rs` carries the full rationale.

use crate::harness::{git, plan_of, Repository, World};
use serde_json::{json, Value};

/// The file the scripted dispatch writes into the node's worktree.
///
/// The double names a dispatch's work after the node it ran for, so a criterion
/// here names this and the journey's `work` script decides what it holds.
const WORK: &str = "service.md";

/// One lifecycle node whose bar is the criteria given, stated as a plan states
/// them.
fn node_bounded_by(criteria: &[&str]) -> Value {
    let bar = criteria
        .iter()
        .map(|criterion| format!("- {criterion}"))
        .collect::<Vec<_>>()
        .join("\n");
    json!({
        "id": "service",
        "repo": "service",
        "persona": "engineer",
        "title": "feat: ship the row",
        "task": format!(
            "## What\nWrite the row.\n\n## Why\nA consumer reads it.\n\n\
             ## Acceptance criteria\n\n{bar}\n"
        ),
    })
}

/// Drive one plan to settlement and hand back the run's name.
fn settle(world: &World, name: &str, nodes: Vec<Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

/// A world with a repository that publishes with git alone, and a dispatch that
/// writes `body` into [`WORK`] on the node's branch.
fn writing(name: &str, body: &str) -> World {
    let world = World::new(name);
    world.repository("local-direct", &[]);
    world.script("service.work", body);
    world
}

/// Every comparison one run recorded, as the run's own record carries it.
fn comparisons(world: &World, run: &str) -> Vec<Value> {
    world
        .events_of(run, "criterion-checked")
        .into_iter()
        .map(|event| event["payload"].clone())
        .collect()
}

/// Every finding one run raised to its manager.
fn findings(world: &World, run: &str) -> Vec<String> {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == "finding")
        .filter_map(|event| {
            event["payload"]["message"]
                .as_str()
                .map(std::string::ToString::to_string)
        })
        .collect()
}

/// How a node settled, in the three fields that say it.
///
/// Not the whole record: two runs cut two branches and name two sessions, and a
/// comparison including those would fail on the one difference that has nothing
/// to do with what is being compared.
fn settled_as(world: &World, run: &str) -> Value {
    let node = world.run_json(run, "result.json")["nodes"][0].clone();
    json!({
        "status": node["status"],
        "outcome": node["outcome"],
        "detail": node["detail"],
    })
}

#[test]
fn a_criterion_the_branch_contradicts_is_a_finding_naming_the_file_and_both_values() {
    let world = writing("criteria-mismatch", "complete_dataset: false\n");
    let run = settle(
        &world,
        "contradicted",
        vec![node_bounded_by(&[
            "the shared journey row in `service.md` is `complete_dataset: true`",
        ])],
    );

    // The comparison is on the run's own record, as a mismatch, carrying both
    // values: the one the bar named and the one the branch holds.
    let compared = comparisons(&world, &run);
    assert_eq!(compared.len(), 1, "{compared:?}");
    assert_eq!(compared[0]["answer"], "mismatch", "{compared:?}");
    assert_eq!(compared[0]["file"], WORK);
    assert_eq!(compared[0]["expected"], "complete_dataset: true");
    assert_eq!(compared[0]["holds"], "`complete_dataset: false`");

    // And the manager is handed a finding naming all four, because a manager
    // who has to open the branch to learn what the run already read is being
    // told nothing.
    let raised = findings(&world, &run);
    assert_eq!(raised.len(), 1, "{raised:?}");
    for named in [
        "the shared journey row in `service.md` is `complete_dataset: true`",
        WORK,
        "complete_dataset: true",
        "complete_dataset: false",
    ] {
        assert!(
            raised[0].contains(named),
            "the finding does not name {named}:\n{}",
            raised[0]
        );
    }
}

#[test]
fn a_criterion_the_branch_holds_raises_nothing() {
    let world = writing("criteria-match", "complete_dataset: true\n");
    let run = settle(
        &world,
        "held",
        vec![node_bounded_by(&[
            "the shared journey row in `service.md` is `complete_dataset: true`",
        ])],
    );

    // Read and answered — the comparison is on the record, so this is a check
    // that ran rather than one that was skipped …
    let compared = comparisons(&world, &run);
    assert_eq!(compared.len(), 1, "{compared:?}");
    assert_eq!(compared[0]["answer"], "match", "{compared:?}");
    // … and nothing was raised for it. A tier that reported every comparison it
    // made is one a manager learns to skim.
    assert_eq!(findings(&world, &run), Vec::<String>::new());
}

#[test]
fn a_contradicted_criterion_settles_the_node_exactly_as_an_uncontradicted_one_does() {
    // Two runs over the same repository shape, the same dispatch and the same
    // file on the branch. The one difference is the sentence the bar is written
    // in: one names the row and the value, and the other says the same thing in
    // prose this check cannot parse.
    let contradicted = writing("criteria-unchanged-a", "complete_dataset: false\n");
    let control = writing("criteria-unchanged-b", "complete_dataset: false\n");
    let checked = settle(
        &contradicted,
        "contradicted",
        vec![node_bounded_by(&[
            "the shared journey row in `service.md` is `complete_dataset: true`",
        ])],
    );
    let plain = settle(
        &control,
        "plain",
        vec![node_bounded_by(&["the shared journey row is complete"])],
    );

    // The node settled on its own work, both times, identically: the check is
    // beside the settlement and never part of it.
    assert_eq!(
        settled_as(&contradicted, &checked),
        settled_as(&control, &plain)
    );
    assert_eq!(
        contradicted.run_json(&checked, "result.json")["state"],
        control.run_json(&plain, "result.json")["state"]
    );
    assert_eq!(
        settled_as(&contradicted, &checked)["status"],
        "done",
        "the journey did not reach the settlement it is comparing"
    );

    // And the finding is beside it: raised on the run whose branch contradicted
    // its bar, and on no other.
    assert_eq!(findings(&contradicted, &checked).len(), 1);
    assert_eq!(findings(&control, &plain), Vec::<String>::new());
    assert_eq!(comparisons(&control, &plain), Vec::<Value>::new());
}

#[test]
fn criteria_this_check_cannot_parse_are_reported_nowhere() {
    let world = writing("criteria-prose", "complete_dataset: false\n");
    let run = settle(
        &world,
        "prose",
        vec![node_bounded_by(&[
            // Ordinary prose, naming neither half.
            "the row reads the way the consumer expects",
            // A file, and no literal to hold it to.
            "`service.md` is tidier than it was",
            // A literal, and no file to look in.
            "the row is `complete_dataset: true`",
            // Both halves, in a sentence whose meaning is the absence — which
            // this reading would answer backwards.
            "`service.md` no longer holds `complete_dataset: false`",
        ])],
    );

    // Nothing compared, nothing raised, and no warning that something was
    // skipped: a criterion this check cannot parse is silence, because a
    // checker that guessed would report false findings on sound work.
    assert_eq!(comparisons(&world, &run), Vec::<Value>::new());
    assert_eq!(findings(&world, &run), Vec::<String>::new());
    assert_eq!(
        settled_as(&world, &run)["status"],
        "done",
        "the journey did not reach a settlement"
    );
}

#[test]
fn a_file_the_branch_will_not_give_up_is_the_check_declining_to_answer() {
    let world = World::new("criteria-unread");
    let repository = world.repository("local-direct", &[]);
    // A directory where the criterion names a file. It is on the branch — this
    // journey commits it and pushes it — so the check reaches it, opens it, and
    // gets nothing it can compare.
    unreadable(&world, &repository, "rows.md");
    world.script("service.work", "complete_dataset: false\n");
    let run = settle(
        &world,
        "unread",
        vec![node_bounded_by(&[
            "the shared journey row in `rows.md` is `complete_dataset: true`",
        ])],
    );

    // The third answer, in the run's own record, told apart from both the
    // others: nothing was compared, so nothing disagreed.
    let compared = comparisons(&world, &run);
    assert_eq!(compared.len(), 1, "{compared:?}");
    assert_eq!(compared[0]["answer"], "unread", "{compared:?}");
    assert_ne!(compared[0]["answer"], "match");
    assert_ne!(compared[0]["answer"], "mismatch");
    assert!(
        compared[0]["reason"]
            .as_str()
            .is_some_and(|why| !why.is_empty()),
        "the check declined to answer and said nothing about why: {compared:?}"
    );
    assert!(
        compared[0].get("holds").is_none(),
        "a file nobody could read was reported as holding something: {compared:?}"
    );

    // A file that could not be read is not work that disagreed, so nobody is
    // asked to rule on it.
    assert_eq!(findings(&world, &run), Vec::<String>::new());
}

/// Commit a directory where a criterion will name a file.
///
/// git tracks files rather than directories, so the directory is made by
/// committing something inside it — which is exactly how one arrives in a real
/// tree, and leaves the named path unreadable as a file for any reader.
fn unreadable(world: &World, repository: &Repository, path: &str) {
    let blocked = repository.checkout.join(path);
    std::fs::create_dir_all(&blocked).expect("a directory where a file was named");
    std::fs::write(blocked.join("keep.txt"), "this path is a directory\n")
        .expect("the directory has something in it to be committed");
    git(world, &repository.checkout, &["add", "-A"]);
    git(
        world,
        &repository.checkout,
        &["commit", "-m", "chore: a directory where a file was named"],
    );
    git(world, &repository.checkout, &["push", "origin", "main"]);
}
