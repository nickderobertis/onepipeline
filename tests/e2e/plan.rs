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

use crate::harness::{
    agent, human, plan_of, World, NOTHING_DRIVING, REFUSED, RENDEZVOUS_SECONDS_ENV,
    STALL_AFTER_ENV, USAGE_ERROR,
};
use serde_json::json;

/// Run a plan to settlement, attached, and return the run id.
fn settle(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
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
    let mut command = world.cmd(&["start", &path.to_string_lossy(), "--attach"]);
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
        world.run_file("bounded", "result.json").is_file()
    });
    assert_eq!(world.events_of("bounded", "node-dispatched").len(), 3);
}

#[test]
fn a_plan_the_schema_refuses_never_starts_a_run() {
    let world = World::new("plan-refuse");
    let cases: &[(&str, &str, &str)] = &[
        (
            "cycle",
            r#"{"schema_version":2,"tasks":[
                {"id":"a","persona":"e","task":"t","deps":["b"]},
                {"id":"b","persona":"e","task":"t","deps":["a"]}]}"#,
            "cycle",
        ),
        (
            "dangling",
            r#"{"schema_version":2,"tasks":[{"id":"a","persona":"e","task":"t","deps":["nowhere"]}]}"#,
            "not in the plan",
        ),
        (
            "duplicate",
            r#"{"schema_version":2,"tasks":[{"id":"a","persona":"e","task":"t"},{"id":"a","persona":"e","task":"t"}]}"#,
            "duplicate node id",
        ),
        (
            "typo",
            r#"{"schema_version":2,"concurency":2,"tasks":[{"id":"a","persona":"e","task":"t"}]}"#,
            "concurency",
        ),
        (
            "notmapping",
            "[1, 2, 3]",
            "must be a JSON mapping, got list",
        ),
        (
            "humanpersona",
            r#"{"schema_version":2,"tasks":[{"id":"a","kind":"human","task":"t","persona":"e"}]}"#,
            "no persona or turn budget",
        ),
        (
            "nodiffpersona",
            r#"{"schema_version":2,"tasks":[{"id":"a","task":"t","expects_no_diff":true,"persona":"e"}]}"#,
            "takes no persona or turn budget",
        ),
        // A control declared where no dispatch will ever read it. The bar this
        // whole schema change is about: a node control this crate accepts and
        // cannot apply refuses the launch instead of defaulting in silence.
        (
            "humanbudget",
            r#"{"schema_version":2,"tasks":[{"id":"a","kind":"human","task":"t","max_turns":45}]}"#,
            "no persona or turn budget",
        ),
        (
            "stepsbudget",
            r#"{"schema_version":2,"tasks":[{"id":"a","repo":"o/r","max_turns":45,"steps":[
                {"id":"s","persona":"e","task":"t"}]}]}"#,
            "takes its persona, task, and turn budget from them",
        ),
        (
            "nodiffbudget",
            r#"{"schema_version":2,"tasks":[{"id":"a","task":"t","expects_no_diff":true,"max_turns":45}]}"#,
            "takes no persona or turn budget",
        ),
        (
            "humanstepbudget",
            r#"{"schema_version":2,"tasks":[{"id":"a","repo":"o/r","steps":[
                {"id":"s","kind":"human","task":"t","max_turns":45}]}]}"#,
            "no persona, turn budget, or expects_no_diff",
        ),
        (
            "nodiffstepbudget",
            r#"{"schema_version":2,"tasks":[{"id":"a","repo":"o/r","steps":[
                {"id":"s","task":"t","expects_no_diff":true,"max_turns":45}]}]}"#,
            "expects_no_diff settles without a dispatch",
        ),
        (
            "zerostepbudget",
            r#"{"schema_version":2,"tasks":[{"id":"a","repo":"o/r","steps":[
                {"id":"s","persona":"e","task":"t","max_turns":0}]}]}"#,
            "no turn at all",
        ),
        (
            "zerobudget",
            r#"{"schema_version":2,"tasks":[{"id":"a","persona":"e","task":"t","max_turns":0}]}"#,
            "no turn at all",
        ),
        // A plan carrying the retired field is answered with the *field's*
        // refusal, not the version's: the field is the thing its author has to
        // move, and a planner told only to change a number would carry the bar
        // straight into the new version.
        (
            "donewhen",
            r#"{"schema_version":1,"tasks":[{"id":"a","persona":"e","task":"t",
                "done_when":"the gate is green"}]}"#,
            "`done_when` is no longer a plan field",
        ),
        // Written at the current version and still carrying it: the same answer,
        // because the schema is what refuses it either way.
        (
            "donewhencurrent",
            r#"{"schema_version":3,"tasks":[{"id":"a","persona":"e","task":"t",
                "done_when":"the gate is green"}]}"#,
            "`done_when` is no longer a plan field",
        ),
        // The one version refusal there is: a number this build has never
        // written, told the versions that are read rather than left to guess.
        (
            "version",
            r#"{"schema_version":7,"tasks":[{"id":"a","persona":"e","task":"t"}]}"#,
            "schema_version 7 is not one this build reads (3, 2, 1)",
        ),
        // A title that is only spacing publishes a commit with no subject at
        // all, which `onevcs` refuses at publication — after a whole dispatch.
        // The over-long title beside it is `lifecycle.rs`'s journey, where a
        // real publication is what proves the bound.
        (
            "blanktitle",
            r#"{"schema_version":2,"tasks":[{"id":"a","repo":"o/r","persona":"e","task":"t",
                "title":"   "}]}"#,
            "the title is blank",
        ),
        // The two rules this schema version added, each keyed to the version the
        // *document* declares: a lifecycle node at 3 states the title its change
        // request opens under...
        (
            "untitled",
            r#"{"schema_version":3,"tasks":[{"id":"publish","repo":"o/r","persona":"e",
                "task":"t"}]}"#,
            "node 'publish': a lifecycle node states the title its change request opens under",
        ),
        // The persona this crate dispatches a change request's drafting under.
        // A node claiming it would be composed as that dispatch — the graph the
        // operator named, and none of the node's own overrides — so the name is
        // refused where a plan is read rather than silently dropping them.
        (
            "reservedpersona",
            r#"{"schema_version":3,"tasks":[{"id":"draft","persona":"pr-author",
                "task":"t"}]}"#,
            "node 'draft': `pr-author` is the persona this crate dispatches",
        ),
        (
            "reservedsteppersona",
            r#"{"schema_version":3,"tasks":[{"id":"service","repo":"o/r","title":"feat: x",
                "steps":[{"id":"draft","persona":"pr-author","task":"t"}]}]}"#,
            "step 'draft': `pr-author` is the persona this crate dispatches",
        ),
        // ...and a plan below it that names `body` is refused by that field's
        // name, exactly as a field no schema ever had is. Silently dropping it
        // would leave its author to find out from the published change request.
        (
            "earlybody",
            r#"{"schema_version":2,"tasks":[{"id":"publish","repo":"o/r","persona":"e",
                "task":"t","title":"feat: ship it","body":"what it landed"}]}"#,
            "node 'publish': `body` is a schema 3 field",
        ),
        // The same answer at the oldest version this build reads, and it is the
        // *field's*: version 1 is a document this build executes, so a planner
        // who wrote a body there has one thing to act on and it is the field.
        (
            "earlybodyv1",
            r#"{"schema_version":1,"tasks":[{"id":"publish","repo":"o/r","persona":"e",
                "task":"t","body":"what it landed"}]}"#,
            "node 'publish': `body` is a schema 3 field",
        ),
    ];

    for (name, body, expected) in cases {
        let path = world.raw_plan(&format!("{name}.plan.json"), body);
        let refused = world.run(&["start", &path.to_string_lossy()]);
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
            "a refused plan left a run directory behind"
        );
    }

    // A plan file is read with its own format's escape semantics, so the two
    // formats reach the schema by different paths. The retired field is named on
    // both, because a planner who writes YAML wrote the same review bar — and
    // this one is a legacy document, as every plan carrying that field is.
    let yaml = world.raw_plan(
        "donewhen.plan.yaml",
        "schema_version: 1\ntasks:\n  - id: a\n    persona: e\n    task: t\n    \
         done_when: the gate is green\n",
    );
    world
        .run(&["start", &yaml.to_string_lossy()])
        .exited(REFUSED)
        .err_has("`done_when` is no longer a plan field")
        .err_has("`## Acceptance criteria` section of its own task")
        .err_lacks("is not one this build reads");
}

