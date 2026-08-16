//! The `oneagentgraph` seam, against the real `oneagentgraph`.
//!
//! Every other journey here substitutes that sibling wholesale, which is what
//! let a run report success while the sibling was refusing every dispatch it was
//! sent: the double accepted a `--label` the real CLI reserves. The journeys
//! here close that gap from the other side — the real binary resolves the
//! graph, supervises the member, and stamps the stream, and the only thing
//! standing in is the paid model turn, replaced at oneharness's own
//! `ONEHARNESS_BIN_CLAUDE_CODE` override.

// llmlint: ignore-file[e2e_not_mocked] the layer under test is this crate's dispatch
// *through* `oneagentgraph`, and that layer is real here: the sibling's own compiled
// binary resolves the graph, prepares the member, supervises it, and stamps the stream.
// What stands in is the innermost layer of the stack — the paid model turn, which
// `oneagentgraph` reaches by calling `oneharness` as a library and which oneharness
// spawns as the harness the member's identity chain selected. It is swapped at
// oneharness's own documented `ONEHARNESS_BIN_CLAUDE_CODE` override, which knows
// nothing about this crate. There is no offline stand-in for a provider turn, and
// these journeys run inside `just check`, which has neither a credential nor a budget
// for one.

use crate::harness::{agent, human, plan_of, World, REFUSED, REPORTING_MEMBER};
use serde_json::{json, Value};

/// The effective oneharness configuration one member's dispatch was prepared
/// with, read off the run's own merged store.
///
/// `oneagentgraph` publishes the path it composed for each member on that
/// member's `member-started` — its base config, the persona delta and every
/// `--set` resolved by the sibling itself — so which file a dispatch really ran
/// under is a fact of this crate's published surface rather than something a
/// double reported back.
fn configs_of(world: &World, run: &str, member: &str) -> Vec<(String, String)> {
    let events = world.journal(run);
    let started: Vec<&Value> = events
        .iter()
        .filter(|event| event["kind"] == "member-started")
        .filter(|event| event["labels"]["member"] == member)
        .collect();
    assert!(
        !started.is_empty(),
        "no member '{member}' started in {run}: {events:#?}"
    );
    started
        .into_iter()
        .map(|event| {
            let path = event["payload"]["config"]
                .as_str()
                .expect("the sibling publishes the config it launched the member with");
            let text =
                std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path} unreadable: {e}"));
            let node = event["labels"]["onepipeline.node"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            (node, text)
        })
        .collect()
}

/// The same, for the one dispatch a journey means: the member's first.
fn config_of(world: &World, run: &str, member: &str) -> String {
    configs_of(world, run, member).swap_remove(0).1
}

