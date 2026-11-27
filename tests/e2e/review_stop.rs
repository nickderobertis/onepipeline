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

use serde_json::{json, Value};

use crate::draft_lifecycle::{rules, GREEN};
use crate::harness::{
    lifecycle, plan_of, projection_attempts, reads_on_board, settled_write_backs_each_one_update,
    task_status_on_board, World, STORE_SOURCE,
};

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
    let world = crate::stacked_drafts::team_world(name);
    world.script("review.work", "the reviewed change\n");
    world.script("held.work", "the held change\n");
    world
}

/// A plan of `nodes`, one dispatch at a time so the host numbers their change requests in plan
/// order, authored in the world's folder of Markdown: its qualified id.
fn authored(world: &World, run: &str, nodes: Vec<Value>) -> String {
    let mut plan = plan_of(run, nodes);
    plan["concurrency"] = json!(1);
    world.plan(run, &plan)
}

/// Launch `project` attached, with `extra` arguments, and hold the launch to having settled.
fn settled(world: &World, project: &str, extra: &[&str]) {
    let mut args = vec!["start", project, "--attach"];
    args.extend_from_slice(extra);
    world.run_from(&world.project, &args).settled();
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
    reads_on_board(&world, run, &project, "review", "in-review");
    reads_on_board(&world, run, &project, "held", "in-review");
    assert_eq!(outcome(&world, run, "review"), "change-review-draft");
    assert_eq!(outcome(&world, run, "held"), "change-draft");
    assert_eq!(
        task_status_on_board(&world, &project, "review"),
        ("in-review".to_owned(), "in review".to_owned())
    );
    for node in ["review", "held"] {
        assert_eq!(
            settled_write_backs_each_one_update(&world, run, node),
            1,
            "{node}"
        );
    }
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
    reads_on_board(&world, run, &project, "review", "done");
    // The landing's record is written back in one store write too.
    assert_eq!(
        settled_write_backs_each_one_update(&world, run, "review"),
        2
    );
    assert_eq!(
        task_status_on_board(&world, &project, "held").0,
        "in-review"
    );

    world.release("after.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(world.run_json(run, "result.json")["state"], "complete");
    reads_on_board(&world, run, &project, "after", "done");
    assert_eq!(
        task_status_on_board(&world, &project, "held").0,
        "in-review"
    );
}

/// Under `writeback.review_stop.author_draft: in-progress`, each draft waiting on its author —
/// a review draft, the plan's own draft, and a rework's republication kept as a review draft
/// with its check verdict still unread — reads `in-progress`, and an adoption replays the
/// setting its launch recorded rather than reading any launch config.
#[test]
fn under_author_draft_in_progress_each_draft_waiting_on_its_author_reads_in_progress() {
    let world = team_world("review-stop-in-progress");
    world.script(
        "review-rework-1.work",
        "the reviewed change, as the review asked\n",
    );
    world.script("review-rework-1.work-file", "review");
    let config = launch_config(
        &world,
        "in-progress",
        "  review_stop:\n    author_draft: in-progress\n",
    );
    let run = "inprogress";
    let project = authored(
        &world,
        run,
        vec![lifecycle("review", &[]), plan_draft("held")],
    );
    settled(&world, &project, &["--launch-config", &config]);
    assert_eq!(outcome(&world, run, "review"), "change-review-draft");
    assert_eq!(outcome(&world, run, "held"), "change-draft");
    reads_on_board(&world, run, &project, "review", "in-progress");
    reads_on_board(&world, run, &project, "held", "in-progress");
    assert_eq!(
        world.run_json(run, "launch.json")["writeback"],
        json!({"review_stop": {"author_draft": "in-progress"}})
    );

    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": onepipeline::channel::REPLY_ENVELOPE_VERSION, "commands": [{
                "op": "rework",
                "id": "review",
                "reason": "a reviewer asked for it on the draft",
                "instruction": "say what the review asked",
                "comments": [{"id": "101",
                              "url": "https://github.com/owner/service/pull/1#discussion_r101"}],
            }]})
            .to_string(),
        )
        .exited(0);
    world.run(&["adopt", run]).exited(0);
    assert_eq!(
        outcome(&world, run, "review-rework-1"),
        "change-review-draft-unchecked"
    );
    reads_on_board(&world, run, &project, "review", "in-progress");
    assert_eq!(
        task_status_on_board(&world, &project, "held").0,
        "in-progress"
    );
}

