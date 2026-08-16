//! The shipped content: the personas, the two agent-graph configs, the example
//! plans, and the executor-rules example.
//!
//! These are what a consumer receives, so they are checked against the schemas
//! that read them rather than only against the eye.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The scenario this journey states is one a real sibling would need paid
// model turns to produce, and `dispatch.rs` is where the real `oneagentgraph` binary is
// driven instead. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{repo_file, World};
use oneagentgraph::config::{GraphConfig, JudgeSide, Member};

fn read(relative: &str) -> String {
    std::fs::read_to_string(repo_file(relative))
        .unwrap_or_else(|e| panic!("{relative} does not ship: {e}"))
}

/// A persona is wrapped prose, so match on its words rather than its line
/// breaks.
fn unwrapped(relative: &str) -> String {
    read(relative)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn graph(relative: &str) -> GraphConfig {
    serde_norway::from_str(&read(relative))
        .unwrap_or_else(|e| panic!("{relative} is not a valid oneagentgraph config: {e}"))
}

#[test]
fn the_dag_scope_graph_is_the_monitor_and_the_resettable_check_in() {
    let config = graph("graphs/dag-scope.yaml");
    assert_eq!(config.name, "dag-scope");

    let monitor = config
        .members
        .get("monitor")
        .expect("the dag-scope graph has a monitor member");
    let Member::Onejudge(monitor) = monitor else {
        panic!("the monitor is a two-party conversation");
    };
    // Its judge side is this crate's own channel server, so the planner reads
    // what the monitor raises through the same conversation it runs in.
    let JudgeSide::Command(judge) = &monitor.judge else {
        panic!("the monitor's judge side is a command provider");
    };
    assert_eq!(judge.command[0], "onepipeline");
    assert_eq!(judge.command[1], "channel");
    assert_eq!(judge.command[2], "serve");
    assert!(
        judge.command[3].contains("ONEPIPELINE_RUN_ID"),
        "the launcher has nowhere to substitute the run id: {:?}",
        judge.command
    );

    let check_in = config
        .members
        .get("check-in")
        .expect("the dag-scope graph paces planner updates");
    let Member::Oneharness(check_in) = check_in else {
        panic!("the check-in member is single-sided");
    };
    let schedule = check_in
        .schedule
        .expect("the check-in member is on a schedule");
    assert!(
        schedule.resettable,
        "the pacemaker cannot be reset, so a run that is already reporting still gets one"
    );
    // The interval the driver seeds by default.
    assert_eq!(schedule.every, 1_800);
}

#[test]
fn the_node_scope_graph_is_the_default_worker_and_judge() {
    let config = graph("graphs/node-scope.yaml");
    assert_eq!(config.name, "node-scope");
    let Member::Onejudge(worker) = config
        .members
        .get("worker")
        .expect("the node-scope graph has a worker")
    else {
        panic!("the worker is a two-party conversation");
    };
    // The persona and the task both come from the node being dispatched, so
    // neither is pinned here.
    assert!(
        worker.persona.is_none(),
        "the node-scope graph pins a persona"
    );
    assert!(worker.task.is_none(), "the node-scope graph pins a task");
    assert!(
        matches!(worker.judge, JudgeSide::Harness(_)),
        "a node's judge is a harness, not a command provider"
    );
}

/// Each shipped persona file, and the role it carries.
///
/// `orchestrator.yaml` holds the **monitor**: the orchestrator persona was
/// rewritten into the observer rather than replaced by a file beside it, so the
/// shipped path a consumer already names keeps resolving to the run's one
/// dag-scope persona.
const SHIPPED_PERSONAS: [(&str, &str); 3] = [
    ("orchestrator", "monitor"),
    ("check-in", "check-in"),
    ("pr-author", "pr-author"),
];

#[test]
fn every_shipped_persona_is_a_persona_with_both_sides() {
    for (file, role) in SHIPPED_PERSONAS {
        let text = read(&format!("personas/{file}.yaml"));
        let document: serde_json::Value = serde_norway::from_str(&text)
            .unwrap_or_else(|e| panic!("personas/{file}.yaml is not valid YAML: {e}"));

        assert_eq!(
            document["agent"]["name"], role,
            "personas/{file}.yaml names a different agent"
        );
        let instructions = document["agent"]["instructions"]
            .as_str()
            .unwrap_or_else(|| panic!("personas/{file}.yaml has no agent instructions"));
        let supervisor = document["user"]["persona"]
            .as_str()
            .unwrap_or_else(|| panic!("personas/{file}.yaml has no supervisor persona"));
        assert!(
            instructions.len() > 200,
            "personas/{file}.yaml says too little"
        );
        assert!(
            supervisor.len() > 100,
            "personas/{file}.yaml supervises too little"
        );
    }
}

#[test]
fn the_monitor_persona_observes_and_never_drives() {
    let text = unwrapped("personas/orchestrator.yaml");
    // It says what it is for, in the vocabulary the channel actually enforces.
    for word in ["Observe one run", "non-blocking", "\"author\": \"monitor\""] {
        assert!(
            text.contains(word),
            "the monitor persona never says `{word}`"
        );
    }
    // The ops it may issue, and the three it is refused, named so the persona
    // and the allowlist cannot drift apart.
    for op in ["retry", "requeue", "cancel", "context", "add"] {
        assert!(
            text.contains(&format!("`{op}`")),
            "the monitor persona does not name the `{op}` op it may issue"
        );
    }
    for refused in ["complete", "attest", "drop"] {
        assert!(
            text.contains(&format!("`{refused}`")),
            "the monitor persona does not name the `{refused}` op it is refused"
        );
    }
    // It never drives, and it never authors the target project's content.
    assert!(text.contains("You do not drive it"));
    assert!(text.contains("never author the target project"));
    // And it must not carry the vocabulary it was ported from.
    for stale in [
        "run-plan",
        "next-round",
        "channel-reply",
        "just orchestrate",
        "onepipeline round run",
        "onepipeline round next",
    ] {
        assert!(
            !text.contains(stale),
            "the monitor persona still says `{stale}`"
        );
    }
}

#[test]
fn the_check_in_persona_reports_and_never_asks_for_a_decision() {
    let text = unwrapped("personas/check-in.yaml");
    assert!(text.contains("never blocks the run waiting for one"));
    assert!(text.contains("Reject any request for a decision"));
}

#[test]
fn the_pr_author_persona_is_off_the_publication_path() {
    let text = unwrapped("personas/pr-author.yaml");
    assert!(text.contains("you are not on the publication path"));
    assert!(text.contains("The change request opens with no body and the change still publishes"));
    // What it answers with, because a body that is not in the schema's own
    // `body` field reaches nobody: the answer is validated and read from there.
    assert!(text.contains("the JSON object the schema your graph names requires"));
}

#[test]
fn both_example_plans_start_a_real_run() {
    for (name, run) in [
        ("single-node.plan.json", "single-node"),
        ("mixed-graph.plan.json", "tracked-release"),
    ] {
        let world = World::new(&format!("shipped-{run}"));
        world.script("driver.wait", "hold");
        let mut parsed: serde_json::Value =
            serde_json::from_str(&read(&format!("examples/{name}"))).expect("the example parses");
        let mut repos = std::collections::BTreeMap::new();
        for task in parsed["tasks"].as_array_mut().into_iter().flatten() {
            if let Some(repo) = task["repo"].as_str().map(str::to_string) {
                let checkout = world.root.join(repo.replace('/', "-"));
                if repos.insert(repo.clone(), checkout.clone()).is_none() {
                    std::fs::create_dir_all(&checkout).expect("an example checkout");
                    let initialized = std::process::Command::new("git")
                        .args(["init", "--initial-branch=main"])
                        .arg(&checkout)
                        .output()
                        .expect("git initializes the example checkout");
                    assert!(
                        initialized.status.success(),
                        "{}",
                        String::from_utf8_lossy(&initialized.stderr)
                    );
                    world.register(&checkout, Some(&format!("https://github.com/{repo}.git")));
                }
                task["repo"] = serde_json::Value::String(checkout.to_string_lossy().into_owned());
            }
        }
        let plan = world.plan(name, &parsed);
        world
            .run(&["start", &plan.to_string_lossy(), "--detach"])
            .exited(0)
            .out_has(run);
        assert!(
            world.run_file(run, "launch.json").exists(),
            "examples/{name} did not start a run"
        );
        world.release("driver.go");
    }
}

#[test]
fn the_examples_reference_the_shipped_node_scope_config() {
    let mixed: serde_json::Value =
        serde_json::from_str(&read("examples/mixed-graph.plan.json")).expect("it parses");
    let referenced: Vec<&str> = mixed["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .filter_map(|node| node["agent_graph"].as_str())
        .collect();
    assert!(
        referenced.contains(&"./graphs/node-scope.yaml"),
        "no example node overrides its graph with the shipped node-scope config: {referenced:?}"
    );
    // And the config it names is the one this repository ships.
    assert!(repo_file("graphs/node-scope.yaml").is_file());
}

#[test]
fn the_executor_rules_example_selects_the_shipped_local_executor() {
    let world = World::new("shipped-rules");
    let plan = world.plan(
        "ruled",
        &crate::harness::plan_of("ruled", vec![crate::harness::agent("build", &[])]),
    );
    let mut command = world.cmd(&["start", &plan.to_string_lossy(), "--attach"]);
    command.env(
        "ONEPIPELINE_EXECUTOR_RULES",
        repo_file("examples/executors.yaml"),
    );
    let output = command.output().expect("the binary runs");
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    world.until("the run to settle", |world| {
        world.run_file("ruled", "result.json").is_file()
    });
    assert_eq!(world.run_json("ruled", "result.json")["state"], "complete");
}

#[test]
fn a_rules_file_the_grammar_refuses_dispatches_nothing() {
    let world = World::new("shipped-badrules");
    let rules = world.root.join("bad-executors.yaml");
    std::fs::write(
        &rules,
        "executors: [{name: local, type: local}]\nrules: [{use: elsewhere}]\n",
    )
    .expect("the rules are written");
    let plan = world.plan(
        "badrules",
        &crate::harness::plan_of("badrules", vec![crate::harness::agent("build", &[])]),
    );
    let mut command = world.cmd(&["start", &plan.to_string_lossy(), "--attach"]);
    command.env("ONEPIPELINE_EXECUTOR_RULES", &rules);
    let output = command.output().expect("the binary runs");
    // The loop refuses; the run is recorded but nothing is dispatched.
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not declared")
            || world
                .journal("badrules")
                .iter()
                .all(|event| event["kind"] != "node-dispatched"),
        "a rules file naming an undeclared executor still dispatched: {output:?}"
    );
}

#[test]
fn a_memory_limit_in_a_unit_the_grammar_cannot_read_dispatches_nothing() {
    let world = World::new("shipped-badunit");
    let rules = world.root.join("bad-unit-executors.yaml");
    // Decimal `GB` rather than binary `GiB`. Read leniently this means *no
    // limit*, so the one file written to keep dispatches off an exhausted host
    // would be the file that removed the bound.
    std::fs::write(
        &rules,
        "executors: [{name: local, type: local, min_free_mem: 2GB}]\nrules: [{use: local}]\n",
    )
    .expect("the rules are written");
    let plan = world.plan(
        "badunit",
        &crate::harness::plan_of("badunit", vec![crate::harness::agent("build", &[])]),
    );
    let mut command = world.cmd(&["start", &plan.to_string_lossy(), "--attach"]);
    command.env("ONEPIPELINE_EXECUTOR_RULES", &rules);
    command.output().expect("the binary runs");
    assert!(
        world
            .journal("badunit")
            .iter()
            .all(|event| event["kind"] != "node-dispatched"),
        "a rules file with an unreadable limit still dispatched"
    );

    // And it refuses by name, on the stderr the operator who typed `start` is
    // already reading: the launch drives the run itself.
    let mut again = world.cmd(&["start", &plan.to_string_lossy(), "--attach"]);
    again.env("ONEPIPELINE_EXECUTOR_RULES", &rules);
    let refused = again.output().expect("the binary runs");
    let said = String::from_utf8_lossy(&refused.stderr).to_string();
    assert!(!refused.status.success(), "the run ran anyway: {said}");
    assert!(said.contains("min_free_mem"), "{said}");
    assert!(
        said.contains("GiB"),
        "the refusal did not say what to write instead: {said}"
    );
}

/// A rules file whose only rule tests a node label, with no fallback. The
/// absence of a fallback is what makes the selection observable through the
/// binary: a node the label rule does not match has nowhere to dispatch, and the
/// launch says so by name.
fn label_routed_rules(world: &World, persona: &str) -> std::path::PathBuf {
    let rules = world.root.join(format!("label-{persona}-executors.yaml"));
    std::fs::write(
        &rules,
        format!(
            "executors: [{{name: local, type: local}}]\n\
             rules: [{{when: {{node_label: {{persona: {persona}}}}}, use: local}}]\n"
        ),
    )
    .expect("the rules are written");
    rules
}

#[test]
fn a_node_label_rule_routes_the_node_it_names_and_only_that_node() {
    let world = World::new("shipped-labelrule");
    let rules = label_routed_rules(&world, "engineer");
    // `agent` builds an `engineer` node, which is the persona the rule names.
    let plan = world.plan(
        "labelled",
        &crate::harness::plan_of("labelled", vec![crate::harness::agent("build", &[])]),
    );
    let mut command = world.cmd(&["start", &plan.to_string_lossy(), "--attach"]);
    command.env("ONEPIPELINE_EXECUTOR_RULES", &rules);
    let output = command.output().expect("the binary runs");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    world.until("the run to settle", |world| {
        world.run_file("labelled", "result.json").is_file()
    });
    assert_eq!(
        world.run_json("labelled", "result.json")["state"],
        "complete"
    );

    // The same plan against a rule naming a persona this node does not carry has
    // nowhere to dispatch, and the run refuses rather than picking somewhere.
    let world = World::new("shipped-labelrule-miss");
    let elsewhere = label_routed_rules(&world, "reviewer");
    let said = refused_launch(&world, "unlabelled", &elsewhere);
    assert!(said.contains("nothing can dispatch"), "{said}");
}

/// Launch a one-node run under these rules and return what the launch said.
///
/// Attached, because the launch drives the run itself: the refusal is this
/// command's own stderr, asserted where it was produced rather than read back
/// out of what some other process recorded.
fn refused_launch(world: &World, name: &str, rules: &std::path::Path) -> String {
    let plan = world.plan(
        name,
        &crate::harness::plan_of(name, vec![crate::harness::agent("build", &[])]),
    );
    let refused = world
        .cmd(&["start", &plan.to_string_lossy(), "--attach"])
        .env("ONEPIPELINE_EXECUTOR_RULES", rules)
        .output()
        .expect("the binary runs");
    let said = String::from_utf8_lossy(&refused.stderr).to_string();
    assert!(
        !refused.status.success(),
        "the run dispatched anyway: {said}"
    );
    said
}

#[test]
fn a_rule_testing_a_label_that_is_not_selectable_is_refused_by_name() {
    let world = World::new("shipped-badlabel");
    let rules = world.root.join("bad-label-executors.yaml");
    // `step` is a real reserved label, and still not one an executor rule can
    // test: the choice is made once per node, before any step runs.
    std::fs::write(
        &rules,
        "executors: [{name: local, type: local}]\n\
         rules: [{when: {node_label: {step: implement}}, use: local}, {use: local}]\n",
    )
    .expect("the rules are written");
    let said = refused_launch(&world, "badlabel", &rules);
    assert!(said.contains("step"), "{said}");
    assert!(
        said.contains("persona"),
        "the refusal did not name what a rule may test instead: {said}"
    );
}

/// `round` is the other label a rule may not test, and for a stronger reason
/// than `step`: nothing stamps one at all any more, so a rule naming one could
/// never hold under any run.
#[test]
fn a_rule_testing_the_retired_round_label_is_refused_by_name() {
    let world = World::new("shipped-roundlabel");
    let rules = world.root.join("round-label-executors.yaml");
    std::fs::write(
        &rules,
        "executors: [{name: local, type: local}]\n\
         rules: [{when: {node_label: {round: \"1\"}}, use: local}, {use: local}]\n",
    )
    .expect("the rules are written");
    let said = refused_launch(&world, "roundlabel", &rules);
    assert!(said.contains("round"), "{said}");
    assert!(
        said.contains("run_id") && said.contains("node") && said.contains("persona"),
        "the refusal did not name what a rule may test instead: {said}"
    );
}
