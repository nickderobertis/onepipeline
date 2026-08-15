//! What a plan file may say, what it may not, and the order the engine starts
//! what it says. A plan is external input, so every refusal here happens before
//! any provider time is spent.
//!
//! Ported from `test_plan_e2e`, `test_single_node_plan_e2e`, and `test_scheduling_e2e`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The scenario this journey states is one a real sibling would need paid
// model turns to produce, and `dispatch.rs` is where the real `oneagentgraph` binary is
// driven instead. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{agent, human, plan_of, World, QUEUED, REFUSED};
use serde_json::json;

/// Run a plan to settlement, attached, and return the run id.
fn settle(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the run to settle", |world| {
        !world.events_of(name, "round-finished").is_empty()
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

    let result = world.run_json(&run, "round-01/result.json");
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
    // The driver is held, so this round is run once and by this test. Left to
    // race, the driver's own `round run` takes the ownership lock first and the
    // refusal being asserted has already happened somewhere nothing can read.
    world.script("driver.wait", "hold");
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    let mut command = world.cmd(&["round", "run", "pinned"]);
    command.env("ONEPIPELINE_EXECUTOR_RULES", &rules);
    let refused = command.output().expect("the binary runs");
    world.release("driver.go");
    let said = String::from_utf8_lossy(&refused.stderr).to_string();
    assert!(
        !refused.status.success(),
        "the round dispatched anyway: {said}"
    );
    assert!(said.contains("a-cluster-nobody-declared"), "{said}");
    assert!(said.contains("do not declare"), "{said}");
}

#[test]
fn a_single_node_plan_runs_to_completion_and_records_its_evidence() {
    let world = World::new("plan-single");
    let run = settle(&world, "single", vec![agent("build", &[])]);

    world.until("the graph to complete", |world| {
        world.run_file(&run, "round-01/result.json").exists()
    });
    let result = world.run_json(&run, "round-01/result.json");
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
        "round-started",
        "node-dispatched",
        "node-settled",
        "round-finished",
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

    let result = world.run_json(&run, "round-01/result.json");
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

    let result = world.run_json(&run, "round-01/result.json");
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

    let result = world.run_json(&run, "round-01/result.json");
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
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

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
        !world.events_of("bounded", "round-finished").is_empty()
    });
    assert_eq!(world.events_of("bounded", "node-dispatched").len(), 3);
}

#[test]
fn a_plan_the_schema_refuses_never_starts_a_run() {
    let world = World::new("plan-refuse");
    let cases: &[(&str, &str, &str)] = &[
        (
            "cycle",
            r#"{"schema_version":1,"tasks":[
                {"id":"a","persona":"e","task":"t","deps":["b"]},
                {"id":"b","persona":"e","task":"t","deps":["a"]}]}"#,
            "cycle",
        ),
        (
            "dangling",
            r#"{"schema_version":1,"tasks":[{"id":"a","persona":"e","task":"t","deps":["nowhere"]}]}"#,
            "not in the plan",
        ),
        (
            "duplicate",
            r#"{"schema_version":1,"tasks":[{"id":"a","persona":"e","task":"t"},{"id":"a","persona":"e","task":"t"}]}"#,
            "duplicate node id",
        ),
        (
            "typo",
            r#"{"schema_version":1,"concurency":2,"tasks":[{"id":"a","persona":"e","task":"t"}]}"#,
            "concurency",
        ),
        (
            "notmapping",
            "[1, 2, 3]",
            "must be a JSON mapping, got list",
        ),
        (
            "humanpersona",
            r#"{"schema_version":1,"tasks":[{"id":"a","kind":"human","task":"t","persona":"e"}]}"#,
            "no persona or turn budget",
        ),
        (
            "nodiffpersona",
            r#"{"schema_version":1,"tasks":[{"id":"a","task":"t","expects_no_diff":true,"persona":"e"}]}"#,
            "takes no persona or turn budget",
        ),
        // A control declared where no dispatch will ever read it. The bar this
        // whole schema change is about: a node control this crate accepts and
        // cannot apply refuses the launch instead of defaulting in silence.
        (
            "humanbudget",
            r#"{"schema_version":1,"tasks":[{"id":"a","kind":"human","task":"t","max_turns":45}]}"#,
            "no persona or turn budget",
        ),
        (
            "stepsbudget",
            r#"{"schema_version":1,"tasks":[{"id":"a","repo":"o/r","max_turns":45,"steps":[
                {"id":"s","persona":"e","task":"t"}]}]}"#,
            "takes its persona, task, and turn budget from them",
        ),
        (
            "nodiffbudget",
            r#"{"schema_version":1,"tasks":[{"id":"a","task":"t","expects_no_diff":true,"max_turns":45}]}"#,
            "takes no persona or turn budget",
        ),
        (
            "humanstepbudget",
            r#"{"schema_version":1,"tasks":[{"id":"a","repo":"o/r","steps":[
                {"id":"s","kind":"human","task":"t","max_turns":45}]}]}"#,
            "no persona, turn budget, or expects_no_diff",
        ),
        (
            "nodiffstepbudget",
            r#"{"schema_version":1,"tasks":[{"id":"a","repo":"o/r","steps":[
                {"id":"s","task":"t","expects_no_diff":true,"max_turns":45}]}]}"#,
            "expects_no_diff settles without a dispatch",
        ),
        (
            "zerobudget",
            r#"{"schema_version":1,"tasks":[{"id":"a","persona":"e","task":"t","max_turns":0}]}"#,
            "no turn at all",
        ),
        // The retired field, named rather than answered with `unknown field`.
        (
            "donewhen",
            r#"{"schema_version":1,"tasks":[{"id":"a","persona":"e","task":"t",
                "done_when":"the gate is green"}]}"#,
            "`done_when` is no longer a plan field",
        ),
        (
            "version",
            r#"{"schema_version":7,"tasks":[{"id":"a","persona":"e","task":"t"}]}"#,
            "schema_version",
        ),
    ];

    for (name, body, expected) in cases {
        let path = world.raw_plan(&format!("{name}.plan.json"), body);
        world
            .run(&["start", &path.to_string_lossy()])
            .exited(REFUSED)
            .err_has(expected);
        assert!(
            !world.runs.join(name).exists(),
            "a refused plan left a run directory behind"
        );
    }

    // A plan file is read with its own format's escape semantics, so the two
    // formats reach the schema by different paths. The retired field is named on
    // both, because a planner who writes YAML wrote the same review bar.
    let yaml = world.raw_plan(
        "donewhen.plan.yaml",
        "schema_version: 1\ntasks:\n  - id: a\n    persona: e\n    task: t\n    \
         done_when: the gate is green\n",
    );
    world
        .run(&["start", &yaml.to_string_lossy()])
        .exited(REFUSED)
        .err_has("`done_when` is no longer a plan field")
        .err_has("`## Acceptance criteria` section of its own task");
}

