//! The version-1 edit envelope: all nine ops, each op's required fields, its
//! refusal cases, and the exit codes the contract assigns — `0` applied, `1`
//! accepted-not-yet-reconciled, `2` refused or malformed.
//!
//! Every edit is **applied or rejected with a reason**, and edits require a live
//! round: a bare `complete` verdict stays legal at a round boundary, and a
//! structural edit does not.
//!
//! Ported from `test_live_edit_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary; `onevcs` is not substituted at all, and the two lifecycle journeys here
// open real sessions on a real git origin. The scenario the double states is one a real
// sibling would need paid model turns to produce, and `dispatch.rs` is where the real
// `oneagentgraph` binary is driven instead. `harness.rs` carries the same suppression and
// the full rationale.

use crate::harness::{agent, human, lifecycle, plan_of, World, REFUSED};
use serde_json::{json, Value};

/// Start a run whose nodes are held open, so edits land against a live round.
fn live(world: &World, name: &str, nodes: Vec<Value>, hold: &[&str]) -> String {
    for node in hold {
        world.script(&format!("{node}.wait"), "hold");
    }
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("a node to be in flight", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });
    name.to_string()
}

fn envelope(commands: Value) -> String {
    json!({"version": 1, "commands": commands}).to_string()
}

/// The commands the reconciler committed, in order.
fn committed(world: &World, run: &str) -> Vec<String> {
    world
        .events_of(run, "edit-committed")
        .iter()
        .filter_map(|event| {
            event["payload"]["command"]["op"]
                .as_str()
                .map(str::to_string)
        })
        .collect()
}

#[test]
fn add_reparent_and_context_are_applied_and_reported_applied() {
    let world = World::new("edit-apply");
    let run = live(&world, "applied", vec![agent("slow", &[])], &["slow"]);

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([
                {"op": "add", "node": {"id": "extra", "persona": "engineer", "task": "## What\nextra"}},
                {"op": "reparent", "id": "extra", "deps": ["slow"]},
                {"op": "context", "id": "extra", "note": "the fixture moved"},
            ])),
        )
        .exited(0)
        .out_has("\"applied\"");

    assert_eq!(committed(&world, &run), vec!["add", "reparent", "context"]);
    world.release("slow.go");
    world.until("the run to settle", |world| {
        !world.events_of(&run, "round-finished").is_empty()
    });

    // The note reached the dispatch it was aimed at, as its own section.
    let relayed = world
        .journal(&run)
        .into_iter()
        .rfind(|event| {
            event["labels"]["node"] == "extra"
                && event["source"] == "agentgraph"
                && event["kind"] == "turn-activity"
        })
        .expect("the added node dispatched");
    let task = relayed["payload"]["task"].as_str().expect("task prose");
    assert!(task.contains("## Planner context"), "{task}");
    assert!(task.contains("the fixture moved"), "{task}");
    assert!(task.contains("adds no acceptance criteria"), "{task}");
}

#[test]
fn cancel_parks_a_node_and_requeue_returns_it_to_the_frontier() {
    let world = World::new("edit-park");
    let run = live(
        &world,
        "parked",
        vec![agent("slow", &[]), agent("sweep", &["slow"])],
        &["slow"],
    );

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "cancel", "id": "sweep"}])),
        )
        .exited(0);
    // Parked is a held state, not a failed one: the flag lives on the node
    // definition, so the plan of record carries it.
    world.until("the park to commit", |world| {
        committed(world, &run).contains(&"cancel".to_string())
    });
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "cancel", "id": "sweep"}])),
        )
        .exited(REFUSED)
        .err_has("already parked");

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "requeue", "id": "sweep", "amend": {"max_turns": 32}}])),
        )
        .exited(0);
    world.until("the requeue to commit", |world| {
        committed(world, &run).contains(&"requeue".to_string())
    });

    world.release("slow.go");
    world.until("the run to settle", |world| {
        !world.events_of(&run, "round-finished").is_empty()
    });
    let result = world.run_json(&run, "round-01/result.json");
    let sweep = result["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|node| node["id"] == "sweep")
        .expect("sweep");
    assert_eq!(
        sweep["status"], "done",
        "a requeued node was not dispatched"
    );
}

