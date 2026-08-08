//! Ported from the lifecycle-node composition halves of `test_lifecycle_e2e`.
//!
//! A lifecycle node is this crate composing a `onevcs` session with the
//! dispatches that work in it. The branch, the worktree, the gate, and the
//! publication are all the sibling's; the DAG, the rounds, and the pr-author
//! composition are this one's.

use crate::harness::{lifecycle, plan_of, World};
use serde_json::json;

fn settle(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path.to_string_lossy(), "--attach"]);
    world.until("the run to settle", |world| {
        !world.events_of(name, "round-finished").is_empty()
    });
    name.to_string()
}

/// The `onevcs` invocations, in order.
fn vcs_calls(world: &World) -> Vec<Vec<String>> {
    world
        .invocations()
        .iter()
        .filter(|invocation| invocation["tool"] == "onevcs")
        .map(|invocation| {
            invocation["args"]
                .as_array()
                .expect("args")
                .iter()
                .filter_map(|arg| arg.as_str().map(str::to_string))
                .collect()
        })
        .collect()
}

#[test]
fn a_lifecycle_node_opens_a_session_works_in_it_and_publishes_through_onevcs() {
    let world = World::new("lifecycle-publish");
    let run = settle(&world, "shipped", vec![lifecycle("service", &[])]);

    let calls = vcs_calls(&world);
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with(&["session".into(), "open".into()])),
        "no session was opened: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.first().is_some_and(|verb| verb == "publish")),
        "nothing was published: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with(&["session".into(), "close".into()])),
        "the session was never closed: {calls:?}"
    );

    // The dispatch ran *in the worktree the session handed back*, which is what
    // `WorkspaceSpec::VcsSession` means: the machine running the dispatch opens
    // the session there.
    let dispatched = world
        .journal(&run)
        .into_iter()
        .find(|event| event["source"] == "agentgraph" && event["labels"]["node"] == "service")
        .expect("the lifecycle node dispatched");
    let dir = dispatched["payload"]["dir"].as_str().expect("a directory");
    assert!(
        dir.contains("worktrees"),
        "the dispatch did not run in the session's worktree: {dir}"
    );

    // The session's own stream joins the merged one.
    assert!(
        world
            .journal(&run)
            .iter()
            .any(|event| event["source"] == "vcs" && event["kind"] == "session-opened"),
        "the session opening is missing from the merged store"
    );
    assert!(
        world
            .journal(&run)
            .iter()
            .any(|event| event["source"] == "vcs" && event["kind"] == "published"),
        "the publication is missing from the merged store"
    );

    let result = world.run_json(&run, "round-01/result.json");
    assert_eq!(result["nodes"][0]["status"], "done");
    assert_eq!(result["state"], "complete");
}

