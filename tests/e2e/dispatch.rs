//! The `oneagentgraph` seam, against the real `oneagentgraph`.
//!
//! Every other journey here substitutes that sibling wholesale, which is what
//! let a run report success while the sibling was refusing every dispatch it was
//! sent: the double accepted a `--label` the real CLI reserves. The journeys
//! here close that gap from the other side — the real binary resolves the
//! graph, supervises the member, and stamps the stream, and the only thing
//! standing in is the paid model turn, replaced at that library's own
//! `ONEAGENTGRAPH_ONEHARNESS_BIN` override.

// llmlint: ignore-file[e2e_not_mocked] the layer under test is this crate's dispatch
// *through* `oneagentgraph`, and that layer is real here: the sibling's own compiled
// binary resolves the graph, prepares the member, supervises it, and stamps the stream.
// What stands in is one layer below it — the paid model turn `oneagentgraph` itself
// spawns, swapped at that library's own documented `ONEAGENTGRAPH_ONEHARNESS_BIN`
// override, which knows nothing about this crate. There is no offline stand-in for a
// provider turn, and these journeys run inside `just check`, which has neither a
// credential nor a budget for one.

use crate::harness::{agent, human, plan_of, World};
use serde_json::{json, Value};

/// The prose a `member-started` says its member was launched with.
///
/// `oneagentgraph` publishes the whole argv it prepared, so the task a member
/// was actually given is in the run's own merged store rather than only in the
/// process it was passed to.
fn prompt_of(event: &Value) -> Option<String> {
    let args = event["payload"]["args"].as_array()?;
    let at = args.iter().position(|arg| arg == "--prompt")?;
    args.get(at + 1)?.as_str().map(str::to_string)
}