/// Leave a run whose one dispatchable node is ready and whose driver has gone.
///
/// A human gate that has been attested: the loop settled on it and returned, so
/// the run is undriven with work still to do — which is exactly the state an
/// `adopt` picks up, and the state a corrupt ledger has to be refused from.
fn ready_and_undriven(world: &World, run: &str, node: Value) {
    let path = world.plan(run, &plan_of(run, vec![human("approve", &[]), node]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);
    world.run(&["attest", run, "approve"]).exited(0);
}

/// Both shipped relative graph paths are bound to the directory `start` was
/// launched from before either graph can create a workspace. The direct node
/// proves the second graph was read and its member actually ran.
#[test]
fn relative_default_graphs_dispatch_from_the_launch_directory() {
    let world = World::new("real-relative-defaults");
    world.write_graphs();
    let path = world.plan(
        "relative-defaults",
        &plan_of("relative-defaults", vec![agent("build", &[])]),
    );
    let mut command = world.agentgraph_cmd(&[
        "start",
        &path.to_string_lossy(),
        "--attach",
        // Relative, and resolved against the launch directory below.
        "--dag-graph",
        "graphs/dag-scope.yaml",
    ]);
    command
        .current_dir(&world.root)
        .env_remove("ONEPIPELINE_NODE_GRAPH");

    let started = world.run_on(command, "start relative defaults");
    started.exited(0).settled();
    assert!(
        world
            .turns()
            .iter()
            .any(|turn| turn.prompt.contains("Do build.")),
        "the node-scope graph did not dispatch its member: {}",
        world.dump()
    );
    let launch = world.run_json("relative-defaults", "launch.json");
    for field in ["graph", "node_graph"] {
        assert!(
            std::path::Path::new(launch[field].as_str().expect("a graph path")).is_absolute(),
            "{field} was not resolved at launch: {launch}"
        );
    }
    // The directory every member of this run worked in, and the sibling's own id
    // for the graph that drove it — both read back off the record rather than
    // inferred, because this is the *library* backend an attached launch takes,
    // and the other one is what the detached journeys exercise.
    assert_eq!(launch["dir"], json!(world.root));
    // Held against the run's own merged store rather than against anything this
    // test knows: the driver's `graph-started` carries the run id `oneagentgraph`
    // stamped on it, so a record naming anything else is a record naming a run
    // that never drove this one.
    let announced = world
        .journal("relative-defaults")
        .into_iter()
        .find(|event| event["kind"] == "graph-started" && event["labels"]["node"].is_null())
        .expect("the driver announced itself into the merged store");
    assert_eq!(
        launch["graph_run"], announced["labels"]["run_id"],
        "the record names a different graph run from the one that drove the run"
    );
}

/// Plan-owned graph references have the same launch-directory semantics as
/// the defaults. Both levels actually dispatch through the real sibling: the
/// node graph runs the first lifecycle step and the step graph runs the second.
#[test]
fn relative_node_and_step_graph_overrides_dispatch_from_the_launch_directory() {
    let world = World::new("real-relative-plan-overrides");
    world.write_graphs();
    world.repository("local-direct", &["true"]);
    for (source, target) in [
        ("node-scope.yaml", "node-override.yaml"),
        ("node-scope.yaml", "step-override.yaml"),
    ] {
        std::fs::copy(world.graphs().join(source), world.root.join(target))
            .expect("the relative graph override is written");
    }
    // The copied graphs name their member's config relative to themselves, so
    // the file has to travel with them under the name they name it by.
    let worker_config = "oneharness-worker.toml";
    std::fs::copy(
        world.graphs().join(worker_config),
        world.root.join(worker_config),
    )
    .expect("the relative graphs' harness config is written");
    let node = json!({
        "id": "service",
        "repo": "service",
        "agent_graph": "node-override.yaml",
        "steps": [
            {"id": "implement", "persona": "engineer", "task": "## What\nimplement"},
            {
                "id": "review",
                "persona": "reviewer",
                "task": "## What\nreview",
                "deps": ["implement"],
                "agent_graph": "step-override.yaml",
            },
        ],
    });
    let path = world.plan(
        "relative-plan-overrides",
        &plan_of("relative-plan-overrides", vec![node]),
    );
    let mut command = world.agentgraph_cmd(&["start", &path.to_string_lossy(), "--attach"]);
    command.current_dir(&world.root);

    world
        .run_on(command, "start relative plan graph overrides")
        .exited(0)
        .settled();

    for (step, graph) in [
        ("implement", world.root.join("node-override.yaml")),
        ("review", world.root.join("step-override.yaml")),
    ] {
        let graph = graph
            .canonicalize()
            .expect("the expected relative graph path resolves");
        assert!(
            world
                .journal("relative-plan-overrides")
                .iter()
                .any(|event| {
                    event["kind"] == "graph-started"
                        && event["labels"]["node"] == "service"
                        && event["labels"]["step"] == step
                        && event["payload"]["graph"].as_str().is_some_and(|actual| {
                            std::fs::canonicalize(actual)
                                .map(|actual| actual == graph)
                                .unwrap_or(false)
                        })
                }),
            "{step} did not dispatch with its resolved graph: {}",
            world.dump()
        );
    }
}

#[test]
fn lifecycle_and_title_drafting_keep_the_node_graph_resolved_at_launch() {
    let world = World::new("lifecycle-recorded-default-graph");
    world.repository("local-direct", &["true"]);
    world.script("driver.wait", "hold");
    world.script("service.work", "the worker wrote this\n");
    let launch_graph = crate::harness::repo_file("graphs/node-scope.yaml");
    let later_graph = world.root.join("later-node-scope.yaml");
    std::fs::copy(&launch_graph, &later_graph).expect("the later graph is written");
    let mut service = crate::harness::lifecycle("service", &["approve"]);
    service["deps"] = json!(["approve"]);
    let path = world.plan(
        "recorded-lifecycle-graph",
        &plan_of(
            "recorded-lifecycle-graph",
            vec![human("approve", &[]), service],
        ),
    );
    let mut start = world.cmd(&["start", &path.to_string_lossy(), "--attach"]);
    start.env("ONEPIPELINE_NODE_GRAPH", &launch_graph);
    world
        .run_on(start, "start recorded lifecycle graph")
        .exited(0);
    world
        .run(&["attest", "recorded-lifecycle-graph", "approve"])
        .exited(0);
    // A fresh driver, under an environment naming a *different* node graph: what
    // the dispatch runs under is the reference resolved at launch and recorded,
    // never whatever this process happens to be pointed at.
    let mut adopted = world.cmd(&["adopt", "recorded-lifecycle-graph"]);
    adopted.env("ONEPIPELINE_NODE_GRAPH", &later_graph);
    world
        .run_on(adopted, "adopt with a changed live node graph")
        .exited(0);

    let invocations = world.invocations();
    let relevant: Vec<&Value> = invocations
        .iter()
        .filter(|call| {
            call["tool"] == "oneagentgraph"
                && call["args"]
                    .as_array()
                    .is_some_and(|args| args.iter().any(|arg| arg == "onepipeline.node=service"))
        })
        .collect();
    assert!(
        relevant
            .iter()
            .any(|call| call["args"].as_array().is_some_and(|args| {
                args.iter()
                    .any(|arg| arg == "onepipeline.persona=pr-author")
            })),
        "the title drafting dispatch did not run: {relevant:?}"
    );
    assert!(
        relevant
            .iter()
            .all(|call| call["args"][1] == launch_graph.to_string_lossy().as_ref()),
        "a lifecycle dispatch re-read the live graph instead of launch state: {relevant:?}"
    );
}

#[test]
fn an_unreadable_relative_graph_names_its_launch_base() {
    let world = World::new("relative-graph-error");
    let path = world.plan(
        "relative-error",
        &plan_of("relative-error", vec![agent("build", &[])]),
    );
    let mut command = world.agentgraph_cmd(&[
        "start",
        &path.to_string_lossy(),
        "--attach",
        "--dag-graph",
        "graphs/missing-dag.yaml",
    ]);
    command.current_dir(&world.root);

    let failed = world.run_on(command, "start missing relative graph");
    failed.exited(crate::harness::REFUSED);
    failed.err_has("graphs/missing-dag.yaml");
    failed.err_has(&world.root.to_string_lossy());
}

#[test]
fn an_unreadable_relative_node_graph_names_its_launch_base() {
    let world = World::new("relative-node-graph-error");
    world.write_graphs();
    let path = world.plan(
        "relative-node-error",
        &plan_of("relative-node-error", vec![agent("build", &[])]),
    );
    let mut command = world.agentgraph_cmd(&["start", &path.to_string_lossy(), "--attach"]);
    command
        .current_dir(&world.root)
        .env("ONEPIPELINE_NODE_GRAPH", "graphs/missing-node.yaml");

    let failed = world.run_on(command, "start missing relative node graph");
    failed.exited(crate::harness::REFUSED);
    failed.err_has("graphs/missing-node.yaml");
    failed.err_has(&world.root.to_string_lossy());
}

#[test]
fn unreadable_relative_plan_graphs_name_their_path_and_launch_base() {
    let world = World::new("relative-plan-graph-errors");
    world.write_graphs();
    world.repository("local-direct", &["true"]);
    let cases = [
        (
            "missing-node-override",
            json!({
                "id": "build",
                "persona": "engineer",
                "task": "## What\nbuild",
                "agent_graph": "graphs/missing-node-override.yaml",
            }),
            "graphs/missing-node-override.yaml",
        ),
        (
            "missing-step-override",
            json!({
                "id": "service",
                "repo": "service",
                "steps": [{
                    "id": "implement",
                    "persona": "engineer",
                    "task": "## What\nimplement",
                    "agent_graph": "graphs/missing-step-override.yaml",
                }],
            }),
            "graphs/missing-step-override.yaml",
        ),
    ];

    for (name, node, missing) in cases {
        let path = world.plan(name, &plan_of(name, vec![node]));
        let mut command = world.agentgraph_cmd(&["start", &path.to_string_lossy(), "--attach"]);
        command.current_dir(&world.root);

        let failed = world.run_on(command, &format!("start {name}"));
        failed.exited(crate::harness::REFUSED);
        failed.err_has(missing);
        failed.err_has(&world.root.to_string_lossy());
    }
}

#[test]
fn broken_launch_records_refuse_the_adoption_before_direct_or_lifecycle_dispatch() {
    // llmlint: ignore-block[tests_mirror_real_usage] no CLI command corrupts or removes
    // its own ledger. These are external-state faults (partial write or cleanup), so the
    // arrangement mutates that persisted boundary; every observation and asserted
    // refusal still goes through the compiled CLI.
    let direct = World::new("corrupt-launch-direct");
    let mut build = agent("build", &["approve"]);
    build["deps"] = json!(["approve"]);
    ready_and_undriven(&direct, "corrupt-direct", build);
    std::fs::write(direct.run_file("corrupt-direct", "launch.json"), "not json")
        .expect("the launch record is corrupted");
    direct
        .run(&["adopt", "corrupt-direct"])
        .exited(crate::harness::REFUSED)
        .err_has("launch.json");

    let lifecycle_world = World::new("missing-launch-lifecycle");
    lifecycle_world.repository("local-direct", &["true"]);
    let mut service = crate::harness::lifecycle("service", &["approve"]);
    service["deps"] = json!(["approve"]);
    ready_and_undriven(&lifecycle_world, "missing-lifecycle", service);
    std::fs::remove_file(lifecycle_world.run_file("missing-lifecycle", "launch.json"))
        .expect("the launch record is removed");
    lifecycle_world
        .run(&["adopt", "missing-lifecycle"])
        .exited(crate::harness::REFUSED)
        .err_has("launch.json");
    // llmlint: ignore-end[tests_mirror_real_usage]
}

#[test]
fn a_legacy_launch_without_a_node_graph_fails_instead_of_reading_live_environment() {
    // llmlint: ignore-block[tests_mirror_real_usage] an older launch-record producer is
    // not a CLI operation this build can invoke. Writing that historical schema shape is
    // the necessary fault arrangement; the adoption and its refusal use the compiled CLI.
    let world = World::new("legacy-empty-node-graph");
    let mut build = agent("build", &["approve"]);
    build["deps"] = json!(["approve"]);
    ready_and_undriven(&world, "legacy-empty", build);
    let path = world.run_file("legacy-empty", "launch.json");
    let mut launch: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the launch record reads"))
            .expect("the launch record parses");
    launch["node_graph"] = json!("");
    std::fs::write(&path, serde_json::to_vec_pretty(&launch).unwrap())
        .expect("the legacy launch record is written");

    let mut driving = world.cmd(&["adopt", "legacy-empty"]);
    driving.env(
        "ONEPIPELINE_NODE_GRAPH",
        world.graphs().join("node-scope.yaml"),
    );
    world
        .run_on(driving, "adopt legacy-empty")
        .exited(crate::harness::REFUSED)
        .err_has("has no resolved node graph");
    // llmlint: ignore-end[tests_mirror_real_usage]
}

#[test]
fn launch_overrides_reach_the_graphs_that_actually_run() {
    let world = World::new("real-overrides");
    world.write_graphs();
    std::fs::write(
        world.graphs().join("dag-override.toml"),
        "run_mode = \"fallback\"\nharnesses = [\"claude-code\"]\n# DAG_OVERRIDE\n",
    )
    .expect("the dag override config is written");
    std::fs::write(
        world.graphs().join("node-override.toml"),
        "run_mode = \"fallback\"\nharnesses = [\"claude-code\"]\n# NODE_OVERRIDE\n",
    )
    .expect("the node override config is written");
    let path = world.plan(
        "overrides",
        &plan_of("overrides", vec![agent("build", &[])]),
    );

    let started = world.run_on_agentgraph(&[
        "start",
        &path.to_string_lossy(),
        "--attach",
        "--dag-graph",
        &world.dag_graph(),
        "--set",
        "members.monitor.oneharness_config=./dag-override.toml",
        "--node-set",
        "members.worker.oneharness_config=./node-override.toml",
    ]);
    started.exited(0).settled();

    // Which config each member was prepared with, off the run's own store, and
    // that each of them then really ran — an override that reached a member
    // nobody started is an override that reached nothing.
    let turns = world.turns();
    for (member, marker, job) in [
        ("monitor", "DAG_OVERRIDE", "Observe this run"),
        ("worker", "NODE_OVERRIDE", "Do build."),
    ] {
        let config = config_of(&world, "overrides", member);
        assert!(
            config.contains(marker),
            "the {member} member did not receive its override: {config}"
        );
        assert!(
            turns.iter().any(|turn| turn.prompt.contains(job)),
            "the {member} member never ran its turn: {turns:?}"
        );
    }
}

/// The plan's persona is a graph setting, not merely a label on the dispatch.
/// The real sibling's content-addressed run record proves that it resolved the
/// requested persona while preparing the member that subsequently ran. That is
/// evidence from the actual graph invocation, not this crate's event label.
#[test]
fn a_plan_persona_reaches_the_member_that_actually_runs() {
    let world = World::new("real-plan-persona");
    world.write_graphs();
    std::fs::write(
        world.graphs().join("requested-reviewer.yaml"),
        "agent:\n  name: requested-reviewer\n  instructions: Review the change.\nuser:\n  persona: Demand evidence.\n",
    )
    .expect("the requested persona is written");
    let mut node = agent("review", &[]);
    node["persona"] = Value::from("./requested-reviewer.yaml");
    let path = world.plan("plan-persona", &plan_of("plan-persona", vec![node]));

    let started = world.run_on_agentgraph(&["start", &path.to_string_lossy(), "--attach"]);
    started.exited(0).settled();

    let turns = world.turns();
    assert!(
        turns.iter().any(|turn| turn.prompt.contains("Do review.")),
        "the node's member never ran: {turns:?}"
    );

    let records: Vec<Value> = std::fs::read_dir(world.root.join("graph-state"))
        .expect("oneagentgraph wrote its state root")
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("record.json")).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    assert!(
        records
            .iter()
            .any(|record| record["refs"].as_array().is_some_and(|refs| refs
                .iter()
                .any(|reference| { reference["origin"] == "./requested-reviewer.yaml" }))),
        "the graph that dispatched the member did not resolve the plan's persona: {records:?}"
    );
}