#[test]
fn a_json_plan_keeps_json_escape_semantics_all_the_way_to_the_dispatch() {
    let world = World::new("plan-emoji");
    // What a JSON writer emits for one emoji is a surrogate pair. Read as YAML
    // it is two unpaired halves and the node fails on its own task prose.
    let path = world.raw_plan(
        "emoji.plan.json",
        r#"{"schema_version":2,"name":"emoji","tasks":[
            {"id":"build","persona":"engineer","task":"😀 ship it"}]}"#,
    );
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
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
        "the surrogate pair did not survive as one character: {task:?}"
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
        .run(&["start", &path.to_string_lossy(), "--attach"])
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
    let mut command = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
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
    let mut command = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
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
    let mut command = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
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
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
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

/// Everything under a directory, relative and sorted.
///
/// A whole-world listing rather than a check for a run directory by name,
/// because *mints nothing* is a claim about everything a launch would have
/// written — the runs root, the run's own ledger, an `onevcs` session's state —
/// and naming only the shapes this journey happened to think of would pass a
/// verb that wrote one it did not.
fn everything_under(root: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            found.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned(),
            );
            if path.is_dir() {
                pending.push(path);
            }
        }
    }
    found.sort();
    found
}

/// `validate` is the launch's own validation asked as a question.
///
/// The refusal a plan-writing agent could never see: a plan is refused at a
/// launch long after its author has finished and gone. This verb is that same
/// reading, answered while the author is still there — so what it has to prove
/// is that it answers *identically* and costs nothing.
#[test]
fn validate_answers_as_start_does_and_mints_nothing_either_way() {
    let world = World::new("plan-validate");
    let valid = world.plan(
        "valid",
        &plan_of(
            "valid",
            vec![agent("build", &[]), human("sign", &["build"])],
        ),
    );
    // A lifecycle node at the version that requires a title, naming none — the
    // refusal that a planner met at launch, with nobody left to fix it.
    let untitled = world.raw_plan(
        "untitled.plan.json",
        r#"{"schema_version":3,"name":"untitled","tasks":[
            {"id":"publish","repo":"service","persona":"engineer","task":"t"}]}"#,
    );

    // The harness's own git config is written the first time it builds a
    // command, so it is written here — before the snapshot — rather than left to
    // appear inside it and read as something the binary put there.
    world.gitconfig();
    let before = everything_under(&world.root);

    let accepted = world.run(&["validate", &valid.to_string_lossy()]);
    accepted.exited(0);
    assert!(
        accepted.stdout.is_empty() && accepted.stderr.is_empty(),
        "a plan that validates is answered by the status alone, and this one also said\n\
         stdout: {}\nstderr: {}",
        accepted.stdout,
        accepted.stderr
    );

    let refused = world.run(&["validate", &untitled.to_string_lossy()]);
    refused
        .exited(REFUSED)
        .err_has("node 'publish'")
        .err_has("names no `title`");
    assert!(
        refused.stdout.is_empty(),
        "the refusal reached stdout, where a caller reading the plan's verdict off \
         that stream would take it for output:\n{}",
        refused.stdout
    );

    // A plan file that could not be read at all is the same refusal: it is not a
    // plan this launch would accept, and the code says so rather than the text.
    world
        .run(&["validate", "no-such.plan.json"])
        .exited(REFUSED)
        .err_has("no-such.plan.json");

    // Neither run wrote anything anywhere in the world: no run root, no run id,
    // no session, no ledger entry.
    assert_eq!(
        before,
        everything_under(&world.root),
        "a read-only verb wrote into the world it was pointed at"
    );
    assert!(
        world.invocations().is_empty(),
        "`validate` launched an agent graph: {:?}",
        world.invocations()
    );

    // The same plan, the same words, on the same stream: this verb is `start`'s
    // own validation rather than a second reading of the schema beside it.
    let launched = world.run(&["start", &untitled.to_string_lossy()]);
    launched.exited(REFUSED);
    assert_eq!(
        refused.stderr, launched.stderr,
        "`validate` refuses a plan in different words from the launch it stands for"
    );
}

