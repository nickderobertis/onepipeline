//! The version-1 edit envelope: all nine ops, each op's required fields, its
//! refusal cases, and the exit codes the contract assigns — `0` applied, `1`
//! accepted-not-yet-reconciled, `2` refused or malformed.
//!
//! Every edit is **applied or rejected with a reason**. There is no round for an
//! edit to need: a run being driven queues it for the loop, and one nothing is
//! driving takes the ownership lock and applies it there.
//!
//! Ported from `test_live_edit_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary; `onevcs` is not substituted at all, and the two lifecycle journeys here
// open real sessions on a real git origin. The scenario the double states is one a real
// sibling would need paid model turns to produce, and `dispatch.rs` is where the real
// `oneagentgraph` binary is driven instead. `harness.rs` carries the same suppression and
// the full rationale.

use crate::harness::{agent, human, plan_of, World, REFUSED};

use crate::harness::lifecycle;
use onevcs::provenance::SUBJECT_LIMIT;
use serde_json::{json, Value};

/// Start a run whose nodes are held open, so edits land against a live loop.
fn live(world: &World, name: &str, nodes: Vec<Value>, hold: &[&str]) -> String {
    for node in hold {
        world.script(&format!("{node}.wait"), "hold");
    }
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--detach"]).exited(0);
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
    // The note names a node that has never been dispatched, so there is no turn
    // to deliver it into — which is what every `context` edit written before
    // delivery had modes relied on. It rides the next dispatch, as it always
    // did, and the record says so.
    let context = world
        .events_of(&run, "edit-committed")
        .into_iter()
        .find(|event| event["payload"]["command"]["op"] == "context")
        .expect("the note was committed");
    assert_eq!(context["payload"]["operations"][0]["delivery"], "deferred");
    assert!(
        world.events_of(&run, "turn-interrupted").is_empty(),
        "a node with no dispatch had its turn reached for"
    );

    world.release("slow.go");
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
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
        world.run_file(&run, "result.json").is_file()
    });
    let result = world.run_json(&run, "result.json");
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

/// A parked node stays parked for as long as the run lasts, and `requeue` is
/// the only way back.
///
/// `cancel` is documented as the way to idle a node, and `requeue` as what
/// resumes it — so a park the loop forgot would make `cancel` a pause wearing
/// the name of a stop, and there would be no way at all to say "stop working on
/// this". A planner who cancelled a node because they were taking the
/// deliverable over themselves would find a second execution path opened
/// against it, which is the one thing the one-path rule exists to prevent.
///
/// Both halves are asserted, because a re-dispatch has two possible causes and
/// they need different fixes: the graph the run is executing is checked for the
/// flag — that is the state *surviving the fold* — and the dispatch record is
/// checked for the node — that is the scheduler *reading* it.
#[test]
fn a_parked_node_is_never_dispatched_however_long_the_run_goes_on() {
    let world = World::new("edit-park-durable");
    // `sweep` waits on `flaky`, so it is pending rather than running when it is
    // cancelled — the state a planner reaches for `cancel` in.
    world.script("flaky.wait", "hold");
    let run = live(
        &world,
        "held-back",
        vec![agent("flaky", &[]), agent("sweep", &["flaky"])],
        &[],
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

    // Its dependency settles, which is exactly when an unparked node would be
    // dispatched — the loop starts what became ready on that same pass.
    world.release("flaky.go");
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });

    // The flag lives on the node definition, so the run's own result is where a
    // park that did not survive the fold shows up.
    let result = world.run_json(&run, "result.json");
    let sweep = result["nodes"]
        .as_array()
        .expect("the result lists every node")
        .iter()
        .find(|node| node["id"] == "sweep")
        .expect("the parked node is still in the graph, not dropped");
    assert_eq!(
        sweep["status"], "parked",
        "the park did not survive the fold: {sweep}"
    );

    // And nothing dispatched it.
    let dispatched: Vec<Value> = world
        .events_of(&run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "sweep")
        .collect();
    assert!(
        dispatched.is_empty(),
        "a parked node was dispatched anyway: {dispatched:?}"
    );
}