/// Node-scope overrides survive losing the driver that originally launched the
/// run. The adopted driver, rather than the original one, dispatches the node.
#[test]
fn adoption_retains_node_overrides_for_later_dispatches() {
    let world = World::new("real-adopted-node-override");
    world.write_graphs();
    // The first dispatch fails, so the run settles with work still to do and
    // nothing driving it — which is the state `adopt` is for.
    world.script("harness.fail", "");
    std::fs::write(
        world.graphs().join("adopted-node.toml"),
        "run_mode = \"fallback\"\nharnesses = [\"claude-code\"]\n# ADOPTED_NODE_OVERRIDE\n",
    )
    .expect("the adopted node config is written");
    let path = world.plan(
        "adopted-override",
        &plan_of("adopted-override", vec![agent("build", &[])]),
    );
    let mut start = world.agentgraph_cmd(&[
        "start",
        &path.to_string_lossy(),
        "--attach",
        "--node-set",
        "members.worker.oneharness_config=./adopted-node.toml",
    ]);
    start
        .current_dir(&world.root)
        .env("ONEPIPELINE_NODE_GRAPH", "graphs/node-scope.yaml");
    world.run_on(start, "start adopted-override");
    world.until("the run to settle on the failure", |world| {
        world.run_file("adopted-override", "result.json").is_file()
    });

    // A replacement for the failed node, applied to a run nothing is driving.
    std::fs::remove_file(world.fakes.join("harness.fail")).expect("the failure is cleared");
    world
        .run_with_stdin(
            &["reply", "adopted-override"],
            &json!({
                "version": 1,
                "commands": [{
                    "op": "retry",
                    "id": "build",
                    "node": {"id": "build-2", "persona": "engineer",
                             "task": "## What\nDo build.\n\n## Why\nIt failed.\n\n\
                                      ## Acceptance criteria\n- build is done."},
                }],
            })
            .to_string(),
        )
        .exited(0);

    // The adopted driver dispatches it, from another directory and under an
    // environment naming no graph at all: what it runs under is the overrides
    // the launch recorded.
    let mut adopt = world.agentgraph_cmd(&["adopt", "adopted-override"]);
    adopt
        .current_dir(&world.project)
        .env("ONEPIPELINE_NODE_GRAPH", "missing-node.yaml");
    let adopted = world.run_on(adopt, "adopt adopted-override");
    adopted.exited(0).settled();

    // The replacement node's own dispatch, picked out by the node it was for,
    // and the turn it then ran — the override reaching a member nobody started
    // would be the override reaching nothing.
    let configs = configs_of(&world, "adopted-override", "worker");
    let retried = configs
        .iter()
        .find(|(node, _)| node == "build-2")
        .unwrap_or_else(|| panic!("the replacement node was never dispatched: {configs:?}"));
    assert!(
        retried.1.contains("ADOPTED_NODE_OVERRIDE"),
        "the node dispatched after adoption did not run under its retained override: {}",
        retried.1
    );
    let turns = world.turns();
    assert!(
        turns.iter().any(|turn| turn.prompt.contains("It failed.")),
        "the replacement node's turn never ran: {turns:?}"
    );
}

/// A whole run, dispatched through the real sibling: the plan is launched, its
/// driver is a real graph run, the node's dispatch is another, and a member runs
/// in each.
///
/// This is the journey the reserved-label collision broke. It failed as a run
/// that recorded a launch and never dispatched anything, so the assertions are
/// on what the member was actually asked to do, not only on the exit code.
#[test]
fn a_plan_dispatches_through_the_real_oneagentgraph_and_its_members_run() {
    let world = World::new("real-dispatch");
    world.write_graphs();
    let path = world.plan("real", &plan_of("real", vec![agent("build", &[])]));

    let started = world.run_on_agentgraph(&[
        "start",
        &path.to_string_lossy(),
        "--attach",
        "--dag-graph",
        &world.dag_graph(),
    ]);
    started.exited(0).settled();
    let run = started.json()["run_id"]
        .as_str()
        .expect("the launch named its run")
        .to_string();

    // Two members really ran, each on the job its graph gave it. The run's own
    // store says a member was *started* and which config it was prepared with;
    // what it was asked to do is prose the library hands the turn in memory, so
    // the turn itself is what says it — which is also the stronger claim, a
    // member started and a member that ran being different facts.
    for (member, job) in [("monitor", "Observe this run"), ("worker", "Do build.")] {
        let prompt = world.turn_of(member);
        assert!(
            prompt.contains(job),
            "the {member} member was not given its own job: {prompt}"
        );
        assert!(
            !config_of(&world, &run, member).is_empty(),
            "the {member} member was started with an empty configuration"
        );
    }

    // The envelope the handshake spent is still in the stream. Learning that the
    // graph started means reading its first line, and that line is the event
    // saying the driver began — read to settle the launch and then replayed at
    // the head, not consumed by it. Swallowed, a run's own record would begin
    // with the driver already working and nothing saying it ever started.
    assert!(
        world
            .journal(&run)
            .iter()
            .any(|event| event["source"] == "agentgraph"
                && event["kind"] == "graph-started"
                && event["labels"]["node"].is_null()),
        "the driver's own start never reached the merged store: {}",
        world.dump()
    );

    // And each of them worked: a turn is what the sibling reports when the
    // member it launched produced something, so a graph that only *started* a
    // member does not get one.
    assert!(
        world
            .journal(&run)
            .iter()
            .any(|event| event["kind"] == "turn-activity"),
        "no member reported a turn: {}",
        world.dump()
    );

    // And the node settled on what that member did.
    assert_eq!(
        world.run_json(&run, "result.json")["state"],
        "complete",
        "the run did not settle: {}",
        world.dump()
    );

    // The sibling's own envelopes are in the merged store, under the node they
    // belong to — which is the namespacing working end to end: the label was
    // accepted on the way out and read back on the way in.
    let relayed: Vec<serde_json::Value> = world
        .journal(&run)
        .into_iter()
        .filter(|event| event["source"] == "agentgraph" && event["labels"]["node"] == "build")
        .collect();
    assert!(
        !relayed.is_empty(),
        "no relayed envelope belongs to the node: {}",
        world.dump()
    );
    for event in relayed {
        assert_eq!(
            event["labels"]["onepipeline.run_id"],
            run.as_str(),
            "{event}"
        );
        assert_ne!(
            event["labels"]["run_id"],
            run.as_str(),
            "the graph run's own id was overwritten by this run's: {event}"
        );
    }
}

/// How many events a `status` line reports for one node.
///
/// Read off the rendered line rather than out of the journal: the claim under
/// test is what an operator sees, and a count taken from anywhere else would
/// pass while the line said something different.
fn events_reported(status: &str, node: &str) -> u64 {
    let line = status
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("{node}: running")))
        .unwrap_or_else(|| panic!("`status` has no in-flight line for {node}:\n{status}"));
    let at = line
        .find(" event(s)")
        .unwrap_or_else(|| panic!("`{line}` carries no event count"));
    let digits: String = line[..at]
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect();
    digits
        .chars()
        .rev()
        .collect::<String>()
        .parse()
        .unwrap_or_else(|e| panic!("`{line}` carries no readable count: {e}"))
}

