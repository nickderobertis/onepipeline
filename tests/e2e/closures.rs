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

use crate::draft_lifecycle::{team_repository, GREEN};
use crate::harness::{agent, double, lifecycle, plan_of, World};
use crate::stacked_drafts::{change_from, settled};
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
}

/// The one-board launch directory every later journey launches from.
fn one_board_launch(world: &World, served: &GitHubEndpoint) -> PathBuf {
    launch_dir(world, &board_source("board", served, 1))
}

/// The `onevcs` landing records relayed into `run`'s journal for `node`, by kind.
fn relayed_landings(world: &World, run: &str, node: &str) -> Vec<String> {
    world
        .journal(run)
        .into_iter()
        .filter(|event| event["source"] == "vcs" && event["labels"]["node"] == node)
        .filter_map(|event| event["kind"].as_str().map(str::to_owned))
        .filter(|kind| kind == "change-merged" || kind == "merge-completed")
        .collect()
}

/// A change request the host merged, which GitHub says closes issue 7, moves issue 7's card to
/// `Done` and no other, in one closure call although the run relays both the `change-merged`
/// and the `merge-completed` that report it.
#[test]
fn a_merged_change_request_closes_its_issues_once_for_both_of_its_landing_records() {
    let (world, served) = world_with("closures-change", "change-auto", &one_board());
    let launch = one_board_launch(&world, &served);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.merged", "");
    world.script("gh.closes", "owner/service#7");
    let run = "change";
    let project = world.plan(run, &plan_of(run, vec![lifecycle("service", &[])]));
    start_from(&world, &launch, &project);
    world.until("the landing's closures to be recorded", |world| {
        !closures_of(world, run).is_empty() && world.run_file(run, "result.json").is_file()
    });

    let relayed = relayed_landings(&world, run, "service");
    assert!(
        relayed.iter().any(|kind| kind == "change-merged")
            && relayed.iter().any(|kind| kind == "merge-completed"),
        "the run did not relay both landing records: {relayed:?}"
    );
    assert_eq!(
        card(&served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
    assert_eq!(card(&served, "1", "9"), (json!("Todo"), issue("OPEN", None)));
    assert_eq!(
        writes_of(&served),
        [
            json!({"board": 1, "issue": 7, "field": "Status", "value": "Done"}),
            json!({"issue": 7, "state": "CLOSED", "reason": "COMPLETED"}),
        ]
    );
    read_no_board_whole(&served);
    let records = closures_of(&world, run);
    assert_eq!(records.len(), 1, "{records:#?}");
    assert_eq!(records[0]["payload"]["closes"], json!(["owner/service#7"]));
    assert_eq!(records[0]["payload"]["outcome"], "projected");
}

/// A landing that closes no issue sends the store nothing: the board's request log is as empty
/// after the run as before it, and the run journals no `landing-closures` record.
#[test]
fn a_landing_that_closes_no_issue_sends_the_board_no_request() {
    let (world, served) = world_with("closures-none", "local-direct", &one_board());
    let launch = one_board_launch(&world, &served);
    world.script("service.work", "the worker wrote this\n");
    world.script("service.commit-saying", "feat: ship the service\n\nNo issue is closed here.");
    let run = "none";
    let project = world.plan(run, &plan_of(run, vec![lifecycle("service", &[])]));
    start_from(&world, &launch, &project);
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });

    let landed: Vec<Value> = world
        .journal(run)
        .into_iter()
        .filter(|event| event["source"] == "vcs" && event["kind"] == "merge-completed")
        .collect();
    assert_eq!(landed.len(), 1, "the node did not land: {landed:#?}");
    assert_eq!(landed[0]["payload"]["closes"], json!([]), "{landed:#?}");
    assert!(
        log_of(&served).is_empty(),
        "a landing that closes nothing sent the board a request: {:#?}",
        log_of(&served)
    );
    assert!(closures_of(&world, run).is_empty());
}

/// A team world — `change-open`, approvals required, every required check green — whose
/// loopback GitHub serves the one board, so a node settles as a review draft a person merges
/// later.
fn team(name: &str) -> (World, GitHubEndpoint) {
    let world = World::new(name)
        .with_env(TOKEN_ENV, "loopback-token")
        .with_env("ONEVCS_BRANCH_PREFIX", "me/");
    team_repository(&world);
    world.script("gh.checks", GREEN);
    let served = GitHubEndpoint::serve_issues(&world, &one_board());
    (world, served)
}

/// Launch `service` — whose change request GitHub says closes issue 7 — beside `hold`, an
/// agent node held until the journey lets it go, so the run stays open; and wait for `service`
/// to settle `done` as a review draft.
fn drafted(world: &World, served: &GitHubEndpoint, run: &str) {
    let launch = one_board_launch(world, served);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.closes", "owner/service#7");
    world.script("hold.wait", "hold");
    let project = world.plan(
        run,
        &plan_of(run, vec![lifecycle("service", &[]), agent("hold", &[])]),
    );
    start_from(world, &launch, &project);
    world.until("service to settle as a review draft", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "service")
    });
    let service = settled(world, run, "service");
    assert_eq!(service["status"], "done", "{service}");
    assert_eq!(service["outcome"], "change-review-draft", "{service}");
    assert!(closures_of(world, run).is_empty());
    assert!(log_of(served).is_empty(), "{:#?}", log_of(served));
}