#[test]
fn a_json_plan_keeps_json_escape_semantics_all_the_way_to_the_dispatch() {
    let world = World::new("plan-emoji");
    // What a JSON writer emits for one emoji is a surrogate pair. Read as YAML
    // it is two unpaired halves and the node fails on its own task prose.
    let path = world.raw_plan(
        "emoji.plan.json",
        r#"{"schema_version":1,"name":"emoji","tasks":[
            {"id":"build","persona":"engineer","task":"😀 ship it"}]}"#,
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the run to settle", |world| {
        !world.events_of("emoji", "round-finished").is_empty()
    });

    let relayed = world
        .journal("emoji")
        .into_iter()
        .find(|event| event["source"] == "agentgraph" && event["kind"] == "turn-activity")
        .expect("the dispatch relayed what it was doing");
    let task = relayed["payload"]["task"].as_str().expect("the task prose");
    assert!(
        task.starts_with('\u{1f600}'),
        "the surrogate pair did not survive as one character: {task:?}"
    );
}

#[test]
fn a_round_that_settles_unfinished_exits_one_rather_than_zero() {
    let world = World::new("plan-exit");
    world.script("build.fail", "1");
    let path = world.plan("exit", &plan_of("exit", vec![agent("build", &[])]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the round to finish", |world| {
        !world.events_of("exit", "round-finished").is_empty()
    });
    // Driving the same round again is the engine verb a caller reads the exit
    // status off: a failed graph is unfinished, not an error.
    world.run(&["round", "run", "exit"]).exited(QUEUED);
}

#[test]
fn a_worker_that_goes_quiet_is_surfaced_without_blocking_the_round() {
    let world = World::new("plan-quiet");
    world.script("slow.wait", "hold");
    let path = world.plan(
        "quiet",
        &plan_of("quiet", vec![agent("slow", &[]), agent("busy", &[])]),
    );
    let mut command = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
    command.env("ONEPIPELINE_STALL_AFTER_SECONDS", "1");
    command.output().expect("the binary runs");

    // A stall is evidence rather than a verdict, so the surface is
    // non-blocking: the round's other workers are not stopped to ask.
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
    // non-blocking surface did not stop the round from making progress.
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
        !world.events_of("quiet", "round-finished").is_empty()
    });
}

#[test]
fn a_round_that_outlives_its_budget_cancels_its_workers_and_asks_the_planner() {
    let world = World::new("plan-budget");
    world.script("slow.wait", "hold");
    let path = world.plan("budgeted", &plan_of("budgeted", vec![agent("slow", &[])]));
    world
        .run(&[
            "start",
            &path.to_string_lossy(),
            "--detach",
            "--round-budget",
            "1",
        ])
        .exited(0);

    // Blocking, so a wedged dispatch layer cannot leave the planner silent.
    let surfaced = world.surfaced("budgeted", "round-budget");
    assert_eq!(surfaced["payload"]["blocking"], true);

    // Written before the surface was raised, so it is there once the surface is.
    let exceeded = world.events_of("budgeted", "round-budget-exceeded");
    assert_eq!(exceeded[0]["payload"]["budget_seconds"], 1);

    world.release("slow.go");
    world.until("the round to finish", |world| {
        !world.events_of("budgeted", "round-finished").is_empty()
    });
    // The in-flight work was cancelled cooperatively rather than killed.
    let result = world.run_json("budgeted", "round-01/result.json");
    assert_eq!(result["nodes"][0]["status"], "cancelled", "{result}");
}
