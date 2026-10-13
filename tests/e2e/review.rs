//! Review discovery drives the compiled binary and real linked onevcs over real
//! git. The gh wire adapter delegates review data and costs to onevcs-testing's
//! remote double. Feedback is seeded at that host boundary, never in the engine.

use crate::harness::{lifecycle, plan_of, World};
use crate::stacked_drafts::{settled, team_world};
use onevcs::{ChangeId, CommentId, CommentKind};
use onevcs_testing::{FileHost, HostComment, HostState};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::process::{Child, Output, Stdio};

// Every held subprocess belongs to this test and is reaped even on an assertion
// failure. The captured Child identifies it; no process pattern is signalled.
struct Held(Child);
impl Held {
    fn output(&mut self) -> Output {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        self.stdout
            .take()
            .unwrap()
            .read_to_end(&mut stdout)
            .unwrap();
        self.stderr
            .take()
            .unwrap()
            .read_to_end(&mut stderr)
            .unwrap();
        Output {
            status: self.wait().unwrap(),
            stdout,
            stderr,
        }
    }
}
impl std::ops::Deref for Held {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}
impl std::ops::DerefMut for Held {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}
impl Drop for Held {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

const AT: &str = "2026-10-10T18:34:13Z";

// llmlint: ignore-block[e2e_not_mocked] The task explicitly requires onevcs-testing’s remote double at GitHub’s boundary. The binary calls real linked onevcs, which sends GraphQL to fake-gh; no review intake or state derivation is mocked. Node op-review owns the real GitHub and board onepipeline-smoke journey.
fn host(world: &World, run: &str, nodes: &[&str]) -> FileHost {
    let mut state = HostState::default();
    for (offset, node) in nodes.iter().enumerate() {
        let settlement = settled(world, run, node);
        state.changes.push(
            serde_json::from_value(json!({
                "id": (offset + 1).to_string(), "url": settlement["change_url"],
                "head_sha": settlement["head"], "base": settlement["base"],
            }))
            .expect("the real publication supplied this change request"),
        );
    }
    FileHost::seeded(world.fakes.join("review-host.json"), state).expect("host seeded")
}
// llmlint: ignore-end[e2e_not_mocked]

fn comment(id: &str, kind: CommentKind, number: u32) -> HostComment {
    HostComment {
        id: CommentId(id.to_owned()),
        kind,
        author: "same-login".to_owned(),
        body: format!("Please fix comment {id}"),
        url: format!("https://github.com/owner/service/pull/{number}#issuecomment-{id}"),
        created_at: AT.to_owned(),
        updated_at: AT.to_owned(),
        in_reply_to: None,
        revision: 0,
    }
}

fn add(host: &FileHost, number: u32, comment: HostComment) {
    host.add_comment(&ChangeId(number.to_string()), comment)
        .expect("reviewer posts feedback");
}

fn surfaces(world: &World, run: &str) -> Vec<Value> {
    std::fs::read_to_string(world.run_file(run, "channel/surfaces.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|surface| surface["kind"] == "review-comment")
        .collect()
}

fn review(world: &World, run: &str) -> crate::harness::Run {
    let output = world.run_with_stdin(&["review", run, "--json"], "this is deliberately not JSON");
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&output.stdout).unwrap()["state"],
        "performed"
    );
    output
}

fn launched(world: &World, run: &str) -> String {
    launched_with(world, run, &[])
}

// llmlint: ignore-block[e2e_not_mocked] The plan deliberately chooses scripted agent subprocesses and the onevcs-testing remote double for this node’s offline journeys. Draft creation still runs the compiled CLI, actual oneagentgraph and onevcs, and real git. Node op-review owns live agents/GitHub/board onepipeline-smoke; this setup mocks none of the review layer under test.
fn launched_with(world: &World, run: &str, extra: &[&str]) -> String {
    world.script("a.work", "first draft\n");
    world.script("b.work", "second draft\n");
    let mut plan = plan_of(run, vec![lifecycle("a", &[]), lifecycle("b", &[])]);
    plan["concurrency"] = json!(1);
    let project = world.plan(run, &plan);
    let mut args = vec!["start", project.as_str(), "--attach"];
    args.extend_from_slice(extra);
    let output = world.run_from(&world.project, &args);
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    project
}
// llmlint: ignore-end[e2e_not_mocked]

fn reply(world: &World, run: &str, commands: Value) -> crate::harness::Run {
    world.run_with_stdin(
        &["reply", run],
        &json!({"version": 3, "commands": commands}).to_string(),
    )
}

#[test]
fn discovers_all_comment_kinds_once_per_revision_without_starting_work_or_writing_the_store() {
    let world = team_world("review-discovery");
    let run = "drafts";
    let project = launched(&world, run);
    let host = host(&world, run, &["a", "b"]);
    add(
        &host,
        1,
        comment(
            "101",
            CommentKind::ReviewThread {
                thread: "thread-101".into(),
                path: "src/reader.rs".into(),
                line: Some(7),
                outdated: false,
                resolved: false,
            },
            1,
        ),
    );
    add(
        &host,
        1,
        comment(
            "102",
            CommentKind::Review {
                state: "commented".into(),
            },
            1,
        ),
    );
    for id in ["105", "106"] {
        let mut threaded = comment(
            id,
            CommentKind::ReviewThread {
                thread: "thread-without-line".into(),
                path: "src/header.rs".into(),
                line: None,
                outdated: false,
                resolved: false,
            },
            1,
        );
        if id == "106" {
            threaded.in_reply_to = Some(CommentId("105".into()));
        }
        add(&host, 1, threaded);
    }
    let mut long = comment("103", CommentKind::Conversation, 2);
    long.body.push_str(&" review details λ".repeat(400));
    long.body.push_str(" THE END OF THE COMMENT");
    add(&host, 2, long);
    let mut ours = comment("104", CommentKind::Conversation, 2);
    ours.body = "Already replied\n<!-- onevcs:reply in-reply-to=103 key=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa -->".into();
    add(&host, 2, ours);
    let before = world.journal(run);
    let projections = world.run_file(run, onepipeline::cli::WRITEBACK_PROJECTIONS_FILE);
    let projected_before = std::fs::read(&projections).unwrap();

    let stored_before = world.store_tasks(&project);
    review(&world, run);
    let found = surfaces(&world, run);
    assert_eq!(found.len(), 5, "{found:#?}");
    for (id, node, location) in [
        ("101", "a", "src/reader.rs:7"),
        ("102", "a", "review summary"),
        ("103", "b", "conversation comment"),
        ("105", "a", "src/header.rs"),
        ("106", "a", "src/header.rs"),
    ] {
        let surface = found
            .iter()
            .find(|surface| surface["correlation"].as_str().unwrap().contains(id))
            .unwrap();
        assert_eq!(surface["workstream"], node);
        assert_eq!(surface["blocking"], false);
        let message = surface["message"].as_str().unwrap();
        if id == "105" || id == "106" {
            assert!(message.contains("at src/header.rs\n"), "{message}");
        }
        for text in [
            node,
            "https://github.com/owner/service/pull/",
            "same-login",
            location,
            "#issuecomment-",
            "Please fix",
            "rework",
            "comments",
            "decline",
        ] {
            assert!(message.contains(text), "missing {text}: {message}");
        }
    }
    assert!(found.iter().any(|surface| surface["message"]
        .as_str()
        .unwrap()
        .contains("THE END OF THE COMMENT")));
    for record in world.events_of(run, "planner-surface-queued") {
        if record["payload"]["kind"] == "review-comment" {
            assert!(
                record["payload"]["message"].as_str().unwrap().len()
                    <= onepipeline::event::MAX_PAYLOAD_TEXT_BYTES
            );
        }
    }
    assert_eq!(world.events_of(run, "review-comments-read").len(), 2);
    if let Some(path) = std::env::var_os("ONEPIPELINE_CAPTURE_REVIEW_RECORD") {
        let record = world.events_of(run, "review-comments-read")[0].clone();
        std::fs::write(path, serde_json::to_string(&record).unwrap()).unwrap();
    }
    for record in world.events_of(run, "review-comments-read") {
        assert_eq!(
            record["payload"]["cost"],
            json!({"graphql_points":1,"rest_requests":0})
        );
        assert!(record["payload"]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|comment| comment.get("body").is_none()));
        if record["payload"]["node"] == "a" {
            let child = record["payload"]["comments"]
                .as_array()
                .unwrap()
                .iter()
                .find(|comment| comment["id"] == "106")
                .unwrap();
            assert_eq!(child["in_reply_to"], "105");
            assert_eq!(child["path"], "src/header.rs");
            assert_eq!(child["thread"], "thread-without-line");
            assert!(child.get("line").is_none());
        }
        assert!(std::path::Path::new(record["artifacts"][0]["id"].as_str().unwrap()).is_file());
        let body = std::fs::read(record["artifacts"][0]["id"].as_str().unwrap()).unwrap();
        assert_eq!(record["artifacts"][0]["bytes"], body.len());
        if record["payload"]["node"] == "b" {
            let comments: Vec<onevcs::ReviewComment> = serde_json::from_slice(&body).unwrap();
            assert!(comments
                .iter()
                .any(|comment| comment.body.ends_with("THE END OF THE COMMENT")));
        }
    }
    assert_eq!(
        world.events_of(run, "node-dispatched").len(),
        before
            .iter()
            .filter(|record| record["kind"] == "node-dispatched")
            .count()
    );
    assert!(world.events_of(run, "edit-committed").is_empty());
    assert_eq!(world.store_tasks(&project), stored_before);
    assert_eq!(std::fs::read(&projections).unwrap(), projected_before);
    review(&world, run);
    assert_eq!(surfaces(&world, run).len(), 5);
    host.edit_comment(
        &ChangeId("1".into()),
        &CommentId("101".into()),
        "Edited feedback",
    )
    .unwrap();
    review(&world, run);
    assert_eq!(surfaces(&world, run).len(), 6);
    review(&world, run);
    assert_eq!(surfaces(&world, run).len(), 6);
    assert_eq!(std::fs::read(&projections).unwrap(), projected_before);
    let next = world.run(&["next", run]);
    assert!(next.stdout.contains("Please fix"));
}