#[test]
fn requeue_refuses_to_rewrite_what_add_and_reparent_own() {
    let world = World::new("edit-requeue-refuse");
    let run = live(
        &world,
        "amend",
        vec![agent("slow", &[]), agent("sweep", &[])],
        &["slow", "sweep"],
    );

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "cancel", "id": "sweep"}])),
        )
        .exited(0);
    world.until("the park to commit", |world| {
        committed(world, &run).contains(&"cancel".to_string())
    });

    for key in ["id", "deps"] {
        world
            .run_with_stdin(
                &["reply", &run],
                &envelope(json!([{"op": "requeue", "id": "sweep", "amend": {key: "other"}}])),
            )
            .exited(REFUSED)
            .err_has("cannot amend");
    }
    world.release("slow.go");
    world.release("sweep.go");
}

#[test]
fn retry_supersedes_a_running_node_and_redirects_its_dependents() {
    let world = World::new("edit-retry");
    let run = live(
        &world,
        "retried",
        vec![agent("flaky", &[]), agent("after", &["flaky"])],
        &["flaky"],
    );

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{
                "op": "retry",
                "id": "flaky",
                "node": {"id": "flaky-2", "persona": "engineer", "task": "## What\nagain"},
            }])),
        )
        .exited(0);
    world.until("the retry to commit", |world| {
        committed(world, &run).contains(&"retry".to_string())
    });
    world.release("flaky.go");
    world.until("the round to finish", |world| {
        !world.events_of(&run, "round-finished").is_empty()
    });

    let result = world.run_json(&run, "round-01/result.json");
    let ids: Vec<&str> = result["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect();
    assert!(
        ids.contains(&"flaky-2"),
        "the replacement is missing: {ids:?}"
    );

    // The superseded node stays in the executed graph, cancelled, and the
    // transition removes it exactly as a `drop` would.
    let executed = world.events_of(&run, "node-settled");
    assert!(!executed.is_empty());
    world.run(&["round", "next", &run]).exited(0);
    if world.run_file(&run, "round-02/plan.json").exists() {
        let next = world.run_json(&run, "round-02/plan.json");
        let carried: Vec<&str> = next["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .filter_map(|node| node["id"].as_str())
            .collect();
        assert!(
            !carried.contains(&"flaky"),
            "the superseded node was carried: {carried:?}"
        );
    }
}

#[cfg(not(windows))]
#[test]
fn a_retry_may_name_only_one_branch() {
    let world = World::new("edit-branch");
    world.repository("local-direct", &["true"]);
    let run = live(
        &world,
        "branchy",
        vec![crate::harness::lifecycle("service", &[])],
        &["service"],
    );

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{
                "op": "retry",
                "id": "service",
                "node": {
                    "id": "service-2",
                    "repo": "owner/service",
                    "persona": "engineer",
                    "task": "## What\nagain",
                    "branch": "pinned",
                    "resume": {"branch": "preserved"},
                },
            }])),
        )
        .exited(REFUSED)
        .err_has("only one branch");

    world.release("service.go");
}

#[test]
fn drop_requires_a_dependents_fate_and_detach_keeps_them() {
    let world = World::new("edit-drop");
    let run = live(
        &world,
        "dropped",
        vec![
            agent("slow", &[]),
            agent("victim", &[]),
            agent("dependent", &["victim"]),
        ],
        &["slow", "victim"],
    );

    // `dependents` is required, so an envelope without it never reaches the
    // reconciler.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "drop", "id": "victim"}])),
        )
        .exited(REFUSED);

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "drop", "id": "victim", "dependents": "detach"}])),
        )
        .exited(0);
    world.until("the drop to commit", |world| {
        committed(world, &run).contains(&"drop".to_string())
    });

    world.release("slow.go");
    world.release("victim.go");
    world.until("the round to finish", |world| {
        !world.events_of(&run, "round-finished").is_empty()
    });
    let result = world.run_json(&run, "round-01/result.json");
    let ids: Vec<&str> = result["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect();
    assert!(
        !ids.contains(&"victim"),
        "the dropped node survived: {ids:?}"
    );
    assert!(
        ids.contains(&"dependent"),
        "detach removed the dependent too"
    );
}

