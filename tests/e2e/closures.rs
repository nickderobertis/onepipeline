//! The plan-store items whose GitHub issues a landing closed, moved to `done` by the store the
//! run already loads: entry 132 of `docs/contract-divergences.md`.
//!
//! Every journey here lands real work through the compiled binary and the linked `onevcs` —
//! a `local-direct` squash on a real repository, or a change request against the `gh`
//! stand-in — and reads what reached the boards back off the GitHub loopback
//! (`github_loopback.py`), which serves numbered issues by repository and number and logs every
//! request under the board that sent it. Nothing here names a source to the engine: each
//! launch directory's `onetaskgraph.yaml` is all the store configuration there is, and the
//! store decides which of its sources hold an item for each issue.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{agent, lifecycle, plan_of, World};
use crate::linear_loopback::Linear;
use crate::linear_writeback::GitHubEndpoint;

/// The variable every loopback board's source reads its token from.
const TOKEN_ENV: &str = "ONEPIPELINE_LOOPBACK_BOARD_TOKEN";

/// The variable the Linear source reads its key from.
const LINEAR_KEY_ENV: &str = "ONEPIPELINE_LOOPBACK_LINEAR_KEY";

/// The repository every journey lands in, as its origin names it.
const REPOSITORY: &str = "owner/service";

/// A board's status mapping, indented as it sits under the source's `config`.
const BOARD_MAPPING: &str = "      status_mapping:
        todo: Todo
        queued: Queued
        in-progress: In Progress
        unknown: Needs Attention
        done: Done
        cancelled: Canceled
";

/// One loopback board's source, by its number, as a launch directory declares it.
fn board_source(name: &str, served: &GitHubEndpoint, number: u64) -> String {
    format!(
        "  {name}:\n    plugin: github-projects\n    config:\n      owner: owner\n      \
         project_number: {number}\n      token_env: {TOKEN_ENV}\n      endpoint: {}\n\
         {BOARD_MAPPING}",
        served.board(number)
    )
}

/// A launch directory under `world` whose `onetaskgraph.yaml` declares `sources` and nothing
/// else — no setting naming a closure of any kind.
fn launch_dir(world: &World, sources: &str) -> PathBuf {
    let dir = world.root.join("launch");
    std::fs::create_dir_all(&dir).expect("the launch directory");
    std::fs::write(
        dir.join("onetaskgraph.yaml"),
        format!("sources:\n{sources}"),
    )
    .expect("the store configuration");
    dir
}

/// A world whose loopback GitHub serves `issues` and whose one repository publishes `policy`.
fn world_with(name: &str, policy: &str, issues: &Value) -> (World, GitHubEndpoint) {
    let world = World::new(name)
        .with_env(TOKEN_ENV, "loopback-token")
        .with_env(LINEAR_KEY_ENV, "a-loopback-key");
    world.repository(policy, &[]);
    let served = GitHubEndpoint::serve_issues(&world, issues);
    (world, served)
}

/// Issue `number` of the repository, with its state and the reason it was closed for.
fn issue(state: &str, reason: Option<&str>) -> Value {
    json!({"state": state, "reason": reason})
}

/// The one board every journey but the first reads: issue 7 open at `Todo`, issue 9 open at
/// `Todo` and never named.
fn one_board() -> Value {
    json!({
        "repository": REPOSITORY,
        "issues": {"7": issue("OPEN", None), "9": issue("OPEN", None)},
        "boards": {"1": {"7": "Todo", "9": "Todo"}},
    })
}

/// Start `project` from `dir`, detached.
fn start_from(world: &World, dir: &Path, project: &str) {
    let mut start = world.cmd(&["start", project, "--detach"]);
    start.current_dir(dir);
    world.run_on(start, "start --detach").exited(0);
}

/// Every `landing-closures` record of `run`.
fn closures_of(world: &World, run: &str) -> Vec<Value> {
    world.events_of(run, "landing-closures")
}