/// What a live node is doing, read while it is doing it.
///
/// Mid-run, `status` used to say a node had been in flight for thirty-four
/// minutes and nothing else — the readout a healthy node has twice been
/// reported dead against. The producer emits the tool summary this needs; the
/// claim here is that it is read, and that it **advances** between two readings
/// of a dispatch that is still in flight for both of them.
#[test]
fn status_says_what_a_live_dispatch_is_doing_and_the_readout_advances() {
    let world = World::new("real-activity");
    world.write_graphs();
    world.script("turn.hold", "hold");
    let path = world.plan("watched", &plan_of("watched", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);

    world.until("the dispatch to report a turn", |world| {
        !world.events_of("watched", "turn-activity").is_empty()
    });
    // Read through the ordinary view wiring: `status` only reads the merged
    // store, so the sibling behind it is the health probe's and nothing else.
    let first = world.run(&["status", "watched"]);
    first
        .exited(0)
        .out_has("build: running")
        .out_has("now bash echo the turn ran")
        .out_has("event(s)")
        .out_has("ago");
    let before = events_reported(&first.stdout, "build");

    world.release("turn.go");
    world.until("the dispatch to report a second turn", |world| {
        world.events_of("watched", "turn-activity").len() > 1
    });
    let second = world.run(&["status", "watched"]);
    second
        .exited(0)
        .out_has("build: running")
        .out_has("now bash cargo llvm-cov --workspace");
    assert!(
        events_reported(&second.stdout, "build") > before,
        "the readout did not advance while the node was still in flight:\n{}",
        second.stdout
    );

    world.release("turn.settle");
    world.until("the run to settle", |world| {
        world.run_file("watched", "result.json").is_file()
    });
}

/// The tools a real dispatched turn used, read back off the CLI.
///
/// There was no transcript verb at all: the evidence was retained — the
/// sibling stores each settled member's full onejudge report and says where —
/// and nothing read it, so an agent supervising a run could see that a turn
/// happened and never what it did.
#[test]
fn transcript_renders_a_real_dispatched_turns_tools_and_words() {
    let world = World::new("real-transcript");
    world.write_graphs();
    let path = world.plan("read", &plan_of("read", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0)
        .settled();

    let transcript = world.run(&["transcript", "read", "build"]);
    transcript.exited(0).out_has("read  build");
    // The tools, from the turn summaries the sibling emitted as it ran...
    transcript.out_has("tool_call bash  echo the turn ran");
    // ...and the words, out of the report that member settled with.
    transcript.out_has("report ");
    transcript.out_has("Ran what the task asked for.");
    assert!(
        !transcript.stdout.contains("unreadable from this host"),
        "the retained report was named and not read:\n{}",
        transcript.stdout
    );

    // A node this run has no record for is refused by name rather than answered
    // with an empty transcript, which reads identically to a quiet one.
    world
        .run(&["transcript", "read", "nowhere"])
        .exited(crate::harness::REFUSED)
        .err_has("has recorded nothing for node 'nowhere'")
        .err_has("build");
}

/// A launch the sibling refuses is a failed launch.
///
/// The defect this guards against is not that the graph said no — it is that
/// saying no was invisible: the launcher exited 0 and printed the pid of a
/// process that had already died, and the reason was in a stream nobody read.
///
/// Both launch forms, because the graph's words are somewhere different in each:
/// a detaching launcher gives its driver a log file, an attaching one a pipe,
/// and a refusal that only one of them reported would leave the other silent.
#[test]
fn a_launch_the_graph_refuses_fails_with_the_graphs_own_words() {
    let world = World::new("real-refusal");
    // Deliberately not written, so the graph the driver is launched with names a
    // file the sibling cannot read — a refusal it reports in its own words.
    let path = world.plan("refused", &plan_of("refused", vec![agent("build", &[])]));

    for form in ["--detach", "--attach"] {
        let started = world.run_on_agentgraph(&[
            "start",
            &path.to_string_lossy(),
            form,
            "--dag-graph",
            &world.dag_graph(),
        ]);

        started.exited(crate::harness::REFUSED);
        started.err_has("oneagentgraph");
        started.err_has("dag-scope.yaml");
        assert!(
            !started.stdout.contains("\"pid\""),
            "`start {form}` still printed a pid to drive:\n{}",
            started.stdout
        );
    }
}

/// An adoption whose graph refuses is a failed adoption.
///
/// `adopt` is the other launcher, and it is the one reached from a run that has
/// already lost a driver: an adoption that reported success while starting
/// nothing would leave that run undriven a second time, with the offered way
/// back looking like it had worked.
#[test]
fn an_adoption_the_graph_refuses_fails_rather_than_leaving_the_run_undriven() {
    let world = World::new("real-adopt-refusal");
    world.write_graphs();
    // A human action nothing can clear, so the loop returns and the run is left
    // intact and undriven — which is what `adopt` is for.
    let path = world.plan(
        "orphaned",
        &plan_of("orphaned", vec![human("approve", &[])]),
    );
    world
        .run_on_agentgraph(&[
            "start",
            &path.to_string_lossy(),
            "--detach",
            "--dag-graph",
            &world.dag_graph(),
        ])
        .exited(0);
    world.until("the driver to be gone", |world| {
        world
            .run_on_agentgraph(&["status", "orphaned"])
            .stdout
            .contains("DRIVER DEAD")
    });

    // The graph the launch record names goes away under it, so the relaunch the
    // adoption performs is refused by the sibling.
    std::fs::remove_file(world.graphs().join("dag-scope.yaml")).expect("the graph is removed");

    let adopted = world.run_on_agentgraph(&["adopt", "orphaned"]);
    adopted.exited(crate::harness::REFUSED);
    adopted.err_has("oneagentgraph");
    assert!(
        world.events_of("orphaned", "driver-adopted").len() == 1,
        "the adoption was recorded more than once: {:?}",
        world.events_of("orphaned", "driver-adopted")
    );
}

/// The environment keys and fallbacks this crate restates are still the ones
/// the sibling's own CLI applies.
///
/// `run::start`, `run::signal`, and `control::interrupt` take their environment
/// as a parameter, which is what lets a consumer hold two runs on two installs
/// — but the *names* in it, and the fallbacks around them, are private
/// `const`s and private functions in the sibling's **binary**. So
/// `src/agentgraph.rs` restates them, and nothing in the type system says when
/// they stop being right: renamed upstream, this crate would keep resolving the
/// old spelling and put a run's state somewhere the sibling's own verbs cannot
/// find it. Recorded as divergence 20; this is the drift gate that stands in
/// until it closes.
///
/// Held with **both** sides real, which is the only way it gates anything: a
/// dispatch this crate ran places the run state, and the sibling's own binary
/// — the one `Cargo.lock` pins — is then asked, through the same variable,
/// what it can find there. Neither side's answer is written down here.
///
/// So a rename lands as a failure whichever side it happens on. Drifted in
/// `src/agentgraph.rs`, the resolution falls back to `$HOME/.local/state` and
/// the run is not under the directory the launch named; drifted upstream, the
/// sibling looks somewhere else for it. Either way `history` lists nothing.
#[test]
fn the_run_state_this_crate_places_is_where_the_sibling_looks_for_it() {
    let world = World::new("state-dir-drift");
    world.write_graphs();
    // The directory `agentgraph_cmd` hands the launch at the variable under
    // test, and the one the sibling is asked about below.
    let state = world.root.join("graph-state");
    let path = world.plan(
        "state-drift",
        &plan_of("state-drift", vec![agent("build", &[])]),
    );
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0)
        .settled();

    let listed = std::process::Command::new(crate::harness::oneagentgraph_binary())
        .arg("history")
        // The one variable under test. Everything else is left alone, so a
        // listing that comes back empty is this directory being empty rather
        // than the sibling being pointed elsewhere.
        .env("ONEAGENTGRAPH_STATE_DIR", &state)
        .output()
        .expect("the real oneagentgraph runs");
    let listed = String::from_utf8_lossy(&listed.stdout);
    // The graph the node dispatch runs, so the line names a run this crate's
    // own launch created rather than any run that happened to be there.
    assert!(
        listed.lines().any(|line| line.contains("node-scope")),
        "the sibling found no run where this crate placed one — the state-directory variable, \
         or the fallback around it, has drifted on one side:\n{listed}\n{}",
        world.dump()
    );
}

/// The exit codes this crate maps the sibling's `Error` onto are still the ones
/// its own CLI exits with.
///
/// The subprocess path read a code off a child; the library path is handed an
/// `Error` and `src/agentgraph.rs`'s `exit_for` turns it into the code the CLI
/// would have carried. That mapping is a copy of a private function upstream —
/// the other half of divergence 20 — so a run must not settle differently
/// depending on which path drove it. Driven against the real binary, so the
/// left-hand side of the comparison is the sibling's own answer.
// llmlint: ignore-block[tests_mirror_real_usage] this is a drift gate over a *sibling's*
// exit codes, not a journey: what it compares is the code the sibling's own binary carries
// a refusal out on against the constant `src/agentgraph.rs` maps that refusal onto, and
// only one side of that comparison is reachable through this crate's interface. Driving
// `onepipeline` here would put its own error handling between the two things being held to
// each other. The journeys that do drive the binary are every other test in this file, and
// the two drift gates either side of this one both go through it.
#[test]
fn the_siblings_own_refusals_still_exit_with_the_codes_this_crate_maps_onto() {
    let world = World::new("exit-code-drift");
    let missing = world.root.join("no-such-graph.yaml");
    let refused = std::process::Command::new(crate::harness::oneagentgraph_binary())
        .args(["run", &missing.to_string_lossy(), "--task", "anything"])
        .env("ONEAGENTGRAPH_STATE_DIR", world.root.join("graph-state"))
        .output()
        .expect("the real oneagentgraph runs");
    assert_eq!(
        refused.status.code(),
        Some(oneagentgraph::error::EXIT_INVALID_CONFIG),
        "an unreadable graph is no longer the invalid-config exit this crate maps \
         `Error::InvalidConfig` onto: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
} // llmlint: ignore-end[tests_mirror_real_usage]

/// The `oneharness` executable the sibling drives is still named by the
/// variable this crate restates.
///
/// The third restated key, and the launch it reaches has moved: from
/// `oneagentgraph 0.2.18` a **single-sided** member's turn is an
/// `oneharness_core` library call with no `oneharness` process in it at all, so
/// that member no longer reads the variable and a journey aimed at one would go
/// green on a value nothing consumed. What still reads it is a `kind: onejudge`
/// member, whose conversation drives each side as `oneharness run` — the
/// sibling writes the executable into the provider block of the config it
/// composes, and publishes that config's path on `member-started`.
///
/// So the assertion is on the launch the sibling prepared rather than on a
/// failure to start: it is published before the turn runs, which is what lets
/// this stay offline. `src/agentgraph.rs` restates the key for its own
/// `interrupt` delivery, and this is the one surface that says the sibling still
/// spells it the same way.
#[test]
fn the_sibling_still_takes_its_harness_from_the_variable_this_crate_restates() {
    let world = World::new("harness-bin-drift");
    world.write_graphs();
    write_supervised_node_graph(&world);
    write_persona(&world, "engineer");
    let mut node = agent("build", &[]);
    node["persona"] = Value::from("./engineer.yaml");
    let path = world.plan("harness-bin", &plan_of("harness-bin", vec![node]));

    let named = "oneharness-that-is-not-installed";
    let mut command = world.agentgraph_cmd(&["start", &path.to_string_lossy(), "--attach"]);
    command.env("ONEAGENTGRAPH_ONEHARNESS_BIN", named);
    world.run_on(command, "start --attach").settled();

    let config = config_of(&world, "harness-bin", "worker");
    assert!(
        config.contains(named),
        "the config the sibling composed does not name the harness it was told to \
         drive, so the variable was not read — it has drifted:\n{config}"
    );
}

/// A `context` note reaches the **real** sibling's interrupt, and what it
/// answers is what the run records.
///
/// The other `context` journeys state their scenario at
/// `ONEPIPELINE_ONEAGENTGRAPH_BIN`, which is the override path; this one takes
/// the default, so the delivery is `oneagentgraph::control::interrupt` called
/// in this process. The sibling addresses the turn out of the member's own
/// scratch and answers for itself.
///
/// The answer here is that there is no controllable turn: the member is real
/// and running, and the harness standing in for its paid turn is not one
/// oneharness can reach a lever into. That is a genuine case rather than a
/// contrivance — it is what a harness with no out-of-band control gives — and
/// it is the one the `auto` fall-through exists for. Both halves are asserted:
/// the note is deferred onto the next dispatch, and the `turn-interrupted`
/// envelope saying the lever was pulled and nothing came of it reaches the
/// merged store, stamped with the node it is about.
#[test]
fn a_note_delivered_through_the_real_sibling_records_what_its_lever_answered() {
    let world = World::new("real-context");
    world.write_graphs();
    world.script("turn.hold", "hold");
    let path = world.plan("noted", &plan_of("noted", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the dispatch to report a turn", |world| {
        !world.events_of("noted", "turn-activity").is_empty()
    });

    let note = "the fixture moved to tests/data; stop editing src/old.rs";
    let submitted = world.run_with_stdin(
        &["reply", "noted"],
        &json!({
            "version": 1,
            "commands": [{"op": "context", "id": "build", "note": note}],
        })
        .to_string(),
    );
    submitted.exited(0);

    world.until("the note to be reconciled", |world| {
        !world.events_of("noted", "edit-committed").is_empty()
    });
    let committed = world.events_of("noted", "edit-committed");
    assert_eq!(
        committed[0]["payload"]["operations"][0]["delivery"],
        json!("deferred"),
        "a note the sibling could not land live was not deferred onto the next dispatch: {:?}",
        committed
    );

    let interrupted = world.events_of("noted", "turn-interrupted");
    assert_eq!(
        interrupted.len(),
        1,
        "the lever was pulled and the run does not say so: {}",
        world.dump()
    );
    assert_eq!(interrupted[0]["payload"]["delivered"], json!(false));
    assert_eq!(interrupted[0]["payload"]["member"], json!("worker"));
    assert_eq!(
        interrupted[0]["payload"]["input_bytes"],
        json!(note.len()),
        "the envelope does not say how much redirection was offered"
    );
    assert!(
        interrupted[0]["payload"]["reason"].is_string(),
        "an interrupt that did not land carries no reason: {}",
        interrupted[0]
    );
    assert_eq!(
        interrupted[0]["labels"]["node"],
        json!("build"),
        "the envelope is not stamped with the node it is about — its producer cannot know it, \
         so this crate has to"
    );

    world.release("turn.go");
    world.release("turn.settle");
}

/// Consuming a planner surface restarts the **real** pacemaker's clock.
///
/// `next` is the channel's only consumer, and consumption is what resets the
/// pacemaker — so this is the one journey that reaches
/// `oneagentgraph::run::signal` on the default path rather than through the
/// override, against a real graph that really declares a resettable `check-in`
/// member.
///
/// The address is what this crate owns, and it is what a real sibling can
/// judge: `oneagentgraph::run::signal` reads the run's record, refuses a member
/// that run never declared, and writes the signal under the run's own
/// directory. All three only work out for the id `oneagentgraph` minted, never
/// for this crate's, so a reset that lands there was addressed correctly.
///
/// What happens to the signal *after* it lands is the sibling's: it starts a
/// scheduled member's clock only once every member of that member's wave has
/// settled, and the monitor shares the pacemaker's wave and runs for the whole
/// run — so nothing consumes the signal while the run it paces is alive.
#[test]
fn consuming_a_surface_restarts_the_real_pacemakers_clock() {
    let world = World::new("real-pacemaker");
    world.write_graphs_with_pacemaker();
    let path = world.plan("paced", &plan_of("paced", vec![human("approve", &[])]));
    world
        .run_on_agentgraph(&[
            "start",
            &path.to_string_lossy(),
            "--detach",
            "--dag-graph",
            &world.dag_graph(),
        ])
        .exited(0);

    // The sibling minted this, and it is not this run's id. Everything below
    // rests on the difference.
    let graph_run = world.run_json("paced", "launch.json")["graph_run"]
        .as_str()
        .expect("the launch record names the graph run driving this run")
        .to_string();
    assert_ne!(graph_run, "paced");

    world
        .run_on_agentgraph(&[
            "surface",
            "paced",
            "--kind",
            "check-in",
            "--message",
            "steady",
        ])
        .exited(0);

    let read = world.run_on(world.agentgraph_cmd(&["next", "paced"]), "next paced");
    read.exited(0).out_has("\"surface\"");
    assert!(
        !read
            .stderr
            .contains("could not reset the check-in pacemaker"),
        "the real sibling refused the reset: {}",
        read.stderr
    );

    // llmlint: ignore-block[tests_mirror_real_usage] a pacemaker reset has no product-facing
    // result: `next` returns the surface either way, by design, because a clock that could
    // not be restarted must not cost the planner the update they asked for. The sibling's
    // signal directory is where the reset *is*, and it is a documented location its own
    // `signal`/`cancel` API both derive — so this is the outcome, read where the outcome
    // lives, and asserting only on the absent error would pass against a reset that went to
    // the wrong run. The run's own clock restarting is the sibling's half; see the module
    // note below.
    // Where the sibling's scheduler watches, derived from the graph run's id and
    // nothing else. A reset addressed with this crate's run id never reaches it:
    // `signal` refuses a run its history has no record of, which is the failure
    // this journey used to characterise.
    let signalled = world
        .graph_state()
        .join(&graph_run)
        .join("signals")
        .join("check-in.reset");
    assert!(
        signalled.is_file(),
        "the reset did not reach the run's own signal directory: {}",
        signalled.display()
    );
    // llmlint: ignore-end[tests_mirror_real_usage]
}

/// A view still renders when the provider-health block comes from the library.
///
/// `status` asks the sibling what this host's identities are, and on the
/// default path that ask is `oneagentgraph::health::read` rather than a
/// process. What the answer *is* depends on the host's own oneharness
/// configuration and is therefore not something a journey can assert; what the
/// contract fixes is the other half — a probe that cannot run is silence and
/// not a failure, so the view reports everything else it knows either way.
///
/// That is the half held here, and it is the half that broke when the call
/// moved: a library read that refused, or that panicked on a host with no
/// identities configured, would take the whole view down where the old
/// `Command` merely failed to start. The run is a real one so the rest of the
/// view has something to render, which is what makes "everything else it knows"
/// checkable rather than vacuous.
#[test]
fn a_view_renders_with_the_health_block_read_through_the_library() {
    let world = World::new("real-health");
    world.write_graphs();
    let path = world.plan("probed", &plan_of("probed", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0)
        .settled();

    // No `ONEPIPELINE_ONEAGENTGRAPH_BIN`, so the probe is the library call.
    let status = world.run_on(world.agentgraph_cmd(&["status", "probed"]), "status probed");
    status.exited(0).out_has("probed").out_has("SETTLED");
    // The override's own answer must not be what came back: that string is the
    // double's, and seeing it here would mean the default path had not been taken.
    assert!(
        !status.stdout.contains("fake-provider"),
        "the view carried the override's health block on the default path:\n{}",
        status.stdout
    );
}

/// The launcher has one answer about what a graph document may contain,
/// whichever way a run is launched.
///
/// A launcher holding a second, staler parser refused a document the runner
/// accepted — the same file, one flag apart. So the document declares
/// [`oneagentgraph::config::SCHEMA_VERSION`](oneagentgraph::config::SCHEMA_VERSION)
/// and uses a field only that version allows, read off the runner rather than
/// written down here, and `PATH` is emptied so neither form can resolve a
/// sibling by name.
///
/// What the document *means* is the runner's business; this crate's claim is
/// only that it holds one parser, so the journey ends there.
#[test]
fn a_document_the_runner_accepts_launches_whichever_way_it_is_asked_for() {
    for form in ["--attach", "--detach"] {
        let world = World::new(&format!("runner-schema-{}", form.trim_start_matches("--")));
        world.write_graphs_at_the_runners_schema();
        let path = world.plan("schema", &plan_of("schema", vec![agent("build", &[])]));

        let mut command = world.agentgraph_cmd(&[
            "start",
            &path.to_string_lossy(),
            form,
            "--dag-graph",
            &world.dag_graph(),
        ]);
        command.env("PATH", world.empty_path());
        let started = world.run_on(command, &format!("start {form}"));
        // The whole of the defect, in one exit code: the launch that refused
        // this document refused it here, naming a field list that predates the
        // one it carries.
        started.exited(0);

        // And it is a launch rather than a parse: the run reaches settlement,
        // which takes the graph running, the loop driving, and the node it
        // dispatched reporting back. Read through `status`, where an operator
        // reads it.
        world.until("the run to settle", |world| {
            world.run(&["status", "schema"]).stdout.contains("SETTLED")
        });
        let results = world.run(&["results", "schema"]);
        results.exited(0).out_has("build");
    }
}

/// Every dag-scope member is handed what the run *is*, and its own job around
/// it.
///
/// The launcher's one `--task` reaches every member of the graph carrying none
/// of its own, so it names the run and its goal and stops; a member that must be
/// told what to do about it composes its own `task` from `{task}`.
///
/// Read off the turn each member actually ran, which is what it was really
/// asked to do rather than anything this crate wrote down about it. Each member
/// names itself out of its own harness config's `[env]`, so what is compared is
/// the job that reached *that* member.
#[test]
fn every_dag_scope_member_is_given_the_runs_description_and_its_own_job() {
    let world = World::new("neutral-run-task");
    world.write_graphs_at_the_runners_schema();
    let path = world.plan("neutral", &plan_of("neutral", vec![agent("build", &[])]));
    world
        .run_on(
            world.agentgraph_cmd(&[
                "start",
                &path.to_string_lossy(),
                "--attach",
                "--dag-graph",
                &world.dag_graph(),
            ]),
            "start neutral",
        )
        .exited(0)
        .settled();

    // The dag-scope members only: a node's dispatch is the `worker`, and it is
    // given that node's task, which is a different composition entirely.
    let monitor = world.turn_of("monitor");
    let reporter = world.turn_of(REPORTING_MEMBER);

    for (member, prompt) in [("monitor", &monitor), (REPORTING_MEMBER, &reporter)] {
        // What the run is, and what it is for. `plan_of` states the goal, so a
        // member that never received it is one the run description did not reach.
        for expected in ["neutral", "Deliver neutral"] {
            assert!(
                prompt.contains(expected),
                "member '{member}' was not told {expected:?}: {prompt}"
            );
        }
    }
    // And each member's job is its own: the reporter carrying the monitor's is
    // the defect.
    assert!(
        monitor.contains("Observe this run"),
        "the monitor was not given its own job: {monitor}"
    );
    assert!(
        !reporter.contains("Observe this run"),
        "a member whose job is not the monitor's was given it: {reporter}"
    );
    assert!(
        reporter.contains("Report on this run"),
        "the reporter was not given its own job: {reporter}"
    );
}

/// The retained driver relays its graph's stream and answers with its code.
///
/// `drive` is what `start --detach` spawns of this binary, and it is the whole
/// reason a detached launch composes the same `oneagentgraph` an attached one
/// does. Nothing but the launcher types it, so `--help` gives no one a reason to
/// notice it broke — and a launcher reads two things off it: the NDJSON on its
/// stdout, which is how the announcement and every later envelope arrive, and
/// its exit status, which is the graph's own answer rather than a second opinion
/// about it.
#[test]
fn the_retained_driver_relays_its_graphs_stream_and_exits_with_its_code() {
    let world = World::new("drive-relay");
    world.write_graphs();
    let graph = world.graphs().join("node-scope.yaml");
    let dir = world.root.join("driven");
    std::fs::create_dir_all(&dir).expect("a directory for the driven graph");

    let driven = world.run_on(
        world.agentgraph_cmd(&[
            "drive",
            &graph.to_string_lossy(),
            "--task",
            "Do the work and settle.",
            "--dir",
            &dir.to_string_lossy(),
        ]),
        "drive node-scope",
    );
    driven.exited(0);

    let relayed: Vec<Value> = driven
        .stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("`drive` wrote a line that is not an envelope: {error}\n{line}")
            })
        })
        .collect();
    assert!(
        relayed
            .iter()
            .any(|event| event["kind"] == "member-started"),
        "the relay carried no member-started:\n{}",
        driven.stdout
    );
    assert!(
        relayed
            .iter()
            .all(|event| event["source"] == "agentgraph" && event["v"] == 1),
        "the relay rewrote the envelopes it was given:\n{}",
        driven.stdout
    );
}