#[cfg(not(windows))]
#[test]
fn drop_refuses_to_remove_the_last_unresolved_publication_anchor() {
    let world = World::new("edit-anchor");
    world.repository("local-direct", &["true"]);
    // Two lifecycle nodes on one repository: the second is stacked on the
    // first, so the first is what carries both of them to publication.
    let mut stacked = lifecycle("stacked", &["anchor"]);
    stacked["id"] = json!("stacked");
    let run = live(
        &world,
        "anchored",
        vec![agent("slow", &[]), lifecycle("anchor", &[]), stacked],
        &["slow", "anchor"],
    );

    // Dropping it would leave work for that repository with nothing left to
    // publish it — the stack would build on a branch that never lands.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "drop", "id": "anchor", "dependents": "detach"}])),
        )
        .exited(REFUSED)
        .err_has("publication anchor");

    world.release("slow.go");
    world.release("anchor.go");
}

#[test]
fn complete_is_journalled_without_touching_the_graph() {
    let world = World::new("edit-complete");
    let run = live(&world, "completed", vec![agent("slow", &[])], &["slow"]);

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "complete", "reason": "publication verified"}])),
        )
        .exited(0);
    let requested = world.events_of(&run, "completion-requested");
    assert_eq!(requested.len(), 1, "{requested:?}");
    assert_eq!(requested[0]["payload"]["reason"], "publication verified");
    world.release("slow.go");
}

#[test]
fn every_ops_refusal_case_is_answered_with_its_reason_and_exit_two() {
    let world = World::new("edit-refusals");
    let run = live(
        &world,
        "refusals",
        vec![agent("running", &[]), agent("pending", &["running"])],
        &["running"],
    );

    let cases: &[(&str, Value, &str)] = &[
        (
            "add over an existing id",
            json!([{"op": "add", "node": {"id": "running", "persona": "e", "task": "t"}}]),
            "already exists",
        ),
        (
            "add with a dangling dependency",
            json!([{"op": "add", "node": {"id": "new", "persona": "e", "task": "t", "deps": ["nowhere"]}}]),
            "not in the plan",
        ),
        (
            "reparent a started node",
            json!([{"op": "reparent", "id": "running", "deps": []}]),
            "already started",
        ),
        (
            "reparent an unknown node",
            json!([{"op": "reparent", "id": "ghost", "deps": []}]),
            "no node 'ghost'",
        ),
        (
            "retry a node that is not running, failed, or cancelled",
            json!([{"op": "retry", "id": "pending", "node": {"id": "p2", "persona": "e", "task": "t"}}]),
            "not running, failed, or cancelled",
        ),
        (
            "retry onto an id that already exists",
            json!([{"op": "retry", "id": "running", "node": {"id": "pending", "persona": "e", "task": "t"}}]),
            "must be new",
        ),
        (
            "cancel an unknown node",
            json!([{"op": "cancel", "id": "ghost"}]),
            "no node 'ghost'",
        ),
        (
            "requeue a node that is not parked",
            json!([{"op": "requeue", "id": "pending"}]),
            "not parked",
        ),
        (
            "attest something that is not a waiting human action",
            json!([{"op": "attest", "ref": "running"}]),
            "not a ready, waiting human action",
        ),
        (
            "context on an unknown node",
            json!([{"op": "context", "id": "ghost", "note": "hello"}]),
            "no node 'ghost'",
        ),
        (
            "context with an empty note",
            json!([{"op": "context", "id": "pending", "note": "   "}]),
            "cannot be empty",
        ),
        (
            "drop the node an edit would leave in a cycle",
            json!([{"op": "reparent", "id": "pending", "deps": ["pending"]}]),
            "depends on itself",
        ),
    ];

    for (what, commands, expected) in cases {
        let run_result = world.run_with_stdin(&["reply", &run], &envelope(commands.clone()));
        run_result.exited(REFUSED);
        assert!(
            run_result.stderr.contains(expected),
            "{what}: {:?} lacks {expected:?}",
            run_result.stderr
        );
    }
    // Nothing was applied: a refused edit never reaches the graph.
    assert!(committed(&world, &run).is_empty());
    world.release("running.go");
}

