//! A write-back's create of a GitHub board item, interrupted after GitHub filed the issue and
//! before its answer reached the store, retried by the write-back itself: the retry reaches the
//! issue the interrupted attempt made rather than filing a second.
//!
//! GitHub is `tests/e2e/github_world.py`, the loopback that executes every document the linked
//! `github-projects` plugin sends against a model of repositories, boards and issues; its
//! `/drop-create` runs one chosen `createIssue` and then closes the connection unanswered.
//! Everything else is the compiled binary and the real linked store.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// GitHub is the loopback beside this file — real HTTP on a real socket, reached by the real
// `github-projects` plugin the engine links — because an offline check may not reach the
// hosted API. `harness.rs` carries the same suppression and the full rationale.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::World;
use crate::linear_writeback::{from_the_launch_directory, projection_records};
use crate::private_github_home::{described, metadata_of, GitHub};

/// The repository the board files into.
const REPOSITORY: &str = "owner/service";
/// The board homing the plan.
const BOARD: u64 = 1;
/// The environment variable the board source reads its token from.
const TOKEN_ENV: &str = "ONEPIPELINE_LOOPBACK_BOARD_TOKEN";
/// The node the live edit adds, whose create is interrupted.
const ADDED: &str = "examples";
/// The source the write-back's shadow store is configured as, which every shadow id names.
const SHADOW_SOURCE: &str = "onepipeline-writeback";

/// The shadow document the write-back copies `node` from, under the run's `writeback` directory.
fn shadow_of(world: &World, run: &str, node: &str) -> PathBuf {
    let tasks = world.run_file(run, "writeback/tasks");
    let found: Vec<PathBuf> = std::fs::read_dir(&tasks)
        .unwrap_or_else(|e| panic!("{} cannot be listed: {e}", tasks.display()))
        .flat_map(|project| {
            std::fs::read_dir(project.expect("a project's shadow").path())
                .expect("a project's shadow tasks")
                .map(|entry| entry.expect("a shadow task").path())
        })
        .filter(|path| {
            std::fs::read_to_string(path)
                .is_ok_and(|text| text.contains(&format!("onepipeline.id: {node}")))
        })
        .collect();
    assert_eq!(found.len(), 1, "one shadow carries {node}: {found:?}");
    found.into_iter().next().expect("one shadow")
}

/// The metadata a shadow document's front matter records.
fn shadow_metadata(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).expect("the shadow document");
    let front = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
        .map(|(front, _)| front)
        .unwrap_or_else(|| panic!("{} has no front matter:\n{text}", path.display()));
    let front: Value = serde_saphyr::from_str(front)
        .unwrap_or_else(|e| panic!("{} front matter: {e}\n{text}", path.display()));
    front["metadata"].clone()
}

