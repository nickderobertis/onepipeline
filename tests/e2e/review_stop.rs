//! A node that stopped for review, as its plan's board shows it.
//!
//! A lifecycle node that settles `done` with its change unlanded and an outcome a person reads
//! before it can land — `change-open`, or a draft waiting on its author: `change-draft`,
//! `change-review-draft`, `change-review-draft-unchecked` — reads `in-review` on a board that
//! has a review column, and `done` on one that does not, in the settlement's one write. A draft
//! waiting on its author reads what the launch config's `writeback.review_stop.author_draft`
//! says — `in-review` by default, or `in-progress` — with a per-source override winning for a
//! plan homed in that source. Once the run records the node's landing as `landed`, its task reads
//! `done`.
//!
//! Each journey drives the compiled binary over the real linked `onevcs`, real git and a real
//! origin on disk, its plan homed in the world's folder of Markdown — whose shipped vocabulary
//! has the review column — or in the loopback Linear the e2e suite serves.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary over the real linked `onevcs` and the real linked plan store. `oneagentgraph` is
// substituted at its subprocess boundary so a journey states a dispatch outcome rather than
// paying for a model turn, GitHub at `onevcs`'s own `ONEVCS_GH` seam so a change request's
// checks and draft state can be stated offline, and Linear by the loopback that answers the
// linked plugin's own GraphQL. `harness.rs` carries the same suppression and the full rationale.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::draft_lifecycle::{rules, team_repository, GREEN};
use crate::harness::{lifecycle, plan_of, World};

/// The world's plan source, as its store names it.
const HOME: &str = "plans";

/// The status one node's task reads on the board `project` is homed in: its category and the
/// name the store holds it under.
fn board(world: &World, project: &str, node: &str) -> (String, String) {
    let tasks = world.store_tasks(project);
    let task = tasks
        .iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .unwrap_or_else(|| panic!("the board holds no task for {node}: {tasks:#?}"));
    let status = &task["item"]["status"];
    (
        status["category"].as_str().unwrap_or_default().to_owned(),
        status["name"].as_str().unwrap_or_default().to_owned(),
    )
}

/// Wait until `node`'s task reads `category` on the board, and every projection attempt of
/// `run` has landed.
fn reads(world: &World, run: &str, project: &str, node: &str, category: &str) {
    world.until_store(&format!("{node} to read {category}"), |world| {
        board(world, project, node).0 == category && every_attempt_landed(world, run)
    });
}

/// Whether every projection attempt `run` recorded landed.
fn every_attempt_landed(world: &World, run: &str) -> bool {
    projections(world, run)
        .iter()
        .all(|record| record["outcome"] == "projected")
}

/// Every attempt `run`'s write-back recorded.
fn projections(world: &World, run: &str) -> Vec<Value> {
    let path = world.run_file(run, "writeback-projections.jsonl");
    std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("a projection record"))
        .collect()
}

/// The outcome `node` settled on in `run`.
fn outcome(world: &World, run: &str, node: &str) -> String {
    crate::stacked_drafts::settled(world, run, node)["outcome"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// A lifecycle node the plan asks to leave as a draft for a person to read.
fn plan_draft(id: &str) -> Value {
    let mut node = lifecycle(id, &[]);
    node["draft"] = json!(true);
    node
}

/// A launch config at schema 16 carrying `writeback`, written under `world`.
fn launch_config(world: &World, name: &str, writeback: &str) -> String {
    let path = world.root.join(format!("{name}.launch.yaml"));
    std::fs::write(
        &path,
        format!("schema_version: 16\nwriteback:\n{writeback}"),
    )
    .expect("the launch config is written");
    path.to_string_lossy().into_owned()
}

/// A team repository — `change-open`, approvals required — whose checks come back green: a
/// change is kept as a draft for its user's review, and a node the plan asked to leave as a
/// draft is left as one.
fn team_world(name: &str) -> World {
    let world = World::new(name);
    team_repository(&world);
    world.script("gh.checks", GREEN);
    world
}

/// A repository whose change is lifted for someone to merge: `change-open`, no approvals.
fn open_world(name: &str) -> World {
    let world = World::new(name);
    world.repository("change-open", &[]);
    rules(&world, "change-open", "none", None);
    world.script("gh.checks", GREEN);
    world
}

/// On the world's folder of Markdown, whose shipped vocabulary has a review column, a change
/// kept as a draft for its user's review and a draft the plan asked for each read `in-review`
/// once the run settles, the node itself `done` and its dependent dispatched — and once a
/// `settle` records the review draft's landing as `landed`, its task reads `done`, while the
/// plan's own draft, still unlanded, keeps reading `in-review`.
#[test]
fn a_draft_waiting_on_its_author_reads_in_review_and_done_once_its_landing_is_recorded() {
    let world = team_world("review-stop-drafts");
    world.script("review.work", "the reviewed change\n");
    world.script("held.work", "the held change\n");
    // The dependent holds the run open, so its driver is the one projecting the `settle`.
    world.script("after.wait", "hold");
    let run = "reviewstops";
    let project = world.plan(
        run,
        &plan_of(
            run,
            vec![
                lifecycle("review", &[]),
                plan_draft("held"),
                crate::harness::agent("after", &["review"]),
            ],
        ),
    );
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    reads(&world, run, &project, "review", "in-review");
    reads(&world, run, &project, "held", "in-review");
    assert_eq!(outcome(&world, run, "review"), "change-review-draft");
    assert_eq!(outcome(&world, run, "held"), "change-draft");
    assert_eq!(
        board(&world, &project, "review"),
        ("in-review".to_owned(), "in review".to_owned())
    );
    // The node is `done` as before: its dependent was dispatched.
    world.until("the dependent to dispatch", |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "after")
    });

    let url = crate::stacked_drafts::settled(&world, run, "review")["change_url"]
        .as_str()
        .expect("the review draft's change request")
        .to_owned();
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": onepipeline::channel::REPLY_ENVELOPE_VERSION, "commands": [{
                "op": "settle", "id": "review", "outcome": "done",
                "evidence": "its change request merged", "landing": url,
            }]})
            .to_string(),
        )
        .exited(0);
    reads(&world, run, &project, "review", "done");
    assert_eq!(board(&world, &project, "held").0, "in-review");

    world.release("after.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(world.run_json(run, "result.json")["state"], "complete");
    reads(&world, run, &project, "after", "done");
    assert_eq!(board(&world, &project, "held").0, "in-review");
}