/// The same, for a node that was **in flight** when it was cancelled.
///
/// The other half of `cancel`'s stated range, and the one a planner reaches for
/// under pressure: the node is already running, and stopping it is the point.
/// Parking a running node also settles its dispatch, so there is a second write
/// to the node's recorded state after the park — which is exactly where a park
/// could be overwritten and the node handed back to the loop as ordinary work.
#[test]
fn a_node_parked_while_it_was_running_stays_parked_and_holds_its_dependents() {
    let world = World::new("edit-park-inflight");
    // `slow` is held open so the cancel lands on a dispatch that is genuinely in
    // flight, and `keep` holds the run open past the park so the loop has every
    // chance to dispatch what it should not.
    let run = live(
        &world,
        "stopped-mid-flight",
        vec![
            agent("keep", &[]),
            agent("slow", &[]),
            agent("after", &["slow"]),
        ],
        &["slow", "keep"],
    );
    world.until("the held node to be in flight", |world| {
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "slow")
    });

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "cancel", "id": "slow"}])),
        )
        .exited(0);
    world.until("the park to commit", |world| {
        committed(world, &run).contains(&"cancel".to_string())
    });

    world.release("slow.go");
    world.until("the parked node to settle", |world| {
        world
            .events_of(&run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "slow")
    });
    world.release("keep.go");
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });

    let result = world.run_json(&run, "result.json");
    let slow = result["nodes"]
        .as_array()
        .expect("the result lists every node")
        .iter()
        .find(|node| node["id"] == "slow")
        .expect("the parked node is still in the graph, not dropped");
    assert_eq!(
        slow["status"], "parked",
        "the park did not survive the settlement that followed it: {slow}"
    );

    let redispatched: Vec<Value> = world
        .events_of(&run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "slow")
        .collect();
    assert_eq!(
        redispatched.len(),
        1,
        "a node parked mid-flight was dispatched again: {redispatched:?}"
    );
    // And its dependent stayed behind the gate rather than being freed by the
    // park, which would be the same second execution path by another route.
    let dependents: Vec<Value> = world
        .events_of(&run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "after")
        .collect();
    assert!(
        dependents.is_empty(),
        "a parked node's dependent was dispatched: {dependents:?}"
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
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });

    let result = world.run_json(&run, "result.json");
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

    // The superseded node left the graph with the same edit that replaced it,
    // exactly as a `drop` would take it — and nothing dispatched it again.
    assert!(
        !ids.contains(&"flaky"),
        "the superseded node is still in the graph: {ids:?}"
    );
    let dispatched: Vec<Value> = world
        .events_of(&run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "flaky")
        .collect();
    assert_eq!(
        dispatched.len(),
        1,
        "the superseded node was dispatched again: {dispatched:?}"
    );
}

#[test]
fn a_retry_may_name_only_one_branch() {
    let world = World::new("edit-branch");
    world.repository("local-direct", &[]);
    let run = live(
        &world,
        "branchy",
        vec![lifecycle("service", &[])],
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
/// A planner who writes a review bar into an edit is told where it goes.
///
/// Both halves of the retired field's story reach this crate through the
/// channel: an `add` carrying it never parses into a command at all, and a
/// `requeue` carrying it parses and is refused by the reconciler. Neither may
/// answer with the schema's bare `unknown field`, because a planner reading that
/// learns only that the field is gone.
#[test]
fn an_edit_carrying_the_retired_bar_is_refused_by_name_at_both_boundaries() {
    let world = World::new("edit-donewhen");
    let run = live(
        &world,
        "retired",
        vec![agent("slow", &[]), agent("sweep", &["slow"])],
        &["slow"],
    );

    let refusal = world.run_with_stdin(
        &["reply", &run],
        &envelope(json!([{"op": "add", "node": {
            "id": "extra", "persona": "engineer", "task": "## What\nextra",
            "done_when": "the gate is green"}}])),
    );
    refusal
        .exited(REFUSED)
        .err_has("`done_when` is no longer a plan field")
        .err_has("`## Acceptance criteria` section of its own task")
        .err_has("under `user.done_when`");

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "cancel", "id": "sweep"}])),
        )
        .exited(0);
    world.until("the park to commit", |world| {
        committed(world, &run).contains(&"cancel".to_string())
    });
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(
                json!([{"op": "requeue", "id": "sweep", "amend": {"done_when": "the gate is green"}}]),
            ),
        )
        .exited(REFUSED)
        .err_has("`done_when` is no longer a plan field");

    world.release("slow.go");
}

/// A planner who asks a node to verify via CI is told what already watches the
/// checks.
///
/// The second retired field, on the boundary a planner actually reaches it
/// through: a publication that failed its checks routes the failure back as
/// work, and "verify via CI this time" is the amendment that invites. An `add`
/// carrying it never parses into a command at all, so the envelope reader is
/// what has to name the field.
#[test]
fn an_added_node_asking_to_verify_via_ci_is_refused_by_name_at_the_envelope() {
    let world = World::new("edit-verifyci-add");
    // Registered, so the retired field is the only thing wrong with the node
    // below rather than a repository nobody declared.
    world.repository("local-direct", &[]);
    let run = live(&world, "ciadd", vec![agent("slow", &[])], &["slow"]);

    let mut node = lifecycle("extra", &[]);
    node["verify_via_ci"] = json!(true);
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "add", "node": node}])),
        )
        .exited(REFUSED)
        .err_has("'extra': `verify_via_ci` is no longer a plan field")
        .err_has("`merge_policy` is `change-auto`")
        .err_has("`checks-failed`")
        .err_lacks("unknown field");

    world.release("slow.go");
}