fn open_second_round(world: &World, run: &str, node: Value) {
    world.script("driver.wait", "hold");
    let path = world.plan(run, &plan_of(run, vec![human("approve", &[]), node]));
    world
        .run(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.run(&["round", "run", run]).exited(1);
    world.run(&["attest", run, "approve"]).exited(0);
    world
        .run(&["round", "next", run])
        .exited(0)
        .out_has("continuing");
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
    let mut command = world.agentgraph_cmd(&["start", &path.to_string_lossy(), "--attach"]);
    command
        .current_dir(&world.root)
        .env_remove("ONEPIPELINE_DAG_GRAPH")
        .env_remove("ONEPIPELINE_NODE_GRAPH");

    let started = world.run_on(command, "start relative defaults");
    started.exited(0).settled();
    assert!(
        world
            .journal("relative-defaults")
            .iter()
            .filter_map(prompt_of)
            .any(|prompt| prompt.contains("Do build.")),
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
    std::fs::copy(
        world.graphs().join("oneharness.toml"),
        world.root.join("oneharness.toml"),
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
    let mut start = world.cmd(&["start", &path.to_string_lossy(), "--detach"]);
    start.env("ONEPIPELINE_NODE_GRAPH", &launch_graph);
    world
        .run_on(start, "start recorded lifecycle graph")
        .exited(0);
    world
        .run(&["round", "run", "recorded-lifecycle-graph"])
        .exited(1);
    world
        .run(&["attest", "recorded-lifecycle-graph", "approve"])
        .exited(0);
    world
        .run(&["round", "next", "recorded-lifecycle-graph"])
        .exited(0);
    let mut round = world.cmd(&["round", "run", "recorded-lifecycle-graph"]);
    round.env("ONEPIPELINE_NODE_GRAPH", &later_graph);
    world
        .run_on(round, "round with changed live node graph")
        .exited(0);
    world.release("driver.go");

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
    let mut command = world.agentgraph_cmd(&["start", &path.to_string_lossy(), "--attach"]);
    command
        .current_dir(&world.root)
        .env("ONEPIPELINE_DAG_GRAPH", "graphs/missing-dag.yaml");

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
fn broken_launch_records_refuse_rounds_before_direct_or_lifecycle_dispatch() {
    // llmlint: ignore-block[tests_mirror_real_usage] no CLI command corrupts or removes
    // its own ledger. These are external-state faults (partial write or cleanup), so the
    // arrangement mutates that persisted boundary; every observation and asserted
    // refusal still goes through the compiled CLI.
    let direct = World::new("corrupt-launch-direct");
    let mut build = agent("build", &["approve"]);
    build["deps"] = json!(["approve"]);
    open_second_round(&direct, "corrupt-direct", build);
    std::fs::write(direct.run_file("corrupt-direct", "launch.json"), "not json")
        .expect("the launch record is corrupted");
    direct
        .run(&["round", "run", "corrupt-direct"])
        .exited(crate::harness::REFUSED)
        .err_has("launch.json");
    direct.release("driver.go");

    let lifecycle_world = World::new("missing-launch-lifecycle");
    lifecycle_world.repository("local-direct", &["true"]);
    let mut service = crate::harness::lifecycle("service", &["approve"]);
    service["deps"] = json!(["approve"]);
    open_second_round(&lifecycle_world, "missing-lifecycle", service);
    std::fs::remove_file(lifecycle_world.run_file("missing-lifecycle", "launch.json"))
        .expect("the launch record is removed");
    lifecycle_world
        .run(&["round", "run", "missing-lifecycle"])
        .exited(crate::harness::REFUSED)
        .err_has("launch.json");
    lifecycle_world.release("driver.go");
    // llmlint: ignore-end[tests_mirror_real_usage]
}

#[test]
fn a_legacy_launch_without_a_node_graph_fails_instead_of_reading_live_environment() {
    // llmlint: ignore-block[tests_mirror_real_usage] an older launch-record producer is
    // not a CLI operation this build can invoke. Writing that historical schema shape is
    // the necessary fault arrangement; the round and refusal use the compiled CLI.
    let world = World::new("legacy-empty-node-graph");
    let mut build = agent("build", &["approve"]);
    build["deps"] = json!(["approve"]);
    open_second_round(&world, "legacy-empty", build);
    let path = world.run_file("legacy-empty", "launch.json");
    let mut launch: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the launch record reads"))
            .expect("the launch record parses");
    launch["node_graph"] = json!("");
    std::fs::write(&path, serde_json::to_vec_pretty(&launch).unwrap())
        .expect("the legacy launch record is written");

    let mut round = world.cmd(&["round", "run", "legacy-empty"]);
    round.env(
        "ONEPIPELINE_NODE_GRAPH",
        world.graphs().join("node-scope.yaml"),
    );
    world
        .run_on(round, "round run legacy-empty")
        .exited(crate::harness::REFUSED)
        .err_has("has no resolved node graph");
    world.release("driver.go");
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
        "--set",
        "members.orchestrator.oneharness_config=./dag-override.toml",
        "--node-set",
        "members.worker.oneharness_config=./node-override.toml",
    ]);
    started.exited(0).settled();

    let configs: Vec<Value> = world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneharness-config")
        .collect();
    assert!(
        configs.iter().any(|call| {
            call["args"][0]
                .as_str()
                .is_some_and(|prompt| prompt.contains("onepipeline round run"))
                && call["args"][1]
                    .as_str()
                    .is_some_and(|config| config.contains("DAG_OVERRIDE"))
        }),
        "the running dag member did not receive its override: {configs:?}"
    );
    assert!(
        configs.iter().any(|call| {
            call["args"][0]
                .as_str()
                .is_some_and(|prompt| prompt.contains("Do build."))
                && call["args"][1]
                    .as_str()
                    .is_some_and(|config| config.contains("NODE_OVERRIDE"))
        }),
        "the running node member did not receive its override: {configs:?}"
    );
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

    let invocations: Vec<Value> = world
        .invocations()
        .into_iter()
        .filter(|call| {
            call["tool"] == "oneharness-config"
                && call["args"][0]
                    .as_str()
                    .is_some_and(|prompt| prompt.contains("Do review."))
        })
        .collect();
    assert!(
        !invocations.is_empty(),
        "the node's member never ran: {invocations:?}"
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
    world.script("driver.wait", "hold");
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
        "--detach",
        "--node-set",
        "members.worker.oneharness_config=./adopted-node.toml",
    ]);
    start
        .current_dir(&world.root)
        .env("ONEPIPELINE_DAG_GRAPH", "graphs/dag-scope.yaml")
        .env("ONEPIPELINE_NODE_GRAPH", "graphs/node-scope.yaml");
    world.run_on(start, "start adopted-override").exited(0);

    world.until("the original driver to park before dispatch", |world| {
        let mut status = world.agentgraph_cmd(&["status", "adopted-override"]);
        status.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
        String::from_utf8_lossy(&status.output().expect("status runs").stdout).contains("PARKED")
    });
    // Only the original driver saw this rendezvous. Removing its trigger lets
    // the fresh graph launched by adopt drive immediately, while the original
    // remains held until the assertion is complete.
    std::fs::remove_file(world.root.join("fakes/driver.wait")).expect("the adoption is not held");
    let mut adopt = world.agentgraph_cmd(&["adopt", "adopted-override"]);
    adopt
        .current_dir(&world.project)
        .env("ONEPIPELINE_DAG_GRAPH", "missing-dag.yaml")
        .env("ONEPIPELINE_NODE_GRAPH", "missing-node.yaml")
        .env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    let adopted = world.run_on(adopt, "adopt adopted-override");
    adopted.exited(0).settled();

    let configs = world.invocations();
    assert!(
        configs.iter().any(|call| {
            call["tool"] == "oneharness-config"
                && call["args"][0]
                    .as_str()
                    .is_some_and(|prompt| prompt.contains("Do build."))
                && call["args"][1]
                    .as_str()
                    .is_some_and(|config| config.contains("ADOPTED_NODE_OVERRIDE"))
        }),
        "the node dispatched after adoption did not run under its retained override: {configs:?}"
    );
    world.release("driver.go");
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

    let started = world.run_on_agentgraph(&["start", &path.to_string_lossy(), "--attach"]);
    started.exited(0).settled();
    let run = started.json()["run_id"]
        .as_str()
        .expect("the launch named its run")
        .to_string();

    // Two members really ran, and the run's own record is where that is
    // readable: `oneagentgraph` publishes the launch it prepared for each — the
    // program, its arguments, and the prose it was given — into the stream this
    // crate merges. Nothing here is asserted from anywhere a user could not look.
    let launches: Vec<String> = world
        .journal(&run)
        .iter()
        .filter(|event| event["kind"] == "member-started")
        .filter_map(prompt_of)
        .collect();
    assert!(
        launches
            .iter()
            .any(|task| task.contains("onepipeline round run")),
        "no member was launched to drive the run: {launches:?}"
    );
    assert!(
        launches.iter().any(|task| task.contains("Do build.")),
        "the node's own task never reached a member: {launches:?}"
    );

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
        world.run_json(&run, "round-01/result.json")["state"],
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
/// Mid-round, `status` used to say a node had been in flight for thirty-four
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
        !world.events_of("watched", "round-finished").is_empty()
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
        let started = world.run_on_agentgraph(&["start", &path.to_string_lossy(), form]);

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
    // A human action settles the round without a person, so the driver finishes
    // and the run is left intact and undriven — which is what `adopt` is for.
    let path = world.plan(
        "orphaned",
        &plan_of("orphaned", vec![human("approve", &[])]),
    );
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--detach"])
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
/// variable this crate restates — and reaches an interrupt's delivery too.
///
/// The third restated key. Pointed at something that is not there, the sibling
/// says so, and the failure names what it could not start: that is the variable
/// having been read. Asked through a real dispatch rather than a probe, because
/// what has to keep working is a member launch.
#[test]
fn the_sibling_still_takes_its_harness_from_the_variable_this_crate_restates() {
    let world = World::new("harness-bin-drift");
    world.write_graphs();
    let path = world.plan(
        "harness-bin",
        &plan_of("harness-bin", vec![agent("build", &[])]),
    );
    let mut command = world.agentgraph_cmd(&["start", &path.to_string_lossy(), "--attach"]);
    command.env(
        "ONEAGENTGRAPH_ONEHARNESS_BIN",
        "oneharness-that-is-not-installed",
    );
    let started = world.run_on(command, "start --attach");
    started.settled();

    let failed: Vec<_> = world
        .journal("harness-bin")
        .into_iter()
        .filter(|event| {
            let rendered = event.to_string();
            rendered.contains("oneharness-that-is-not-installed")
        })
        .collect();
    assert!(
        !failed.is_empty(),
        "no event named the harness the graph was told to drive, so the variable was not read \
         — it has drifted:\n{}",
        world.dump()
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
/// It replaces a characterisation of the defect it now holds the fix for: the
/// reset used to be addressed with **this** run's id, and a graph run has an id
/// of its own that `oneagentgraph` mints. The subprocess path sent the same
/// wrong argument and the double answered `0` to anything, so for as long as
/// that double existed nothing anywhere said so — the reset simply never
/// happened, and a `resettable` schedule quietly degraded to a fixed interval
/// that ignores everything the run is already telling the planner.
///
/// The reset is carried the whole way to where the sibling's own scheduler
/// watches for it: `oneagentgraph::run::signal` reads the run's record, refuses
/// a member that run never declared, and writes the signal under the run's own
/// directory. All three only work out for the id `oneagentgraph` minted, so a
/// reset that lands there is a reset that was addressed correctly — which is
/// exactly what this crate owns.
///
/// What happens to the signal *after* it lands is the sibling's half: it starts
/// a scheduled member's clock only once every member of that member's wave has
/// settled, and the orchestrator shares the pacemaker's wave and runs for the
/// whole run. See the report accompanying this change.
#[test]
fn consuming_a_surface_restarts_the_real_pacemakers_clock() {
    let world = World::new("real-pacemaker");
    world.write_graphs_with_pacemaker();
    let path = world.plan("paced", &plan_of("paced", vec![human("approve", &[])]));
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--detach"])
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