/// A relay that cannot write says so, rather than reporting a run that settled.
///
/// The launcher points a retained driver's stdout at a file and reads its
/// evidence back out of it, so a write that fails is a full disk under a live
/// run. What must not happen is that the driver swallows it and exits 0: the
/// launcher would record a graph that ran and said nothing, which is
/// indistinguishable from one that had nothing to say.
///
/// `/dev/full` is the deterministic version of a full disk — every write to it
/// fails with `ENOSPC` — and it is Linux's, which is why this is scoped to it.
#[cfg(target_os = "linux")]
#[test]
fn a_retained_driver_that_cannot_write_its_relay_refuses() {
    let world = World::new("drive-nospace");
    world.write_graphs();
    let graph = world.graphs().join("node-scope.yaml");
    let dir = world.root.join("driven");
    std::fs::create_dir_all(&dir).expect("a directory for the driven graph");

    let mut command = world.agentgraph_cmd(&[
        "drive",
        &graph.to_string_lossy(),
        "--task",
        "Do the work and settle.",
        "--dir",
        &dir.to_string_lossy(),
    ]);
    command.stdout(
        std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .expect("/dev/full"),
    );
    let refused = world.run_on(command, "drive onto a full disk");
    assert_ne!(
        refused.code, 0,
        "a driver that could not relay its own stream reported success:\n{}",
        refused.stderr
    );
    refused.err_has("relaying graph event");
}