/// The same field on the other way in, and the likelier one: the node whose
/// publication failed is already in the graph, so a planner recovering it amends
/// that node rather than writing a new one.
///
/// A `requeue` parses — the amendment is a free-form mapping merged onto the
/// node — so nothing upstream refuses it and the reconciler is what has to.
#[test]
fn a_requeue_amending_a_node_to_verify_via_ci_is_refused_by_name_at_the_reconciler() {
    let world = World::new("edit-verifyci-requeue");
    world.repository("local-direct", &[]);
    let run = live(
        &world,
        "cirequeue",
        vec![agent("slow", &[]), lifecycle("publish", &["slow"])],
        &["slow"],
    );

    // Parked first, because a requeue is what returns a parked node to the
    // frontier: against a node nothing has cancelled it is refused with `is not
    // parked`, and the amendment would never be looked at.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "cancel", "id": "publish"}])),
        )
        .exited(0);
    world.until("the park to commit", |world| {
        committed(world, &run).contains(&"cancel".to_string())
    });

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(
                json!([{"op": "requeue", "id": "publish", "amend": {"verify_via_ci": true}}]),
            ),
        )
        .exited(REFUSED)
        .err_has("requeue: node 'publish': `verify_via_ci` is no longer a plan field")
        .err_has("`merge_policy` is `change-auto`")
        .err_has("`checks-unsettled`")
        .err_lacks("unknown field");

    world.release("slow.go");
}

/// A title `onevcs` will not commit under is refused wherever it enters.
///
/// The project is one way in and the channel is the other — and the channel
/// has three ways to write a publication subject: `add` states one outright, a
/// `retry` replacement carries one for the node superseding the failure, and a
/// `requeue` amendment writes one onto a node already in the graph. All three
/// reach the same check, which matters most for the latter two: they are what a
/// planner writes *after* a publication failed, so refusing them here is what
/// stops the same title being recomputed and refused identically.
#[test]
fn an_edit_writing_an_unpublishable_title_is_refused_with_the_limit_it_broke() {
    let world = World::new("edit-longtitle");
    let run = live(
        &world,
        "edittitle",
        vec![agent("slow", &[]), agent("sweep", &["slow"])],
        &["slow"],
    );

    let over = "t".repeat(SUBJECT_LIMIT + 1);
    let refuses = |command: Value, node: &str| {
        world
            .run_with_stdin(&["reply", &run], &envelope(json!([command])))
            .exited(REFUSED)
            .err_has(&format!("node '{node}'"))
            .err_has(&format!("{} characters", SUBJECT_LIMIT + 1))
            .err_has(&format!("{SUBJECT_LIMIT}-character limit"));
    };

    refuses(
        json!({"op": "add", "node": {
            "id": "publish", "persona": "engineer", "task": "## What\npublish",
            "repo": "service", "title": &over}}),
        "publish",
    );

    refuses(
        json!({"op": "retry", "id": "slow", "node": {
            "id": "slow-2", "persona": "engineer", "task": "## What\nagain",
            "repo": "service", "title": &over}}),
        "slow-2",
    );

    // A node has to be off the frontier before it can be requeued, so the park
    // is the setup rather than the subject — it is the one edit here that lands.
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "cancel", "id": "sweep"}])),
        )
        .exited(0);
    world.until("the park to commit", |world| {
        committed(world, &run).contains(&"cancel".to_string())
    });
    refuses(
        json!({"op": "requeue", "id": "sweep",
            "amend": {"repo": "service", "title": &over}}),
        "sweep",
    );

    assert_eq!(
        committed(&world, &run),
        vec!["cancel".to_string()],
        "a refused edit reached the graph"
    );

    world.release("slow.go");
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
    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    let result = world.run_json(&run, "result.json");
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

#[test]
fn drop_refuses_to_remove_the_last_unresolved_publication_anchor() {
    let world = World::new("edit-anchor");
    world.repository("local-direct", &[]);
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

/// An edit needs no live round, because there is no round: what decides where it
/// is applied is whether anything holds the run's ownership lock.
#[test]
fn an_edit_to_a_run_no_loop_is_driving_is_applied_under_the_lock() {
    let world = World::new("edit-undriven");
    let path = world.plan(
        "undrivengraph",
        &plan_of("undrivengraph", vec![human("approve", &[])]),
    );
    world.run(&["start", &path, "--attach"]).exited(0);

    world
        .run_with_stdin(
            &["reply", "undrivengraph"],
            &envelope(json!([{"op": "add", "node": {"id": "late", "persona": "e", "task": "t"}}])),
        )
        .exited(0)
        .out_has("\"applied\"");
    assert!(
        committed(&world, "undrivengraph").contains(&"add".to_string()),
        "the edit was not committed: {:?}",
        world.kinds("undrivengraph")
    );

    world
        .run_with_stdin(
            &["reply", "undrivengraph"],
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
        world.run_file(&run, "result.json").is_file()
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