/// What the loopback's request log holds, by the path each request was sent to.
fn log_of(served: &GitHubEndpoint) -> Vec<Value> {
    served.state()["log"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every write the loopback took for a numbered issue.
fn writes_of(served: &GitHubEndpoint) -> Vec<Value> {
    served.state()["writes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// No request the loopback logged read a board's `items` or searched: the store read the
/// named issues by repository and number and nothing else.
fn read_no_board_whole(served: &GitHubEndpoint) {
    for request in log_of(served) {
        let query = request["query"].as_str().unwrap_or_default();
        assert!(
            !query.contains("items(") && !query.contains("search("),
            "a request read a board's items or searched: {request}"
        );
    }
}

/// The card board `board` holds for issue `number`, and the issue's own state and reason.
fn card(served: &GitHubEndpoint, board: &str, number: &str) -> (Value, Value) {
    let state = served.state();
    (
        state["numbered"]["boards"][board][number].clone(),
        state["numbered"]["issues"][number].clone(),
    )
}

/// The commit `main` stands at on the world's origin.
fn landed_on_main(world: &World) -> String {
    crate::harness::git(
        world,
        &world.root.join("origin.git"),
        &["rev-parse", "main"],
    )
    .trim()
    .to_owned()
}

/// The two-board world of the first journey: board A holds issue 7 open, issue 8 closed as not
/// planned, issue 9 open and issue 10 closed as completed, each at `Todo`; board B holds only
/// issue 9, at `Todo`.
fn two_boards() -> Value {
    json!({
        "repository": REPOSITORY,
        "issues": {
            "7": issue("OPEN", None),
            "8": issue("CLOSED", Some("NOT_PLANNED")),
            "9": issue("OPEN", None),
            "10": issue("CLOSED", Some("COMPLETED")),
        },
        "boards": {
            "1": {"7": "Todo", "8": "Todo", "9": "Todo", "10": "Todo"},
            "2": {"9": "Todo"},
        },
    })
}

/// A landing on a real `local-direct` repository whose commit closes issue 7 and fixes issue 8
/// moves exactly board A's card for issue 7 to `Done` and closes the issue as completed, with
/// every source the launch directory configures asked what it can hold and no setting naming
/// any: board A's not-planned issue 8 is skipped, board B — which holds neither — is asked once
/// and written nothing, the `local-md` and Linear sources are sent nothing, and the run
/// journals one `landing-closures` record naming the landing commit and what the store did.
#[test]
fn a_landing_moves_exactly_the_cards_of_the_issues_it_closes_on_every_configured_board() {
    let (world, served) = world_with("closures-two-boards", "local-direct", &two_boards());
    let linear = Linear::serve(
        &[("Todo", "unstarted"), ("Done", "completed")],
        &[("Planned", "planned"), ("Completed", "completed")],
        "closures",
        "",
        "Planned",
    );
    let notes = world.root.join("notes");
    std::fs::create_dir_all(&notes).expect("the notes source's root");
    std::fs::write(notes.join("README.txt"), "notes nobody writes\n").expect("a notes file");
    let launch = launch_dir(
        &world,
        &format!(
            "{}{}  notes:\n    plugin: local-md\n    config:\n      root: {}\n  tracker:\n    \
             plugin: linear\n    config:\n      api_key_env: {LINEAR_KEY_ENV}\n      team: {}\n      \
             endpoint: {}\n      status_mapping:\n        todo: Todo\n        done: Done\n",
            board_source("board-a", &served, 1),
            board_source("board-b", &served, 2),
            notes.display(),
            crate::linear_loopback::TEAM,
            linear.endpoint(),
        ),
    );
    world.script("service.work", "the worker wrote this\n");
    world.script(
        "service.commit-saying",
        "feat: ship the service\n\nCloses #7\nFixes owner/service#8",
    );
    let run = "two-boards";
    let project = world.plan(run, &plan_of(run, vec![lifecycle("service", &[])]));
    let before = served.state();
    start_from(&world, &launch, &project);
    world.until("the landing's closures to be recorded", |world| {
        !closures_of(world, run).is_empty() && world.run_file(run, "result.json").is_file()
    });

    // Board A's card for the closed issue reads `Done`, and the issue is closed as completed.
    assert_eq!(
        card(&served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
    // Every other card and issue is exactly as it was.
    let after = served.state();
    for number in ["8", "9", "10"] {
        assert_eq!(
            after["numbered"]["boards"]["1"][number], before["numbered"]["boards"]["1"][number],
            "board A's card for issue {number} moved"
        );
        assert_eq!(
            after["numbered"]["issues"][number], before["numbered"]["issues"][number],
            "issue {number} moved"
        );
    }
    assert_eq!(
        after["numbered"]["boards"]["2"], before["numbered"]["boards"]["2"],
        "board B moved"
    );
    // Board B was asked once, about the named issues, and written nothing.
    let board_b: Vec<Value> = log_of(&served)
        .into_iter()
        .filter(|request| request["path"] == "/board/2/graphql")
        .collect();
    assert_eq!(board_b.len(), 1, "{board_b:#?}");
    let read = &board_b[0];
    assert!(
        read["query"]
            .as_str()
            .is_some_and(|query| query.contains("r0:repository(owner:$owner0")
                && !query.trim_start().starts_with("mutation")),
        "{read}"
    );
    let named: Vec<(Value, Value)> = (0..2)
        .map(|slot| {
            (
                read["variables"][format!("name{slot}")].clone(),
                read["variables"][format!("number{slot}")].clone(),
            )
        })
        .collect();
    assert_eq!(
        named,
        [(json!("service"), json!(7)), (json!("service"), json!(8))]
    );
    read_no_board_whole(&served);
    // The writes the store made are the card and the issue of issue 7, on board A.
    assert_eq!(
        writes_of(&served),
        [
            json!({"board": 1, "issue": 7, "field": "Status", "value": "Done"}),
            json!({"issue": 7, "state": "CLOSED", "reason": "COMPLETED"}),
        ]
    );
    // The Linear workspace and the `local-md` folder were sent nothing.
    assert!(
        linear.log().is_empty(),
        "the Linear source was asked something"
    );
    assert_eq!(
        std::fs::read_dir(&notes)
            .expect("the notes root")
            .map(|entry| entry.expect("an entry").file_name())
            .collect::<Vec<_>>(),
        ["README.txt"]
    );
    assert_eq!(
        std::fs::read_to_string(notes.join("README.txt")).expect("the notes file"),
        "notes nobody writes\n"
    );

    // One record, naming the landing commit and what the store did.
    let records = closures_of(&world, run);
    assert_eq!(records.len(), 1, "{records:#?}");
    let record = &records[0];
    assert_eq!(record["labels"]["node"], "service");
    let payload = &record["payload"];
    assert_eq!(payload["node"], "service");
    assert_eq!(payload["commit"], landed_on_main(&world));
    assert_eq!(
        payload["closes"],
        json!(["owner/service#7", "owner/service#8"])
    );
    assert_eq!(payload["outcome"], "projected");
    assert_eq!(payload["class"], Value::Null);
    assert_eq!(payload["reason"], Value::Null);
    let actions: Vec<(Value, Value, Value)> = payload["items"]
        .as_array()
        .expect("the store's items")
        .iter()
        .map(|item| {
            (
                item["source"].clone(),
                item["issue"].clone(),
                item["action"].clone(),
            )
        })
        .collect();
    assert_eq!(
        actions,
        [
            (json!("board-a"), json!("owner/service#7"), json!("updated")),
            (json!("board-a"), json!("owner/service#8"), json!("skipped")),
            (json!("board-b"), json!("owner/service#7"), json!("absent")),
            (json!("board-b"), json!("owner/service#8"), json!("absent")),
        ]
    );
    let mut unsupported: Vec<String> = serde_json::from_value(payload["unsupported"].clone())
        .expect("the store's unsupported sources");
    unsupported.sort();
    assert_eq!(unsupported, ["notes", "plans", "tracker"]);
    assert_eq!(
        payload
            .as_object()
            .expect("a payload")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "node",
            "commit",
            "closes",
            "outcome",
            "items",
            "unsupported",
            "class",
            "reason"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    let _ = agent;
}
