//! What a plan may say, what it may not, and the order the engine starts what it
//! says. A plan is one project of a real `onetaskgraph` store, read through that
//! product's own binary, and it is external input — so every refusal here
//! happens at the point the project is read, before any provider time is spent
//! and before a run root exists.
//!
//! Ported from `test_plan_e2e`, `test_single_node_plan_e2e`, and `test_scheduling_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The scenario this journey states is one a real sibling would need paid
// model turns to produce, and `dispatch.rs` is where the real `oneagentgraph` binary is
// driven instead. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{
    agent, human, plan_of, World, NOTHING_DRIVING, REFUSED, RENDEZVOUS_SECONDS_ENV, STALL_AFTER_ENV,
};
use serde_json::{json, Value};

/// Run a plan to settlement, attached, and return the run id.
fn settle(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

#[test]
fn a_cross_dag_dependency_is_accepted_and_gates_only_the_node_that_names_it() {
    let world = World::new("plan-crossdag");
    let mut consumer = agent("consume", &[]);
    // A reference into another run's DAG. It names no node of *this* graph, so
    // the dangling-dependency refusal must not fire on it — and it is not
    // satisfied either, so the node that names it cannot start.
    consumer["deps"] = json!(["run:upstream#build"]);
    let run = settle(
        &world,
        "crossdag",
        vec![agent("independent", &[]), consumer],
    );

    let result = world.run_json(&run, "result.json");
    let node = |id: &str| {
        result["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["id"] == id)
            .unwrap_or_else(|| panic!("{id} is missing from {result}"))
            .clone()
    };
    // The plan was accepted: a run exists and the unrelated node ran in it.
    assert_eq!(node("independent")["status"], "done", "{result}");
    // An upstream this run cannot see leaves its consumer blocked rather than
    // failing it — the upstream may still arrive — and nothing dispatched it.
    assert_eq!(node("consume")["status"], "blocked", "{result}");
    assert!(
        world
            .events_of(&run, "node-dispatched")
            .iter()
            .all(|event| event["labels"]["node"] != "consume"),
        "a node gated by an unresolved upstream was dispatched anyway"
    );
}

fn graphs_dispatched(world: &World) -> Vec<(String, String)> {
    world
        .invocations()
        .iter()
        .filter(|invocation| {
            invocation["tool"] == "oneagentgraph" && invocation["args"][0] == "run"
        })
        .filter_map(|invocation| {
            let graph = invocation["args"][1].as_str()?.to_string();
            let node = invocation["args"]
                .as_array()?
                .iter()
                .filter_map(|arg| arg.as_str())
                .find_map(|arg| arg.strip_prefix("onepipeline.node="))?
                .to_string();
            Some((node, graph))
        })
        .collect()
}

#[test]
fn a_node_dispatches_under_the_agent_graph_it_names() {
    let world = World::new("plan-nodegraph");
    // A config of its own, so the assertion cannot pass on the default.
    let special = world.root.join("special-node-scope.yaml");
    std::fs::copy(
        crate::harness::repo_file("graphs/node-scope.yaml"),
        &special,
    )
    .expect("the override config is written");

    let mut overridden = agent("special", &[]);
    overridden["agent_graph"] = json!(special.to_string_lossy());
    settle(&world, "graphs", vec![agent("ordinary", &[]), overridden]);

    let dispatched = graphs_dispatched(&world);
    let graph_of = |node: &str| {
        dispatched
            .iter()
            .find(|(id, _)| id == node)
            .unwrap_or_else(|| panic!("{node} never dispatched: {dispatched:?}"))
            .1
            .clone()
    };
    assert_eq!(graph_of("special"), special.to_string_lossy());
    // The node that named none still gets the shipped default.
    assert!(
        graph_of("ordinary").ends_with("node-scope.yaml")
            && graph_of("ordinary") != special.to_string_lossy(),
        "the override leaked onto a node that never asked for it: {dispatched:?}"
    );
}

#[test]
fn a_node_pinned_to_an_executor_the_rules_do_not_declare_is_refused_by_name() {
    let world = World::new("plan-pin");
    let rules = world.root.join("only-local.yaml");
    std::fs::write(
        &rules,
        "executors: [{name: local, type: local}]\nrules: [{use: local}]\n",
    )
    .expect("the rules are written");

    let mut pinned = agent("build", &[]);
    // Naming where the work runs is the planner deciding it. A pin nothing
    // declares is a scheduling decision that can never be honoured, so it fails
    // before any provider time is spent rather than silently falling back.
    pinned["executor"] = json!("a-cluster-nobody-declared");
    let path = world.plan("pinned", &plan_of("pinned", vec![pinned]));
    // The launch drives the run itself, so the refusal is the launch's own: it
    // reaches the operator on the stderr they are already reading.
    let mut command = world.cmd(&["start", &path, "--attach"]);
    command.env("ONEPIPELINE_EXECUTOR_RULES", &rules);
    let refused = command.output().expect("the binary runs");
    let said = String::from_utf8_lossy(&refused.stderr).to_string();
    assert!(
        !refused.status.success(),
        "the run dispatched anyway: {said}"
    );
    assert!(said.contains("a-cluster-nobody-declared"), "{said}");
    assert!(said.contains("do not declare"), "{said}");
}

#[test]
fn a_single_node_plan_runs_to_completion_and_records_its_evidence() {
    let world = World::new("plan-single");
    let run = settle(&world, "single", vec![agent("build", &[])]);

    world.until("the graph to complete", |world| {
        world.run_file(&run, "result.json").exists()
    });
    let result = world.run_json(&run, "result.json");
    assert_eq!(result["state"], "complete", "{result}");
    assert_eq!(result["ok"], true);
    assert_eq!(result["nodes"][0]["id"], "build");
    assert_eq!(result["nodes"][0]["status"], "done");

    assert!(world.was_invoked(
        "oneagentgraph",
        &["run", "--label", "onepipeline.node=build"]
    ));

    let kinds = world.kinds(&run);
    for expected in [
        "run-started",
        "node-ready",
        "node-dispatched",
        "node-settled",
    ] {
        assert!(
            kinds.contains(&expected.to_string()),
            "{kinds:?} lacks {expected}"
        );
    }
}

#[test]
fn a_dependent_node_starts_only_after_its_dependency_is_done() {
    let world = World::new("plan-order");
    let run = settle(
        &world,
        "ordered",
        vec![agent("first", &[]), agent("second", &["first"])],
    );

    let order: Vec<String> = world
        .journal(&run)
        .iter()
        .filter(|event| event["kind"] == "node-dispatched" || event["kind"] == "node-settled")
        .filter_map(|event| {
            Some(format!(
                "{}:{}",
                event["labels"]["node"].as_str()?,
                event["kind"].as_str()?
            ))
        })
        .collect();
    let dispatched_second = order
        .iter()
        .position(|entry| entry == "second:node-dispatched")
        .expect("second was dispatched");
    let settled_first = order
        .iter()
        .position(|entry| entry == "first:node-settled")
        .expect("first settled");
    assert!(
        settled_first < dispatched_second,
        "second started before first settled: {order:?}"
    );
}

#[test]
fn a_failed_dependency_skips_its_descendant_rather_than_blocking_it() {
    let world = World::new("plan-skip");
    world.script("build.fail", "1");
    let run = settle(
        &world,
        "skipped",
        vec![agent("build", &[]), agent("ship", &["build"])],
    );

    let result = world.run_json(&run, "result.json");
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["ok"], false);
    let status = |id: &str| {
        result["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["id"] == id)
            .expect(id)["status"]
            .as_str()
            .expect("a status")
            .to_string()
    };
    assert_eq!(status("build"), "failed");
    assert_eq!(status("ship"), "skipped");
}

#[test]
fn a_ready_human_action_waits_and_blocks_what_it_unblocks() {
    let world = World::new("plan-human");
    let run = settle(
        &world,
        "gated",
        vec![
            agent("build", &[]),
            human("approve", &["build"]),
            agent("ship", &["approve"]),
        ],
    );

    let result = world.run_json(&run, "result.json");
    assert_eq!(result["state"], "waiting", "{result}");
    let node = |id: &str| {
        result["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["id"] == id)
            .expect(id)
            .clone()
    };
    assert_eq!(node("approve")["status"], "waiting");
    assert_eq!(node("approve")["unblocks"], json!(["ship"]));
    assert_eq!(node("ship")["status"], "blocked");
    assert_eq!(node("ship")["blocked_by"], json!(["approve"]));
    assert!(!world.was_invoked(
        "oneagentgraph",
        &["run", "--label", "onepipeline.node=approve"]
    ));
}

#[test]
fn an_expects_no_diff_node_settles_without_spending_a_dispatch() {
    let world = World::new("plan-nodiff");
    let run = settle(
        &world,
        "nodiff",
        vec![json!({
            "id": "handoff",
            "task": "## What\nRecord that nothing changes.",
            "expects_no_diff": true,
        })],
    );

    let result = world.run_json(&run, "result.json");
    assert_eq!(result["nodes"][0]["status"], "done");
    assert_eq!(result["nodes"][0]["outcome"], "no-changes");
    assert!(
        !world.was_invoked(
            "oneagentgraph",
            &["run", "--label", "onepipeline.node=handoff"]
        ),
        "an expects_no_diff node spent a dispatch"
    );
}

/// A node readied by a settlement **the pass itself made** is still dispatched.
///
/// An `expects_no_diff` node never spends a dispatch, so nothing will ever report
/// its settlement back and the pass that starts what it readied is the very next
/// one. Attached deliberately: the frontier and the launch's return are one fact
/// here, and a detached form would prove the first and leave the second to a
/// reader.
#[test]
fn a_node_readied_by_a_settlement_no_dispatch_reported_is_still_dispatched() {
    let world = World::new("plan-nodiff-dependent");
    let path = world.plan(
        "handoff",
        &plan_of(
            "handoff",
            vec![
                json!({
                    "id": "record",
                    "task": "## What\nRecord that nothing changes.",
                    "expects_no_diff": true,
                }),
                agent("build", &["record"]),
            ],
        ),
    );

    world
        .run(&["start", &path, "--attach"])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");

    let result = world.run_json("handoff", "result.json");
    let node = |id: &str| {
        result["nodes"]
            .as_array()
            .expect("the result lists its nodes")
            .iter()
            .find(|node| node["id"] == id)
            .unwrap_or_else(|| panic!("the result has no node '{id}': {result}"))
            .clone()
    };
    assert_eq!(node("record")["outcome"], "no-changes");
    assert_eq!(node("build")["status"], "done");
    assert!(
        world.was_invoked(
            "oneagentgraph",
            &["run", "--label", "onepipeline.node=build"]
        ),
        "the node whose only dependency settled without a dispatch was never dispatched"
    );
}

#[test]
fn concurrency_bounds_how_many_nodes_are_in_flight_at_once() {
    let world = World::new("plan-concurrency");
    for node in ["a", "b", "c"] {
        world.script(&format!("{node}.wait"), "hold");
    }
    let mut plan = plan_of(
        "bounded",
        vec![agent("a", &[]), agent("b", &[]), agent("c", &[])],
    );
    plan["concurrency"] = json!(2);
    let path = world.plan("bounded", &plan);
    world.run(&["start", &path, "--detach"]).exited(0);

    world.until("two dispatches to be in flight", |world| {
        world.events_of("bounded", "node-dispatched").len() == 2
    });
    // Give a third a chance to appear if the bound were not honoured.
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        world.events_of("bounded", "node-dispatched").len(),
        2,
        "concurrency 2 dispatched a third node"
    );

    for node in ["a", "b", "c"] {
        world.release(&format!("{node}.go"));
    }
    world.until("the run to settle", |world| {
        world.run_file("bounded", "result.json").is_file()
    });
    assert_eq!(world.events_of("bounded", "node-dispatched").len(), 3);
}

#[test]
fn a_project_the_schema_refuses_never_starts_a_run() {
    let world = World::new("plan-refuse");
    // The far end of a dependency edge that leaves the project: a real task of
    // this store, carrying a node id of its own, which this plan has no node
    // for. It is what makes the reference rule reachable the way a store
    // reaches it.
    world.stray_task("dangling", "elsewhere", "nowhere");

    let cases: &[(&str, Value, &str)] = &[
        (
            "cycle",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "persona": "e", "task": "t", "deps": ["b"]},
                {"id": "b", "persona": "e", "task": "t", "deps": ["a"]}]}),
            "cycle",
        ),
        (
            "dangling",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "persona": "e", "task": "t", "deps": ["elsewhere"]}]}),
            "not in the plan",
        ),
        // Two tasks of one project carrying one node id. A plan's dependencies
        // name a node by that id, so the store is where the collision is caught
        // and both ends of it are the author's to fix.
        (
            "duplicate",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "persona": "e", "task": "t"},
                {"id": "a", "persona": "e", "task": "t"}]}),
            "is already the id of another task",
        ),
        // A plan-level setting no field answers to, refused by the name the
        // project wrote it under rather than dropped.
        (
            "typo",
            json!({"schema_version": 2, "concurency": 2,
                   "tasks": [{"id": "a", "persona": "e", "task": "t"}]}),
            "concurency",
        ),
        // A reserved key of the wrong JSON type: the schema's own types are what
        // a project is held to, at the point it is read.
        (
            "mistyped",
            json!({"schema_version": "three",
                   "tasks": [{"id": "a", "persona": "e", "task": "t"}]}),
            "schema_version",
        ),
        // A task carrying no node id at all: a plan's dependencies name a node
        // by that key, so a task without one is not a node.
        (
            "unidentified",
            json!({"schema_version": 3, "tasks": [
                {"persona": "e", "task": "t"}]}),
            "carries no `onepipeline.id`",
        ),
        // The two keys the mapping fills from the task itself. A project stating
        // one is told which end to edit, rather than having its value lose in
        // silence to what the task already says.
        (
            "ownprose",
            json!({"schema_version": 3, "tasks": [
                {"id": "a", "persona": "e", "task": "t", "onepipeline-task": "other"}]}),
            "`onepipeline.task` is not a node field",
        ),
        (
            "owntitle",
            json!({"schema_version": 3, "tasks": [
                {"id": "a", "persona": "e", "task": "t", "onepipeline-title": "other"}]}),
            "`onepipeline.title` is not a node field",
        ),
        // A node id is the name every dependency of this plan calls the node by,
        // so one that is not a string is not a name at all.
        (
            "numberedid",
            json!({"schema_version": 3, "tasks": [
                {"id": "a", "persona": "e", "task": "t", "onepipeline-id": 7}]}),
            "a node id is a string",
        ),
        // A node lands in one repository, and the reserved key is only for an
        // identity a normalized origin cannot hold — so a task naming one both
        // ways is refused rather than one of the two quietly losing.
        (
            "tworepos",
            json!({"schema_version": 3, "tasks": [
                {"id": "a", "persona": "e", "task": "t", "title": "feat: x",
                 "repo": "github.com/owner/service",
                 "onepipeline-repo": "/var/checkouts/service"}]}),
            "names a repository in both `repositories` and `onepipeline.repo`",
        ),
        // A dependency on another node of this plan is an edge between two
        // tasks, so the reserved key carries cross-DAG references and nothing
        // else — otherwise the backend would draw a graph missing that arrow.
        (
            "recordeddep",
            json!({"schema_version": 3, "tasks": [
                {"id": "a", "persona": "e", "task": "t"},
                {"id": "b", "persona": "e", "task": "t", "onepipeline-deps": ["a"]}]}),
            "is a dependency edge between the two tasks",
        ),
        (
            "humanpersona",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "kind": "human", "task": "t", "persona": "e"}]}),
            "no persona or turn budget",
        ),
        (
            "nodiffpersona",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "task": "t", "expects_no_diff": true, "persona": "e"}]}),
            "takes no persona or turn budget",
        ),
        // A control declared where no dispatch will ever read it. The bar this
        // whole schema is about: a node control this crate accepts and cannot
        // apply refuses the launch instead of defaulting in silence.
        (
            "humanbudget",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "kind": "human", "task": "t", "max_turns": 45}]}),
            "no persona or turn budget",
        ),
        (
            "stepsbudget",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "repo": "o/r", "max_turns": 45,
                 "steps": [{"id": "s", "persona": "e", "task": "t"}]}]}),
            "takes its persona, task, and turn budget from them",
        ),
        (
            "nodiffbudget",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "task": "t", "expects_no_diff": true, "max_turns": 45}]}),
            "takes no persona or turn budget",
        ),
        (
            "humanstepbudget",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "repo": "o/r",
                 "steps": [{"id": "s", "kind": "human", "task": "t", "max_turns": 45}]}]}),
            "no persona, turn budget, or expects_no_diff",
        ),
        (
            "nodiffstepbudget",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "repo": "o/r",
                 "steps": [{"id": "s", "task": "t", "expects_no_diff": true,
                            "max_turns": 45}]}]}),
            "expects_no_diff settles without a dispatch",
        ),
        (
            "zerostepbudget",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "repo": "o/r",
                 "steps": [{"id": "s", "persona": "e", "task": "t", "max_turns": 0}]}]}),
            "no turn at all",
        ),
        (
            "zerobudget",
            json!({"schema_version": 2, "tasks": [
                {"id": "a", "persona": "e", "task": "t", "max_turns": 0}]}),
            "no turn at all",
        ),
        // A task carrying the retired field is answered with the *field's*
        // refusal, not the version's: the field is the thing its author has to
        // move, and a planner told only to change a number would carry the bar
        // straight into the new version.
        (
            "donewhen",
            json!({"schema_version": 1, "tasks": [
                {"id": "a", "persona": "e", "task": "t",
                 "done_when": "the gate is green"}]}),
            "`done_when` is no longer a plan field",
        ),
        // Written at the current version and still carrying it: the same answer,
        // because the schema is what refuses it either way.
        (
            "donewhencurrent",
            json!({"schema_version": 3, "tasks": [
                {"id": "a", "persona": "e", "task": "t",
                 "done_when": "the gate is green"}]}),
            "`done_when` is no longer a plan field",
        ),
        // The second retired field, answered the same way and about itself: a
        // plan that set it asked for the host's checks to be the merge-path
        // verification and got a run in which nothing read the field at all, so
        // the refusal names the policy that asks for one now.
        (
            "verifyviaci",
            json!({"schema_version": 3, "tasks": [
                {"id": "a", "repo": "o/r", "title": "feat: x", "persona": "e", "task": "t",
                 "verify_via_ci": true}]}),
            "`verify_via_ci` is no longer a plan field",
        ),
        // And at a version written before it was ever accepted, because a
        // retired field is not a version's business.
        (
            "verifyviaciold",
            json!({"schema_version": 1, "tasks": [
                {"id": "a", "repo": "o/r", "persona": "e", "task": "t",
                 "verify_via_ci": false}]}),
            "`verify_via_ci` is no longer a plan field",
        ),
        // The one version refusal there is: a number this build has never
        // written, told the versions that are read rather than left to guess.
        (
            "version",
            json!({"schema_version": 7, "tasks": [
                {"id": "a", "persona": "e", "task": "t"}]}),
            "schema_version 7 is not one this build reads (3, 2, 1)",
        ),
        // A lifecycle node at 3 states the title its change request opens under.
        // Every source carries a title, so a blank one is how a store carries
        // none — and this node's task states nothing there.
        (
            "untitled",
            json!({"schema_version": 3, "tasks": [
                {"id": "publish", "repo": "o/r", "persona": "e", "task": "t"}]}),
            "node 'publish': a lifecycle node states the title its change request opens under",
        ),
        // The persona this crate dispatches a change request's drafting under. A
        // node claiming it would be composed as that dispatch — the graph the
        // operator named, and none of the node's own overrides — so the name is
        // refused where a plan is read rather than silently dropping them.
        (
            "reservedpersona",
            json!({"schema_version": 3, "tasks": [
                {"id": "draft", "persona": "pr-author", "task": "t"}]}),
            "node 'draft': `pr-author` is the persona this crate dispatches",
        ),
        (
            "reservedsteppersona",
            json!({"schema_version": 3, "tasks": [
                {"id": "service", "repo": "o/r", "title": "feat: x",
                 "steps": [{"id": "draft", "persona": "pr-author", "task": "t"}]}]}),
            "step 'draft': `pr-author` is the persona this crate dispatches",
        ),
        // ...and a plan below it that names `body` is refused by that field's
        // name, exactly as a field no schema ever had is. Silently dropping it
        // would leave its author to find out from the published change request.
        (
            "earlybody",
            json!({"schema_version": 2, "tasks": [
                {"id": "publish", "repo": "o/r", "persona": "e", "task": "t",
                 "title": "feat: ship it", "body": "what it landed"}]}),
            "node 'publish': `body` is a schema 3 field",
        ),
        // The same answer at the oldest version this build reads, and it is the
        // *field's*: version 1 is a document this build executes, so a planner
        // who wrote a body there has one thing to act on and it is the field.
        (
            "earlybodyv1",
            json!({"schema_version": 1, "tasks": [
                {"id": "publish", "repo": "o/r", "persona": "e", "task": "t",
                 "body": "what it landed"}]}),
            "node 'publish': `body` is a schema 3 field",
        ),
    ];

    for (name, plan, expected) in cases {
        // The two keys a journey has to write *as a key of the store* rather
        // than as a plan field: the writer moves a plan field onto its reserved
        // key, and these two are about what happens when the key itself is
        // wrong. Spelled with a dash here and swapped to the dot below, because
        // the writer would otherwise prefix them a second time.
        let stated = serde_json::to_string(plan)
            .expect("a plan serialises")
            .replace("onepipeline-", "onepipeline.");
        let plan: Value = serde_json::from_str(&stated).expect("it re-reads");
        let project = world.plan(name, &plan);
        let refused = world.run(&["start", &project]);
        refused.exited(REFUSED).err_has(expected);
        if name.starts_with("donewhen") || name.starts_with("earlybody") {
            // Each of these declares a version *earlier* than this build writes,
            // and each is answered about the field it names rather than about
            // that number: an earlier version is a document this build reads, so
            // the field is the only thing its author has to act on.
            refused.err_lacks("is not one this build reads");
        }
        assert!(
            !world.runs.join(name).exists(),
            "a refused project left a run directory behind"
        );
    }
}