#[test]
fn an_edit_needs_a_live_round_but_a_bare_complete_verdict_does_not() {
    let world = World::new("edit-liveround");
    let path = world.plan(
        "boundary",
        &plan_of("boundary", vec![human("approve", &[])]),
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the round to finish", |world| {
        !world.events_of("boundary", "round-finished").is_empty()
    });

    world
        .run_with_stdin(
            &["reply", "boundary"],
            &envelope(json!([{"op": "add", "node": {"id": "late", "persona": "e", "task": "t"}}])),
        )
        .exited(REFUSED)
        .err_has("no round executing");

    world
        .run_with_stdin(
            &["reply", "boundary"],
            &envelope(json!([{"op": "complete", "reason": "nothing left to do"}])),
        )
        .exited(0);
}

#[test]
fn a_rejected_edit_is_surfaced_as_a_proposal_rather_than_silently_dropped() {
    let world = World::new("edit-surfaced");
    let run = live(&world, "rejected", vec![agent("slow", &[])], &["slow"]);

    // llmlint: ignore-block[tests_mirror_real_usage] this deliberately writes the
    // durable queue rather than going through `reply`, because the case under test is
    // the one a user *cannot* type: an edit that passed submission and then lost a race
    // to the frontier it was validated against. Through the front door the submission
    // check would reject it first, and the reconciler's own rejection path — which is
    // what surfaces the proposal and writes `edit-rejected` — would never run.
    let queue = world.run_file(&run, "channel/commands.jsonl");
    std::fs::write(
        &queue,
        format!(
            "{}\n",
            json!({"id": 0, "commands": [{"op": "cancel", "id": "nowhere"}]})
        ),
    )
    .expect("the command is queued");

    // llmlint: ignore-end[tests_mirror_real_usage]
    world.until("the reconciler to reject it", |world| {
        !world.events_of(&run, "edit-rejected").is_empty()
    });
    let rejected = world.events_of(&run, "edit-rejected");
    assert!(
        rejected[0]["payload"]["reason"]
            .as_str()
            .expect("a reason")
            .contains("no node"),
        "{rejected:?}"
    );

    world.release("slow.go");
    world.until("the run to settle", |world| {
        !world.events_of(&run, "round-finished").is_empty()
    });
    let surfaced = world.events_of(&run, "planner-surface-queued");
    assert!(
        surfaced.iter().any(|event| event["payload"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("reconciler: rejected"))),
        "no rejection surfaced: {surfaced:?}"
    );
}

#[test]
fn edits_accepted_but_not_reconciled_in_time_are_reported_queued() {
    let world = World::new("edit-queued");
    let run = live(&world, "queued", vec![agent("slow", &[])], &["slow"]);

    // The reconciler drains the queue on every pass, so to observe the queued
    // verdict the wait has to expire first.
    let mut command = world.cmd(&["reply", &run]);
    command.env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1");
    let cursor = world.run_file(&run, "channel/commands-cursor.json");
    // llmlint: ignore-block[tests_mirror_real_usage] advancing the reconciler's cursor
    // past this reply is how a reader-starved queue is arranged on purpose. The exit-1
    // verdict exists for a reconciler that did not get to the command in time, and there
    // is no invocation a planner can type that guarantees that timing.
    std::fs::write(&cursor, "99").expect("the cursor is advanced");

    // llmlint: ignore-end[tests_mirror_real_usage]
    let reply = world.run_with_stdin_on(
        command,
        &envelope(json!([{"op": "context", "id": "slow", "note": "a note"}])),
    );
    reply.exited(crate::harness::QUEUED).out_has("\"queued\"");

    world.release("slow.go");
}