/// A change lifted for anyone to merge reads `in-review` under either setting for a draft
/// waiting on its author: the setting chooses only for a draft.
#[test]
fn a_change_lifted_for_anyone_to_merge_reads_in_review_under_author_draft_in_progress() {
    let world = open_world("review-stop-open");
    world.script("open.work", "the opened change\n");
    let config = launch_config(
        &world,
        "in-progress",
        "  review_stop:\n    author_draft: in-progress\n",
    );
    let run = "openstop";
    let project = authored(&world, run, vec![lifecycle("open", &[])]);
    settled(&world, &project, &["--launch-config", &config]);
    assert_eq!(outcome(&world, run, "open"), "change-open");
    reads_on_board(&world, run, &project, "open", "in-review");
    assert_eq!(settled_write_backs_each_one_update(&world, run, "open"), 1);
}

/// A setting under `writeback.sources.<source>` wins for a plan homed in that source, in both
/// directions, and one naming another source changes nothing for this plan.
#[test]
fn a_per_source_setting_wins_over_the_launch_s_own_for_a_plan_homed_in_that_source() {
    let world = team_world("review-stop-per-source");
    let cases = [
        (
            "overreview",
            format!(
                "  review_stop:\n    author_draft: in-progress\n  sources:\n    \
                 {STORE_SOURCE}:\n      review_stop:\n        author_draft: in-review\n"
            ),
            "in-review",
        ),
        (
            "overprogress",
            format!(
                "  sources:\n    elsewhere:\n      review_stop:\n        author_draft: \
                 in-review\n    {STORE_SOURCE}:\n      review_stop:\n        author_draft: \
                 in-progress\n"
            ),
            "in-progress",
        ),
    ];
    for (run, writeback, shown) in cases {
        let config = launch_config(&world, run, &writeback);
        let project = authored(&world, run, vec![lifecycle("review", &[])]);
        settled(&world, &project, &["--launch-config", &config]);
        assert_eq!(
            outcome(&world, run, "review"),
            "change-review-draft",
            "{run}"
        );
        reads_on_board(&world, run, &project, "review", shown);
    }
}

/// An `author_draft` that is neither `in-review` nor `in-progress` refuses the launch config as
/// it loads, naming the key it is under — the launch-wide one or a source's own — and a
/// `writeback` block in a config declaring schema 15, an empty one included, is refused naming
/// the version that added it.
/// No run is made for any of them.
#[test]
fn an_author_draft_setting_other_than_its_two_words_is_refused_naming_its_key() {
    let world = World::new("review-stop-refused");
    let project = authored(&world, "refused", vec![crate::harness::agent("only", &[])]);
    let cases = [
        (
            "schema_version: 16\nwriteback:\n  review_stop:\n    author_draft: someday\n",
            "`writeback.review_stop.author_draft` is \"someday\"",
        ),
        (
            "schema_version: 16\nwriteback:\n  sources:\n    plans:\n      review_stop:\n        \
             author_draft: in_review\n",
            "`writeback.sources.plans.review_stop.author_draft` is \"in_review\"",
        ),
        (
            "schema_version: 15\nwriteback:\n  review_stop:\n    author_draft: in-progress\n",
            "`writeback` is a schema 16 key",
        ),
        (
            "schema_version: 15\nwriteback: {}\n",
            "`writeback` is a schema 16 key",
        ),
    ];
    for (index, (document, said)) in cases.into_iter().enumerate() {
        let path = world.root.join(format!("refused-{index}.yaml"));
        std::fs::write(&path, document).expect("the launch config is written");
        let path = path.to_string_lossy().into_owned();
        let refused = world.run_from(
            &world.project,
            &["start", &project, "--attach", "--launch-config", &path],
        );
        assert_ne!(refused.code, 0, "{document}: {}", refused.stdout);
        refused.err_has(said);
    }
    assert!(
        !world.run_file("refused", "launch.json").exists(),
        "a refused launch config made a run"
    );
}