/// A graph whose member fails reaches the launcher as its own exit code.
///
/// The retained driver is the only thing between a failing graph and the launch
/// log an operator reads afterwards, and it must not improve on what it saw: the
/// exit status is the graph's own answer rather than a second opinion about it.
/// A driver that exited 0 here would hand the launcher a run that started and
/// settled — the silent total failure this whole change is about.
///
/// The member fails *after* it has started and streamed, which is the case the
/// settlement decides: a graph that refuses before it runs never reaches the
/// settlement at all, because the relay carries that refusal out of the event
/// loop instead.
#[test]
fn a_retained_driver_carries_a_failing_graphs_own_exit_code() {
    let world = World::new("drive-failed");
    world.write_graphs();
    let graph = world.graphs().join("node-scope.yaml");
    let dir = world.root.join("driven-failed");
    std::fs::create_dir_all(&dir).expect("a directory for the driven graph");

    // A turn that ran and did not get there: it starts, streams, and settles on
    // a non-zero exit paired with a `turn_failed` report, which is the shape a
    // caller reading the graph's settlement actually sees.
    world.script("harness.fail", "the turn did not get there");
    let failed = world.run_on(
        world.agentgraph_cmd(&[
            "drive",
            &graph.to_string_lossy(),
            "--task",
            "Do the work and settle.",
            "--dir",
            &dir.to_string_lossy(),
        ]),
        "drive a graph whose member fails",
    );
    // The graph's *own* code, not merely "not success": a launcher reading this
    // process's exit reads the sibling's answer, and the sibling answers a member
    // that failed with this one.
    assert_eq!(
        failed.code,
        oneagentgraph::error::EXIT_MEMBER_FAILED,
        "a driver did not carry its graph's own exit code:\nstdout: {}\nstderr: {}",
        failed.stdout,
        failed.stderr
    );
    // It really ran: the member started and streamed before it failed, so this
    // is the settlement's answer rather than a refusal on the way in.
    assert!(
        failed
            .stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|event| event["kind"] == "member-started"),
        "the graph never started a member, so its code is not a settlement:\n{}",
        failed.stdout
    );
}

/// The turn ceiling the dispatch of `node` — or of one of its steps — was
/// actually handed.
///
/// Read through the run's **own merged stream**, which is this crate's published
/// surface: `oneagentgraph` publishes the configuration path it launched a member
/// with on that member's `member-started`, so the run says which file each
/// dispatch was given rather than a test guessing at one. That file is the
/// sibling's *effective* configuration for the member — its base config, the
/// persona delta, and every `--set` applied, resolved by the sibling itself —
/// and it is what onejudge is handed. Nothing else offline can state a turn
/// ceiling: a two-party member needs a provider turn to spend one, and this
/// suite has no stand-in for a paid turn.
fn turns_dispatched(world: &World, run: &str, node: &str, step: Option<&str>) -> u64 {
    let events = world.journal(run);
    let started = events
        .iter()
        .filter(|event| event["kind"] == "member-started")
        .find(|event| {
            event["labels"]["onepipeline.node"] == node
                && step.is_none_or(|step| event["labels"]["onepipeline.step"] == step)
        })
        .unwrap_or_else(|| panic!("no member started for {node}/{step:?}: {events:?}"));
    let config = started["payload"]["config"]
        .as_str()
        .expect("the sibling publishes the config it launched the member with");
    let text = std::fs::read_to_string(config).expect("that configuration is on disk");
    let effective: Value = serde_norway::from_str(&text).expect("it parses");
    effective["user"]["max_turns"]
        .as_u64()
        .unwrap_or_else(|| panic!("{config} states no turn ceiling: {text}"))
}