fn marked(host: &FileHost, id: &str, revision: &str, label: Option<&str>) {
    let key: String = Sha256::digest(format!("a:{id}:{revision}"))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut response = comment(&format!("reply-{id}"), CommentKind::Conversation, 1);
    response.body = format!(
        "Replied\n<!-- onevcs:reply in-reply-to={id} key={key}{} -->",
        label
            .map(|label| format!(" label={label}"))
            .unwrap_or_default()
    );
    add(host, 1, response);
}

#[test]
fn state_comes_from_current_host_replies_and_rework_provenance() {
    let world = team_world("review-derived-state");
    let run = "states";
    launched(&world, run);
    let host = host(&world, run, &["a", "b"]);
    for id in ["201", "202", "203", "204", "205"] {
        add(&host, 1, comment(id, CommentKind::Conversation, 1));
    }
    marked(&host, "201", AT, None);
    marked(&host, "202", AT, Some("declined"));
    marked(&host, "203", "2026-10-09T18:34:13Z", None);
    world.script("a-rework-1.work", "reworked draft\n");
    let reworked = reply(
        &world,
        run,
        json!([{"op":"rework","id":"a","reason":"review feedback", "instruction":"Fix the feedback", "comments":[{"id":"204","url":"https://github.com/owner/service/pull/1#issuecomment-204"}]}]),
    );
    assert_eq!(reworked.code, 0, "{}", reworked.stderr);
    assert_eq!(world.run(&["adopt", run]).code, 0);
    review(&world, run);
    let human = world.run(&["review", run]);
    assert_eq!(human.code, 0, "{}", human.stderr);
    assert_eq!(human.stdout.trim(), "review performed");
    let pending = surfaces(&world, run);
    assert_eq!(pending.len(), 2, "{pending:#?}");
    assert!(pending
        .iter()
        .any(|surface| surface["correlation"].as_str().unwrap().contains("203:")));
    assert!(pending
        .iter()
        .any(|surface| surface["correlation"].as_str().unwrap().contains("205:")));
    let output = world.run(&["results", run, "--json"]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    let results: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(results["schema_version"], 1);
    let golden: Value =
        serde_json::from_str(include_str!("../golden/review-results-v1.json")).unwrap();
    let mut selected = results.clone();
    selected["nodes"]
        .as_array_mut()
        .unwrap()
        .retain(|node| node["id"] == "a-rework-1");
    assert_eq!(selected, golden);
    let empty = results["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "b")
        .unwrap();
    assert!(empty.get("comments").is_none());
    let node = results["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "a-rework-1")
        .unwrap();
    for (id, expected) in [
        ("201", "answered"),
        ("202", "declined"),
        ("203", "pending"),
        ("204", "being-reworked"),
        ("205", "pending"),
    ] {
        let comment = node["comments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|comment| comment["id"] == id)
            .unwrap();
        assert_eq!(comment["state"], expected, "{comment}");
    }
    for verb in ["status", "results"] {
        let output = world.run(&[verb, run]);
        for state in [
            "answered:",
            "declined:",
            "pending",
            "being reworked by a-rework-1",
        ] {
            assert!(output.stdout.contains(state), "{}", output.stdout);
        }
    }
    let mut remaining = host.state().unwrap();
    remaining
        .review_comments
        .get_mut(&ChangeId("1".into()))
        .unwrap()
        .retain(|comment| comment.id != CommentId("reply-201".into()));
    // llmlint: ignore[e2e_not_mocked] The task requires onevcs-testing's remote double; this reseed is a reviewer deleting a reply at that host boundary. The binary still uses real onevcs GraphQL and state derivation. Node op-review owns the live GitHub/board onepipeline-smoke journey.
    let _host = FileHost::seeded(world.fakes.join("review-host.json"), remaining).unwrap();
    let output = world.run(&["results", run, "--json"]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    let results: Value = serde_json::from_str(&output.stdout).unwrap();
    let restored = results["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "a-rework-1")
        .unwrap()["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|comment| comment["id"] == "201")
        .unwrap();
    assert_eq!(restored["state"], "pending", "{restored}");
    review(&world, run);
    review(&world, run);
    assert_eq!(surfaces(&world, run).len(), 3);
}

#[test]
fn refuses_unknown_or_foreign_runs_and_journals_successful_reads_before_a_host_failure() {
    let world = team_world("review-refusals");
    assert_eq!(
        world
            .run_with_stdin(&["review", "absent"], "bad input")
            .code,
        2
    );
    launched(&world, "refusals");
    let foreign = world.as_session("other-session");
    assert_eq!(
        foreign
            .run_with_stdin(&["review", "refusals"], "bad input")
            .code,
        2
    );
    let host = host(&world, "refusals", &["a"]);
    add(&host, 1, comment("301", CommentKind::Conversation, 1));
    let output = world.run_with_stdin(&["review", "refusals"], "bad input");
    assert_eq!(output.code, 1, "{}\n{}", output.stdout, output.stderr);
    assert!(output.stdout.is_empty(), "{}", output.stdout);
    assert!(
        output.stderr.contains("review failed:"),
        "{}",
        output.stderr
    );
    for verb in ["status", "results"] {
        let output = world.run(&[verb, "refusals"]);
        assert_eq!(output.code, 0, "{}", output.stderr);
        assert!(
            output.stdout.contains("review state could not be read:"),
            "{}",
            output.stdout
        );
    }
    let output = world.run(&["results", "refusals", "--json"]);
    assert_eq!(output.code, 2);
    assert!(output.stdout.is_empty());
    assert!(output.stderr.contains("onevcs"), "{}", output.stderr);
    let refused = reply(
        &world,
        "refusals",
        json!([{"op":"review"}, {"op":"finding","message":"must not apply"}]),
    );
    assert_eq!(refused.code, 2, "{}\n{}", refused.stdout, refused.stderr);
    assert!(
        refused
            .stderr
            .contains("review discovery must be requested alone"),
        "{}",
        refused.stderr
    );
    assert!(world.events_of("refusals", "finding-recorded").is_empty());
    let read = world.events_of("refusals", "review-comments-read");
    assert_eq!(read.len(), 1);
    assert_eq!(read[0]["payload"]["node"], "a");
    assert_eq!(surfaces(&world, "refusals").len(), 1);
}

#[test]
fn discovery_is_explicit_and_a_live_driver_answers_the_durable_request() {
    let world = team_world("review-live");
    world.script("a.work", "first draft\n");
    world.script("b.work", "second draft\n");
    world.script("b.wait", "");
    let mut plan = plan_of("live", vec![lifecycle("a", &[]), lifecycle("b", &["a"])]);
    plan["concurrency"] = json!(1);
    let project = world.plan("live", &plan);
    let mut driver = world
        .cmd(&["start", &project, "--attach"])
        .current_dir(&world.project)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(Held)
        .unwrap();
    world.until("a settled and b dispatched", |world| {
        !world.events_of("live", "node-settled").is_empty()
            && world.events_of("live", "node-dispatched").len() == 2
    });
    let host = host(&world, "live", &["a"]);
    add(&host, 1, comment("401", CommentKind::Conversation, 1));
    std::thread::sleep(std::time::Duration::from_millis(250));
    assert!(world.events_of("live", "review-comments-read").is_empty());
    assert!(surfaces(&world, "live").is_empty());
    review(&world, "live");
    world.script("b.go", "");
    assert!(driver.wait().unwrap().success());
    assert_eq!(surfaces(&world, "live").len(), 1);
    let commands =
        std::fs::read_to_string(world.run_file("live", "channel/commands.jsonl")).unwrap();
    assert!(commands.contains("\"op\":\"review\""));
    let launch: Value = serde_json::from_str(
        &std::fs::read_to_string(world.run_file("live", "launch.json")).unwrap(),
    )
    .unwrap();
    assert!(!launch.to_string().contains("discovery_interval"));
    assert_eq!(
        world
            .run(&["start", &project, "--discovery-interval", "1"])
            .code,
        2
    );
}

#[test]
fn a_cold_host_restores_rework_provenance_from_the_store_without_review_state_keys() {
    let world = team_world("review-cold");
    let project = launched(&world, "warm");
    let host = host(&world, "warm", &["a", "b"]);
    for id in ["501", "502"] {
        add(&host, 1, comment(id, CommentKind::Conversation, 1));
    }
    review(&world, "warm");
    assert_eq!(surfaces(&world, "warm").len(), 2);
    world.script("a-rework-1.work", "reworked draft\n");
    let output = reply(
        &world,
        "warm",
        json!([{"op":"rework", "id":"a", "reason":"review feedback", "instruction":"Fix comment 501", "comments":[{"id":"501", "url":"https://github.com/owner/service/pull/1#issuecomment-501"}]}]),
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(world.run(&["adopt", "warm"]).code, 0);
    let warm = world.run(&["results", "warm", "--json"]);
    assert_eq!(warm.code, 0, "{}", warm.stderr);
    let warm: Value = serde_json::from_str(&warm.stdout).unwrap();
    let warm_comments = warm["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "a-rework-1")
        .unwrap()["comments"]
        .clone();
    let tasks = world.store_tasks(&project);
    for task in &tasks {
        for key in task["item"]["metadata"].as_object().unwrap().keys() {
            assert!(
                !key.starts_with("onepipeline.review"),
                "review state leaked: {key}"
            );
        }
    }
    let original = tasks
        .iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == "a")
        .unwrap()["item"]["metadata"]
        .clone();
    let cold = world.on_host("cold-review-host");
    std::fs::remove_dir_all(&world.runs).unwrap();
    std::fs::remove_dir_all(world.onevcs_home()).unwrap();
    let continued = cold.run_from(&cold.project, &["start", &project, "--attach"]);
    assert_eq!(
        continued.code, 0,
        "{}\n{}",
        continued.stdout, continued.stderr
    );
    let run = "warm";
    let cold_json = cold.run(&["results", run, "--json"]);
    assert_eq!(cold_json.code, 0, "{}", cold_json.stderr);
    let cold_json: Value = serde_json::from_str(&cold_json.stdout).unwrap();
    let cold_comments = cold_json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "a")
        .unwrap()["comments"]
        .clone();
    assert_eq!(cold_comments, warm_comments);
    let status = cold.run(&["status", run]);
    assert!(status.stdout.contains("being reworked by a-rework-1"));
    assert!(status.stdout.contains("502: pending"));
    review(&cold, run);
    assert_eq!(surfaces(&cold, run).len(), 1);
    assert!(surfaces(&cold, run)[0]["correlation"]
        .as_str()
        .unwrap()
        .contains("502:"));
    review(&cold, run);
    assert_eq!(surfaces(&cold, run).len(), 1);
    for host in [&cold, &cold.on_host("third-review-host")] {
        if host.session != cold.session {
            let output = host.run_from(&host.project, &["start", &project, "--attach"]);
            assert_eq!(output.code, 0, "{}", output.stderr);
            let result = host.run(&["results", run, "--json"]);
            assert_eq!(result.code, 0, "{}", result.stderr);
            assert!(result.stdout.contains("a-rework-1"));
        }
        let projected = host
            .store_tasks(&project)
            .into_iter()
            .find(|task| task["item"]["metadata"]["onepipeline.id"] == "a")
            .unwrap()["item"]["metadata"]
            .clone();
        for key in ["onepipeline.rework", "onepipeline.node"] {
            assert_eq!(
                projected[key].to_string().as_bytes(),
                original[key].to_string().as_bytes(),
                "{key}"
            );
        }
    }
    world.script("a-rework-2.work", "the second rework\n");
    let output = reply(
        &cold,
        run,
        json!([{"op":"rework","id":"a","replacement":"a-rework-2",
            "reason":"new feedback","instruction":"Fix comment 502",
            "comments":[{"id":"502","url":"https://github.com/owner/service/pull/1#issuecomment-502"}]}]),
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(cold.run(&["adopt", run]).code, 0);
    let projected = cold
        .store_tasks(&project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == "a")
        .unwrap()["item"]["metadata"]
        .clone();
    assert_eq!(projected["onepipeline.node"], "a-rework-2");
    assert_eq!(projected["onepipeline.rework"]["comments"][0]["id"], "502");
    assert_eq!(projected["onepipeline.rework"]["reason"], "new feedback");
    assert_ne!(
        projected["onepipeline.rework"],
        original["onepipeline.rework"]
    );
    let hot = cold.run(&["results", run, "--json"]);
    assert_eq!(hot.code, 0, "{}", hot.stderr);
    let hot: Value = serde_json::from_str(&hot.stdout).unwrap();
    let hot_comments = hot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "a-rework-2")
        .unwrap()["comments"]
        .clone();
    let fresh = cold.on_host("fourth-review-host");
    let output = fresh.run_from(&fresh.project, &["start", &project, "--attach"]);
    assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
    let fresh_results = fresh.run(&["results", run, "--json"]);
    assert_eq!(fresh_results.code, 0, "{}", fresh_results.stderr);
    let fresh_results: Value = serde_json::from_str(&fresh_results.stdout).unwrap();
    let fresh_comments = fresh_results["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "a")
        .unwrap()["comments"]
        .clone();
    assert_eq!(hot_comments, fresh_comments);
    for (id, expected) in [("501", "pending"), ("502", "being-reworked")] {
        let comment = fresh_comments
            .as_array()
            .unwrap()
            .iter()
            .find(|comment| comment["id"] == id)
            .unwrap();
        assert_eq!(comment["state"], expected);
    }
    for (host, count) in [(&cold, 2), (&fresh, 1)] {
        review(host, run);
        review(host, run);
        let found = surfaces(host, run);
        assert_eq!(found.len(), count);
        assert!(found
            .iter()
            .any(|surface| surface["correlation"].as_str().unwrap().contains("501:")));
    }
}

#[test]
fn an_undriven_pass_holds_the_lock_without_reopening_the_run_or_firing_its_hook() {
    let world = team_world("review-lock");
    let record = crate::run_end_hooks::records(&world);
    std::fs::create_dir_all(&record).unwrap();
    let world = world.with_env(crate::run_end_hooks::RECORD_ENV, &record.to_string_lossy());
    let hook = crate::run_end_hooks::hook(&world);
    launched_with(
        &world,
        "lock",
        &["--success-hook", &hook, "--hook-timeout", "5"],
    );
    let host = host(&world, "lock", &["a", "b"]);
    add(&host, 1, comment("601", CommentKind::Conversation, 1));
    let before_launch = std::fs::read(world.run_file("lock", "launch.json")).unwrap();
    let before_hooks = world.events_of("lock", "run-hook-fired");
    assert_eq!(before_hooks.len(), 1);
    let before_adoptions = world.events_of("lock", "driver-adopted");
    let seam = world.root.join("review-seam");
    std::fs::create_dir_all(&seam).unwrap();
    std::fs::write(seam.join("after-read"), "").unwrap();
    let mut pass = world
        .cmd(&["review", "lock", "--json"])
        .env("ONEPIPELINE_REVIEW_FAILPOINT", &seam)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(Held)
        .unwrap();
    world.until("review holds the ownership lock", |_| {
        seam.join("after-read.reached").is_file()
    });
    let lock: Value = serde_json::from_str(
        &std::fs::read_to_string(world.run_file("lock", "owner.lock")).unwrap(),
    )
    .unwrap();
    assert_eq!(lock["pid"], pass.id());
    let mut writer = world
        .cmd(&["reply", "lock"])
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "3")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(Held)
        .unwrap();
    use std::io::Write;
    writer.stdin.take().unwrap().write_all(json!({"version":3,"commands":[{"op":"retitle","id":"a","title":"fix(a): reviewed title","reason":"reviewed"}]}).to_string().as_bytes()).unwrap();
    world.until("the reply is durable", |world| {
        world.run_file("lock", "channel/commands.jsonl").is_file()
    });
    assert!(world.events_of("lock", "change-retitled").is_empty());
    std::fs::write(seam.join("after-read.release"), "").unwrap();
    assert!(pass.wait().unwrap().success());
    assert!(writer.wait().unwrap().success());
    assert!(!world.run_file("lock", "owner.lock").exists());
    assert_eq!(
        std::fs::read(world.run_file("lock", "launch.json")).unwrap(),
        before_launch
    );
    assert_eq!(world.events_of("lock", "run-hook-fired"), before_hooks);
    assert_eq!(world.events_of("lock", "driver-adopted"), before_adoptions);
    assert_eq!(world.events_of("lock", "change-retitled").len(), 1);
    let status = world.run(&["status", "lock"]);
    assert!(status.stdout.contains("SETTLED"), "{}", status.stdout);
}

#[test]
fn a_crashed_pass_recovers_each_write_boundary_without_losing_or_duplicating_feedback() {
    for stage in ["after-read", "after-surface"] {
        let world = team_world(&format!("review-crash-{stage}"));
        launched(&world, "crash");
        let host = host(&world, "crash", &["a", "b"]);
        add(&host, 1, comment("701", CommentKind::Conversation, 1));
        let seam = world.root.join("review-crash-seam");
        std::fs::create_dir_all(&seam).unwrap();
        std::fs::write(seam.join(stage), "").unwrap();
        let mut pass = world
            .cmd(&["review", "crash"])
            .env("ONEPIPELINE_REVIEW_FAILPOINT", &seam)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(Held)
            .unwrap();
        world.until("the exact crash boundary", |_| {
            seam.join(format!("{stage}.reached")).is_file()
        });
        pass.kill().unwrap();
        pass.wait().unwrap();
        review(&world, "crash");
        review(&world, "crash");
        assert_eq!(surfaces(&world, "crash").len(), 1, "{stage}");
        let recorded = world
            .events_of("crash", "planner-surface-queued")
            .into_iter()
            .filter(|record| record["payload"]["kind"] == "review-comment")
            .collect::<Vec<_>>();
        assert_eq!(recorded.len(), 1, "{stage}: {recorded:#?}");
        assert_eq!(
            recorded[0]["payload"]["correlation"],
            format!("review-comment:701:{AT}")
        );
    }
}

#[test]
fn a_driver_that_cannot_finish_within_the_reply_window_returns_requested_without_reading_stdin() {
    let world = team_world("review-timeout");
    let seam = world.root.join("review-timeout-seam");
    std::fs::create_dir_all(&seam).unwrap();
    std::fs::write(seam.join("after-read"), "").unwrap();
    let world = world.with_env("ONEPIPELINE_REVIEW_FAILPOINT", &seam.to_string_lossy());
    world.script("a.work", "draft\n");
    world.script("b.work", "second draft\n");
    world.script("b.wait", "");
    let project = world.plan(
        "timeout",
        &plan_of("timeout", vec![lifecycle("a", &[]), lifecycle("b", &["a"])]),
    );
    let mut driver = world
        .cmd(&["start", &project, "--attach"])
        .current_dir(&world.project)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(Held)
        .unwrap();
    world.until("a settled", |world| {
        !world.events_of("timeout", "node-settled").is_empty()
    });
    let host = host(&world, "timeout", &["a"]);
    add(&host, 1, comment("801", CommentKind::Conversation, 1));
    let oversized = world
        .as_session(&world.session)
        .with_env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "18446744073709551615")
        .run_with_stdin(&["review", "timeout", "--json"], "not read");
    assert_eq!(oversized.code, 2, "{}", oversized.stderr);
    assert!(oversized.stderr.contains("review reply timeout is too large"));
    assert!(surfaces(&world, "timeout").is_empty());
    let mut command = world.cmd(&["review", "timeout", "--json"]);
    command
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1")
        .stdin(Stdio::piped());
    let mut pass = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Keep stdin open and unwritten: any read would block past the deadline.
    let input = pass.stdin.take().unwrap();
    // The reply window starts after durable submission, excluding subprocess
    // startup and initial run reads on a shared host.
    world.until("review durably requested", |world| {
        std::fs::read_to_string(world.run_file("timeout", "channel/commands.jsonl"))
            .is_ok_and(|commands| commands.contains("\"op\":\"review\""))
    });
    let submitted = std::fs::metadata(world.run_file("timeout", "channel/commands.jsonl"))
        .unwrap()
        .modified()
        .unwrap();
    let output = pass.wait_with_output().unwrap();
    let elapsed = submitted.elapsed().unwrap();
    drop(input);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    let human_world = world
        .as_session(&world.session)
        .with_env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1");
    let human = human_world.run_with_stdin(&["review", "timeout"], "not read");
    std::fs::write(seam.join("after-read.release"), "").unwrap();
    world.script("b.go", "");
    assert!(driver.wait().unwrap().success());
    assert!(
        elapsed >= std::time::Duration::from_secs(1)
            && elapsed < std::time::Duration::from_millis(1500),
        "{elapsed:?}"
    );
    assert_eq!(receipt["state"], "requested");
    assert!(receipt["request"].is_number());
    assert_eq!(human.code, 0, "{}", human.stderr);
    assert!(
        human.stdout.starts_with("review requested ("),
        "{}",
        human.stdout
    );
    assert_eq!(surfaces(&world, "timeout").len(), 1);
}

#[test]
fn a_mixed_run_reads_only_the_continued_draft_and_the_unlanded_lineage_head() {
    let world = team_world("review-mixed");
    for node in ["a", "b", "s"] {
        world.script(&format!("{node}.work"), &format!("{node}'s draft\n"));
    }
    let mut plan = plan_of(
        "mixed",
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &[]),
            lifecycle("s", &[]),
            crate::harness::agent("no-change", &[]),
        ],
    );
    plan["concurrency"] = json!(1);
    let project = world.plan("mixed", &plan);
    assert_eq!(
        world
            .run_from(&world.project, &["start", &project, "--attach"])
            .code,
        0
    );
    let host = host(&world, "mixed", &["a", "b", "s"]);
    let landing = settled(&world, "mixed", "b")["head"].clone();
    let cold = world.on_host("mixed-cold");
    assert_eq!(
        cold.run_from(&cold.project, &["start", &project, "--attach"])
            .code,
        0
    );
    let corrected = reply(
        &cold,
        "mixed",
        json!([{"op":"settle","id":"b","outcome":"done","evidence":"the change landed","landing":landing}]),
    );
    assert_eq!(corrected.code, 0, "{}", corrected.stderr);
    let results = cold.run(&["results", "mixed"]);
    assert!(
        results
            .stdout
            .lines()
            .any(|line| line.trim_start().starts_with("b ") && line.contains("landed")),
        "{}",
        results.stdout
    );
    assert_eq!(
        reply(
            &cold,
            "mixed",
            json!([{"op":"settle","id":"s","outcome":"failed","evidence":"a correction is needed"}])
        )
        .code,
        0
    );
    cold.script("s2.work", "corrected draft\n");
    let mut replacement = lifecycle("s2", &[]);
    replacement["branch"] = json!("me/mixed/s2");
    assert_eq!(
        reply(
            &cold,
            "mixed",
            json!([{"op":"retry","id":"s","node":replacement}])
        )
        .code,
        0
    );
    assert_eq!(cold.run(&["adopt", "mixed"]).code, 0);
    let mut state = host.state().unwrap();
    state.ledger.clear();
    let settlement = settled(&cold, "mixed", "s2");
    state.changes.push(serde_json::from_value(json!({"id":"4", "url":settlement["change_url"], "head_sha":settlement["head"], "base":settlement["base"]})).unwrap());
    // llmlint: ignore[e2e_not_mocked] This mixed-run fixture reseeds the task's required onevcs-testing remote double so its host records distinguish superseded and current draft URLs. The binary's onevcs reader stays real; node op-review owns the live GitHub/board onepipeline-smoke journey.
    let host = FileHost::seeded(world.fakes.join("review-host.json"), state).unwrap();
    for number in 1..=4 {
        add(
            &host,
            number,
            comment(&format!("90{number}"), CommentKind::Conversation, number),
        );
    }
    review(&cold, "mixed");
    let reads = cold.events_of("mixed", "review-comments-read");
    let nodes: Vec<_> = reads
        .iter()
        .map(|read| read["payload"]["node"].as_str().unwrap())
        .collect();
    assert_eq!(nodes, ["a", "s2"]);
    let queued = surfaces(&cold, "mixed");
    assert_eq!(queued.len(), 2);
    assert_eq!(
        queued
            .iter()
            .map(|surface| surface["workstream"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["a", "s2"]
    );
    let ledger = host.ledger().unwrap();
    let asked: Vec<_> = ledger
        .iter()
        .filter(|call| call.method == "review_comments")
        .flat_map(|call| call.changes.iter().map(|id| id.0.as_str()))
        .collect();
    assert_eq!(asked, ["1", "4"]);
}

#[test]
fn continuation_refuses_malformed_rework_provenance_at_the_store_boundary() {
    let world = team_world("review-provenance-boundary");
    let project = launched(&world, "boundary");
    world.script("a-rework-1.work", "reworked draft\n");
    assert_eq!(reply(&world, "boundary", json!([{"op":"rework","id":"a","reason":"reviewed","instruction":"Fix the feedback","comments":[{"id":"1001","url":"https://github.com/owner/service/pull/1#issuecomment-1001"}]}])).code, 0);
    assert_eq!(world.run(&["adopt", "boundary"]).code, 0);
    let task = world
        .store_tasks(&project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == "a")
        .unwrap();
    let id: onetaskgraph_core::GlobalId = task["id"].as_str().unwrap().parse().unwrap();
    let original = task["item"]["metadata"]["onepipeline.rework"].clone();
    let cold = world.on_host("invalid-provenance-host");
    let set = |name: &str, value: Value| {
        let id = id.clone();
        let key = onetaskgraph_plugin_api::MetadataKey::new(name.to_owned()).unwrap();
        world
            .store_call(|engine| async move { engine.set_task_metadata(&id, &key, &value).await })
            .unwrap();
    };
    for field in ["reason", "instruction", "replaces", "unknown"] {
        let mut malformed = original.clone();
        malformed[field] = json!("");
        set("onepipeline.rework", malformed);
        let output = cold.run_from(&cold.project, &["start", &project, "--attach"]);
        assert_eq!(output.code, 2, "{}\n{}", output.stdout, output.stderr);
        assert!(
            output.stderr.contains("rework provenance"),
            "{}",
            output.stderr
        );
        assert!(!cold.run_file("boundary", "launch.json").exists());
    }
    for (field, value) in [
        ("id", ""),
        ("url", "not a URL"),
        ("url", "file:///tmp/review"),
    ] {
        let mut malformed = original.clone();
        malformed["comments"][0][field] = json!(value);
        set("onepipeline.rework", malformed);
        let output = cold.run_from(&cold.project, &["start", &project, "--attach"]);
        assert_eq!(output.code, 2, "{}\n{}", output.stdout, output.stderr);
        assert!(
            output.stderr.contains("rework provenance"),
            "{}",
            output.stderr
        );
        assert!(!cold.run_file("boundary", "launch.json").exists());
    }
    set("onepipeline.rework", original);
    set("onepipeline.node", Value::Null);
    let output = cold.run_from(&cold.project, &["start", &project, "--attach"]);
    assert_eq!(output.code, 2);
    assert!(
        output.stderr.contains("no replacement"),
        "{}",
        output.stderr
    );
}

#[test]
fn an_edit_after_a_rework_reply_is_pending_and_surfaced_again_including_on_a_cold_host() {
    let world = team_world("review-edited-rework");
    let project = launched(&world, "edited");
    let host = host(&world, "edited", &["a", "b"]);
    add(&host, 1, comment("1101", CommentKind::Conversation, 1));
    review(&world, "edited");
    world.script("a-rework-1.work", "reworked draft\n");
    let output = reply(
        &world,
        "edited",
        json!([{"op":"rework","id":"a","reason":"review feedback","instruction":"Fix comment 1101","comments":[{"id":"1101","url":"https://github.com/owner/service/pull/1#issuecomment-1101"}]}]),
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(world.run(&["adopt", "edited"]).code, 0);
    marked(&host, "1101", AT, None);
    let result = world.run(&["results", "edited", "--json"]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert!(
        result.stdout.contains("\"state\": \"answered\""),
        "{}",
        result.stdout
    );
    host.edit_comment(
        &ChangeId("1".into()),
        &CommentId("1101".into()),
        "The reviewer edited the answered feedback",
    )
    .unwrap();
    let result = world.run(&["results", "edited", "--json"]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert!(
        result.stdout.contains("\"state\": \"pending\""),
        "{}",
        result.stdout
    );
    review(&world, "edited");
    review(&world, "edited");
    assert_eq!(surfaces(&world, "edited").len(), 2);
    let cold = world.on_host("edited-review-cold");
    assert_eq!(
        cold.run_from(&cold.project, &["start", &project, "--attach"])
            .code,
        0
    );
    let result = cold.run(&["results", "edited", "--json"]);
    assert!(
        result.stdout.contains("\"state\": \"pending\""),
        "{}",
        result.stdout
    );
    review(&cold, "edited");
    review(&cold, "edited");
    assert_eq!(surfaces(&cold, "edited").len(), 1);
}

#[test]
fn a_live_driver_records_a_failed_review_outcome() {
    let world = team_world("review-live-failure");
    world.script("a.work", "draft\n");
    world.script("b.work", "second draft\n");
    world.script("b.wait", "");
    let project = world.plan(
        "live-failure",
        &plan_of(
            "live-failure",
            vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
        ),
    );
    let mut driver = world
        .cmd(&["start", &project, "--attach"])
        .current_dir(&world.project)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(Held)
        .unwrap();
    world.until("a settled and b dispatched", |world| {
        !world.events_of("live-failure", "node-settled").is_empty()
            && world.events_of("live-failure", "node-dispatched").len() == 2
    });
    let _host = host(&world, "live-failure", &[]);
    let output = world.run_with_stdin(&["review", "live-failure", "--json"], "not read");
    world.script("b.go", "");
    assert!(driver.wait().unwrap().success());
    assert_eq!(output.code, 1, "{}\n{}", output.stdout, output.stderr);
    let receipt: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(receipt["state"], "performed");
    assert!(receipt["error"]
        .as_str()
        .unwrap()
        .contains("no change request"));
    let outcomes =
        std::fs::read_to_string(world.run_file("live-failure", "channel/command-outcomes.jsonl"))
            .unwrap();
    let outcome: Value = serde_json::from_str(outcomes.lines().last().unwrap()).unwrap();
    assert_eq!(outcome["applied"], false);
    assert!(outcome["reason"]
        .as_str()
        .unwrap()
        .contains("no change request"));
    assert!(world
        .events_of("live-failure", "review-comments-read")
        .is_empty());
    assert!(surfaces(&world, "live-failure").is_empty());
}

#[test]
fn driver_closeout_hands_review_to_the_owner_and_refuses_a_foreign_session() {
    for foreign in [false, true] {
        let world = team_world(&format!("review-closeout-{foreign}"));
        let seam = world.root.join("closeout-seam");
        std::fs::create_dir_all(&seam).unwrap();
        std::fs::write(seam.join("before-submit"), "").unwrap();
        let world = world.with_env("ONEPIPELINE_REVIEW_FAILPOINT", &seam.to_string_lossy());
        world.script("a.work", "draft\n");
        world.script("b.work", "second draft\n");
        world.script("b.wait", "");
        let project = world.plan(
            "closeout",
            &plan_of(
                "closeout",
                vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
            ),
        );
        let mut driver = world
            .cmd(&["start", &project, "--attach"])
            .current_dir(&world.project)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(Held)
            .unwrap();
        world.until("a settled and b dispatched", |world| {
            !world.events_of("closeout", "node-settled").is_empty()
                && world.events_of("closeout", "node-dispatched").len() == 2
        });
        let initial_host = host(&world, "closeout", &["a"]);
        add(
            &initial_host,
            1,
            comment("1201", CommentKind::Conversation, 1),
        );
        let client = if foreign {
            world.as_session("foreign")
        } else {
            world.as_session(&world.session)
        };
        let mut pass = client
            .cmd(&["review", "closeout", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map(Held)
            .unwrap();
        world.until("review detected the live driver", |_| {
            seam.join("before-submit.reached").is_file()
        });
        world.script("b.go", "");
        assert!(driver.wait().unwrap().success());
        let complete_host = host(&world, "closeout", &["a", "b"]);
        add(
            &complete_host,
            1,
            comment("1201", CommentKind::Conversation, 1),
        );
        let launch = std::fs::read(world.run_file("closeout", "launch.json")).unwrap();
        std::fs::write(seam.join("before-submit.release"), "").unwrap();
        let output = pass.output();
        assert_eq!(
            output.status.code(),
            Some(if foreign { 2 } else { 0 }),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read(world.run_file("closeout", "launch.json")).unwrap(),
            launch
        );
        assert_eq!(surfaces(&world, "closeout").len(), usize::from(!foreign));
        assert_eq!(
            world.events_of("closeout", "review-comments-read").len(),
            if foreign { 0 } else { 2 }
        );
    }
}

#[test]
fn a_continued_rework_refuses_projection_when_its_seeded_baseline_cannot_preserve_it() {
    let world = team_world("review-baseline-boundary");
    let project = launched(&world, "baseline");
    world.script("a-rework-1.work", "reworked draft\n");
    let output = reply(
        &world,
        "baseline",
        json!([{"op":"rework","id":"a","reason":"feedback","instruction":"Fix feedback",
            "comments":[{"id":"1301","url":"https://github.com/owner/service/pull/1#issuecomment-1301"}]}]),
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(world.run(&["adopt", "baseline"]).code, 0);
    let tasks = world.store_tasks(&project);
    let original = tasks
        .iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == "a")
        .unwrap()["item"]["metadata"]
        .clone();
    for scenario in [
        "null",
        "unknown",
        "blank",
        "comment",
        "provenance",
        "unreadable",
        "version",
        "project",
        "item",
        "rework",
        "node",
        "replacement",
    ] {
        let cold = world.on_host(&format!("baseline-{scenario}"));
        let output = cold.run_from(&cold.project, &["start", &project, "--attach"]);
        assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
        let path = cold.run_file("baseline", onepipeline::cli::WRITEBACK_LANDED_FILE);
        let mut baseline: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        match scenario {
            "unreadable" => {
                std::fs::write(&path, "not JSON").unwrap();
            }
            _ => {
                match scenario {
                    "null" => {
                        baseline["items"]["a"]["metadata"]["onepipeline.rework"] = Value::Null
                    }
                    "unknown" => {
                        baseline["items"]["a"]["metadata"]["onepipeline.rework"]["extra"] =
                            json!(true)
                    }
                    "blank" => {
                        baseline["items"]["a"]["metadata"]["onepipeline.rework"]["reason"] =
                            json!("")
                    }
                    "comment" => {
                        baseline["items"]["a"]["metadata"]["onepipeline.rework"]["comments"][0]
                            ["url"] = json!("not a URL")
                    }
                    "provenance" => {
                        baseline["items"]["a"]["metadata"]["onepipeline.rework"]["comments"][0]
                            ["id"] = json!("different-comment")
                    }
                    "version" => baseline["schema_version"] = json!(0),
                    "project" => baseline["project"] = json!("plans:another-project"),
                    "item" => {
                        baseline["items"].as_object_mut().unwrap().remove("a");
                    }
                    "rework" => {
                        baseline["items"]["a"]["metadata"]
                            .as_object_mut()
                            .unwrap()
                            .remove("onepipeline.rework");
                    }
                    "node" => {
                        baseline["items"]["a"]["metadata"]
                            .as_object_mut()
                            .unwrap()
                            .remove("onepipeline.node");
                    }
                    "replacement" => {
                        baseline["items"]["a"]["metadata"]["onepipeline.node"] =
                            json!("wrong-replacement")
                    }
                    _ => unreachable!(),
                }
                std::fs::write(&path, serde_json::to_vec(&baseline).unwrap()).unwrap();
            }
        }
        let output = reply(
            &cold,
            "baseline",
            json!([{"op":"add","node":crate::harness::agent("extra", &[])}]),
        );
        assert_eq!(output.code, 0, "{}\n{}", output.stdout, output.stderr);
        world.script("extra.work", "extra task\n");
        let driven = cold.run(&["adopt", "baseline"]);
        assert_eq!(driven.code, 0, "{}\n{}", driven.stdout, driven.stderr);
        let reports = cold.events_of("baseline", "planner-surface-queued");
        let text = serde_json::to_string(&reports).unwrap();
        assert!(
            text.contains("cannot preserve continued rework provenance")
                || text.contains("continued rework 'a'"),
            "{scenario}: {text}\n{}",
            output.stderr
        );
        let after = cold
            .store_tasks(&project)
            .into_iter()
            .find(|task| task["item"]["metadata"]["onepipeline.id"] == "a")
            .unwrap()["item"]["metadata"]
            .clone();
        for key in ["onepipeline.rework", "onepipeline.node"] {
            assert_eq!(after[key], original[key], "{scenario}: {key}");
        }
    }
}

#[test]
fn an_ungranted_review_command_is_refused_at_submission_and_by_the_live_reader() {
    use onemessagebus::{LocalTransport, RawQueue};
    use std::sync::Arc;
    let world = team_world("review-author-refusal");
    let config = world.root.join("review-auth.yaml");
    std::fs::write(&config, "version: 1\ntransport: {kind: local}\nauthors:\n  sentinel:\n    capabilities: [finding]\n").unwrap();
    world.script("a.work", "draft\n");
    world.script("b.work", "second draft\n");
    world.script("b.wait", "");
    let project = world.plan(
        "auth",
        &plan_of("auth", vec![lifecycle("a", &[]), lifecycle("b", &["a"])]),
    );
    let mut driver = world
        .cmd(&[
            "start",
            &project,
            "--attach",
            "--bus-config",
            config.to_str().unwrap(),
        ])
        .current_dir(&world.project)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(Held)
        .unwrap();
    world.until("a settled and b dispatched", |world| {
        !world.events_of("auth", "node-settled").is_empty()
            && world.events_of("auth", "node-dispatched").len() == 2
    });
    let host = host(&world, "auth", &["a"]);
    add(&host, 1, comment("1401", CommentKind::Conversation, 1));
    let commands = world.run_file("auth", "channel/commands.jsonl");
    let before = std::fs::read(&commands).unwrap_or_default();
    let submitted = world.run_with_stdin(
        &["reply", "auth"],
        &json!({"version":3,"author":"sentinel","commands":[{"op":"review"}]}).to_string(),
    );
    let after = std::fs::read(&commands).unwrap_or_default();

    // The durable queue is a second trust boundary. A lower-level transport
    // writer can append an envelope; the compiled live reader must still judge it.
    let spec = onepipeline::channel::layout::queues()
        .into_iter()
        .find(|spec| spec.name.as_str() == onepipeline::channel::layout::COMMANDS)
        .unwrap();
    let queue = RawQueue::open(
        Arc::new(LocalTransport::open(world.run_file("auth", "channel")).unwrap()),
        spec,
        Arc::new(onepipeline::channel::layout::registry()),
    );
    let sent = queue
        .push(json!({"id":0,"author":"sentinel","commands":[{"op":"review"}]}))
        .unwrap();
    let id = sent.id.unwrap();
    let outcome_spec = onepipeline::channel::layout::queues()
        .into_iter()
        .find(|spec| spec.name.as_str() == onepipeline::channel::layout::COMMAND_OUTCOMES)
        .unwrap();
    let outcomes = RawQueue::open(
        Arc::new(LocalTransport::open(world.run_file("auth", "channel")).unwrap()),
        outcome_spec,
        Arc::new(onepipeline::channel::layout::registry()),
    );
    let outcome_of = || {
        outcomes
            .log(None)
            .unwrap()
            .into_iter()
            .map(|(record, _)| record)
            .find(|record| record["id"] == id)
    };
    world.until("the reader's refusal", |_| outcome_of().is_some());
    let outcome = outcome_of().unwrap();
    let reads = world.events_of("auth", "review-comments-read");
    let surfaced = surfaces(&world, "auth");
    let ledger = host.state().unwrap().ledger;
    let granted = world.run_with_stdin(&["review", "auth", "--json"], "not read");
    world.script("b.go", "");
    assert!(driver.wait().unwrap().success());
    assert_eq!(
        submitted.code, 2,
        "{}\n{}",
        submitted.stdout, submitted.stderr
    );
    assert!(submitted
        .stderr
        .contains("'review' is not an op the sentinel may issue"));
    assert_eq!(before, after, "submission appended a refused command");
    assert_eq!(outcome["applied"], false, "{outcome}");
    assert!(outcome["reason"]
        .as_str()
        .unwrap()
        .contains("'review' is not an op the sentinel may issue"));
    assert!(reads.is_empty());
    assert!(surfaced.is_empty());
    assert!(ledger.is_empty());
    assert_eq!(granted.code, 0, "{}", granted.stderr);
    assert_eq!(surfaces(&world, "auth").len(), 1);
}