/// A dependency edge whose far end is not a node is refused where the plan is
/// read, naming both ends.
///
/// The store is a graph of its own, and an edge may leave the project, cross to
/// the project level, or leave the source entirely. None of those is a plan
/// node, so each is refused with both ends named — a planner reading it has to
/// know which task drew the arrow as well as where it pointed.
#[test]
fn a_dependency_edge_whose_far_end_is_no_node_is_refused_naming_both_ends() {
    let world = World::new("plan-faredge");
    // A far task with no node id of its own. The near task is a node; what the
    // edge points at is not.
    world.write_store_item(
        "tasks/unidentified/far.md",
        "---\ntitle: \"a task nothing identifies\"\nproject: \"somewhere-else\"\n---\n\n",
    );
    let project = world.plan(
        "faredge",
        &json!({"schema_version": 3, "name": "faredge", "tasks": [
            {"id": "near", "persona": "e", "task": "t", "deps": ["store:unidentified/far"]}]}),
    );
    world
        .run(&["start", &project])
        .exited(REFUSED)
        .err_has("near")
        .err_has("depends on")
        .err_has("unidentified/far")
        .err_has("onepipeline.id");
    assert!(
        !world.runs.join("faredge").exists(),
        "a refused project left a run directory behind"
    );
}

/// Task prose reaches the dispatch as the characters the store holds.
///
/// A plan is no longer a file, so nothing about the mapping turns on a file
/// format's escapes: what the store says the task's `content` is, is what the
/// worker is handed. The emoji is the case that used to break — a JSON
/// surrogate pair read as YAML is two unpaired halves, which no UTF-8 encoder
/// accepts — so it is what this holds the whole path with.
#[test]
fn task_prose_reaches_the_dispatch_as_the_characters_the_store_holds() {
    let world = World::new("plan-emoji");
    let project = world.plan(
        "emoji",
        &json!({"schema_version": 2, "name": "emoji", "tasks": [
            {"id": "build", "persona": "engineer", "task": "😀 ship it"}]}),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file("emoji", "result.json").is_file()
    });

    let relayed = world
        .journal("emoji")
        .into_iter()
        .find(|event| event["source"] == "agentgraph" && event["kind"] == "turn-activity")
        .expect("the dispatch relayed what it was doing");
    let task = relayed["payload"]["task"].as_str().expect("the task prose");
    assert!(
        task.starts_with('\u{1f600}'),
        "the emoji did not survive as one character: {task:?}"
    );
}