/// A live addition whose create GitHub ran and whose answer was then dropped is retried by the
/// write-back, and the retry finishes the issue the interrupted attempt filed: the board holds
/// exactly one issue for it, carrying the shadow's origin, the shadow links that issue and holds
/// no create in flight, and the write-back record says the retry created nothing.
#[test]
fn a_write_back_create_interrupted_after_the_board_filed_it_reaches_that_issue_on_retry() {
    let name = "github-create-retry";
    let world = World::new(name);
    // The checkout every node of the plan names, registered as `owner/service`.
    let _service = world.repository("local-direct", &[]);
    let world = world.with_env(TOKEN_ENV, "loopback-token");
    world.script("anchor.wait", "hold");
    let github = GitHub::serve(
        &world,
        &json!({
            "owner": "owner",
            "repositories": {REPOSITORY: "public"},
            "boards": {BOARD.to_string(): {"public": true}},
            "issues": [
                {"id": "plan", "repository": REPOSITORY, "title": name, "board": BOARD,
                 "status": "Todo", "parent": null,
                 "body": described("## Plan\n- Retry a create safely.", &json!({
                     "onepipeline.schema_version": onepipeline::plan::PLAN_SCHEMA_VERSION,
                     "onepipeline.concurrency": 1,
                     "onepipeline.goal": {"text": "Deliver the plan"},
                 }))},
                {"id": "anchor", "repository": REPOSITORY, "title": "chore: anchor",
                 "board": BOARD, "status": "Todo", "parent": "plan",
                 "body": described(
                     "## What\nDo anchor.\n\n## Why\nSo the run can settle.\n\n\
                      ## Acceptance criteria\n- anchor is done.",
                     &json!({"onepipeline.id": "anchor", "onepipeline.persona": "engineer"}))},
            ],
        }),
    );
    let launch = world.root.join("launch-directory");
    std::fs::create_dir_all(&launch).expect("the launch directory");
    std::fs::write(
        launch.join("onetaskgraph.yaml"),
        format!(
            "sources:\n  board:\n    plugin: github-projects\n    config:\n      owner: owner\n      \
             project_number: {BOARD}\n      repository: {REPOSITORY}\n      token_env: {TOKEN_ENV}\n      \
             endpoint: {}\n      status_mapping:\n        todo: Todo\n        queued: Queued\n        \
             in-progress: In Progress\n        unknown: Needs Attention\n        done: Done\n        \
             cancelled: Canceled\n",
            github.board(BOARD),
        ),
    )
    .expect("the store configuration");
    let start = from_the_launch_directory(&world, &launch, &["start", "board:plan", "--detach"]);
    world.run_on(start, "start the GitHub home").exited(0);
    world.until("the anchor to dispatch", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });

    // GitHub will file the added node's issue and drop its answer.
    github.drop_create(ADDED);
    let mut node = crate::harness::agent(ADDED, &[]);
    node["repo"] = json!(format!("github.com/{REPOSITORY}"));
    node["parked"] = json!(true);
    world
        .run_with_stdin(
            &["reply", name],
            &json!({"version": 3, "commands": [{"op": "add", "node": node}]}).to_string(),
        )
        .exited(0);
    world.until(
        "the write-back to retry the interrupted create and land",
        |world| {
            projection_records(world, name).iter().any(|record| {
                record["outcome"] == "projected" && record.to_string().contains(ADDED)
            })
        },
    );
    let records: Vec<Value> = projection_records(&world, name)
        .into_iter()
        .filter(|record| record.to_string().contains(ADDED))
        .collect();
    let state = github.state();

    // The interrupted attempt failed, and the one after it reached what it had made.
    let (failed, landed): (Vec<&Value>, Vec<&Value>) = records
        .iter()
        .partition(|record| record["outcome"] != "projected");
    assert!(
        !failed.is_empty(),
        "the dropped create failed an attempt: {records:#?}\nunsupported: {}",
        state["unsupported"]
    );
    assert_eq!(landed.len(), 1, "one attempt landed: {records:#?}");
    assert_eq!(
        landed[0]["actions"]["created"], 0,
        "the retry created nothing, reaching the issue the interrupted create filed: {records:#?}"
    );

    // Exactly one issue carries the shadow's origin — the id the store copies the added node
    // from — in its body and in its board item's origin field, and it carries the added node.
    let shadow = shadow_of(&world, name, ADDED);
    let metadata = shadow_metadata(&shadow);
    // The origin is an id, its segments joined by `/` on every platform, not a path
    // in the host's own spelling.
    let source = format!(
        "{SHADOW_SOURCE}:{}",
        shadow
            .strip_prefix(world.run_file(name, "writeback/tasks"))
            .expect("under the shadow's tasks")
            .with_extension("")
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    );
    let carrying: Vec<(&String, &Value)> = state["issues"]
        .as_object()
        .expect("the model's issues")
        .iter()
        .filter(|(_, issue)| {
            metadata_of(issue["body"].as_str().unwrap_or_default()).get("onetaskgraph.origin")
                == Some(&json!(source))
        })
        .collect();
    assert_eq!(
        carrying.len(),
        1,
        "exactly one issue carries the origin {source}: {:#}",
        state["issues"]
    );
    let (issue_id, issue) = carrying[0];
    assert_eq!(
        metadata_of(issue["body"].as_str().unwrap_or_default()).get("onepipeline.id"),
        Some(&json!(ADDED)),
        "{issue:#}"
    );
    let items: Vec<&Value> = state["items"]
        .as_object()
        .expect("the model's items")
        .values()
        .filter(|item| item["content"] == json!(issue_id))
        .collect();
    assert_eq!(
        items.len(),
        1,
        "the issue is one board item: {:#}",
        state["items"]
    );
    assert_eq!(
        items[0]["values"][format!("ORIGIN_{BOARD}")],
        json!(source),
        "the board item records the shadow's origin: {:#}",
        items[0]
    );

    // The shadow links that issue and holds no create in flight.
    assert_eq!(
        metadata["onetaskgraph.copies"]["board"],
        json!(format!("board:{issue_id}")),
        "the shadow links the one issue: {metadata:#}"
    );
    assert!(
        metadata.get("onetaskgraph.creating").is_none(),
        "the shadow holds no create in flight: {metadata:#}"
    );
    world.release("anchor.go");
}
