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

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled binary,
// `onevcs` is the real linked library over real git and a real origin on disk, and the plan store
// is the real linked `onetaskgraph` speaking real HTTP to the GitHub loopback. `oneagentgraph` is
// substituted at its subprocess boundary so a journey states what a worker committed, and GitHub's
// CLI at `onevcs`'s own `ONEVCS_GH` override so a change request can be opened, merged by a person
// and read offline. `harness.rs` carries the same suppression and the full rationale.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::draft_lifecycle::{team_repository, GREEN};
use crate::harness::{agent, double, git, lifecycle, onevcs_binary, plan_of, Repository, World};
use crate::linear_loopback::Linear;
use crate::linear_writeback::{from_the_launch_directory, GitHubEndpoint};
use crate::stacked_drafts::{change_from, settled};

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

/// A numbered issue's state, and the reason it was closed for.
fn issue(state: &str, reason: Option<&str>) -> Value {
    json!({"state": state, "reason": reason})
}

/// The one board most journeys read: issue 7 open at `Todo`, and issue 9 open at `Todo`, which
/// those journeys never name.
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

    assert_eq!(
        card(&served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
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
    assert_eq!(
        writes_of(&served),
        [
            json!({"board": 1, "issue": 7, "field": "Status", "value": "Done"}),
            json!({"issue": 7, "state": "CLOSED", "reason": "COMPLETED"}),
        ]
    );
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

/// A launch directory declaring that one board alone.
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
    assert_eq!(
        card(&served, "1", "9"),
        (json!("Todo"), issue("OPEN", None))
    );
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
    world.script(
        "service.commit-saying",
        "feat: ship the service\n\nNo issue is closed here.",
    );
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
    world.until(
        "the adopted driver to close the landing's issues",
        |world| !closures_of(world, run).is_empty(),
    );
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
    world.until(
        "the adopted driver to close the landing's issues",
        |world| !closures_of(world, run).is_empty(),
    );
    closed_once(&world, &served, run);
    finished(&world, run);
}

/// A `local-direct` world whose one node, `service`, lands a commit closing issue 7.
fn landing_closing_seven(name: &str) -> (World, GitHubEndpoint, PathBuf) {
    let (world, served) = world_with(name, "local-direct", &one_board());
    let launch = one_board_launch(&world, &served);
    world.script("service.work", "the worker wrote this\n");
    world.script(
        "service.commit-saying",
        "feat: ship the service\n\nCloses #7",
    );
    (world, served, launch)
}

/// Where in `run`'s journal the first record of `kind` for `node` (any node, for `None`) sits.
fn position(world: &World, run: &str, kind: &str, node: Option<&str>) -> usize {
    world
        .journal(run)
        .iter()
        .position(|event| {
            event["kind"] == kind && node.is_none_or(|node| event["labels"]["node"] == node)
        })
        .unwrap_or_else(|| panic!("{run} journalled no {kind} for {node:?}"))
}

/// A board that refuses the closure's Status write for good changes nothing about the run: the
/// node still settles `done`, the run's one `landing-closures` record says `failed` with the
/// store's class `refused` and its reason, and a non-blocking planner surface reports it.
#[test]
fn a_refused_closure_is_recorded_failed_and_surfaced_without_touching_the_settlement() {
    let (world, served, launch) = landing_closing_seven("closures-refused");
    served.fault("refuse");
    let run = "refused";
    let project = world.plan(run, &plan_of(run, vec![lifecycle("service", &[])]));
    start_from(&world, &launch, &project);
    world.until("the refused closure to be recorded", |world| {
        !closures_of(world, run).is_empty() && world.run_file(run, "result.json").is_file()
    });

    let service = settled(&world, run, "service");
    assert_eq!(service["status"], "done", "{service}");
    assert_eq!(service["outcome"], "merged", "{service}");
    let records = closures_of(&world, run);
    assert_eq!(records.len(), 1, "{records:#?}");
    let payload = &records[0]["payload"];
    assert_eq!(payload["outcome"], "failed", "{payload}");
    assert_eq!(payload["class"], "refused", "{payload}");
    assert!(
        payload["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("the loopback refuses this Status write")),
        "{payload}"
    );
    assert_eq!(payload["items"][0]["action"], "failed", "{payload}");
    let surfaced: Vec<Value> = world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .map(|event| event["payload"].clone())
        .filter(|surface| {
            surface["message"]
                .as_str()
                .is_some_and(|message| message.contains("owner/service#7"))
        })
        .collect();
    assert_eq!(surfaced.len(), 1, "{surfaced:#?}");
    assert_eq!(surfaced[0]["blocking"], false, "{surfaced:#?}");
    assert!(
        surfaced[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("refused")
                && message.contains("the loopback refuses this Status write")),
        "{surfaced:#?}"
    );
    assert_eq!(
        card(&served, "1", "7"),
        (json!("Todo"), issue("OPEN", None))
    );
    read_no_board_whole(&served);
}

/// A board that fails the closure's Status write once and then holds the retry holds nothing
/// of the run back: the node that depends on the landed one is dispatched and settles `done`
/// while the board still holds the retried write, and once it is let go issue 7's card reaches
/// `Done` and the run's one record says `projected` — after that settlement in the journal.
#[test]
fn a_closure_being_retried_holds_back_no_dependent_of_the_landed_node() {
    let (world, served, launch) = landing_closing_seven("closures-retried");
    served.fault("transient");
    world.script("hold.wait", "hold");
    let run = "retried";
    let project = world.plan(
        run,
        &plan_of(
            run,
            vec![
                lifecycle("service", &[]),
                agent("after", &["service"]),
                agent("hold", &[]),
            ],
        ),
    );
    start_from(&world, &launch, &project);
    world.until(
        "the dependent to settle while the board holds the retried write",
        |world| {
            served.state()["holding"] == true
                && world
                    .events_of(run, "node-settled")
                    .iter()
                    .any(|event| event["labels"]["node"] == "after")
        },
    );
    assert_eq!(settled(&world, run, "after")["status"], "done");
    assert!(
        closures_of(&world, run).is_empty(),
        "the closure was recorded while its write was held"
    );
    served.release();
    world.until("the retried closure to be recorded", |world| {
        !closures_of(world, run).is_empty()
    });

    assert_eq!(
        card(&served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
    let records = closures_of(&world, run);
    assert_eq!(records.len(), 1, "{records:#?}");
    assert_eq!(records[0]["payload"]["outcome"], "projected");
    assert!(
        position(&world, run, "node-settled", Some("after"))
            < position(&world, run, "landing-closures", None),
        "the dependent did not settle before the closure finished"
    );
    read_no_board_whole(&served);
    finished(&world, run);
}

/// One of the engine's own landing verbs, over a branch of the world's `local-direct`
/// repository whose commit closes issue 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    PublishBranch,
    RepoRecover,
    Integrate,
}

impl Verb {
    fn name(self) -> &'static str {
        match self {
            Self::PublishBranch => "publish-branch",
            Self::RepoRecover => "repo-recover",
            Self::Integrate => "integrate",
        }
    }

    /// Cut the branch this verb lands — a finished one, or for `repo-recover` one carrying an
    /// unattested incomplete-step marker, preserved on the origin — and answer the verb's
    /// arguments.
    fn prepare(self, world: &World, repository: &Repository) -> Vec<&'static str> {
        let subject = match self {
            Self::RepoRecover => "feat: add the widget (incomplete step)",
            Self::PublishBranch | Self::Integrate => "feat: add the widget",
        };
        let checkout = &repository.checkout;
        git(world, checkout, &["checkout", "-b", "widget"]);
        std::fs::write(checkout.join("widget.txt"), "a widget\n").expect("the work is written");
        git(world, checkout, &["add", "-A"]);
        git(
            world,
            checkout,
            &["commit", "-m", &format!("{subject}\n\nCloses #7")],
        );
        git(world, checkout, &["checkout", "main"]);
        match self {
            Self::PublishBranch => vec!["publish-branch", "widget", "--repo", "service"],
            Self::RepoRecover => {
                let preserved = world
                    .cmd_on(
                        &onevcs_binary(),
                        &["preserve", "widget", "--repo", "service"],
                    )
                    .output()
                    .expect("onevcs runs");
                assert!(
                    preserved.status.success(),
                    "{}",
                    String::from_utf8_lossy(&preserved.stderr)
                );
                vec![
                    "repo-recover",
                    "widget",
                    "--repo",
                    "service",
                    "--title",
                    "feat: add the widget",
                    "--no-draft",
                ]
            }
            Self::Integrate => vec!["integrate", "widget", "--push"],
        }
    }

    /// Where the verb runs from, and where the store configuration it reads sits at or above:
    /// the repository's checkout for `integrate`, which resolves its repository from there,
    /// and a directory of its own for the branch-keyed verbs.
    fn dir(self, world: &World, repository: &Repository) -> PathBuf {
        match self {
            Self::Integrate => repository.checkout.clone(),
            Self::PublishBranch | Self::RepoRecover => {
                let dir = world.root.join("launch");
                std::fs::create_dir_all(&dir).expect("the directory the verb runs in");
                dir
            }
        }
    }

    /// Where its store configuration is written: at the directory it runs from, or — for
    /// `integrate`, whose checkout it must not dirty — the directory above it, which the
    /// store's discovery reaches the same way.
    fn configuration(self, world: &World, repository: &Repository) -> PathBuf {
        match self {
            Self::Integrate => world.root.join("onetaskgraph.yaml"),
            Self::PublishBranch | Self::RepoRecover => {
                self.dir(world, repository).join("onetaskgraph.yaml")
            }
        }
    }
}

/// What one landing verb did: its exit status, both streams, and its world.
struct Landed {
    world: World,
    repository: Repository,
    served: GitHubEndpoint,
    code: i32,
    stdout: String,
    stderr: String,
}

/// What the directory a landing verb runs from configures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Configured {
    /// Nothing at all.
    Nothing,
    /// The loopback board.
    Board,
    /// A document the store cannot load.
    Unloadable,
}