/// A two-party node-scope graph, as the shipped one is, with a base config that
/// states the default turn ceiling every member starts from.
///
/// `12` is that default deliberately: it is the number a node declaring `45` was
/// silently collapsed to for the whole life of the defect this proves fixed.
fn write_supervised_node_graph(world: &World) {
    std::fs::write(
        world.graphs().join("onejudge.base.yaml"),
        "agent:\n  instructions: Do the work.\nuser:\n  persona: Review it.\n  \
         done_when: the original task is complete\n  max_turns: 12\n",
    )
    .expect("the onejudge base config is written");
    std::fs::write(
        world.graphs().join("node-scope.yaml"),
        "version: 1\nname: node-scope\nmembers:\n  worker:\n    kind: onejudge\n    \
         base_config: ./onejudge.base.yaml\n    agent:\n      \
         oneharness_config: ./oneharness.toml\n    judge:\n      \
         oneharness_config: ./oneharness.toml\n    mode: bypass\n",
    )
    .expect("the node-scope graph is written");
}

/// One persona file, so a two-party member has a delta to resolve.
fn write_persona(world: &World, name: &str) {
    std::fs::write(
        world.graphs().join(format!("{name}.yaml")),
        format!("agent:\n  name: {name}\n  instructions: Ship it.\nuser:\n  persona: Review it.\n"),
    )
    .expect("the persona is written");
}

/// A node's turn budget reaches the configuration its dispatch is handed, and
/// beats the run-wide override an operator set.
///
/// Three values are distinct on purpose: the base config's `12`, the operator's
/// run-wide `9`, and the node's own `45`. A budget that never left this crate
/// reads as `12` — which is exactly what the defect this fixes did — one that
/// lost to the run-wide override reads as `9`, and only forwarding it as the more
/// specific of the two reads as `45`.
///
/// One node per run, and the runs are sequential: a node-scope graph run is named
/// for the millisecond and process that minted it, so two dispatched together
/// from one process can collide over the sibling's state directory. That is a
/// fault of its own and not this journey's subject.
#[test]
fn a_nodes_turn_budget_reaches_its_dispatch_and_outranks_the_run_wide_one() {
    let world = World::new("real-turn-budget");
    world.write_graphs();
    write_supervised_node_graph(&world);
    for persona in ["budgeted", "plain"] {
        write_persona(&world, persona);
    }

    let dispatched = |run: &str, node: Value| {
        let path = world.plan(run, &plan_of(run, vec![node]));
        world
            .run_on_agentgraph(&[
                "start",
                &path.to_string_lossy(),
                "--attach",
                "--node-set",
                "members.worker.max_turns=9",
            ])
            .settled();
        turns_dispatched(&world, run, run, None)
    };

    let mut budgeted = agent("budgeted", &[]);
    budgeted["persona"] = Value::from("./budgeted.yaml");
    budgeted["max_turns"] = json!(45);
    let mut plain = agent("plain", &[]);
    plain["persona"] = Value::from("./plain.yaml");

    assert_eq!(
        dispatched("budgeted", budgeted),
        45,
        "the node's own turn budget did not reach the member that runs its work"
    );
    assert_eq!(
        dispatched("plain", plain),
        9,
        "the operator's run-wide override did not reach a node that declared none"
    );
}