/// A plan is validated as the version its own document declares.
///
/// The rule `start` reads it by, inherited rather than tightened: a verb that
/// answered as the strictest version this build knows would refuse plans the
/// launch it stands for accepts, which is worse than no verb at all.
#[test]
fn validate_reads_a_plan_as_the_schema_version_it_declares() {
    let world = World::new("plan-validate-version");
    let untitled = |version: u32| {
        format!(
            r#"{{"schema_version":{version},"name":"v{version}","tasks":[
                {{"id":"publish","repo":"service","persona":"engineer","task":"t"}}]}}"#
        )
    };

    // A lifecycle node's `title` arrived at 3. A plan written before it states
    // none, and publishes under the subject `onevcs` derives instead.
    for version in [1, 2] {
        let path = world.raw_plan(&format!("v{version}.plan.json"), &untitled(version));
        world.run(&["validate", &path.to_string_lossy()]).exited(0);
    }
    let path = world.raw_plan("v3.plan.json", &untitled(3));
    world
        .run(&["validate", &path.to_string_lossy()])
        .exited(REFUSED)
        .err_has("node 'publish'")
        .err_has("names no `title`");

    // And the other direction: a field a declared version never had is refused
    // by that field's own name, which is the reading `start` makes of it too.
    let early = world.raw_plan(
        "earlybody.plan.json",
        r#"{"schema_version":2,"name":"earlybody","tasks":[
            {"id":"publish","repo":"service","persona":"engineer","task":"t",
             "title":"feat: ship it","body":"what it landed"}]}"#,
    );
    world
        .run(&["validate", &early.to_string_lossy()])
        .exited(REFUSED)
        .err_has("node 'publish': `body` is a schema 3 field")
        .err_lacks("is not one this build reads");
}