/// How one landing verb journey is set up.
#[derive(Debug, Clone, Copy)]
struct Setup<'a> {
    configured: Configured,
    /// What answers the closure's Status write, where something other than the board does.
    fault: Option<&'a str>,
    /// Whether a manager session owns the landing, as every world's command says by default.
    owned: bool,
    /// Whether the publishing push leaves the operation's stream unreadable for a moment, as
    /// the lock git for Windows' writer holds over an append does.
    unreadable: bool,
}

impl Setup<'_> {
    const BOARD: Self = Self {
        configured: Configured::Board,
        fault: None,
        owned: true,
        unreadable: false,
    };
    const BARE: Self = Self {
        configured: Configured::Nothing,
        fault: None,
        owned: true,
        unreadable: false,
    };
}

/// Run `verb` over its branch in a world of its own, from a directory configured as `setup`
/// says.
fn landed_by(verb: Verb, setup: Setup<'_>) -> Landed {
    let name = format!(
        "closures-{}-{:?}{}{}{}",
        verb.name(),
        setup.configured,
        setup
            .fault
            .map(|fault| format!("-{fault}"))
            .unwrap_or_default(),
        if setup.owned { "" } else { "-unowned" },
        if setup.unreadable { "-unreadable" } else { "" }
    )
    .to_lowercase();
    let world = World::new(&name).with_env(TOKEN_ENV, "loopback-token");
    // A recovery attests the repository's bar, so its repository has one: a `pre-push` hook
    // that passes.
    let mut bar: Vec<String> = match verb {
        Verb::RepoRecover => vec!["true".to_owned()],
        Verb::PublishBranch | Verb::Integrate => Vec::new(),
    };
    if setup.unreadable {
        // llmlint: ignore[tests_mirror_real_usage] no interface of this build or of `onevcs`
        // makes a read of an operation's stream fail for a moment: what does is the operating
        // system, which on Windows refuses a read meeting the writer's locked append (os error
        // 33). Unix has no such lock, so the hook takes the owner's read bit off the stream for
        // long enough that the relay's reads meet it several times over, and puts it back before
        // the push ends, while `onevcs` appends all the while. Everything asserted is the
        // compiled binary's own output, journal and board writes.
        let streams = format!(
            "'{}'/operations/*/events.ndjson",
            world.onevcs_home().display()
        );
        bar = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            format!("chmod u-r {streams} && sleep 0.3 && chmod u+r {streams}"),
        ];
    }
    let bar: Vec<&str> = bar.iter().map(String::as_str).collect();
    let repository = world.repository("local-direct", &bar);
    let served = GitHubEndpoint::serve_issues(&world, &one_board());
    if let Some(fault) = setup.fault {
        served.fault(fault);
    }
    let args = verb.prepare(&world, &repository);
    let dir = verb.dir(&world, &repository);
    let document = match setup.configured {
        Configured::Nothing => None,
        Configured::Board => Some(format!("sources:\n{}", board_source("board", &served, 1))),
        Configured::Unloadable => Some("sources:\n  board:\n    plugin: nonesuch\n".to_owned()),
    };
    if let Some(document) = document {
        std::fs::write(verb.configuration(&world, &repository), document)
            .expect("the store configuration");
    }
    // Only the directory's own configuration: none of this world's environment layer.
    let mut command = from_the_launch_directory(&world, &dir, &args);
    if !setup.owned {
        command.env_remove("ONEPIPELINE_LAUNCHER_SESSION");
    }
    let output = command.output().expect("the landing verb runs");
    Landed {
        world,
        repository,
        served,
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Every `landing-closures` record the landing journals of `world`'s flows hold.
fn landing_records(world: &World) -> Vec<Value> {
    let mut records = Vec::new();
    let Ok(flows) = std::fs::read_dir(world.runs.join(".flows")) else {
        return records;
    };
    for flow in flows.flatten() {
        let Ok(landings) = std::fs::read_dir(flow.path().join("landings")) else {
            continue;
        };
        for landing in landings.flatten() {
            let journal =
                std::fs::read_to_string(landing.path().join("events.jsonl")).unwrap_or_default();
            records.extend(
                journal
                    .lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .filter(|event| event["kind"] == "landing-closures"),
            );
        }
    }
    records
}

/// The verb landed the branch on the base, carrying its closing line.
fn landed_widget(landed: &Landed) {
    assert_eq!(
        landed.repository.base_file("widget.txt").as_deref(),
        Some("a widget\n"),
        "the branch did not land:\n{}\n{}",
        landed.stdout,
        landed.stderr
    );
    let message = git(
        &landed.world,
        &landed.repository.origin,
        &["log", "-1", "--format=%B", "main"],
    );
    assert!(
        message.lines().any(|line| line.trim() == "Closes #7"),
        "the landing commit does not carry its closing line: {message}"
    );
}

/// The one record a landing verb journalled, for the landing commit it made.
fn the_record(landed: &Landed) -> Value {
    let records = landing_records(&landed.world);
    assert_eq!(
        records.len(),
        1,
        "{records:#?}\n{}\n{}",
        landed.stdout,
        landed.stderr
    );
    let payload = records[0]["payload"].clone();
    assert_eq!(payload["node"], Value::Null, "{payload}");
    assert_eq!(
        payload["commit"],
        landed_on_main(&landed.world),
        "{payload}"
    );
    assert_eq!(payload["closes"], json!(["owner/service#7"]), "{payload}");
    payload
}

/// `verb` from a directory whose store configuration declares the board moves issue 7's card
/// to `Done`, prints what the store answered, exits as it does from a directory declaring
/// nothing — where it asks nothing and says so — and journals one projected record.
fn a_landing_verb_closes_the_issues_its_landing_closes(verb: Verb) {
    let bare = landed_by(verb, Setup::BARE);
    landed_widget(&bare);
    assert!(
        bare.stderr
            .contains("no store configuration here declares a source, so no closure request"),
        "{}",
        bare.stderr
    );
    assert!(
        log_of(&bare.served).is_empty(),
        "{:#?}",
        log_of(&bare.served)
    );
    assert!(landing_records(&bare.world).is_empty());

    let landed = landed_by(verb, Setup::BOARD);
    landed_widget(&landed);
    assert_eq!(landed.code, bare.code, "{}", landed.stderr);
    assert_eq!(landed.code, 0, "{}", landed.stderr);
    assert!(
        landed.stderr.contains("owner/service#7 on board: updated"),
        "{} did not print what the store answered:\n{}",
        verb.name(),
        landed.stderr
    );
    assert_eq!(
        card(&landed.served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
    assert_eq!(
        card(&landed.served, "1", "9"),
        (json!("Todo"), issue("OPEN", None))
    );
    read_no_board_whole(&landed.served);
    let record = the_record(&landed);
    assert_eq!(record["outcome"], "projected", "{record}");
    assert_eq!(record["class"], Value::Null, "{record}");
}

/// `verb` against a board that refuses the closure's Status write still lands, exits as it
/// does from a directory declaring nothing, and records the closure `failed` with the store's
/// class and reason.
fn a_landing_verb_lands_whatever_the_board_answers(verb: Verb) {
    let bare = landed_by(verb, Setup::BARE);
    let landed = landed_by(
        verb,
        Setup {
            fault: Some("refuse"),
            ..Setup::BOARD
        },
    );
    landed_widget(&landed);
    assert_eq!(landed.code, bare.code, "{}", landed.stderr);
    assert_eq!(
        card(&landed.served, "1", "7"),
        (json!("Todo"), issue("OPEN", None))
    );
    let record = the_record(&landed);
    assert_eq!(record["outcome"], "failed", "{record}");
    assert_eq!(record["class"], "refused", "{record}");
    assert!(
        record["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("the loopback refuses this Status write")),
        "{record}"
    );
    assert!(
        landed.stderr.contains("failed (refused)"),
        "{}",
        landed.stderr
    );
}

#[test]
fn publish_branch_closes_the_issues_its_landing_closes() {
    a_landing_verb_closes_the_issues_its_landing_closes(Verb::PublishBranch);
}

#[test]
fn publish_branch_lands_whatever_the_board_answers() {
    a_landing_verb_lands_whatever_the_board_answers(Verb::PublishBranch);
}

#[test]
fn repo_recover_closes_the_issues_its_landing_closes() {
    a_landing_verb_closes_the_issues_its_landing_closes(Verb::RepoRecover);
}

#[test]
fn repo_recover_lands_whatever_the_board_answers() {
    a_landing_verb_lands_whatever_the_board_answers(Verb::RepoRecover);
}

/// `integrate`'s train squashes the branch onto the base carrying its closing line, and the
/// landing it reports closes issue 7 — and nothing else — through the store the directory above
/// the checkout configures.
#[test]
fn integrate_closes_the_issues_its_train_lands() {
    a_landing_verb_closes_the_issues_its_landing_closes(Verb::Integrate);
}

/// A read of the landing's operation stream that fails for a moment — on Windows, the lock
/// `onevcs`'s writer holds over each append — is asked again: the relay still reads the
/// `merge-completed` the push is followed by, so the closure is made and printed and the landing
/// finishes rather than reading as unsupervised.
// A permission bit is what stages the failed read here, and Windows has none that denies its
// owner a read; there the lock itself is what this journey's verb meets in every landing.
#[cfg(unix)]
#[test]
fn a_landing_verb_relays_past_a_read_of_its_stream_that_fails_for_now() {
    let landed = landed_by(
        Verb::PublishBranch,
        Setup {
            unreadable: true,
            ..Setup::BOARD
        },
    );
    landed_widget(&landed);
    assert_eq!(landed.code, 0, "{}", landed.stderr);
    assert!(
        !landed.stderr.contains("could not be relayed"),
        "{}",
        landed.stderr
    );
    assert!(
        landed.stderr.contains("owner/service#7 on board: updated"),
        "{}",
        landed.stderr
    );
    assert_eq!(the_record(&landed)["outcome"], "projected");
}

/// A directory whose store configuration does not load lands the branch all the same, exits as
/// it does from one declaring nothing, and records the closure `failed`, refused, saying the
/// configuration did not load.
#[test]
fn a_landing_verb_lands_where_the_store_configuration_does_not_load() {
    let bare = landed_by(Verb::PublishBranch, Setup::BARE);
    let landed = landed_by(
        Verb::PublishBranch,
        Setup {
            configured: Configured::Unloadable,
            ..Setup::BOARD
        },
    );
    landed_widget(&landed);
    assert_eq!(landed.code, bare.code, "{}", landed.stderr);
    let record = the_record(&landed);
    assert_eq!(record["outcome"], "failed", "{record}");
    assert_eq!(record["class"], "refused", "{record}");
    assert!(
        record["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("the store configuration does not load")),
        "{record}"
    );
    assert!(log_of(&landed.served).is_empty());
}

/// A board that never answers the closure's Status write holds the landing verb no longer than
/// the call's deadline: the branch lands, the verb exits as it does from a directory declaring
/// nothing, and the closure is recorded `failed`, transient, saying the call ran out of time —
/// and is not asked again.
#[test]
fn a_landing_verb_ends_a_closure_the_board_never_answers_at_its_deadline() {
    let bare = landed_by(Verb::PublishBranch, Setup::BARE);
    let landed = landed_by(
        Verb::PublishBranch,
        Setup {
            fault: Some("hold"),
            ..Setup::BOARD
        },
    );
    landed_widget(&landed);
    assert_eq!(landed.code, bare.code, "{}", landed.stderr);
    let record = the_record(&landed);
    assert_eq!(record["outcome"], "failed", "{record}");
    assert_eq!(record["class"], "transient", "{record}");
    assert!(
        record["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("task status complete exceeded")),
        "{record}"
    );
    assert!(
        landed.stderr.contains("failed (transient)"),
        "{}",
        landed.stderr
    );
    let statuses = log_of(&landed.served)
        .iter()
        .filter(|request| {
            request["query"]
                .as_str()
                .is_some_and(|query| query.contains("updateProjectV2ItemFieldValue("))
        })
        .count();
    assert_eq!(statuses, 1, "the verb asked the board again");
    landed.served.release();
}

/// A landing no manager session owns lands unowned, as it always did, and still hands the
/// store the issues it closed: issue 7's card reaches `Done` and the verb prints what the store
/// answered, with no landing journal to record it in.
#[test]
fn an_unowned_landing_closes_the_issues_it_closes() {
    let landed = landed_by(
        Verb::PublishBranch,
        Setup {
            owned: false,
            ..Setup::BOARD
        },
    );
    landed_widget(&landed);
    assert_eq!(landed.code, 0, "{}", landed.stderr);
    assert!(
        landed
            .stderr
            .contains("no manager session owns this landing"),
        "{}",
        landed.stderr
    );
    assert!(
        landed.stderr.contains("owner/service#7 on board: updated"),
        "{}",
        landed.stderr
    );
    assert_eq!(
        card(&landed.served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
    assert!(landing_records(&landed.world).is_empty());
}

/// This host's release-targets document for the repository under test: one human-step target,
/// which executes nothing, so the run reads its settled nodes' session streams on its release
/// pass.
const RELEASES: &str = "version: 1\n\
     repositories:\n\
     \x20 - match: {host: github.com, owner: owner, name: service}\n\
     \x20   default_target: wheel\n\
     \x20   targets:\n\
     \x20   - name: wheel\n\
     \x20     style: human-step\n\
     \x20     action: \"cut the release by hand\"\n\
     default:\n\
     \x20 adoption: fast\n";

/// A landing recorded on a settled node's own session stream while its run's driver still
/// runs — here by a person's own `onevcs change standings` read of the merged change request,
/// which nothing of the run asked for — is met on the driver's next pass over that stream and
/// closes issue 7 once, with no readjustment and no adoption: whether or not the repository
/// declares a release target, since the pass reads an unlanded draft's stream for its landing
/// either way.
fn a_late_landing_on_the_stream_closes_its_issues(name: &str, releases: Option<&str>) {
    let (world, served) = team(name);
    let world = world.with_env("ONEPIPELINE_RELEASE_POLL_SECONDS", "1");
    if let Some(releases) = releases {
        world.releases(releases);
    }
    let run = "stream";
    drafted(&world, &served, run);
    merged_by_a_person(&world, run);
    let change = settled(&world, run, "service")["change_url"]
        .as_str()
        .expect("service names its change request")
        .to_owned();
    let read = world
        .cmd_on(
            &onevcs_binary(),
            &[
                "change",
                "standings",
                "service",
                "--root",
                "main",
                "--change",
                &change,
                "--json",
            ],
        )
        .output()
        .expect("onevcs runs");
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    world.until("the driver to close the landing's issues", |world| {
        !closures_of(world, run).is_empty()
    });
    assert!(
        world.events_of(run, "node-landed").is_empty(),
        "a readjustment found the landing, not the session stream"
    );
    assert_eq!(world.events_of(run, "driver-adopted").len(), 0);
    assert_eq!(
        card(&served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
    assert_eq!(
        writes_of(&served),
        [
            json!({"board": 1, "issue": 7, "field": "Status", "value": "Done"}),
            json!({"issue": 7, "state": "CLOSED", "reason": "COMPLETED"}),
        ]
    );
    let records = closures_of(&world, run);
    assert_eq!(records.len(), 1, "{records:#?}");
    assert_eq!(records[0]["payload"]["node"], "service");
    assert_eq!(records[0]["payload"]["outcome"], "projected");
    read_no_board_whole(&served);
    finished(&world, run);
}

#[test]
fn a_late_landing_on_a_settled_node_s_session_stream_closes_its_issues_while_the_driver_runs() {
    a_late_landing_on_the_stream_closes_its_issues("closures-stream", Some(RELEASES));
}

/// The same, in a repository that declares no release target: nothing about its releases is
/// read, and its unlanded draft's stream is still read for the landing.
#[test]
fn a_late_landing_in_a_repository_that_releases_nothing_closes_its_issues_while_the_driver_runs() {
    a_late_landing_on_the_stream_closes_its_issues("closures-stream-bare", None);
}

/// One store call whose answer fails an item for good and another for now is asked again —
/// the transient failure decides — and the retry, which the board answers, writes both cards:
/// the record says `projected`, after four Status writes, two of them refused.
#[test]
fn a_closure_failing_for_good_and_for_now_at_once_is_asked_again() {
    let issues = json!({
        "repository": REPOSITORY,
        "issues": {"7": issue("OPEN", None), "9": issue("OPEN", None)},
        "boards": {"1": {"7": "Todo", "9": "Todo"}},
    });
    let (world, served) = world_with("closures-mixed", "local-direct", &issues);
    let launch = one_board_launch(&world, &served);
    world.script("service.work", "the worker wrote this\n");
    world.script(
        "service.commit-saying",
        "feat: ship the service\n\nCloses #7\nCloses #9",
    );
    served.faults(&["refuse", "fail-once"]);
    let run = "mixed";
    let project = world.plan(run, &plan_of(run, vec![lifecycle("service", &[])]));
    start_from(&world, &launch, &project);
    world.until("the landing's closures to be recorded", |world| {
        !closures_of(world, run).is_empty() && world.run_file(run, "result.json").is_file()
    });

    let records = closures_of(&world, run);
    assert_eq!(records.len(), 1, "{records:#?}");
    assert_eq!(
        records[0]["payload"]["outcome"], "projected",
        "{records:#?}"
    );
    for number in ["7", "9"] {
        assert_eq!(
            card(&served, "1", number),
            (json!("Done"), issue("CLOSED", Some("COMPLETED")))
        );
    }
    let statuses = log_of(&served)
        .iter()
        .filter(|request| {
            request["query"]
                .as_str()
                .is_some_and(|query| query.contains("updateProjectV2ItemFieldValue("))
        })
        .count();
    assert_eq!(statuses, 4, "the mixed failure was not asked again");
    read_no_board_whole(&served);
}

/// An integration train landing two branches, each closing an issue of its own, makes one
/// closure call and one record per branch it lands, each naming that branch's landing commit
/// and only its own issue.
#[test]
fn an_integrate_train_closes_each_branch_s_issues_with_a_record_of_its_own() {
    let issues = json!({
        "repository": REPOSITORY,
        "issues": {"7": issue("OPEN", None), "9": issue("OPEN", None)},
        "boards": {"1": {"7": "Todo", "9": "Todo"}},
    });
    let world = World::new("closures-train").with_env(TOKEN_ENV, "loopback-token");
    let repository = world.repository("local-direct", &[]);
    let served = GitHubEndpoint::serve_issues(&world, &issues);
    let checkout = &repository.checkout;
    for (branch, number) in [("widget", 7), ("gadget", 9)] {
        git(&world, checkout, &["checkout", "-b", branch, "main"]);
        std::fs::write(
            checkout.join(format!("{branch}.txt")),
            format!("a {branch}\n"),
        )
        .expect("the work is written");
        git(&world, checkout, &["add", "-A"]);
        git(
            &world,
            checkout,
            &[
                "commit",
                "-m",
                &format!("feat: add the {branch}\n\nCloses #{number}"),
            ],
        );
        git(&world, checkout, &["checkout", "main"]);
    }
    std::fs::write(
        world.root.join("onetaskgraph.yaml"),
        format!("sources:\n{}", board_source("board", &served, 1)),
    )
    .expect("the store configuration");
    let output = from_the_launch_directory(
        &world,
        checkout,
        &["integrate", "widget", "gadget", "--push"],
    )
    .output()
    .expect("integrate runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");

    let records = landing_records(&world);
    assert_eq!(records.len(), 2, "{records:#?}\n{stderr}");
    let closes: Vec<Value> = records
        .iter()
        .map(|record| record["payload"]["closes"].clone())
        .collect();
    assert_eq!(
        closes,
        [json!(["owner/service#7"]), json!(["owner/service#9"])]
    );
    assert_ne!(
        records[0]["payload"]["commit"], records[1]["payload"]["commit"],
        "the two branches' landings were recorded as one"
    );
    assert_eq!(records[1]["payload"]["commit"], landed_on_main(&world));
    for record in &records {
        assert_eq!(record["payload"]["node"], Value::Null);
        assert_eq!(record["payload"]["outcome"], "projected");
    }
    for number in ["7", "9"] {
        assert_eq!(
            card(&served, "1", number),
            (json!("Done"), issue("CLOSED", Some("COMPLETED")))
        );
    }
    read_no_board_whole(&served);
}

/// A settled run waits for the closures it still owes no longer than their own close-out
/// window: a board that never answers the Status write leaves the run to end — `driver-exited`
/// inside that bound — with no record of a call that never finished, which the next adoption
/// makes.
#[test]
fn a_settled_run_waits_for_its_closures_no_longer_than_their_window() {
    let (world, served, launch) = landing_closing_seven("closures-window");
    served.fault("hold");
    let run = "window";
    let project = world.plan(run, &plan_of(run, vec![lifecycle("service", &[])]));
    start_from(&world, &launch, &project);
    world.until("the board to hold the closure's Status write", |_| {
        served.state()["holding"] == true
    });
    let held_at = std::time::Instant::now();
    world.until_within(
        std::time::Duration::from_secs(90),
        "the settled run's driver to end",
        |world| !world.events_of(run, "driver-exited").is_empty(),
    );
    let waited = held_at.elapsed();
    assert!(
        waited < std::time::Duration::from_secs(60),
        "the run waited {waited:?} for a closure the board never answered"
    );
    assert_eq!(settled(&world, run, "service")["status"], "done");
    assert!(
        closures_of(&world, run).is_empty(),
        "a call that never finished was recorded: {:#?}",
        closures_of(&world, run)
    );
    assert_eq!(served.state()["holding"], true, "the write was answered");
    served.release();
    assert_eq!(
        card(&served, "1", "7"),
        (json!("Todo"), issue("OPEN", None))
    );
}

/// A landing verb's closure record reaches the journal of every run the landing is associated
/// with — here the run whose node holds a session on the branch a manager lands — exactly as
/// the landing's own transitions do, labelled with the landing and naming no node.
#[test]
fn a_landing_verb_s_closure_record_reaches_the_runs_its_landing_is_associated_with() {
    let (world, served) = world_with("closures-associated", "local-direct", &one_board());
    let launch = one_board_launch(&world, &served);
    world.script("service.wait", "hold");
    let run = "associated";
    let project = world.plan(run, &plan_of(run, vec![lifecycle("service", &[])]));
    start_from(&world, &launch, &project);
    world.until("the node's session to open", |world| {
        world
            .journal(run)
            .iter()
            .any(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
    });
    let opened = world
        .journal(run)
        .into_iter()
        .find(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .expect("the session opened");
    let branch = opened["payload"]["branch"]
        .as_str()
        .expect("the session names its branch")
        .to_owned();
    let worktree = PathBuf::from(
        opened["payload"]["worktree"]
            .as_str()
            .expect("the session names its worktree"),
    );
    std::fs::write(worktree.join("widget.txt"), "a widget\n").expect("the work is written");
    git(&world, &worktree, &["add", "-A"]);
    git(
        &world,
        &worktree,
        &["commit", "-m", "feat: add the widget\n\nCloses #7"],
    );

    let output = from_the_launch_directory(
        &world,
        &launch,
        &["publish-branch", &branch, "--repo", "service", "--no-draft"],
    )
    .output()
    .expect("publish-branch runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");

    let records = landing_records(&world);
    assert_eq!(records.len(), 1, "{records:#?}\n{stderr}");
    let landing = records[0]["labels"]["landing_id"].clone();
    let mirrored: Vec<Value> = closures_of(&world, run)
        .into_iter()
        .filter(|event| event["labels"]["landing_id"] == landing)
        .collect();
    assert_eq!(mirrored.len(), 1, "{:#?}", world.journal(run));
    assert_eq!(mirrored[0]["payload"], records[0]["payload"]);
    assert_eq!(mirrored[0]["payload"]["node"], Value::Null);
    assert_eq!(mirrored[0]["payload"]["outcome"], "projected");
    assert_eq!(
        card(&served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
    world.release("service.go");
}

/// What a landing verb prints is what the store answered, item by item: an issue closed as not
/// planned is reported skipped with the store's reason, and every source holding no item by
/// issue — here a `local-md` folder — is named as sent no request.
#[test]
fn a_landing_verb_prints_each_item_the_store_answered_and_every_source_it_sent_nothing() {
    let issues = json!({
        "repository": REPOSITORY,
        "issues": {"7": issue("OPEN", None), "8": issue("CLOSED", Some("NOT_PLANNED"))},
        "boards": {"1": {"7": "Todo", "8": "Todo"}},
    });
    let world = World::new("closures-printed").with_env(TOKEN_ENV, "loopback-token");
    let repository = world.repository("local-direct", &[]);
    let served = GitHubEndpoint::serve_issues(&world, &issues);
    let checkout = &repository.checkout;
    git(&world, checkout, &["checkout", "-b", "widget"]);
    std::fs::write(checkout.join("widget.txt"), "a widget\n").expect("the work is written");
    git(&world, checkout, &["add", "-A"]);
    git(
        &world,
        checkout,
        &[
            "commit",
            "-m",
            "feat: add the widget\n\nCloses #7\nFixes #8",
        ],
    );
    git(&world, checkout, &["checkout", "main"]);
    let notes = world.root.join("notes");
    std::fs::create_dir_all(&notes).expect("the notes source's root");
    let dir = launch_dir(
        &world,
        &format!(
            "{}  notes:\n    plugin: local-md\n    config:\n      root: {}\n",
            board_source("board", &served, 1),
            notes.display()
        ),
    );
    let output = from_the_launch_directory(
        &world,
        &dir,
        &["publish-branch", "widget", "--repo", "service"],
    )
    .output()
    .expect("publish-branch runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    for said in [
        "closes owner/service#7, owner/service#8; the store answered:",
        "owner/service#7 on board: updated",
        "owner/service#8 on board: skipped",
        "closed as not planned",
        "sent no request (no item by issue): notes",
        "  projected",
    ] {
        assert!(
            stderr.contains(said),
            "the verb did not print {said:?}:\n{stderr}"
        );
    }
    assert_eq!(
        card(&served, "1", "8"),
        (json!("Todo"), issue("CLOSED", Some("NOT_PLANNED")))
    );
}

/// The window is the closures' own: a settled run whose closure the board holds past the
/// projection's close-out window still records it, once the board answers inside the closures'
/// window, and only then ends.
#[test]
fn a_settled_run_records_a_closure_the_board_answers_past_the_projection_window() {
    let (world, served, launch) = landing_closing_seven("closures-window-answered");
    served.fault("transient");
    let run = "window-answered";
    let project = world.plan(run, &plan_of(run, vec![lifecycle("service", &[])]));
    start_from(&world, &launch, &project);
    world.until(
        "the board to hold the closure's retried Status write",
        |_| served.state()["holding"] == true,
    );
    world.until("the run's node to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });
    // Past the projection's close-out window, with the driver still waiting.
    std::thread::sleep(std::time::Duration::from_secs(5));
    assert!(
        world.events_of(run, "driver-exited").is_empty(),
        "the settled run ended inside the projection's window, leaving its closure"
    );
    served.release();
    world.until("the settled run's driver to end", |world| {
        !world.events_of(run, "driver-exited").is_empty()
    });
    let records = closures_of(&world, run);
    assert_eq!(records.len(), 1, "{records:#?}");
    assert_eq!(records[0]["payload"]["outcome"], "projected");
    assert!(
        position(&world, run, "landing-closures", None)
            < position(&world, run, "driver-exited", None),
        "the closure was recorded after the driver ended"
    );
    assert_eq!(
        card(&served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
}

/// A world whose landing closes issue 7, which board A holds open at `Todo` and whose item
/// delivers `ticket`.
fn closing_seven_delivering(name: &str, ticket: &str) -> (World, GitHubEndpoint, PathBuf) {
    let issues = json!({
        "repository": REPOSITORY,
        "issues": {"7": {"state": "OPEN", "reason": null, "delivers": [ticket]}},
        "boards": {"1": {"7": "Todo"}},
    });
    let (world, served) = world_with(name, "local-direct", &issues);
    let launch = one_board_launch(&world, &served);
    world.script("service.work", "the worker wrote this\n");
    world.script(
        "service.commit-saying",
        "feat: ship the service\n\nCloses #7",
    );
    (world, served, launch)
}

/// The item the store wrote for issue 7, as the run's one record carries it verbatim — `updated`,
/// with the delivered ticket it could not keep in step `failed` — and the record itself.
fn delivered_failure_recorded(world: &World, run: &str, ticket: &str) -> Value {
    let records = closures_of(world, run);
    assert_eq!(records.len(), 1, "{records:#?}");
    let payload = records[0]["payload"].clone();
    let item = &payload["items"][0];
    assert_eq!(item["action"], "updated", "{payload}");
    assert_eq!(item["delivered"][0]["ticket"], ticket, "{payload}");
    assert_eq!(item["delivered"][0]["outcome"], "failed", "{payload}");
    assert_eq!(payload["outcome"], "failed", "{payload}");
    let surfaced = |world: &World| -> Vec<Value> {
        world
            .events_of(run, "planner-surface-queued")
            .into_iter()
            .map(|event| event["payload"].clone())
            .filter(|surface| {
                surface["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("owner/service#7"))
            })
            .collect()
    };
    // The loop journals the record and then raises the surface.
    world.until("the failed closure to be surfaced", |world| {
        !surfaced(world).is_empty()
    });
    let surfaced = surfaced(world);
    assert_eq!(surfaced.len(), 1, "{surfaced:#?}");
    assert_eq!(surfaced[0]["blocking"], false);
    // The relayed `merge-completed` starts the write while the node's publication is still
    // closing out, so the record can be journaled before the settlement it does not change.
    world.until("service to settle", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "service")
    });
    assert_eq!(settled(world, run, "service")["status"], "done");
    payload
}

/// A landing whose item the store writes `done` but one of whose delivered tickets it cannot
/// keep in step — a ticket no source holds — is recorded `failed`, refused, with the store's
/// items verbatim, and surfaced; the node's settlement is untouched, and the card it wrote stays
/// `Done`.
#[test]
fn a_delivered_ticket_the_store_cannot_keep_in_step_fails_the_closure_for_good() {
    let ticket = "plans:no-such-ticket";
    let (world, served, launch) = closing_seven_delivering("closures-delivered-refused", ticket);
    let run = "delivered-refused";
    let project = world.plan(run, &plan_of(run, vec![lifecycle("service", &[])]));
    start_from(&world, &launch, &project);
    world.until("the landing's closures to be recorded", |world| {
        !closures_of(world, run).is_empty() && world.run_file(run, "result.json").is_file()
    });
    let payload = delivered_failure_recorded(&world, run, ticket);
    assert_eq!(payload["class"], "refused", "{payload}");
    assert_eq!(
        card(&served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
}

/// A delivered ticket whose read fails for now makes the closure transient, so it is asked
/// again on the worker's schedule; the retry reads the item it wrote as `unchanged` and carries
/// no word on the ticket, so the record kept is the attempt that wrote the item — `failed`,
/// transient, its items verbatim — and it is surfaced.
#[test]
fn a_delivered_ticket_failing_for_now_is_asked_again_and_its_failure_kept() {
    let ticket = "board:ISSUE-77";
    let (world, served, launch) = closing_seven_delivering("closures-delivered-transient", ticket);
    world.script("hold.wait", "hold");
    let run = "delivered-transient";
    let project = world.plan(
        run,
        &plan_of(run, vec![lifecycle("service", &[]), agent("hold", &[])]),
    );
    start_from(&world, &launch, &project);
    world.until("the landing's closures to be recorded", |world| {
        !closures_of(world, run).is_empty()
    });
    let payload = delivered_failure_recorded(&world, run, ticket);
    assert_eq!(payload["class"], "transient", "{payload}");
    let reads = log_of(&served)
        .iter()
        .filter(|request| {
            request["query"]
                .as_str()
                .is_some_and(|query| query.contains("r0:repository(owner:$owner0"))
        })
        .count();
    assert!(reads >= 2, "the transient closure was not asked again");
    assert_eq!(
        card(&served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
    read_no_board_whole(&served);
    finished(&world, run);
}

/// A retry is the last word on a closure whenever it says anything: a ticket failing for now
/// sends the closure round again, and a retry the board refuses for good is what the record
/// keeps — `failed`, refused, its items the retry's own — rather than the earlier attempt's,
/// which is kept only over a retry that reads the item back `unchanged`.
#[test]
fn a_retry_refused_for_good_after_a_delivered_ticket_failed_is_the_record_kept() {
    let (world, served, launch) =
        closing_seven_delivering("closures-delivered-then-refused", "board:ISSUE-77");
    served.reads(&["pass", "refuse"]);
    world.script("hold.wait", "hold");
    let run = "delivered-then-refused";
    let project = world.plan(
        run,
        &plan_of(run, vec![lifecycle("service", &[]), agent("hold", &[])]),
    );
    start_from(&world, &launch, &project);
    world.until("the landing's closures to be recorded", |world| {
        !closures_of(world, run).is_empty()
    });
    let records = closures_of(&world, run);
    assert_eq!(records.len(), 1, "{records:#?}");
    let payload = &records[0]["payload"];
    assert_eq!(payload["outcome"], "failed", "{payload}");
    assert_eq!(payload["class"], "refused", "{payload}");
    assert!(
        payload["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("the loopback refuses this read by reference")),
        "{payload}"
    );
    assert_eq!(payload["items"][0]["action"], "failed", "{payload}");
    assert_eq!(payload["items"][0].get("delivered"), None, "{payload}");
    assert_eq!(
        card(&served, "1", "7"),
        (json!("Done"), issue("CLOSED", Some("COMPLETED")))
    );
    finished(&world, run);
}