/// The directory a two-party member was started in, read off the run's own
/// merged stream.
///
/// `oneagentgraph` publishes each member's prepared launch on its
/// `member-started`, and for a `kind: onejudge` member that launch names the
/// `worktree` it hands onejudge — which onejudge puts on the agent side's
/// `oneharness run --cwd`. So the directory the member really works in is a fact
/// of this crate's published surface, in the store `docs/contract.md` defines,
/// rather than something recovered from a process nobody kept.
///
/// The two-party member is picked out by the engine its launch names: a
/// single-sided member is a process of its own and its record carries `cwd`
/// instead.
fn two_party_worktree(world: &World, run: &str) -> String {
    let started: Vec<Value> = world
        .journal(run)
        .into_iter()
        .filter(|event| event["kind"] == "member-started")
        .collect();
    started
        .iter()
        .find(|event| event["payload"]["engine"] == "onejudge")
        .and_then(|event| event["payload"]["worktree"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("no two-party member was started in {run}: {started:#?}"))
}

/// A two-party member is started in the directory the graph was given.
///
/// The whole of what a `kind: onejudge` member is *for* rests on this. Its agent
/// side does the node's work, and a lifecycle node's work is in a repository — so
/// a member started anywhere else has to guess where its checkout is, and
/// whatever it writes to the one it guesses is never seen again: publication
/// reads the session's own branch and nothing else. `oneagentgraph` below 0.2.12
/// started that side in the member's own scratch,
/// `<state>/runs/<graph run>/members/<member>`, which is not a repository at all.
///
/// Held against the **session's own worktree** — what this crate hands the
/// sibling for a lifecycle node — rather than against a directory this test
/// names, so what is asserted is the composition and not a literal. Both values
/// come off the run's merged store, and the sibling that wrote one of them is
/// real: a dependency that is merely *pinned* rather than linked cannot pass
/// this.
///
/// What this journey deliberately does not assert is a *settled* two-party
/// member. onejudge spawns the agent side as `oneharness run … --prompt-file -`,
/// which `fake-oneharness` does not speak — it stands in for the single-sided
/// invocation `oneagentgraph` builds itself — so the conversation cannot complete
/// offline. The launch is the fact under test and it is published before the turn
/// runs, which is why every two-party journey in this file reads one.
#[test]
fn a_two_party_member_is_started_in_the_directory_the_graph_was_given() {
    let world = World::new("real-two-party-cwd");
    world.write_graphs();
    write_supervised_node_graph(&world);
    write_persona(&world, "engineer");
    world.repository("local-direct", &["true"]);

    let node = json!({
        "id": "service",
        "repo": "service",
        "persona": "./engineer.yaml",
        "task": "## What\nship the thing",
        // Its own title, so the run spends no `pr-author` dispatch: that one has
        // nothing to say about where a member works.
        "title": "feat: land what the member made",
    });
    let path = world.plan("twoparty", &plan_of("twoparty", vec![node]));
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--attach"])
        .settled();

    // Where `onevcs` cut this node's worktree, read off the run's own record of
    // opening the session rather than reconstructed from the sibling's layout.
    // Compared as the two sides spell it rather than as the filesystem resolves
    // it: the session is closed by the time this runs and its worktree is gone,
    // so there is nothing left to canonicalise. Both spellings descend from the
    // world root, which `World::new` already resolved, so there is one spelling
    // of this directory on every platform.
    let session_worktree = world
        .journal("twoparty")
        .into_iter()
        .filter(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .find_map(|event| event["payload"]["worktree"].as_str().map(str::to_string))
        .expect("the lifecycle node's session opened a worktree");

    assert_eq!(
        two_party_worktree(&world, "twoparty"),
        session_worktree,
        "the two-party member was started somewhere other than the directory the graph was \
         given. A member started in its own scratch has no repository to work in, and the work \
         it leaves there is discarded at publication as `no-changes`."
    );
}

/// A step's turn budget reaches that step's own dispatch.
///
/// A workstream's steps are dispatched one at a time on one branch, each with its
/// own persona and its own controls — and a node that declares steps may not
/// declare a budget at all, so the step's is the only budget there is. The
/// repository side is real too: the step runs in a `onevcs` session.
#[test]
fn a_steps_turn_budget_reaches_that_steps_own_dispatch() {
    let world = World::new("real-step-budget");
    world.write_graphs();
    write_supervised_node_graph(&world);
    write_persona(&world, "implementer");
    world.repository("local-direct", &["true"]);

    let node = json!({
        "id": "service",
        "repo": "service",
        // Its own title, so the run spends no `pr-author` dispatch: that one runs
        // under a persona this world has not written, and this journey is about
        // the step.
        "title": "feat: land what the step made",
        "steps": [
            {"id": "implement", "persona": "./implementer.yaml", "task": "## What\nimplement",
             "max_turns": 45},
        ],
    });
    let path = world.plan("stepbudget", &plan_of("stepbudget", vec![node]));
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--attach"])
        .settled();

    assert_eq!(
        turns_dispatched(&world, "stepbudget", "service", Some("implement")),
        45,
        "the step's own turn budget did not reach the dispatch that ran it; the graph's \
         own default is 12"
    );
}

/// `filters.agentgraph` reaches every `oneagentgraph` launch the run starts.
///
/// The real sibling is what filters here: the launch is handed the filter on its
/// own `--event-filter` / `events.filter` surface, so the events never reach this
/// crate at all — which is the point, since a run that relayed them and dropped
/// them would still have paid to relay them.
///
/// Read against a control run in the same world that names no `filters:` block,
/// because "narrowed" is a comparison: the same plan, the same graph, and the
/// same real member, ingested twice.
#[test]
fn a_launchs_agentgraph_filter_reaches_the_real_sibling_and_narrows_what_it_relays() {
    let world = World::new("real-agentgraph-filter");
    world.write_graphs();

    // Read through `monitor --all`, which is the unfiltered view of the merged
    // store a person opens — so what this asserts is what a reader of the run
    // sees, rather than what a file under the run directory happens to hold.
    let relayed = |run: &str| -> String { world.run(&["monitor", run, "--all"]).stdout };

    // No `filters:` block at all: ingestion is what it always was.
    let path = world.plan(
        "unfiltered",
        &plan_of("unfiltered", vec![agent("build", &[])]),
    );
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--attach"])
        .settled();
    let ingested = relayed("unfiltered");
    for kind in ["turn-activity", "member-settled"] {
        assert!(
            ingested.contains(kind),
            "a launch naming no filters did not ingest {kind}:\n{ingested}"
        );
    }

    let path = world.plan("filtered", &plan_of("filtered", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--filter-agentgraph",
            r#"{"exclude": [{"kind": "turn-*"}]}"#,
        ])
        .settled();

    let kinds = relayed("filtered");
    assert!(
        !kinds.contains("turn-"),
        "the source filter did not reach `oneagentgraph`:\n{kinds}"
    );
    // Narrowed, not silenced, and the run still settled on what the member did —
    // a filter says what is emitted, never what the run acts on.
    assert!(
        kinds.contains("member-settled"),
        "the source filter dropped the settlement, which it admits:\n{kinds}"
    );
    world
        .run(&["results", "filtered"])
        .exited(0)
        .out_has("build")
        .out_has("done");
}

/// The observer graph is one of the run's `oneagentgraph` launches too, and the
/// spec may be a file.
///
/// Two things the journey above leaves out, and both are paths an operator
/// reaches: `--dag-graph` starts a *second* graph, launched from somewhere else
/// in this crate entirely, and a filter long enough to be worth writing down is
/// kept in a file and named by path rather than pasted onto one line of argv.
///
/// The observer's envelopes are the ones carrying no `node` label: a node-scope
/// dispatch is stamped with the node it is running and the observer is stamped
/// with the run alone, because it is watching all of them.
#[test]
fn the_observer_graphs_own_stream_is_filtered_too_and_the_spec_may_be_a_file() {
    let world = World::new("real-observer-filter");
    world.write_graphs();

    let spec = world.root.join("relay.json");
    std::fs::write(&spec, r#"{"exclude": [{"kind": "turn-*"}]}"#).expect("the spec is written");

    // llmlint: ignore-block[tests_mirror_real_usage] the claim is about *which of the
    // run's two graph launches* relayed a record, and the label that distinguishes them
    // — a node-scope dispatch is stamped with its node, the observer with the run alone
    // — is not rendered by any view: `monitor` gives every agentgraph line the same
    // `agent:{stream}` id, and the filter grammar has no way to ask for an *absent*
    // label. The merged store is the contract's own artifact ("envelope NDJSON, one
    // store per run"), and it is where this distinction exists to be read.
    let observed = |run: &str| -> Vec<String> {
        world
            .journal(run)
            .iter()
            .filter(|event| event["source"] == "agentgraph" && event["labels"]["node"].is_null())
            .filter_map(|event| event["kind"].as_str().map(str::to_string))
            .collect()
    };
    // llmlint: ignore-end[tests_mirror_real_usage]

    let path = world.plan("watched", &plan_of("watched", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--dag-graph",
            &world.dag_graph(),
        ])
        .settled();
    let ingested = observed("watched");
    assert!(
        ingested.iter().any(|kind| kind.starts_with("turn-")),
        "the observer graph relayed no turn of its own, so this journey could not \
         tell a filtered observer from a quiet one: {ingested:?}\n{}",
        world.dump()
    );

    let path = world.plan("quiet", &plan_of("quiet", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--dag-graph",
            &world.dag_graph(),
            "--filter-agentgraph",
            &spec.to_string_lossy(),
        ])
        .settled();

    let kinds = observed("quiet");
    assert!(
        !kinds.iter().any(|kind| kind.starts_with("turn-")),
        "the source filter did not reach the observer graph's own launch: {kinds:?}"
    );
    // And it is the filter that did it, rather than an observer that never ran:
    // the launch it made is recorded, and the graph it started announced itself.
    assert!(
        !kinds.is_empty(),
        "the observer graph relayed nothing at all, so nothing here is about the filter"
    );
}

/// `adopt` replays the launch's source filter onto the observer it relaunches.
///
/// An adoption starts a **fresh** graph run, from a different process and often
/// from a different directory, so the filter has to come off the launch record
/// rather than off the command line nobody typed this time. A run that filtered
/// its observer until its first driver died, and then relayed everything after
/// it was adopted, would be a run whose ingestion depends on how many drivers it
/// has had.
#[test]
fn an_adoption_relaunches_the_observer_under_the_launchs_own_filter() {
    let world = World::new("real-adopt-filter");
    world.write_graphs();

    // llmlint: ignore-block[tests_mirror_real_usage] the claim is about *which of the
    // run's two graph launches* relayed a record, and the label that distinguishes them
    // — a node-scope dispatch is stamped with its node, the observer with the run alone
    // — is not rendered by any view: `monitor` gives every agentgraph line the same
    // `agent:{stream}` id, and the filter grammar has no way to ask for an *absent*
    // label. The merged store is the contract's own artifact ("envelope NDJSON, one
    // store per run"), and it is where this distinction exists to be read.
    let observed = |run: &str| -> Vec<String> {
        world
            .journal(run)
            .iter()
            .filter(|event| event["source"] == "agentgraph" && event["labels"]["node"].is_null())
            .filter_map(|event| event["kind"].as_str().map(str::to_string))
            .collect()
    };
    // llmlint: ignore-end[tests_mirror_real_usage]

    // A human gate ahead of the work, so the launch settles undriven with the
    // node still to run — which is the state an `adopt` picks up.
    let path = world.plan(
        "readopted",
        &plan_of(
            "readopted",
            vec![human("approve", &[]), agent("build", &["approve"])],
        ),
    );
    world
        .run_on_agentgraph(&[
            "start",
            &path.to_string_lossy(),
            "--attach",
            "--dag-graph",
            &world.dag_graph(),
            "--filter-agentgraph",
            r#"{"exclude": [{"kind": "turn-*"}]}"#,
        ])
        .exited(0);
    world.run(&["attest", "readopted", "approve"]).exited(0);
    let before = observed("readopted").len();

    world
        .run_on_agentgraph(&["adopt", "readopted"])
        .exited(0)
        .settled();

    let kinds = observed("readopted");
    assert!(
        kinds.len() > before,
        "the adoption relaunched no observer, so nothing here is about its filter: {kinds:?}"
    );
    assert!(
        !kinds.iter().any(|kind| kind.starts_with("turn-")),
        "the adoption relaunched the observer without the launch's own filter: {kinds:?}"
    );
}

/// The retained driver reads its own `--event-filter`, and refuses one it could
/// not honour.
///
/// `drive` is the process a detached launch retains, so the spec crosses a
/// process boundary as text and becomes a value again on the far side. A driver
/// that took a spec it could not honour would be a detached run relaying
/// everything, with the refusal in a stream nobody read — and the launcher that
/// started it already gone.
#[test]
fn the_retained_driver_reads_its_own_event_filter_and_refuses_an_unusable_one() {
    let world = World::new("drive-filter");
    world.write_graphs();
    let graph = world.graphs().join("node-scope.yaml");
    let dir = world.root.join("driven");
    std::fs::create_dir_all(&dir).expect("a directory for the driven graph");

    let drive = |spec: &str| {
        world.run_on(
            world.agentgraph_cmd(&[
                "drive",
                &graph.to_string_lossy(),
                "--task",
                "Do the work and settle.",
                "--dir",
                &dir.to_string_lossy(),
                "--event-filter",
                spec,
            ]),
            "drive with an event filter",
        )
    };

    // A spec this build says it will not honour, refused before a graph starts.
    let refused = drive(r#"{"include": [{"role": "agent"}]}"#);
    assert_eq!(refused.code, REFUSED, "{}", refused.stderr);
    assert!(
        refused.stderr.contains("role"),
        "the refusal does not name the offending field:\n{}",
        refused.stderr
    );

    // And one it can honour reaches the graph it drives: the relay is this
    // process's stdout, so what the filter left out is simply not on it.
    let driven = drive(r#"{"exclude": [{"kind": "turn-*"}]}"#);
    assert_eq!(driven.code, 0, "{}", driven.stderr);
    assert!(
        !driven.stdout.contains("turn-activity"),
        "the retained driver relayed what its filter excluded:\n{}",
        driven.stdout
    );
    assert!(
        driven.stdout.contains("member-settled"),
        "the retained driver relayed nothing at all, so nothing here is about the \
         filter:\n{}",
        driven.stdout
    );
}