/// A graph that settled unfinished is reported as unfinished, not as a failure
/// of the command that drove it.
///
/// Read where an operator reads it: an attached launch, which stays for the run
/// and answers with the settlement. `3` is the code, because nothing is driving
/// a run whose graph has stopped moving — and the record says *failed*, which is
/// the distinction a bare exit code cannot draw.
#[test]
fn a_graph_that_settles_unfinished_is_reported_rather_than_erroring() {
    let world = World::new("plan-exit");
    world.script("build.fail", "1");
    let path = world.plan("exit", &plan_of("exit", vec![agent("build", &[])]));
    world
        .run(&["start", &path, "--attach"])
        .exited(NOTHING_DRIVING)
        .out_has("\"settlement\":\"unattended\"");

    let result = world.run_json("exit", "result.json");
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["ok"], json!(false), "{result}");
    world.run(&["results", "exit"]).exited(0).out_has("failed");
}

#[test]
fn a_worker_that_goes_quiet_is_surfaced_without_holding_anything_back() {
    let world = World::new("plan-quiet");
    world.script("slow.wait", "hold");
    let path = world.plan(
        "quiet",
        &plan_of("quiet", vec![agent("slow", &[]), agent("busy", &[])]),
    );
    let mut command = world.cmd(&["start", &path, "--detach"]);
    command.env(STALL_AFTER_ENV, "1");
    command.output().expect("the binary runs");

    // A stall is evidence rather than a verdict, so the surface is
    // non-blocking: nothing is held back to ask.
    world.until("the slow worker to be surfaced", |world| {
        world
            .events_of("quiet", "planner-surface-queued")
            .iter()
            .any(|event| {
                event["payload"]["kind"] == "quiet-worker" && event["labels"]["node"] == "slow"
            })
    });
    let surfaced = world
        .events_of("quiet", "planner-surface-queued")
        .into_iter()
        .find(|event| {
            event["payload"]["kind"] == "quiet-worker" && event["labels"]["node"] == "slow"
        })
        .expect("the slow worker's surface was just seen");
    assert_eq!(surfaced["payload"]["blocking"], false);

    // The other worker settles while `slow` is still held, proving that the
    // non-blocking surface held nothing back.
    world.until("the busy worker to settle", |world| {
        world
            .events_of("quiet", "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "busy")
    });

    // Written before the surface was raised, so it is there once the surface is.
    let reported = world
        .events_of("quiet", "quiet-worker")
        .into_iter()
        .find(|event| event["labels"]["node"] == "slow")
        .expect("the slow worker was reported quiet");
    assert_eq!(reported["payload"]["threshold_seconds"], 1);
    assert_eq!(reported["payload"]["persona"], "engineer");

    // A node is reported once per quiet stretch, not once per pass.
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(
        world
            .events_of("quiet", "quiet-worker")
            .iter()
            .filter(|event| event["labels"]["node"] == "slow")
            .count(),
        1
    );

    world.release("slow.go");
    world.until("the run to settle", |world| {
        world.run_file("quiet", "result.json").is_file()
    });
}

/// A worker that is alive and doing nothing is still reported quiet.
///
/// The watch was reset by the one event whose literal meaning is "nothing is
/// happening", so it could fire for a worker that had *died* and never for one
/// that was wedged. The heartbeat itself is not stopped — a member is declared
/// dead on it one layer down — and the claim here is only that it is not
/// mistaken for work.
#[test]
fn a_worker_that_only_heartbeats_is_reported_quiet_rather_than_active() {
    let world = World::new("plan-heartbeat");
    // Alive, addressable, and producing nothing: the turn is announced and then
    // the dispatch does nothing but say it is still there.
    world.script("stuck.turn-open", "");
    world.script("stuck.wait", "hold");
    world.script("stuck.heartbeat", "50");
    let path = world.plan("wedged", &plan_of("wedged", vec![agent("stuck", &[])]));
    let mut command = world.cmd(&["start", &path, "--detach"]);
    command.env(STALL_AFTER_ENV, "2");
    world.run_on(command, "start --detach").exited(0);

    // Heartbeating well past the threshold, which is the whole scenario: at
    // twenty beats over two seconds nothing that counted them could ever call
    // this dispatch quiet.
    world.until(
        "the wedged worker to heartbeat past the threshold",
        |world| world.events_of("wedged", "member-heartbeat").len() >= 20,
    );
    world.until("the wedged worker to be reported quiet", |world| {
        !world.events_of("wedged", "quiet-worker").is_empty()
    });

    let reported = world
        .events_of("wedged", "quiet-worker")
        .into_iter()
        .find(|event| event["labels"]["node"] == "stuck")
        .expect("the wedged worker was reported quiet");
    assert_eq!(reported["payload"]["threshold_seconds"], 2);
    assert!(
        reported["payload"]["quiet_for_seconds"]
            .as_u64()
            .unwrap_or(0)
            >= 2,
        "the quiet stretch is shorter than the threshold that fired: {reported}"
    );

    // Still beating afterwards: what was reported is a worker that is alive and
    // doing nothing, not one that stopped saying anything.
    let beats = world.events_of("wedged", "member-heartbeat").len();
    world.until("the wedged worker to go on heartbeating", |world| {
        world.events_of("wedged", "member-heartbeat").len() > beats
    });

    world.release("stuck.go");
    world.until("the run to settle", |world| {
        world.run_file("wedged", "result.json").is_file()
    });
}

/// A hold no clock can wait out fails the dispatch rather than dying inside it.
///
/// The bound arrives in the environment, and every command this suite runs sets
/// it — so a mistyped one is a live way for a journey to be misconfigured. Read
/// without a ceiling it becomes a duration added to a clock, and an addition past
/// what an `Instant` can represent panics: the double unwinds mid-dispatch and
/// what reaches the run is a sibling that died saying nothing about the value it
/// was handed. That is the one failure a double cannot report, because reporting
/// a misconfiguration is all it does — so the bound is checked where it is read,
/// and the node settles naming the variable.
#[test]
fn a_hold_longer_than_a_clock_can_wait_fails_the_dispatch() {
    let world = World::new("plan-unwaitable");
    // Scripted to hold: the bound is read only by a dispatch that waits, so one
    // that runs straight through would settle whatever the value was.
    world.script("stuck.wait", "hold");
    let path = world.plan(
        "unwaitable",
        &plan_of("unwaitable", vec![agent("stuck", &[])]),
    );
    let mut command = world.cmd(&["start", &path, "--detach"]);
    command.env(RENDEZVOUS_SECONDS_ENV, u64::MAX.to_string());
    world.run_on(command, "start --detach").exited(0);
    world.until("the run to settle", |world| {
        world.run_file("unwaitable", "result.json").is_file()
    });

    let result = world.run_json("unwaitable", "result.json");
    assert_eq!(result["state"], "failed", "{result}");
    let results = world.run(&["results", "unwaitable"]);
    results.exited(0);
    assert!(
        results.stdout.contains(RENDEZVOUS_SECONDS_ENV),
        "the failure does not name the variable that was out of range, so a \
         journey that mistyped it is left reading a dead sibling:\n{}",
        results.stdout
    );
    assert!(
        !results.stdout.contains("panicked"),
        "the dispatch died inside the hold instead of refusing it:\n{}",
        results.stdout
    );
}

/// A node the planner cancels is parked, not failed.
///
/// `cancel` is the one op that means stop this node, and what it leaves behind
/// is a gate rather than a failure: a parked node holds its dependents where a
/// failed one skips them, so the planner still decides what happens next. The
/// run around it settles either way.
#[test]
fn a_node_the_planner_cancels_settles_parked_and_the_run_still_settles() {
    let world = World::new("plan-cancel");
    world.script("slow.wait", "hold");
    let path = world.plan(
        "cancelled",
        &plan_of("cancelled", vec![agent("slow", &[]), agent("keep", &[])]),
    );
    world.script("keep.wait", "hold");
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the held node to be in flight", |world| {
        world
            .events_of("cancelled", "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "slow")
    });

    world
        .run_with_stdin(
            &["reply", "cancelled"],
            &json!({"version": 1, "commands": [{"op": "cancel", "id": "slow"}]}).to_string(),
        )
        .exited(0);
    world.release("slow.go");
    world.release("keep.go");
    world.until("the run to settle", |world| {
        world.run_file("cancelled", "result.json").is_file()
    });

    let result = world.run_json("cancelled", "result.json");
    let slow = result["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|node| node["id"] == "slow")
        .expect("the cancelled node is still named");
    assert_eq!(slow["status"], "parked", "{result}");
}