/// One operand, both plan formats, and a file that is not a plan at all.
///
/// `validate` reads what `start` reads: a YAML plan as readily as a JSON one,
/// which reach the schema by different paths, and a readable file the schema
/// cannot make a plan of is refused as the document it is rather than reported
/// as one that could not be found. The operand is the whole of the surface — a
/// flag here would be a way to be refused differently from the launch this verb
/// stands for.
#[test]
fn validate_takes_one_plan_operand_and_reads_both_plan_formats() {
    let world = World::new("plan-validate-operand");

    let yaml = world.raw_plan(
        "ok.plan.yaml",
        "schema_version: 3\nname: yamlok\ntasks:\n  - id: build\n    persona: engineer\n    \
         task: ship it\n",
    );
    world.run(&["validate", &yaml.to_string_lossy()]).exited(0);

    let retired = world.raw_plan(
        "retired.plan.yaml",
        "schema_version: 1\nname: yamlbad\ntasks:\n  - id: build\n    persona: engineer\n    \
         task: ship it\n    done_when: the gate is green\n",
    );
    world
        .run(&["validate", &retired.to_string_lossy()])
        .exited(REFUSED)
        .err_has("`done_when` is no longer a plan field")
        .err_has("`## Acceptance criteria` section of its own task");

    // A file that reads but is not a plan. The two failures a caller can act on
    // differently — a path that is not there, and a document that is — must not
    // be answered in each other's words.
    let malformed = world.raw_plan("malformed.plan.json", r#"{"schema_version":3,"tasks":["#);
    world
        .run(&["validate", &malformed.to_string_lossy()])
        .exited(REFUSED)
        .err_has("malformed.plan.json")
        .err_lacks("No such file");

    // The operand and nothing beside it.
    world
        .run(&["validate", &yaml.to_string_lossy(), "second.plan.json"])
        .exited(USAGE_ERROR)
        .err_has("unexpected argument");
    world
        .run(&["validate", &yaml.to_string_lossy(), "--detach"])
        .exited(USAGE_ERROR)
        .err_has("--detach");
    world
        .run(&["validate"])
        .exited(USAGE_ERROR)
        .err_has("<PLAN>");
}