#[test]
fn several_steps_share_one_branch_and_run_serially_in_topological_order() {
    let world = World::new("lifecycle-steps");
    let node = json!({
        "id": "service",
        "repo": "owner/service",
        "steps": [
            {"id": "review", "persona": "reviewer", "task": "## What\nreview", "deps": ["implement"]},
            {"id": "implement", "persona": "engineer", "task": "## What\nimplement"},
        ],
    });
    let run = settle(&world, "workstream", vec![node]);

    let steps: Vec<String> = world
        .journal(&run)
        .iter()
        .filter(|event| event["source"] == "agentgraph")
        .filter_map(|event| event["labels"]["step"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        steps,
        vec!["implement".to_string(), "review".to_string()],
        "the steps did not run in topological order"
    );

    // Concurrent writers cannot safely share a worktree, so every step names
    // the same branch the first one opened.
    let branches: Vec<String> = vcs_calls(&world)
        .iter()
        .filter(|call| call.starts_with(&["session".into(), "open".into()]))
        .filter_map(|call| {
            call.iter()
                .position(|arg| arg == "--branch")
                .and_then(|at| call.get(at + 1).cloned())
        })
        .collect();
    assert!(
        branches.windows(2).all(|pair| pair[0] == pair[1]),
        "the steps did not share one branch: {branches:?}"
    );
    assert_eq!(
        world.run_json(&run, "round-01/result.json")["state"],
        "complete"
    );
}

#[test]
fn a_human_step_holds_the_workstream_rather_than_being_inferred() {
    let world = World::new("lifecycle-human-step");
    let node = json!({
        "id": "service",
        "repo": "owner/service",
        "steps": [
            {"id": "implement", "persona": "engineer", "task": "## What\nimplement"},
            {"id": "staging-approval", "kind": "human", "task": "Exercise the staged service.", "deps": ["implement"]},
        ],
    });
    let run = settle(&world, "gatedstream", vec![node]);

    let result = world.run_json(&run, "round-01/result.json");
    assert_eq!(result["nodes"][0]["status"], "waiting", "{result}");
    // Nothing was published: the workstream is held at its human step.
    assert!(
        !vcs_calls(&world)
            .iter()
            .any(|call| call.first().is_some_and(|verb| verb == "publish")),
        "a workstream published past its human step"
    );
}

#[test]
fn the_pr_author_dispatch_drafts_the_title_and_never_blocks_publication() {
    let world = World::new("lifecycle-pr-author");
    let run = settle(&world, "authored", vec![lifecycle("service", &[])]);

    // One post-verification dispatch, under the `pr-author` persona.
    assert!(
        world.was_invoked("oneagentgraph", &["--label", "persona=pr-author"]),
        "no pr-author dispatch: {:?}",
        world.invocations()
    );
    let drafted = std::fs::read_to_string(world.fakes.join("published.jsonl"))
        .expect("the publication was recorded");
    assert!(
        drafted.contains("feat: drafted from the diff"),
        "the drafted title did not reach publication: {drafted}"
    );
    assert_eq!(
        world.run_json(&run, "round-01/result.json")["state"],
        "complete"
    );
}

#[test]
fn a_planner_supplied_title_wins_over_the_drafting_dispatch() {
    let world = World::new("lifecycle-title");
    let mut node = lifecycle("service", &[]);
    node["title"] = json!("fix: the planner named this");
    settle(&world, "titled", vec![node]);

    let published = std::fs::read_to_string(world.fakes.join("published.jsonl"))
        .expect("the publication was recorded");
    assert!(
        published.contains("fix: the planner named this"),
        "the planner's title was overwritten: {published}"
    );
    assert!(
        !world.was_invoked("oneagentgraph", &["--label", "persona=pr-author"]),
        "a title the planner set still spent a drafting dispatch"
    );
}

#[test]
fn a_drafting_failure_falls_back_deterministic_and_still_publishes() {
    let world = World::new("lifecycle-fallback");
    // Only the drafting dispatch fails. It runs after the branch is already
    // verified and is not on the publication path, so the change still lands.
    world.script("service.pr-author.fail", "1");
    let run = settle(&world, "fallback", vec![lifecycle("service", &[])]);

    let result = world.run_json(&run, "round-01/result.json");
    assert_eq!(
        result["state"], "complete",
        "a drafting failure blocked publication: {result}"
    );
    let published = std::fs::read_to_string(world.fakes.join("published.jsonl"))
        .expect("the publication was recorded");
    assert!(
        published.contains("chore: service"),
        "the deterministic title was not used: {published}"
    );
}

#[test]
fn a_session_that_cannot_open_is_an_infrastructure_failure_by_name() {
    let world = World::new("lifecycle-nosession");
    world.script("session-open.fail", "");
    let run = settle(&world, "nosession", vec![lifecycle("service", &[])]);

    // A dispatch that never started failed for a reason that has nothing to do
    // with the agent, and is reported apart from one that ran and said nothing.
    let result = world.run_json(&run, "round-01/result.json");
    assert_eq!(result["nodes"][0]["status"], "failed", "{result}");
    assert_eq!(result["nodes"][0]["outcome"], "infrastructure-failure");
}

#[test]
fn a_publication_that_its_gate_rejects_settles_the_node_failed_by_name() {
    let world = World::new("lifecycle-gate");
    world.script("publish.fail", "");
    let run = settle(&world, "rejected", vec![lifecycle("service", &[])]);

    let result = world.run_json(&run, "round-01/result.json");
    assert_eq!(result["nodes"][0]["status"], "failed", "{result}");
    assert_eq!(result["nodes"][0]["outcome"], "publication-failed");
    world
        .run(&["results", &run])
        .exited(0)
        .out_has("publication-failed");
}

#[test]
fn a_lifecycle_node_carries_the_pins_the_plan_states_into_its_session() {
    let world = World::new("lifecycle-pins");
    let mut node = lifecycle("service", &[]);
    node["branch"] = json!("feature/pinned");
    node["base_branch"] = json!("release");
    node["execution_checkout"] = json!("primary");
    node["merge_policy"] = json!("change-auto");
    settle(&world, "pinned", vec![node]);

    let opened = vcs_calls(&world)
        .into_iter()
        .find(|call| call.starts_with(&["session".into(), "open".into()]))
        .expect("a session was opened");
    assert!(
        opened.contains(&"--branch".to_string()) && opened.contains(&"feature/pinned".to_string()),
        "{opened:?}"
    );
    assert!(
        opened.contains(&"--base".to_string()) && opened.contains(&"release".to_string()),
        "{opened:?}"
    );
    assert!(
        opened.contains(&"--execution-checkout".to_string())
            && opened.contains(&"primary".to_string()),
        "{opened:?}"
    );

    let published = vcs_calls(&world)
        .into_iter()
        .find(|call| call.first().is_some_and(|verb| verb == "publish"))
        .expect("something was published");
    assert!(
        published.contains(&"--policy".to_string())
            && published.contains(&"change-auto".to_string()),
        "the merge policy did not reach onevcs: {published:?}"
    );
}

#[test]
fn a_session_stream_that_cannot_be_read_is_reported_and_does_not_fail_the_node() {
    let world = World::new("lifecycle-noevents");
    world.script("events.fail", "");
    let run = settle(&world, "silentstream", vec![lifecycle("service", &[])]);

    // The evidence is missing, not the result: the node published and settled.
    let result = world.run_json(&run, "round-01/result.json");
    assert_eq!(result["state"], "complete", "{result}");
    assert!(
        !world
            .journal(&run)
            .iter()
            .any(|event| event["kind"] == "verification-finished"),
        "the unreadable stream still contributed events"
    );
}

#[test]
fn a_session_line_this_build_cannot_read_is_skipped_and_counted() {
    let world = World::new("lifecycle-futureline");
    world.script("events.unreadable", "");
    let run = settle(&world, "futurestream", vec![lifecycle("service", &[])]);

    // A sibling emitting a shape this build does not know must not stop the
    // node, and must not vanish silently either.
    assert_eq!(
        world.run_json(&run, "round-01/result.json")["state"],
        "complete"
    );
    assert!(
        world
            .journal(&run)
            .iter()
            .any(|event| event["source"] == "vcs" && event["kind"] == "published"),
        "the publication still reached the merged store"
    );
}