/// On a folder of Markdown whose `status_mapping` names no `in-review`, a change lifted for
/// anyone to merge reads `done`, as it did before the column existed — in the settlement's one
/// store operation, with no write refused and none sent again.
#[test]
fn a_board_without_a_review_column_reads_done_in_the_settlement_s_one_write() {
    let world = open_world("review-stop-no-column");
    world.script("open.work", "the opened change\n");
    let launch = world.root.join("no-column");
    std::fs::create_dir_all(&launch).expect("the launch directory");
    std::fs::write(
        launch.join("onetaskgraph.yaml"),
        format!(
            "sources:\n  {STORE_SOURCE}:\n    plugin: local-md\n    config:\n      root: {}\n      \
             status_mapping:\n        draft: draft\n        backlog: backlog\n        todo: todo\n        \
             queued: queued\n        in progress: in-progress\n        in-progress: in-progress\n        \
             done: done\n        cancelled: cancelled\n",
            serde_json::to_string(&world.store().to_string_lossy()).expect("a quoted path"),
        ),
    )
    .expect("the store configuration");
    let run = "nocolumn";
    let project = authored(&world, run, vec![lifecycle("open", &[])]);
    world
        .run_from(&launch, &["start", &project, "--attach"])
        .settled();
    assert_eq!(outcome(&world, run, "open"), "change-open");
    reads_on_board(&world, run, &project, "open", "done");

    assert_eq!(settled_write_backs_each_one_update(&world, run, "open"), 1);
    let records = projection_attempts(&world, run);
    assert!(
        records
            .iter()
            .all(|record| record["outcome"] == "projected" && record["class"].is_null()),
        "a write was refused: {records:#?}"
    );
}

/// On a Linear home whose `status_mapping` names a review state, a change lifted for anyone to
/// merge moves its issue to that state, `In Review`, through the linked store's Linear plugin.
#[test]
fn a_change_lifted_for_anyone_to_merge_reaches_a_linear_home_s_review_state() {
    use crate::linear_loopback::{described, Linear, PROJECT, TEAM};
    use crate::linear_writeback::{from_the_launch_directory, KEY_ENV, SOURCE, STATES, STATUSES};

    let name = "review-stop-linear";
    let world = open_world(name).with_env(KEY_ENV, "a-loopback-key");
    world.script("open.work", "the opened change\n");
    let mut states = STATES.to_vec();
    states.push(("Needs Attention", "started"));
    states.push(("In Review", "started"));
    let linear = Linear::serve(
        &states,
        STATUSES,
        name,
        &described(
            "",
            &json!({
                "onepipeline.schema_version": onepipeline::plan::PLAN_SCHEMA_VERSION,
                "onepipeline.concurrency": 1,
                "onepipeline.goal": {"text": "Ship the opened change"},
            }),
        ),
        "Planned",
    );
    let issue = linear.file(
        "feat: ship open",
        &described(
            "## What\nShip open.\n\n## Why\nUsers need it.\n\n## Acceptance criteria\n- open is \
             published.",
            &json!({"onepipeline.id": "open", "onepipeline.persona": "engineer",
                    "onepipeline.repo": "service"}),
        ),
        "Todo",
    );
    let launch = world.root.join("linear-launch");
    std::fs::create_dir_all(&launch).expect("the launch directory");
    std::fs::write(
        launch.join("onetaskgraph.yaml"),
        format!(
            "sources:\n  {SOURCE}:\n    plugin: linear\n    config:\n      api_key_env: {KEY_ENV}\n      \
             team: {TEAM}\n      endpoint: {}\n      status_mapping:\n        \
             backlog:     {{ task: Proposed,        project: Proposal }}\n        \
             draft:       {{ task: Backlog,         project: Idea }}\n        \
             todo:        {{ task: Todo,            project: Planned }}\n        \
             queued:      {{ task: Queued,          project: Accepted }}\n        \
             in-progress: In Progress\n        \
             in-review:   {{ task: In Review }}\n        \
             unknown:     {{ task: Needs Attention, project: Blocked }}\n        \
             done:        {{ task: Done,            project: Completed }}\n        \
             cancelled:   Canceled\n",
            linear.endpoint()
        ),
    )
    .expect("the launch directory's store configuration");
    let project = format!("{SOURCE}:{PROJECT}");
    let start = from_the_launch_directory(&world, &launch, &["start", &project, "--attach"]);
    world.run_on(start, "start the Linear home").settled();
    assert_eq!(outcome(&world, name, "open"), "change-open");
    world.until("the issue to reach its review state", |_| {
        linear.state_of(&issue) == "In Review"
    });
    assert_eq!(settled_write_backs_each_one_update(&world, name, "open"), 1);
}