/// A person merges `service`'s change request on the host, squashing it and keeping its head.
fn merged_by_a_person(world: &World, run: &str) {
    let branch = settled(world, run, "service")["branch"]
        .as_str()
        .expect("service names its branch")
        .to_owned();
    let number = change_from(world, &branch)["number"].to_string();
    let output = std::process::Command::new(double("fake-gh"))
        .args(["__person", "merge", REPOSITORY, &number, "squash", "kept"])
        .env(onepipeline_testfakes::SCRIPT_DIR_ENV, &world.fakes)
        .output()
        .expect("the host double runs");
    assert!(
        output.status.success(),
        "the person's merge was refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// How many times `node` was dispatched in `run`.
fn dispatches(world: &World, run: &str, node: &str) -> usize {
    world
        .events_of(run, "node-dispatched")
        .iter()
        .filter(|event| event["labels"]["node"] == node)
        .count()
}

/// The landing the readjustment found for `service`, by the commit it recorded.
fn found_landed(world: &World, run: &str) -> String {
    world
        .events_of(run, "node-landed")
        .into_iter()
        .find(|event| event["labels"]["node"] == "service")
        .and_then(|event| event["payload"]["commit"].as_str().map(str::to_owned))
        .expect("a readjustment found service landed")
}

/// Issue 7's card is `Done` and its issue closed as completed, by exactly one Status write and
/// one close, and `run` holds exactly one `landing-closures` record, for the landing the
/// readjustment found, projected; no request read a board whole.
fn closed_once(world: &World, served: &GitHubEndpoint, run: &str) {
    assert_eq!(
        card(served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
    assert_eq!(card(served, "1", "9"), (json!("Todo"), issue("OPEN", None)));
    assert_eq!(
        writes_of(served),
        [
            json!({"board": 1, "issue": 7, "field": "Status", "value": "Done"}),
            json!({"issue": 7, "state": "CLOSED", "reason": "COMPLETED"}),
        ]
    );
    let records = closures_of(world, run);
    assert_eq!(records.len(), 1, "{records:#?}");
    assert_eq!(records[0]["payload"]["commit"], found_landed(world, run));
    assert_eq!(records[0]["payload"]["node"], "service");
    assert_eq!(records[0]["payload"]["closes"], json!(["owner/service#7"]));
    assert_eq!(records[0]["payload"]["outcome"], "projected");
    read_no_board_whole(served);
}

/// Let `hold` finish and the run settle.
fn finished(world: &World, run: &str) {
    world.release("hold.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
}

/// A review draft a person merges while nothing drives its run is closed by the driver that
/// adopts the run: once the adopted driver has read the landing, issue 7's card is `Done` and
/// its issue closed, by one Status write and one close, with one record — and stopping and
/// adopting the run again asks the board nothing more and leaves that one record.
#[test]
fn an_adoption_closes_the_issues_of_a_landing_made_while_nothing_drove_the_run() {
    let (world, served) = team("closures-adopted");
    let run = "adopted";
    drafted(&world, &served, run);
    world.run(&["stop", run]).exited(0);
    merged_by_a_person(&world, run);
    assert!(log_of(&served).is_empty());

    world.run(&["adopt", run, "--detach"]).exited(0);
    world.until("the adopted driver to close the landing's issues", |world| {
        !closures_of(world, run).is_empty()
    });
    closed_once(&world, &served, run);

    let asked = log_of(&served).len();
    let held = dispatches(&world, run, "hold");
    world.run(&["stop", run]).exited(0);
    world.run(&["adopt", run, "--detach"]).exited(0);
    world.until("the second adopted driver to pass its catch-up", |world| {
        dispatches(world, run, "hold") > held
    });
    assert_eq!(
        log_of(&served).len(),
        asked,
        "a second adoption asked the board again: {:#?}",
        log_of(&served)
    );
    closed_once(&world, &served, run);
    finished(&world, run);
}

/// A review draft a person merges while its run's driver still runs is closed once the driver
/// reads the landing — here, the `readjust` a person asks of the live driver.
#[test]
fn a_landing_found_while_the_driver_runs_closes_its_issues_once() {
    let (world, served) = team("closures-live");
    let run = "live";
    drafted(&world, &served, run);
    merged_by_a_person(&world, run);
    world.run(&["readjust", run]).exited(0);
    world.until("the driver to close the landing's issues", |world| {
        !closures_of(world, run).is_empty()
    });
    closed_once(&world, &served, run);
    finished(&world, run);
    closed_once(&world, &served, run);
}

/// A closure the driver never saw answered — the board held its Status write until the run was
/// stopped — has no record, so the driver that adopts the run makes the call again and records
/// it once.
#[test]
fn a_closure_a_stopped_driver_never_finished_is_made_by_the_adoption() {
    let (world, served) = team("closures-unfinished");
    let run = "unfinished";
    drafted(&world, &served, run);
    served.fault("hold");
    merged_by_a_person(&world, run);
    world.run(&["readjust", run]).exited(0);
    world.until("the board to hold the closure's Status write", |_| {
        served.state()["holding"] == true
    });
    world.run(&["stop", run]).exited(0);
    assert!(
        closures_of(&world, run).is_empty(),
        "a call that never finished was recorded: {:#?}",
        closures_of(&world, run)
    );
    served.release();
    world.until("the held write to be let go", |_| {
        served.state()["holding"] == false
    });
    assert!(writes_of(&served).is_empty(), "{:#?}", writes_of(&served));

    world.run(&["adopt", run, "--detach"]).exited(0);
    world.until("the adopted driver to close the landing's issues", |world| {
        !closures_of(world, run).is_empty()
    });
    closed_once(&world, &served, run);
    finished(&world, run);
}