/// The plan store's copy of a project into `destination`: what a host refreshing its plan from
/// the board runs — `project copy <board> --to <authoring source> --update-fields status` with
/// `nulled` keys set to null, or, with `writes` at its default, the first copy that files the
/// authoring copy. Answers the project's id at the destination.
fn copied(
    world: &World,
    project: &str,
    destination: &str,
    writes: onetaskgraph_core::CopyWrites,
) -> String {
    let source = crate::harness::global(project);
    let item = source.clone();
    let destination = onetaskgraph_plugin_api::SourceName::new(destination).expect("a source name");
    let report = world.store_call(|engine| async move {
        engine
            .copy_with(
                &onetaskgraph_core::CopyRequest {
                    items: onetaskgraph_core::CopyItems::new(vec![item]).expect("one item"),
                    scope: onetaskgraph_core::CopyScope::Projects {
                        tasks: true,
                        documents: false,
                    },
                    destination,
                    status_policy: onetaskgraph_core::StatusPolicy::Source,
                    target_task: None,
                    match_by: None,
                    recreate: false,
                    create: false,
                    dry_run: false,
                    comments: false,
                    no_link: false,
                    expect: None,
                    statuses: Vec::new(),
                    conditions: onetaskgraph_core::CopyConditions::default(),
                },
                &writes,
            )
            .await
            .expect("the store copies the plan")
    });
    report
        .items
        .iter()
        .find(|item| item.source == source)
        .and_then(|item| item.destination())
        .unwrap_or_else(|| panic!("the copy placed no project: {report:?}"))
        .to_string()
}

/// A replan refresh of a plan whose task stopped for review carries `in-review` back to the
/// plan's authoring source as it carries every other status: the store's own status-only copy
/// from the board, every key `plan writeback-keys` names set to null, leaves the authoring task
/// reading `in-review` with none of the run's keys — neither refused nor rewritten — and a
/// launch of the refreshed plan keeps that task as a node and dispatches it.
#[test]
fn a_refreshed_plan_keeps_its_in_review_task_and_launches_with_it_as_a_node() {
    const AUTHORING: &str = "authoring";
    let world = team_world("review-stop-refresh");
    let root = world.root.join(AUTHORING);
    std::fs::create_dir_all(&root).expect("the authoring source's root");
    let world = world
        .with_env("ONETASKGRAPH_SOURCES__AUTHORING__PLUGIN", "local-md")
        .with_env(
            "ONETASKGRAPH_SOURCES__AUTHORING__CONFIG__ROOT",
            &root.to_string_lossy(),
        );
    let run = "refreshed";
    let board = authored(&world, run, vec![lifecycle("review", &[])]);
    let plan = copied(
        &world,
        &board,
        AUTHORING,
        onetaskgraph_core::CopyWrites::default(),
    );
    settled(&world, &board, &[]);
    assert_eq!(outcome(&world, run, "review"), "change-review-draft");
    reads_on_board(&world, run, &board, "review", "in-review");

    let listed = world.run(&["plan", "writeback-keys", "--json"]);
    listed.exited(0);
    let keys = listed.json();
    let nulled: Vec<(onetaskgraph_plugin_api::MetadataKey, Value)> = ["task", "project"]
        .iter()
        .flat_map(|scope| keys[*scope].as_array().cloned().unwrap_or_default())
        .map(|key| {
            let key = key.as_str().expect("a key").to_owned();
            (
                onetaskgraph_plugin_api::MetadataKey::new(key).expect("a metadata key"),
                Value::Null,
            )
        })
        .collect();
    assert!(
        nulled
            .iter()
            .any(|(key, _)| key.as_str() == "onepipeline.settlement"),
        "{keys}"
    );
    let refreshed = copied(
        &world,
        &board,
        AUTHORING,
        onetaskgraph_core::CopyWrites {
            update_fields: onetaskgraph_core::UpdateFields::Status,
            metadata: nulled.clone(),
        },
    );
    assert_eq!(refreshed, plan, "the refresh filed a second plan");
    let tasks = world.store_tasks(&plan);
    let task = tasks
        .iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == "review")
        .unwrap_or_else(|| panic!("the refreshed plan holds no review task: {tasks:#?}"));
    assert_eq!(
        task["item"]["status"]["category"], "in-review",
        "the refresh did not carry the review stop: {task:#}"
    );
    for (key, _) in &nulled {
        assert!(
            task["item"]["metadata"].get(key.as_str()).is_none(),
            "the refresh left the run's {} on the task: {task:#}",
            key.as_str()
        );
    }

    // Launched, the refreshed plan keeps its `in-review` task as a node and dispatches it: its
    // settlement went with the run's other keys, so nothing says it was settled.
    world.script("review.wait", "hold");
    let launched = world.run_from(&world.project, &["start", &plan, "--detach"]);
    launched.exited(0);
    let again = launched.json()["run_id"]
        .as_str()
        .expect("the launch names its run")
        .to_owned();
    world.until("the refreshed plan's task to dispatch", |world| {
        world
            .events_of(&again, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "review")
    });
    world.run(&["stop", &again]).exited(0);
}
