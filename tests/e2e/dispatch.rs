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

#[cfg(unix)]
use crate::harness::end_process;

/// The bound on how many times one driver starts a run's observer graph again.
///
/// Spelled rather than imported: the crate under test keeps it in a private
/// module, and it is reached here exactly as an operator reaches it, through the
/// environment of the process being driven.
pub const OBSERVER_RESTARTS_ENV: &str = "ONEPIPELINE_OBSERVER_RESTARTS";

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

/// The same for a member dispatched once, which is every journey that does not
/// retry one.
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
    world.run(&["start", &path, "--attach"]).exited(0);
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
        &path,
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
    // test knows: each observer's `graph-started` carries the run id
    // `oneagentgraph` stamped on it, so a record naming anything else is a record
    // naming a run that never watched this one.
    //
    // Every one of them, because a driver starts another observer when the one
    // watching it stops: the record has to name each graph that has watched, in
    // the order they did, or a reader meeting one graph's records in this very
    // store could not say whose observer wrote them.
    let announced: Vec<String> = world
        .journal("relative-defaults")
        .into_iter()
        .filter(|event| event["kind"] == "graph-started" && event["labels"]["node"].is_null())
        .filter_map(|event| event["labels"]["run_id"].as_str().map(str::to_string))
        .collect();
    assert!(
        !announced.is_empty(),
        "no observer announced itself into the merged store"
    );
    let watched: Vec<String> = launch["observer_runs"]
        .as_array()
        .expect("the record names the graphs that have watched")
        .iter()
        .filter_map(|run| run.as_str().map(str::to_string))
        .collect();
    // A prefix rather than the whole list: the last graph this driver started
    // may have been asked to stop before its announcement reached the store,
    // which is a graph the record still has to name.
    assert!(
        watched.starts_with(&announced),
        "the record does not name the graphs that watched this run, in order: \
         {watched:?} against {announced:?}"
    );
    assert_eq!(
        launch["graph_run"],
        json!(watched.last()),
        "the run addresses a graph that is not the last one it started: {launch}"
    );
}

/// Plan-owned graph references have the same launch-directory semantics as
/// the defaults. Both levels actually dispatch through the real sibling: the
/// node graph runs the first lifecycle step and the step graph runs the second.
#[test]
fn relative_node_and_step_graph_overrides_dispatch_from_the_launch_directory() {
    let world = World::new("real-relative-plan-overrides");
    world.write_graphs();
    world.repository("local-direct", &[]);
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
        "title": "feat: land the workstream",
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
    let mut command = world.agentgraph_cmd(&["start", &path, "--attach"]);
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

/// Both graphs a lifecycle node dispatches under are the ones its **launch**
/// resolved, and a fresh driver replays them.
///
/// The node-scope graph its work runs under, and the pr-author graph its change
/// request's body is drafted by: each is resolved once, at `start`, against the
/// directory the operator launched from, and recorded. `adopt` runs from
/// somewhere else, under an environment naming a *different* node graph — what
/// the dispatches run under is the launch record either way.
#[test]
fn a_lifecycle_nodes_two_graphs_are_the_ones_its_launch_resolved() {
    let world = World::new("lifecycle-recorded-default-graph");
    world.repository("local-direct", &[]);
    world.script("driver.wait", "hold");
    world.script("service.work", "the worker wrote this\n");
    let launch_graph = crate::harness::repo_file("graphs/node-scope.yaml");
    let later_graph = world.root.join("later-node-scope.yaml");
    std::fs::copy(&launch_graph, &later_graph).expect("the later graph is written");
    let drafting = world.pr_author_graph();
    let mut service = crate::harness::lifecycle("service", &["approve"]);
    service["deps"] = json!(["approve"]);
    let path = world.plan(
        "recorded-lifecycle-graph",
        &plan_of(
            "recorded-lifecycle-graph",
            vec![human("approve", &[]), service],
        ),
    );
    let mut start = world.cmd(&["start", &path, "--attach", "--pr-author-graph", &drafting]);
    start.env("ONEPIPELINE_NODE_GRAPH", &launch_graph);
    world
        .run_on(start, "start recorded lifecycle graph")
        .exited(0);
    // Recorded, which is what makes it replayable at all: a launcher that
    // resolved the reference and kept it to itself would leave every later
    // driver to guess.
    assert_eq!(
        world.run_json("recorded-lifecycle-graph", "launch.json")["pr_author_graph"],
        json!(drafting),
        "the launch record does not name the graph the launch was given"
    );
    world
        .run(&["attest", "recorded-lifecycle-graph", "approve"])
        .exited(0);
    // A fresh driver, under an environment naming a *different* node graph: what
    // the dispatches run under is the state the launch recorded, never whatever
    // this process happens to be pointed at.
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
    let under = |persona: &str| -> Vec<&Value> {
        relevant
            .iter()
            .filter(|call| {
                call["args"]
                    .as_array()
                    .is_some_and(|args| args.iter().any(|arg| arg == persona))
            })
            .copied()
            .collect()
    };
    let drafts = under("onepipeline.persona=pr-author");
    assert_eq!(
        drafts.len(),
        1,
        "the body drafting dispatch did not run after adoption: {relevant:?}"
    );
    assert_eq!(
        drafts[0]["args"][1], drafting,
        "the drafting dispatch ran a graph the launch did not record: {drafts:?}"
    );
    let worked = under("onepipeline.persona=engineer");
    assert!(
        !worked.is_empty(),
        "the node never dispatched: {relevant:?}"
    );
    assert!(
        worked
            .iter()
            .all(|call| call["args"][1] == launch_graph.to_string_lossy().as_ref()),
        "a lifecycle dispatch re-read the live graph instead of launch state: {worked:?}"
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
        &path,
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

/// A graph reference that is *there* and holds nothing is refused by that, and
/// refused identically wherever the launch runs.
///
/// `""` joined onto the launch directory **is** the launch directory, so a
/// reference holding nothing used to mean whatever the host answers for opening
/// a directory — read on Linux, refused on Windows — and neither of those is
/// what the operator wrote. Refused before anything is joined or opened, so the
/// ending is a property of the launch rather than of the platform it ran on.
/// What names no graph at all names none; both spellings driven here, because a
/// blank arrives from argv and from a plan a planner templated alike.
#[test]
fn a_blank_graph_reference_is_refused_before_any_path_is_read() {
    let world = World::new("blank-graph-reference");
    world.write_graphs();
    let cases: [(&str, Vec<String>, Value); 2] = [
        (
            "blank-observer",
            vec!["--dag-graph".to_string(), String::new()],
            agent("build", &[]),
        ),
        (
            "blank-node-override",
            Vec::new(),
            json!({
                "id": "build",
                "persona": "engineer",
                "task": "## What\nbuild",
                "agent_graph": "",
            }),
        ),
    ];

    for (name, extra, node) in cases {
        let path = world.plan(name, &plan_of(name, vec![node]));
        let mut args = vec!["start".to_string(), path.clone(), "--attach".to_string()];
        args.extend(extra);
        let mut command =
            world.agentgraph_cmd(&args.iter().map(String::as_str).collect::<Vec<_>>());
        command.current_dir(&world.root);

        let failed = world.run_on(command, "start with a blank graph reference");
        failed.exited(REFUSED);
        failed.err_has("graph reference is blank");
        assert!(
            !world.run_file(name, "launch.json").exists(),
            "{name} minted a run for a reference nothing could resolve"
        );
    }
}

#[test]
fn an_unreadable_relative_node_graph_names_its_launch_base() {
    let world = World::new("relative-node-graph-error");
    world.write_graphs();
    let path = world.plan(
        "relative-node-error",
        &plan_of("relative-node-error", vec![agent("build", &[])]),
    );
    let mut command = world.agentgraph_cmd(&["start", &path, "--attach"]);
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
    world.repository("local-direct", &[]);
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
                "title": "feat: land the workstream",
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
        let mut command = world.agentgraph_cmd(&["start", &path, "--attach"]);
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
    lifecycle_world.repository("local-direct", &[]);
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

/// A run launched before projects replaced plan paths remains operable.
// llmlint: ignore-block[tests_mirror_real_usage] an older launch-record producer is not a
// CLI operation this build can invoke. Replacing only the source field with the exact
// historical `plan` spelling arranges persisted pre-upgrade input; the current public
// `status` and `adopt` commands drive the compatibility behavior under test.
#[test]
fn a_legacy_plan_path_launch_record_is_still_reportable_and_adoptable() {
    let world = World::new("legacy-plan-launch-record");
    let mut build = agent("build", &["approve"]);
    build["deps"] = json!(["approve"]);
    ready_and_undriven(&world, "legacy-plan", build);

    let path = world.run_file("legacy-plan", "launch.json");
    let mut launch = world.run_json("legacy-plan", "launch.json");
    launch
        .as_object_mut()
        .expect("a launch record")
        .remove("project");
    launch["plan"] = json!("/retired/plan.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&launch).unwrap())
        .expect("the historical launch record is installed");

    world.run(&["status", "legacy-plan"]).exited(0);
    world
        .run(&["adopt", "legacy-plan"])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");
}
// llmlint: ignore-end[tests_mirror_real_usage]

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
        &path,
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
/// The real sibling's content-addressed run record, read back through the
/// sibling's own `history` reader, proves that it resolved the requested persona
/// while preparing the member that subsequently ran. That is evidence from the
/// actual graph invocation, not this crate's event label.
#[test]
fn a_plan_persona_reaches_the_member_that_actually_runs() {
    let world = World::new("real-plan-persona");
    world.write_graphs();
    std::fs::write(
        world.graphs().join("requested-reviewer.yaml"),
        "name: requested-reviewer\nsystem_prompt: Review the change.\nuser:\n  persona: Demand evidence.\n",
    )
    .expect("the requested persona is written");
    let mut node = agent("review", &[]);
    node["persona"] = Value::from("./requested-reviewer.yaml");
    let path = world.plan("plan-persona", &plan_of("plan-persona", vec![node]));

    // Held open, and launched detached, so the record is read at the one moment
    // it has no writer. The sibling resolves every ref and records the inventory
    // *before* it starts a member, and replaces that record in place once more as
    // the run settles — on a process the dispatch's own teardown is by then
    // entitled to end. A journey that read afterwards was racing that
    // replacement, and a record it caught part way through parses as nothing,
    // which reads exactly like a persona that never arrived: that is how this
    // journey failed twice on a loaded runner while the persona had in fact
    // reached the member. With the turn held there is no writer left to race —
    // the inventory is on disk and nothing touches it again until this journey
    // lets the turn go.
    world.script("turn.hold", "");
    let launch = world.agentgraph_cmd(&["start", &path, "--detach"]);
    world.run_on(launch, "start plan-persona").exited(0);
    world.until("the node's member to take its turn", |world| {
        world
            .turns()
            .iter()
            .any(|turn| turn.prompt.contains("Do review."))
    });

    // Read the record back through `oneagentgraph history`, which is the reader
    // the sibling publishes for it — `history` lists the runs under the state
    // directory and `history show` prints one record whole. Nothing here opens a
    // file under that directory: where the record is kept, and whether it is a
    // file at all, is the sibling's to change.
    let state = world.graph_state();
    let sibling = |args: &[&str]| -> std::process::Output {
        std::process::Command::new(crate::harness::oneagentgraph_binary())
            .args(args)
            .env("ONEAGENTGRAPH_STATE_DIR", &state)
            .output()
            .expect("the real oneagentgraph runs")
    };
    // Each read is asserted rather than skipped on, because the member is running
    // and the run it belongs to therefore exists: a listing that answers with
    // nothing, and a record the sibling cannot print, are two different faults and
    // neither of them is a persona that failed to arrive. Reported as themselves,
    // with what the sibling said, so a failure here names the fault it is.
    let listed = sibling(&["history"]);
    let runs: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter_map(|line| line.split('\t').next().map(str::to_string))
        .collect();
    assert!(
        !runs.is_empty(),
        "the sibling lists no run at all while its member is taking a turn: it exited {:?} \
         saying {:?}",
        listed.status.code(),
        String::from_utf8_lossy(&listed.stderr)
    );
    let records: Vec<Value> = runs
        .iter()
        .map(|run| {
            let shown = sibling(&["history", "show", run]);
            serde_json::from_slice(&shown.stdout).unwrap_or_else(|error| {
                panic!(
                    "the sibling cannot print the record it listed for {run}: {error}; it exited \
                     {:?} saying {:?}",
                    shown.status.code(),
                    String::from_utf8_lossy(&shown.stderr)
                )
            })
        })
        .collect();
    assert!(
        records.iter().any(|record| {
            record["refs"].as_array().is_some_and(|refs| {
                refs.iter()
                    .any(|reference| reference["origin"] == "./requested-reviewer.yaml")
            })
        }),
        "the graph that dispatched the member did not resolve the plan's persona: {records:?}"
    );

    // Let the held turn finish, so the run this journey started ends the way any
    // other does rather than being taken down with the world.
    world.release("turn.go");
    world.release("turn.settle");
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
        &path,
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
                "version": 2,
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

/// The scratch directory a node dispatch is promised reaches the turn the
/// **library backend** runs.
///
/// The stack production takes. Read off the turn process itself, the harness
/// child at the bottom of it, so what is asserted is what an agent would hold.
/// No `ONEPIPELINE_ONEAGENTGRAPH_BIN`, because its absence is what makes the
/// graph under the dispatch this build's own rather than an installed sibling's.
#[test]
fn a_dispatchs_scratch_directory_reaches_the_turn_the_library_backend_runs() {
    let world = World::new("real-scratch");
    world.write_graphs();
    let path = world.plan("scratch", &plan_of("scratch", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&[
            "start",
            &path,
            "--attach",
            "--dag-graph",
            &world.dag_graph(),
        ])
        .exited(0)
        .settled();

    let turns = world.turns();
    let worker = turns
        .iter()
        .find(|turn| turn.member == "worker")
        .unwrap_or_else(|| panic!("the node's own member never ran a turn: {turns:?}"));
    let at = std::path::Path::new(&worker.scratch);
    assert!(
        at.is_absolute(),
        "the turn was handed {:?}, which is not an absolute path\n{}",
        worker.scratch,
        world.dump()
    );
    assert!(
        at.is_dir(),
        "the turn was handed {}, which is not a directory that exists\n{}",
        at.display(),
        world.dump()
    );
    std::fs::write(
        at.join("written"),
        "by a journey standing where the turn stood",
    )
    .unwrap_or_else(|error| panic!("{} is not writable: {error}", at.display()));
}

/// Every reading a turn took of the scratch directory it was given, as
/// `(job, phase, path)`, in the order the turns took them.
fn scratch_readings(world: &World) -> Vec<(String, String, String)> {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "claude-scratch")
        .map(|call| {
            let at = |n: usize| {
                call["args"][n]
                    .as_str()
                    .unwrap_or_else(|| panic!("a scratch reading is three strings: {call}"))
                    .to_string()
            };
            (at(0), at(1), at(2))
        })
        .collect()
}

/// Two dispatches running at once each hold their **own** scratch directory, and
/// hold it for as long as they run.
///
/// The promise is per *dispatch*, and the only thing that can break it is another
/// dispatch — so the scenario has to be two of them alive at the same instant.
/// Both turns are held at a barrier that releases when both have arrived, so each
/// one's second reading is taken while the other is demonstrably inside its own
/// dispatch. A value the two shared, or one either could overwrite, is a value
/// that has been overwritten by then.
///
/// Both halves are asserted, because either alone proves nothing: that the two
/// turns hold *different* directories, and that neither turn's own value moved
/// between its two readings. And each directory is written to at both readings,
/// so "exists and is writable" is a fact about the whole dispatch rather than
/// about the moment it started.
///
/// Driven against the real `oneagentgraph` — no `ONEPIPELINE_ONEAGENTGRAPH_BIN` —
/// because the sharing this closes was the sibling's library path composing a
/// member's environment from the hosting process's.
#[test]
fn concurrent_dispatches_each_hold_their_own_scratch_directory_throughout() {
    let world = World::new("real-scratch-concurrent");
    world.write_graphs();
    // Two parties: the two nodes below, which have nothing between them and so
    // are ready on the same pass.
    world.script("turn.concurrent", "2");
    let path = world.plan(
        "concurrent",
        &plan_of(
            "concurrent",
            vec![agent("first", &[]), agent("second", &[])],
        ),
    );
    world
        .run_on_agentgraph(&[
            "start",
            &path,
            "--attach",
            "--dag-graph",
            &world.dag_graph(),
        ])
        .exited(0)
        .settled();

    // The barrier released, so both dispatches really were in flight together —
    // a journey where one ran after the other would have failed in the double —
    // and each party is the directory it was holding when it arrived, so this
    // file *is* the set of directories that were live at one instant.
    let arrived = std::fs::read_to_string(world.fakes.join("turn.concurrent.arrived"))
        .unwrap_or_else(|error| panic!("no barrier was reached: {error}\n{}", world.dump()));
    let live: std::collections::BTreeSet<&str> = arrived
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(
        live.len(),
        2,
        "two dispatches in flight at one instant were not holding two directories: \
         {arrived:?}\n{}",
        world.dump()
    );

    let readings = scratch_readings(&world);
    let held = |job: &str| -> Vec<(String, String)> {
        readings
            .iter()
            .filter(|(prompt, _, _)| prompt.contains(job))
            .map(|(_, phase, at)| (phase.clone(), at.clone()))
            .collect()
    };

    let mut each = Vec::new();
    for job in ["Do first.", "Do second."] {
        let taken = held(job);
        assert_eq!(
            taken.len(),
            2,
            "{job} did not read its scratch directory on both sides of the barrier: \
             {readings:?}\n{}",
            world.dump()
        );
        assert_eq!(taken[0].0, "entered");
        assert_eq!(taken[1].0, "beside the others");
        assert_eq!(
            taken[0].1, taken[1].1,
            "{job} was holding one scratch directory when it started and another while \
             its sibling dispatch ran: {taken:?}"
        );
        let at = std::path::PathBuf::from(&taken[1].1);
        assert!(
            at.is_absolute(),
            "{job} was given {at:?}, which is not absolute"
        );
        assert!(
            at.is_dir(),
            "{job}'s scratch directory {} is gone\n{}",
            at.display(),
            world.dump()
        );
        std::fs::write(at.join("read-back"), job)
            .unwrap_or_else(|error| panic!("{} is not writable: {error}", at.display()));
        each.push(at);
    }
    assert_ne!(
        each[0], each[1],
        "two dispatches running at once were handed one directory between them: {each:?}"
    );
    // And they are the two the barrier saw live together, so the readings above
    // are of the same instant this journey proved was concurrent.
    assert_eq!(
        each.iter()
            .map(|at| at.display().to_string())
            .collect::<std::collections::BTreeSet<String>>(),
        live.iter().map(|at| (*at).to_owned()).collect(),
    );

    // And both nodes really ran to a settlement, so none of the above is a
    // property of a dispatch that never happened.
    let settled = world.events_of("concurrent", "node-settled");
    assert_eq!(settled.len(), 2, "{settled:?}\n{}", world.dump());
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
        &path,
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
    let journal = world.journal(&run);
    let graph_settled = journal
        .iter()
        .position(|event| {
            event["source"] == "agentgraph"
                && event["kind"] == "graph-settled"
                && event["labels"]["node"] == "build"
        })
        .expect("the linked graph published its terminal event");
    let node_settled = journal
        .iter()
        .position(|event| event["kind"] == "node-settled" && event["labels"]["node"] == "build")
        .expect("the terminal graph event settled its node");
    assert!(
        graph_settled < node_settled,
        "the node settled without relaying the linked graph's terminal answer: {}",
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

/// A terminal graph event settles its node without waiting for graph-final
/// process reaping to disconnect the sibling's event channel.
///
/// The library run is still where it always was — one process further down, in
/// the `drive` child a dispatch is retained with — so what this reaches is the
/// same channel it always reached, through the pipe that child writes on.
#[cfg(unix)]
#[test]
fn a_dispatch_settles_on_its_terminal_event_while_the_graphs_final_reaper_runs() {
    let world = World::new("real-terminal-before-reap");
    world.write_graphs();
    world.script("harness.outlives-graph", "");
    let path = world.plan(
        "terminal-before-reap",
        &plan_of("terminal-before-reap", vec![agent("build", &[])]),
    );
    let mut command = world.agentgraph_cmd(&["start", &path, "--attach"]);
    let mut launch = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the attached launch starts");
    world.until("the node to settle", |world| {
        !world
            .events_of("terminal-before-reap", "node-settled")
            .is_empty()
    });

    let pid: u32 = std::fs::read_to_string(world.fakes.join("harness.outlives-graph.pid"))
        .expect("the process held for graph-final teardown recorded its pid")
        .trim()
        .parse()
        .expect("the process recorded a pid");
    let still_running = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .expect("the host answers about the fixture process")
        .success();
    if still_running {
        end_process(pid);
    }
    assert!(
        still_running,
        "the node waited for graph-final reaping instead of settling on graph-settled"
    );
    let status = launch.wait().expect("the attached launch exits");
    assert!(status.success(), "the attached launch failed: {status}");
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

/// Two dispatches of one run are two registrations, and a stop over them is a
/// clean stop.
///
/// A registry keyed by pid alone held one entry for both of them — the second
/// overwrote the first, and the first to end took the survivor's registration
/// away — and the two of them racing one temporary would leave an entry no
/// reader could parse, which is now a `stop` that refuses a perfectly healthy
/// run. The key is a pid **and** a claim for that reason, and the claim is what
/// still separates two dispatches that do share a process: a launch whose
/// environment is the driver's own stays in it, and so does every dispatch a
/// consumer that linked this crate makes without going through its command line.
///
/// The second dispatch is released by an attestation rather than started beside
/// the first, which is how a real run reaches this state — a node becoming ready
/// while another is already in flight — and it also keeps this journey clear of
/// the sibling's own naming of a library run's state directory, which is the
/// clock and the process.
///
/// `#[cfg(unix)]` because of what it reads the registry *through*: a `stop`
/// reporting `signalled` over the roots this run holds, which is
/// `sys::platform_stop`'s fold — and that fold's Windows arm is `taskkill`'s,
/// held there by the ungated journeys `src/sys.rs` names beside it. The gate is
/// therefore about where the teardown half is proven per platform and not about
/// anything here being unix-shaped; nothing this journey reaches for is. It has
/// never been run on Windows, so it is not claimed for that platform either.
#[cfg(unix)]
#[test]
fn two_dispatches_of_one_run_are_stopped_as_one_run() {
    let world = World::new("real-shared-process");
    world.write_graphs();
    world.script("turn.hold", "hold");
    let path = world.plan(
        "shared",
        &plan_of(
            "shared",
            vec![
                agent("first", &[]),
                human("approve", &[]),
                agent("second", &["approve"]),
            ],
        ),
    );
    world
        .run_on_agentgraph(&["start", &path, "--detach"])
        .exited(0);

    let dispatched = |world: &World| -> Vec<String> {
        world
            .events_of("shared", "node-dispatched")
            .iter()
            .filter_map(|event| event["labels"]["node"].as_str().map(str::to_string))
            .collect()
    };
    world.until(
        "the first node to be in flight beside the person",
        |world| {
            dispatched(world).contains(&"first".to_string())
                && !world.events_of("shared", "node-settled").is_empty()
        },
    );

    // The second becomes ready while the first is still in flight, so the driver
    // is running two dispatches inside itself.
    world.run(&["attest", "shared", "approve"]).exited(0);
    world.until("both nodes to be in flight", |world| {
        dispatched(world).contains(&"second".to_string())
    });
    world
        .run(&["status", "shared"])
        .exited(0)
        .out_has("first: running")
        .out_has("second: running");

    let stopped = world.run(&["stop", "shared"]);
    stopped.exited(0).out_has("\"stopped\":true");
    assert_eq!(
        stopped.json()["teardown"],
        json!("signalled"),
        "a stop over two dispatches in one driver did not report reaching them:\n{}",
        stopped.stdout
    );
    world.release("turn.go");
    world.release("turn.settle");
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
        .run_on_agentgraph(&["start", &path, "--detach"])
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
    // A second **call**, not a second activity: a turn publishes the observation
    // that answered a call as well as the call, so counting activities would let
    // this through on the answer to the first one — with the readout still
    // saying what the first call said, which is the thing under test.
    world.until("the dispatch to report a second tool call", |world| {
        world
            .events_of("watched", "turn-activity")
            .iter()
            .filter(|event| event["payload"]["kind"] == "tool_call")
            .count()
            > 1
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

/// A change request body drafted through the **real** siblings, end to end.
///
/// Every layer between the plan and the published body is the real thing: real
/// `oneagentgraph` resolves the pr-author graph and prepares its member, real
/// `oneharness` reads that member's own config, sees the `schema_file` it
/// declares, runs the turn buffered rather than streamed, validates what comes
/// back against that schema, and stores it at `results[].structured` of the
/// result that ran. This crate retains that report as it ingests the settlement,
/// reads the body out of its own copy, and hands it to `onevcs`, which opens the
/// change request with it. Only the paid model turn stands in.
///
/// That chain is a cross-repository contract with no shared type, so it is
/// proven rather than assumed: a sibling that stopped putting a validated answer
/// where this crate reads it would publish an empty change request and nothing
/// else would say so.
#[test]
fn a_drafted_body_reaches_the_change_request_through_the_real_siblings() {
    let world = World::new("real-pr-author");
    world.write_graphs();
    // A change request left open for review: a body is prose on one, and a
    // direct merge opens none.
    world.repository("change-open", &[]);
    world.script("harness.work", "the worker wrote this");
    let drafted = "## What\nRead off the branch's own diff.\n\n## Why\nSo a reviewer knows.";
    world.script("harness.body", drafted);
    let drafting = world.pr_author_graph();
    let node = json!({
        "id": "service",
        "repo": "service",
        "persona": "engineer",
        "title": "feat: land what the member made",
        "task": "## What\nship the thing",
    });
    let path = world.plan("authored", &plan_of("authored", vec![node]));
    let launched =
        world.run_on_agentgraph(&["start", &path, "--attach", "--pr-author-graph", &drafting]);
    launched.settled();
    // What the host was asked to open the change request with.
    let opened = world.changes_opened();
    assert_eq!(opened.len(), 1, "{opened:?}\n{}", world.dump());
    assert_eq!(
        opened[0]["body"],
        drafted,
        "the drafted body did not reach the change request: {opened:?}\n{}",
        world.dump()
    );

    // And it came off this run's **own** copy of the report, which is the file
    // the reader opens: the run kept one, and the validated answer is where the
    // producing library puts it rather than where this crate hoped.
    let kept: Vec<serde_json::Value> = std::fs::read_dir(world.run_file("authored", "reports"))
        .expect("the run kept the reports its dispatches settled with")
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    assert!(
        kept.iter().any(|report| {
            report["results"]
                .as_array()
                .is_some_and(|results| results.iter().any(|result| {
                    result["schema_valid"] == json!(true) && result["structured"]["body"] == drafted
                }))
        }),
        "no report this run retained carries the validated answer the body was read from: {kept:#?}"
    );
}

/// An answer the schema accepted that carries no body publishes without one.
///
/// The other ending of a drafting dispatch that *worked*: the member ran, the
/// harness answered, and the producing library validated what came back — so
/// every failure path is untaken and `schema_valid` is true — and the body in it
/// is blank. Distinct from the refused-graph journey below, where no answer
/// exists at all: here one does, this crate reads it, and what it decides is
/// that blank prose is not a body worth publishing.
///
/// Worth driving rather than leaving to the reader's unit tests, because the
/// blankness has to survive four hand-offs to be observable — the harness's
/// structured answer, the library's validation, the report this run retains, and
/// the publish request — and a crate that passed `Some("")` on would open a
/// change request whose body is a blank line nobody wrote.
#[test]
fn a_validated_answer_carrying_no_body_publishes_the_change_request_without_one() {
    let world = World::new("blank-pr-author");
    world.write_graphs();
    world.repository("change-open", &[]);
    world.script("harness.work", "the worker wrote this");
    // Spacing only, which is what a turn that answered the schema and said
    // nothing looks like: the schema requires the key, not prose under it.
    world.script("harness.body", "   \n");
    let drafting = world.pr_author_graph();
    let node = json!({
        "id": "service",
        "repo": "service",
        "persona": "engineer",
        "title": "feat: land it with a blank draft",
        "task": "## What\nship the thing",
    });
    let path = world.plan("blankdraft", &plan_of("blankdraft", vec![node]));
    let launched =
        world.run_on_agentgraph(&["start", &path, "--attach", "--pr-author-graph", &drafting]);
    launched.settled();

    // Published, with the plan's own title and no body — not a body of spaces.
    let opened = world.changes_opened();
    assert_eq!(opened.len(), 1, "{opened:?}\n{}", world.dump());
    assert_eq!(opened[0]["title"], "feat: land it with a blank draft");
    assert_eq!(
        opened[0]["body"], "",
        "a validated answer with no body in it still put one on the change request: {opened:?}"
    );
    assert_eq!(
        world.run_json("blankdraft", "result.json")["state"],
        "complete",
        "a drafting dispatch that answered blank took the publication with it:\n{}",
        world.dump()
    );

    // And the answer really was accepted: the emptiness is this crate's reading
    // of a validated answer, not a validation the dispatch failed. Without this
    // the journey would pass just as well against a member that never ran.
    let kept: Vec<serde_json::Value> = std::fs::read_dir(world.run_file("blankdraft", "reports"))
        .expect("the run kept the reports its dispatches settled with")
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    assert!(
        kept.iter().any(|report| {
            report["results"].as_array().is_some_and(|results| {
                results.iter().any(|result| {
                    result["schema_valid"] == json!(true) && result["structured"]["body"] == ""
                })
            })
        }),
        "no report this run retained carries a validated answer with a blank body: {kept:#?}"
    );

    // And the run says so: a drafter that answers inside its schema with
    // nothing in it is a drafter to correct, and it is named apart from one
    // whose answer the schema refused and one that never ran.
    let undrafted = world.events_of("blankdraft", "body-not-drafted");
    assert_eq!(undrafted.len(), 1, "{undrafted:?}\n{}", world.dump());
    assert_eq!(undrafted[0]["payload"]["ending"], "no-body");
    assert_eq!(undrafted[0]["labels"]["node"], "service");
}

/// A drafting graph the runner refuses costs the change request its body and
/// nothing else.
///
/// The document exists — a launch resolves the reference against its own
/// directory and refuses one it cannot read, so a reference that got this far
/// names a file — and the **runner** is what will not have it. That refusal
/// arrives where the drafting dispatch is built, after the branch is verified
/// and while the session still holds the work, which is exactly the moment
/// nothing may take the publication down with it.
///
/// The real sibling, because the refusal is its: a launcher holding a second
/// opinion about what a graph document may contain is the defect this file
/// exists for.
#[test]
fn a_drafting_graph_the_runner_refuses_still_publishes_the_change_request() {
    let world = World::new("real-pr-author-refused");
    world.write_graphs();
    world.repository("change-open", &[]);
    world.script("harness.work", "the worker wrote this");
    // A readable file that is not a graph the runner will run: it names a member
    // kind that does not exist, which `oneagentgraph` refuses in its own words.
    let refused = world.graphs().join("unrunnable.yaml");
    std::fs::write(
        &refused,
        format!(
            "version: {}\nname: pr-author\nmembers:\n  author:\n    kind: nonesuch\n",
            oneagentgraph::config::SCHEMA_VERSION
        ),
    )
    .expect("the unrunnable graph is written");
    let node = json!({
        "id": "service",
        "repo": "service",
        "persona": "engineer",
        "title": "feat: land it with no body",
        "task": "## What\nship the thing",
    });
    let path = world.plan("refuseddraft", &plan_of("refuseddraft", vec![node]));
    let launched = world.run_on_agentgraph(&[
        "start",
        &path,
        "--attach",
        "--pr-author-graph",
        &refused.to_string_lossy(),
    ]);
    launched.settled();

    // The node published, and the change request carries the plan's own title
    // and no body at all.
    let opened = world.changes_opened();
    assert_eq!(opened.len(), 1, "{opened:?}\n{}", world.dump());
    assert_eq!(opened[0]["title"], "feat: land it with no body");
    assert_eq!(opened[0]["body"], "", "{opened:?}");
    assert_eq!(
        world.run_json("refuseddraft", "result.json")["state"],
        "complete",
        "a drafting graph the runner refused took the publication with it:\n{}",
        world.dump()
    );
    // And it is not silent: a launch that named a drafting graph and drafted
    // nothing reads exactly like one that named none. The drafting dispatch is
    // given a process of its own — its environment is its own — so the runner's
    // refusal is that process's settlement rather than a refusal to this one,
    // and the operator is told either way.
    for said in [
        "node 'service'",
        "the drafting dispatch settled without succeeding",
        "so it publishes with no body",
    ] {
        assert!(
            launched.stderr.contains(said),
            "the refusal never reached the operator, which lacks {said:?}:\n{}",
            launched.stderr
        );
    }
    // And it reaches the run's own record too, where a reader who was not
    // watching stderr finds it: the sibling's refusal, under the ending that
    // says the dispatch is what failed rather than its answer.
    let undrafted = world.events_of("refuseddraft", "body-not-drafted");
    assert_eq!(undrafted.len(), 1, "{undrafted:?}\n{}", world.dump());
    assert_eq!(undrafted[0]["payload"]["ending"], "dispatch-failed");
    assert_eq!(undrafted[0]["labels"]["node"], "service");
    let detail = undrafted[0]["payload"]["detail"]
        .as_str()
        .unwrap_or_default();
    assert!(
        detail.contains("the drafting dispatch settled without succeeding"),
        "the recorded ending does not carry the sibling's refusal: {detail}"
    );
    assert!(
        detail.contains("nonesuch") || detail.contains("kind"),
        "the recorded ending does not carry the runner's own words: {detail}"
    );
    // The node settled on its publication, and its detail says the same thing.
    let settled = world.events_of("refuseddraft", "node-settled");
    assert_eq!(
        settled[0]["payload"]["detail"], undrafted[0]["payload"]["detail"],
        "the settlement of a node whose drafter would not start did not name it"
    );
}

/// The tools a real dispatched turn used, read back off the CLI.
///
/// There was no transcript verb at all: the evidence was retained — the
/// sibling stores each settled member's full report and says where — and nothing
/// read it, so an agent supervising a run could see that a turn happened and
/// never what it did.
///
/// The member here is `kind: oneharness`, so the retained report is
/// **oneharness's own run report** rather than onejudge's conversation: one entry
/// per harness the chain attempted, carrying that harness's final answer and the
/// actions it took. `src/report.rs::turns` reads both shapes, and this is where
/// the second one is driven — through the verb, against a report the real
/// `oneharness_core` composed. A reader that knew only the first answered
/// `it carries no transcript` for every single-sided dispatch.
#[test]
fn transcript_renders_a_real_dispatched_turns_tools_and_words() {
    let world = World::new("real-transcript");
    world.write_graphs();
    let path = world.plan("read", &plan_of("read", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&["start", &path, "--attach"])
        .exited(0)
        .settled();

    let transcript = world.run(&["transcript", "read", "build"]);
    transcript.exited(0).out_has("read  build");
    transcript.out_has("tool_call bash  echo the turn ran");
    transcript.out_has("report ");
    transcript.out_has("Ran what the task asked for.");
    // Both sources carry what the tool **returned**, which is the half a reader
    // was never shown: a `tool_result` states its text under `output` and states
    // no `detail` at all, so a third column read out of `detail` rendered every
    // observation a dispatch made as a blank. Split at the report line, because
    // the two sources render the same exchange and only the indent tells them
    // apart.
    let (from_the_store, from_the_report) = transcript
        .stdout
        .split_once("\n  report ")
        .expect("the transcript renders the store's summaries and then the report");
    assert!(
        from_the_store
            .lines()
            .any(|line| line == "    tool_result   the turn ran"),
        "what the tool returned is a blank column in the store's own summaries:\n{}",
        transcript.stdout
    );
    assert!(
        from_the_report
            .lines()
            .any(|line| line == "      tool_result   the turn ran"),
        "what the tool returned is a blank column in the retained report:\n{}",
        transcript.stdout
    );
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

/// A transcript names the identity that answered, and shows no turn for the ones
/// the chain stepped past.
///
/// A single-sided member's report is one entry per harness the run **attempted**,
/// so a two-candidate chain whose first is not installed retains two — and the
/// one it stepped past neither answered nor acted. Rendered, that is a turn with
/// a provider's name and nothing under it, above the only turn a reader came for;
/// a chain of four would bury it entirely. And the turn that did run has to be
/// attributed, because a reader with two entries in front of them has no other
/// way to tell which identity said which.
///
/// The chain is real rather than scripted: this launch's `ONEHARNESS_BIN_CODEX`
/// names a path that does not exist, so oneharness falls through that candidate
/// exactly as it would on a host where the harness is not installed. Named
/// rather than left to the `PATH`, because the `PATH` a launch inherits is the
/// developer's: on a host that has `codex`, the first candidate answered, the
/// chain stepped past nothing, and this journey failed reporting the fall-through
/// it was written to observe as missing.
///
/// The `PATH` is narrowed underneath that anyway, and checked. `ONEHARNESS_BIN_*`
/// is what decides the candidate, but the `PATH`
/// [`World::agentgraph_cmd`](crate::harness::World::agentgraph_cmd) builds keeps
/// the inherited one behind the siblings' directories, so a launch left on it can
/// resolve whatever else the operator installed. This one is given a `PATH`
/// holding nothing but what a dispatch resolves by name, and
/// [`World::resolved_on`](crate::harness::World::resolved_on) refuses before a
/// dispatch is spent if `codex` is reachable on it after all — so the premise is
/// arranged at both seams and stated as a checked fact at the one an override
/// could be dropped from.
#[test]
fn a_transcript_names_the_harness_that_answered_and_skips_the_ones_it_stepped_past() {
    let world = World::new("real-fallback-transcript");
    world.write_graphs();
    std::fs::write(
        world.graphs().join("chain.toml"),
        "run_mode = \"fallback\"\nharnesses = [\"codex\", \"claude-code\"]\n",
    )
    .expect("the two-candidate chain is written");
    let path = world.plan("chained", &plan_of("chained", vec![agent("build", &[])]));
    let mut launch = world.agentgraph_cmd(&[
        "start",
        &path,
        "--attach",
        "--node-set",
        "members.worker.oneharness_config=./chain.toml",
    ]);
    launch.env(
        "ONEHARNESS_BIN_CODEX",
        world.graphs().join("no-codex-on-this-host"),
    );
    launch.env("PATH", world.path_with_only_what_a_dispatch_resolves());
    if let Some(found) = World::resolved_on(&launch, "codex") {
        panic!(
            "this launch can resolve codex at {}, so its first candidate would run rather than \
             be stepped past and the fall-through below would never happen",
            found.display()
        );
    }
    // The second candidate is named by `ONEHARNESS_BIN_CLAUDE_CODE` and not by
    // the `PATH` above, which is what lets the first be unresolvable without the
    // chain running out.
    world.run_on(launch, "start --attach").exited(0).settled();

    // The chain really did step past the first candidate, which is what makes
    // the transcript below a claim about a fall-through rather than about a
    // one-candidate run.
    let advanced = world.events_of("chained", "fallback-advanced");
    assert!(
        advanced
            .iter()
            .any(|event| event["payload"]["identity"] == "codex"),
        "the chain never stepped past its first candidate: {advanced:#?}"
    );

    let transcript = world.run(&["transcript", "chained", "build"]);
    transcript
        .exited(0)
        .out_has("claude-code")
        .out_has("Ran what the task asked for.");
    assert!(
        !transcript.stdout.contains("codex"),
        "a candidate the chain stepped past was rendered as a turn:\n{}",
        transcript.stdout
    );
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
        let started =
            world.run_on_agentgraph(&["start", &path, form, "--dag-graph", &world.dag_graph()]);

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
            &path,
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
        .run_on_agentgraph(&["start", &path, "--attach"])
        .exited(0)
        .settled();

    let history = std::process::Command::new(crate::harness::oneagentgraph_binary())
        .arg("history")
        // The one variable under test. Everything else is left alone, so a
        // listing that comes back empty is this directory being empty rather
        // than the sibling being pointed elsewhere.
        .env("ONEAGENTGRAPH_STATE_DIR", &state)
        .output()
        .expect("the real oneagentgraph runs");
    // A refusal also prints nothing to stdout, so reading only stdout would report
    // a sibling that *answered* "no runs here" when it never answered at all — the
    // drift this gate names would be the one thing the failure did not say. Asked
    // first, and separately, so each failure carries its own cause.
    assert!(
        history.status.success(),
        "the sibling refused to list the directory this crate placed a run in, so this gate \
         learned nothing about drift — it exited {} saying:\n{}\n{}",
        history.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&history.stderr).trim(),
        world.dump()
    );
    let listed = String::from_utf8_lossy(&history.stdout);
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

/// The double standing at the paid model turn refuses an argument Claude Code
/// does not take.
///
/// Every journey in this file that reaches a member's turn reaches it through
/// `fake-claude`, so what those journeys are worth is what that double refuses.
/// An argv waved through here would let `oneharness` start sending a flag the
/// real CLI exits on while every member in this file still settled green, and
/// the first thing to say otherwise would be a provider. The accepting half is
/// already the rest of the file — the real `oneharness` drives this binary and
/// those members run — so this is the half no passing journey can show.
// llmlint: ignore-block[tests_mirror_real_usage] the subject is a *double*, driven at the
// process boundary the real `oneharness` reaches it on and with the argv that sibling sends
// plus one flag. Going through `onepipeline` would prove the opposite of the point: this
// crate never composes a harness argv, so there is no journey that can make the sibling send
// an undeclared flag on purpose.
#[test]
fn the_model_turn_double_refuses_an_argument_the_real_claude_does_not_take() {
    let world = World::new("claude-argv");
    let sent = |extra: &[&str]| {
        let mut args = vec![
            "-p",
            "Do build.",
            "--permission-mode",
            "acceptEdits",
            "--output-format",
            "json",
        ];
        args.extend_from_slice(extra);
        std::process::Command::new(crate::harness::double("fake-claude"))
            .args(&args)
            .env(onepipeline_testfakes::SCRIPT_DIR_ENV, &world.fakes)
            .output()
            .expect("the double runs")
    };

    let refused = sent(&["--dangerously-skip-permissions"]);
    let said = String::from_utf8_lossy(&refused.stderr).to_string();
    assert_eq!(
        refused.status.code(),
        Some(i32::from(onepipeline_testfakes::USAGE)),
        "an argv the real claude exits on ran a turn instead: {said}"
    );
    assert!(
        said.contains("--dangerously-skip-permissions"),
        "the refusal does not name what it refused: {said}"
    );

    // A declared flag with nothing after it, which the real CLI refuses the same
    // way it refuses an undeclared one. Read as a usage refusal rather than as
    // the flag never having been sent: leniently, an option `oneharness` started
    // sending without its value would settle every member here green and die
    // against a provider, which is the one thing this double is worth.
    let truncated = sent(&["--input-format"]);
    let said = String::from_utf8_lossy(&truncated.stderr).to_string();
    assert_eq!(
        truncated.status.code(),
        Some(i32::from(onepipeline_testfakes::USAGE)),
        "an option sent with no value after it ran a turn instead: {said}"
    );
    assert!(
        said.contains("--input-format"),
        "the refusal does not name the option that was left without a value: {said}"
    );

    // The same line without it, so the refusal is about that flag rather than
    // about the argv every other journey here sends.
    let ran = sent(&[]);
    assert_eq!(
        ran.status.code(),
        Some(0),
        "the argv `oneharness` really sends was refused: {}",
        String::from_utf8_lossy(&ran.stderr)
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
    world.write_supervised_node_graph();
    write_persona(&world, "engineer");
    let mut node = agent("build", &[]);
    node["persona"] = Value::from("./engineer.yaml");
    let path = world.plan("harness-bin", &plan_of("harness-bin", vec![node]));

    let named = "oneharness-that-is-not-installed";
    let mut command = world.agentgraph_cmd(&["start", &path, "--attach"]);
    command.env("ONEAGENTGRAPH_ONEHARNESS_BIN", named);
    world.run_on(command, "start --attach").settled();

    let config = config_of(&world, "harness-bin", "worker");
    assert!(
        config.contains(named),
        "the config the sibling composed does not name the harness it was told to \
         drive, so the variable was not read — it has drifted:\n{config}"
    );
}

/// A note reaches the **real** sibling's two-party note seam, and what that
/// conversation answers is what the run records.
///
/// The other live-edit journeys state their scenario at
/// `ONEPIPELINE_ONEAGENTGRAPH_BIN`, which is the override path; this one takes
/// the default, so the delivery is `oneagentgraph::control::note` called in this
/// process. The sibling addresses the member out of its own state and answers for
/// itself.
///
/// The answer here is that there is no conversation to hand it to: the member is
/// real and running, and the harness standing in for its paid turn is a
/// single-sided one with no two-party conversation behind it. That is a genuine
/// case rather than a contrivance, and it is the one `persist` exists for — so
/// both halves are asserted: the run records `carried` rather than a delivery to
/// a turn, and the note really rides the node's next dispatch.
#[test]
fn a_note_delivered_through_the_real_sibling_records_what_the_conversation_answered() {
    let world = World::new("real-note");
    world.write_graphs();
    world.script("turn.hold", "hold");
    let path = world.plan("noted", &plan_of("noted", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&["start", &path, "--detach"])
        .exited(0);
    world.until("the dispatch to report a turn", |world| {
        !world.events_of("noted", "turn-activity").is_empty()
    });

    let note = "the fixture moved to tests/data; stop editing src/old.rs";
    let submitted = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", "noted"]),
        &json!({
            "version": 2,
            "commands": [{
                "op": "note", "id": "build", "addressee": "worker", "text": note
            }],
        })
        .to_string(),
    );
    submitted.exited(0);

    world.until("the note to be reconciled", |world| {
        !world.events_of("noted", "edit-committed").is_empty()
    });
    let committed = world.events_of("noted", "edit-committed");
    assert_eq!(
        committed[0]["payload"]["operations"][0]["reached"],
        json!("carried"),
        "a note no turn of the real conversation took was not carried to the next \
         dispatch: {committed:?}"
    );
    assert_eq!(
        committed[0]["payload"]["operations"][0]["text"],
        json!(note),
        "the record does not carry what the note said: {committed:?}"
    );
    // The note seam pulls no interrupt lever: it is a conversation's own verb,
    // and a run that published a `turn-interrupted` for it would be reporting a
    // mechanism nobody used.
    assert!(
        world.events_of("noted", "turn-interrupted").is_empty(),
        "a note reached for the interrupt lever: {:?}",
        world.kinds("noted")
    );

    world.release("turn.go");
    world.release("turn.settle");
}

/// A `cancel` stops a dispatch running on the **library** backend, through that
/// library's own two levers.
///
/// The other cancellation journeys state their scenario at
/// `ONEPIPELINE_ONEAGENTGRAPH_BIN`, which is the process backend: the run is
/// addressed through the sibling's CLI and the teardown reaps a child this
/// crate started. This one takes the default, so both halves are library calls
/// in this process — `oneagentgraph::control::interrupt` for the ask, and the
/// sibling's own cancel for the teardown — and neither may go silent because of
/// how the run happens to be reached.
///
/// The lever answers that there is no controllable turn, which is a genuine
/// case rather than a contrivance: the harness standing in for the member's paid
/// turn is not one `oneharness` can reach a lever into, which is why the note
/// beside it is carried to the next dispatch rather than taken by a turn. That is the answer a cancellation must carry on from — and the
/// deadline is what actually stops this dispatch, which is the escalation
/// running end to end against the real sibling.
#[test]
fn a_cancel_against_a_real_dispatch_asks_its_lever_and_reaps_it_at_the_deadline() {
    let world = World::new("real-cancel");
    world.write_graphs();
    // Held open and, being a harness with no out-of-band control, deaf to the
    // ask: the dispatch is still there when the deadline arrives.
    world.script("turn.hold", "hold");
    let path = world.plan("stopped", &plan_of("stopped", vec![agent("build", &[])]));
    let mut launch = world.agentgraph_cmd(&["start", &path, "--detach"]);
    launch.env(crate::harness::CANCEL_GRACE_ENV, "1");
    world.run_on(launch, "start --detach").exited(0);
    world.until("the dispatch to report a turn", |world| {
        !world.events_of("stopped", "turn-activity").is_empty()
    });

    world
        .run_with_stdin(
            &["reply", "stopped"],
            &json!({"version": 2, "commands": [{"op": "cancel", "id": "build"}]}).to_string(),
        )
        .exited(0);

    // The ask reached the sibling and it answered, and the run says what it
    // answered rather than only that a cancel was issued.
    world.until("the interrupt to be recorded", |world| {
        !world.events_of("stopped", "turn-interrupted").is_empty()
    });
    let interrupted = world.events_of("stopped", "turn-interrupted");
    assert_eq!(interrupted[0]["payload"]["delivered"], json!(false));
    assert_eq!(
        interrupted[0]["labels"]["node"], "build",
        "the envelope is not stamped with the node it is about: {}",
        interrupted[0]
    );
    assert!(
        interrupted[0]["payload"]["input_bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0),
        "the cancellation offered the turn no redirection at all: {}",
        interrupted[0]
    );

    // And the deadline tore it down, through the sibling's own cancel — which is
    // what ends a held turn nothing could redirect.
    world.until("the deadline to expire", |world| {
        world
            .events_of("stopped", "planner-surface-queued")
            .iter()
            .any(|event| event["payload"]["kind"] == "dispatch-killed")
    });
    world.until("the cancelled node to settle", |world| {
        world
            .events_of("stopped", "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "build")
    });
    let settled = world
        .events_of("stopped", "node-settled")
        .into_iter()
        .find(|event| event["labels"]["node"] == "build")
        .expect("the settlement was just seen");
    assert_eq!(settled["payload"]["status"], "cancelled", "{settled}");

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
            &path,
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
        .run_on_agentgraph(&["start", &path, "--attach"])
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

/// A launch's own environment reaches the turn the library backend runs.
///
/// The launcher hands the observer graph two pairs — the run's id, and where its
/// ledger lives — and a member of that graph is an agent whose job is to look at
/// the run they name. The subprocess backend sets them on the child it spawns.
/// The library backend has no child to set them on: `oneagentgraph 0.2.18` runs a
/// single-sided member's turn in this process and the harness it spawns inherits
/// *this* process's environment, so a launch that only put them in the map it
/// hands the sibling would hand its members nothing — and the member would go
/// looking for a run named by nothing.
///
/// Read through the observer's own record of what it found: it writes the run it
/// was started for and whether that run's ledger was there to read, which is both
/// halves at once and is what the member itself saw rather than anything this
/// crate wrote down. `--attach` and no `ONEPIPELINE_ONEAGENTGRAPH_BIN`, because
/// that pair is what selects the library backend.
#[test]
fn a_launchs_own_environment_reaches_the_member_the_library_backend_runs() {
    let world = World::new("real-launch-env");
    world.write_graphs();
    let path = world.plan("carried", &plan_of("carried", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&[
            "start",
            &path,
            "--attach",
            "--dag-graph",
            &world.dag_graph(),
        ])
        .exited(0)
        .settled();

    let saw = world.observer_saw();
    assert_eq!(
        saw.first().map(|saw| saw["run"].clone()),
        Some(json!("carried")),
        "the observer was not told which run it was started for: {saw:?}\n{}",
        world.dump()
    );
    assert_eq!(
        saw[0]["launch_record"],
        json!(true),
        "the observer was not told where the run's ledger lives: {saw:?}"
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
/// only that it holds one parser, so the journey ends where the graph runs.
///
/// It ends there rather than at settlement, and what happens after it is a fact
/// about the **host** rather than about the launch — decided after the launch
/// this journey is about, and asserted below so the two are never confused for
/// one another. That fact is the one thing here the two platforms do not share,
/// because the question a dispatch is registered against — when its process
/// started — is asked through `ps` on macOS/other Unix, procfs on Linux, and the
/// process itself on Windows. So it is stated per platform at the assertion,
/// and no platform is left asserting nothing.
#[test]
fn a_document_the_runner_accepts_launches_whichever_way_it_is_asked_for() {
    for form in ["--attach", "--detach"] {
        let world = World::new(&format!("runner-schema-{}", form.trim_start_matches("--")));
        world.write_graphs_at_the_runners_schema();
        let path = world.plan("schema", &plan_of("schema", vec![agent("build", &[])]));

        let mut command =
            world.agentgraph_cmd(&["start", &path, form, "--dag-graph", &world.dag_graph()]);
        command.env("PATH", world.empty_path());
        let started = world.run_on(command, &format!("start {form}"));
        // The whole of the defect, in one line of the run's own record: the
        // launcher that refused this document refused it here, naming a field
        // list that predates the one it carries — so no graph ever started. It
        // is a launch rather than a parse, and this is the graph the runner
        // accepted, ran, and settled.
        assert!(
            !started.stderr.contains("schema_version"),
            "the launch refused the document the runner accepts:\n{}",
            started.stderr
        );
        // Read off what the graph's own member did, because that is the same
        // evidence either way a run is launched: an attached launch relays the
        // observer's envelopes into the run's store and a detached one hands
        // them to its driver log, but the member runs in both.
        world.until("the graph the launch named to run", |world| {
            !world.observer_saw().is_empty()
        });

        // And the loop drove: it dispatched the node, and the node settled. What
        // it settled *as* is the per-platform half below.
        world.until("the run to settle", |world| {
            world.run_file("schema", "result.json").is_file()
        });
        assert!(
            !world.events_of("schema", "node-dispatched").is_empty(),
            "the loop never dispatched the node:\n{}",
            world.dump()
        );
        let settled = world.events_of("schema", "node-settled");
        // On macOS and other non-Linux Unix, `sys::process_start_token` asks
        // `ps`, resolved by name off the empty `PATH`, so the dispatch cannot be
        // stamped and is refused rather than run blind.
        #[cfg(all(unix, not(target_os = "linux")))]
        assert_eq!(
            settled[0]["payload"]["outcome"],
            json!("infrastructure-failure"),
            "a dispatch nothing could stamp settled as something else: {}",
            settled[0]
        );
        // Linux asks procfs and Windows asks the **process**, neither of which
        // resolves a program by name. So an emptied `PATH` takes nothing away
        // here — the dispatch is stamped and registered, and it runs, because
        // everything below it in this world is named by absolute path.
        // Asserted rather than gated away, so the platform that *can* stamp is
        // held to running the node through to a settlement rather than to
        // whatever it happened to do.
        #[cfg(any(target_os = "linux", windows))]
        assert_eq!(
            settled[0]["payload"]["status"],
            json!("done"),
            "a dispatch this host could stamp did not run: {}",
            settled[0]
        );
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
                &path,
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

/// A classification the harness's own record contradicts does not kill the
/// member: the turn that record describes is carried as a settlement instead.
///
/// The loss this is named for, and why this crate cares rather than only the
/// sibling. A `member-died` is the one event a node is settled **dead** on —
/// `src/engine.rs` reads the producer's cause straight off it — so a node was
/// settled `failed` on `{"cause":"rate_limit"}` while the harness's record for
/// the same turn read `status: ok`, `exit_code: 0` and billed usage, and the
/// finished work went with it. `oneagentgraph` 0.3.14 weighs the two before it
/// publishes a death, and that is a floor **this build links** rather than a rule
/// written here: below it this journey fails rather than reading differently.
///
/// Driven through the real supervisor: the real sibling drives a real two-party
/// member, and the contradiction is a record `oneharness`'s own double writes in
/// that library's own types — `status: ok`, `exit_code: 0`, billed usage, beside
/// `failure_kind: rate_limit`. Nothing is asserted at a seam: what this reads is
/// the sibling's published events and the report it stored.
#[test]
fn a_classification_the_harness_record_contradicts_settles_rather_than_dies() {
    let world = World::new("drive-reconciled");
    world.write_graphs();
    // Two-party, because the reconciliation is the supervisor's: a classification
    // is only ever weighed against a record where there is a judge to publish a
    // death in place of a settlement.
    world.write_supervised_node_graph();
    let graph = world.graphs().join("node-scope.yaml");
    let dir = world.root.join("driven-reconciled");
    std::fs::create_dir_all(&dir).expect("a directory for the driven graph");

    world.script("harness.rejects", "");
    let driven = world.run_on(
        world.agentgraph_cmd(&[
            "drive",
            &graph.to_string_lossy(),
            "--task",
            "Be billed for a turn the provider then rejects.",
            "--dir",
            &dir.to_string_lossy(),
        ]),
        "drive a graph whose classification its own record contradicts",
    );
    let published: Vec<Value> = driven
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();

    // Still a member that did not reach its bar — so still the sibling's own
    // member-failed code — but one that *failed its task* rather than one that
    // died, which is the whole of the distinction a node is settled on.
    assert_eq!(
        driven.code,
        oneagentgraph::error::EXIT_MEMBER_FAILED,
        "a driver did not carry its graph's own exit code:\nstdout: {}\nstderr: {}",
        driven.stdout,
        driven.stderr
    );
    assert!(
        !published.iter().any(|event| event["kind"] == "member-died"),
        "a turn the harness recorded as completed and billed was published as a \
         death, which is what a node destroys finished work on:\n{}",
        driven.stdout
    );
    let settled: Vec<&Value> = published
        .iter()
        .filter(|event| event["kind"] == "member-settled")
        .collect();
    assert_eq!(
        settled.len(),
        1,
        "the carried turn did not settle exactly once:\n{}",
        driven.stdout
    );
    assert_eq!(
        settled[0]["payload"]["completed"],
        json!(false),
        "a turn that never reached its bar settled as one that did: {}",
        settled[0]
    );

    // And the reconciliation is on the stored report rather than left for nobody
    // to find: the classification, and the record's own facts beside it.
    let stored = settled[0]["payload"]["report_path"]
        .as_str()
        .unwrap_or_else(|| panic!("the settle named no stored report: {}", settled[0]));
    let report: Value = serde_json::from_str(
        &std::fs::read_to_string(stored).expect("the stored report is readable"),
    )
    .expect("the stored report is JSON");
    let why = report["settled_reason"]
        .as_str()
        .unwrap_or_else(|| panic!("the carried turn said nothing about why: {report}"));
    for said in ["rate_limit", "status ok", "exit code 0"] {
        assert!(
            why.contains(said),
            "the carried turn's reason does not name {said:?}: {why}"
        );
    }
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

/// One persona file, so a two-party member has a delta to resolve.
fn write_persona(world: &World, name: &str) {
    std::fs::write(
        world.graphs().join(format!("{name}.yaml")),
        format!("name: {name}\nsystem_prompt: Ship it.\nuser:\n  persona: Review it.\n"),
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
    world.write_supervised_node_graph();
    for persona in ["budgeted", "plain"] {
        write_persona(&world, persona);
    }

    let dispatched = |run: &str, node: Value| {
        let path = world.plan(run, &plan_of(run, vec![node]));
        world
            .run_on_agentgraph(&[
                "start",
                &path,
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
/// which the harness double does not speak — it stands in for the provider CLI
/// `oneharness` itself spawns, one layer further down — so the conversation
/// cannot complete offline. The launch is the fact under test and it is published before the turn
/// runs, which is why every two-party journey in this file reads one.
#[test]
fn a_two_party_member_is_started_in_the_directory_the_graph_was_given() {
    let world = World::new("real-two-party-cwd");
    world.write_graphs();
    world.write_supervised_node_graph();
    write_persona(&world, "engineer");
    world.repository("local-direct", &[]);

    let node = json!({
        "id": "service",
        "repo": "service",
        "persona": "./engineer.yaml",
        "task": "## What\nship the thing",
        // The title its change request opens under, which a lifecycle node
        // states from plan schema 3 on.
        "title": "feat: land what the member made",
    });
    let path = world.plan("twoparty", &plan_of("twoparty", vec![node]));
    world
        .run_on_agentgraph(&["start", &path, "--attach"])
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
    world.write_supervised_node_graph();
    write_persona(&world, "implementer");
    world.repository("local-direct", &[]);

    let node = json!({
        "id": "service",
        "repo": "service",
        // The title its change request opens under, which a lifecycle node
        // states from plan schema 3 on.
        "title": "feat: land what the step made",
        "steps": [
            {"id": "implement", "persona": "./implementer.yaml", "task": "## What\nimplement",
             "max_turns": 45},
        ],
    });
    let path = world.plan("stepbudget", &plan_of("stepbudget", vec![node]));
    world
        .run_on_agentgraph(&["start", &path, "--attach"])
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
        .run_on_agentgraph(&["start", &path, "--attach"])
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
            &path,
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
            &path,
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
            &path,
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

/// An observer killed outright — mid-turn, having written no ending — is the
/// death only the `owner.lock` can report, and this drives it.
///
/// The process to kill comes from that graph run's own lock rather than from
/// anything matched against a command line, which knows nothing about whose work
/// it matched.
#[cfg(unix)]
#[test]
fn a_run_whose_observer_graph_is_watching_and_then_is_killed_reads_as_each() {
    // Restarting switched off, because the subject here is the *reading* — what
    // a view answers about a graph run whose owner is gone — and a driver that
    // started a replacement would put a live graph in front of it before the
    // question could be asked. Zero is the operator's own off switch and is
    // exactly what every build before restarting did; the restart itself is
    // driven by
    // `a_killed_observer_is_replaced_and_the_run_goes_on_being_watched` below.
    let world = World::new("real-observer-dead").with_env(OBSERVER_RESTARTS_ENV, "0");
    world.write_graphs();
    // The node's turn is held for the whole journey, so both runs are executing
    // throughout: an observer verdict is about a run that is *working*.
    world.script("turn.hold", "hold");
    // And the observing turn is held, so the graph is mid-turn when it is killed
    // rather than a graph that had already finished and said so.
    world.script("observer.wait", "hold");

    for (run, dag_graph) in [("watched", true), ("bare", false)] {
        let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
        let mut launch = vec!["start".to_string(), path];
        launch.push("--detach".into());
        if dag_graph {
            launch.push("--dag-graph".into());
            launch.push(world.dag_graph());
        }
        world
            .run_on_agentgraph(&launch.iter().map(String::as_str).collect::<Vec<_>>())
            .exited(0);
    }

    let graph_run = || {
        world.run_json("watched", "launch.json")["graph_run"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    world.until("the observer graph to be recorded", |_| {
        !graph_run().is_empty()
    });
    let graph_dir = world.graph_state().join(graph_run());
    let record = || {
        std::fs::read_to_string(graph_dir.join(oneagentgraph::run::RECORD_FILE)).unwrap_or_default()
    };
    let line = |run: &str, view: &[&str]| -> String {
        let rendered = world.run_on_agentgraph(view);
        rendered.exited(0);
        rendered
            .stdout
            .lines()
            .find(|line| line.contains(run))
            .unwrap_or_else(|| panic!("no line for {run} in:\n{}", rendered.stdout))
            .to_string()
    };

    for view in [vec!["runs"], vec!["status"]] {
        assert!(
            !record().contains("finished_ms"),
            "the observer settled before it was read"
        );
        // Watching: nothing says the graph has stopped, so the line says nothing
        // about the observer beyond the run being driven.
        let watching = line("watched", &view);
        assert!(
            watching.contains("ACTIVE") && !watching.contains("OBSERVER"),
            "a run whose observer is still taking its turn is reported unwatched: {watching}"
        );
        // Never observed: a different fact, on the same line, from the start.
        let bare = line("bare", &view);
        assert!(
            bare.contains("NO OBSERVER") && !bare.contains("OBSERVER DEAD"),
            "a run launched with no observer reads as one whose observer died: {bare}"
        );
        assert_ne!(watching, bare);
    }

    // The process that owns this graph run's state, as that run's own lock names
    // it: `<pid> <start token>`. Nothing is searched for and nothing is matched —
    // the sibling wrote down which process holds this directory.
    let lock = std::fs::read_to_string(graph_dir.join(oneagentgraph::liveness::OWNER_LOCK_FILE))
        .expect("the graph run records who owns its state");
    let owner: i32 = lock
        .split_whitespace()
        .next()
        .and_then(|pid| pid.parse().ok())
        .unwrap_or_else(|| panic!("the owner lock names no process: {lock:?}"));
    assert!(
        oneagentgraph::scratch::reclaimable(&graph_dir).is_err(),
        "the graph run's state was already unowned, so killing its owner proves nothing"
    );
    // SAFETY: `kill` takes a pid and a signal and reports failure in its return
    // value; nothing is borrowed. The pid is the one this run's own ownership
    // record names, which is the only process this journey may end.
    assert_eq!(
        unsafe { libc::kill(owner, libc::SIGKILL) },
        0,
        "could not end the process the graph run's own lock names"
    );

    // Killed outright, so no ending was ever written: what answers now is the
    // ownership record alone, which is the half of this verdict a settled record
    // cannot cover.
    world.until("the driver to notice its observer is gone", |_| {
        line("watched", &["runs"]).contains("OBSERVER DEAD")
    });
    assert!(
        !record().contains("finished_ms"),
        "the observer wrote an ending after being killed, so this proves the \
         record rather than the lock: {}",
        record()
    );

    for view in [vec!["runs"], vec!["status"]] {
        let dead = line("watched", &view);
        assert!(
            dead.contains("ACTIVE") && dead.contains("OBSERVER DEAD"),
            "a run whose observer was killed is still reported as watched: {dead}"
        );
        let bare = line("bare", &view);
        assert!(
            bare.contains("NO OBSERVER") && !bare.contains("OBSERVER DEAD"),
            "the run that never had an observer changed when another run's died: {bare}"
        );
        assert_ne!(dead, bare);
    }

    // And the driver said so where a detached run's driver says everything: an
    // operator following the log learns its monitor went, rather than only
    // noticing it had stopped saying anything.
    //
    // Waited for rather than read once: the verdict above answers off the
    // ownership lock, which is true the instant the process dies, while this
    // line comes from the driver's own watch on its own poll — so reading the
    // log the moment the views agree is a race the driver loses on a loaded
    // host, and did, on macOS.
    world.until_run_file_holds("watched", "driver.log", "has stopped watching");
    world.release("observer.go");
    world.release("turn.go");
    world.release("turn.settle");
}

/// A killed observer is replaced, and the run goes on being watched.
///
/// Against the real sibling, because the claim is that a graph run this host can
/// prove is over has another in its place: a run whose observer has gone
/// executes with nothing comparing what it is doing against what it was asked to
/// do, while every view reports a plain `ACTIVE`.
///
/// The observer here really ends — its owning process is killed outright, mid
/// turn — while the run is still being driven, and what is asserted is that the
/// run is watched *again* afterwards, by a graph run of its own rather than by
/// the one that was killed.
#[cfg(unix)]
#[test]
fn a_killed_observer_is_replaced_and_the_run_goes_on_being_watched() {
    let world = World::new("real-observer-restarted");
    world.write_graphs();
    // The node's turn is held for the whole journey, so the run is being driven
    // throughout: a restart is for a run that is still working, and one whose
    // loop had finished would need no observer at all.
    world.script("turn.hold", "hold");
    // And every observing turn is held, so each graph is genuinely watching —
    // the first when it is killed, the second when it is read.
    world.script("observer.wait", "hold");

    let path = world.plan(
        "rewatched",
        &plan_of("rewatched", vec![agent("build", &[])]),
    );
    world
        .run_on_agentgraph(&[
            "start",
            &path,
            "--detach",
            "--dag-graph",
            &world.dag_graph(),
        ])
        .exited(0);

    let recorded = || world.run_json("rewatched", "launch.json");
    let graph_run = || {
        recorded()["graph_run"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    world.until("the observer graph to be recorded", |_| {
        !graph_run().is_empty()
    });
    let killed = graph_run();

    // The process that owns that graph run's state, as the run's own lock names
    // it: `<pid> <start token>`. Nothing is searched for and nothing is matched
    // against a command line, which knows nothing about whose work it matched.
    let graph_dir = world.graph_state().join(&killed);
    let lock = std::fs::read_to_string(graph_dir.join(oneagentgraph::liveness::OWNER_LOCK_FILE))
        .expect("the graph run records who owns its state");
    let owner: i32 = lock
        .split_whitespace()
        .next()
        .and_then(|pid| pid.parse().ok())
        .unwrap_or_else(|| panic!("the owner lock names no process: {lock:?}"));
    // SAFETY: `kill` takes a pid and a signal and reports failure in its return
    // value; nothing is borrowed. The pid is the one this run's own ownership
    // record names, which is the only process this journey may end.
    assert_eq!(
        unsafe { libc::kill(owner, libc::SIGKILL) },
        0,
        "could not end the process the graph run's own lock names"
    );

    world.until("the run to be watched by another graph", |_| {
        let now = graph_run();
        !now.is_empty() && now != killed
    });
    let record = recorded();
    // Both of them, in order, so a reader meeting either graph's records in the
    // merged store can still say whose observer wrote them.
    assert_eq!(
        record["observer_runs"],
        json!([killed, graph_run()]),
        "the run does not name the graphs that have watched it: {record}"
    );
    assert!(
        record["observer_ending"].is_null(),
        "a run that is being watched again says nothing is watching it: {record}"
    );

    for view in [vec!["runs"], vec!["status"]] {
        let rendered = world.run_on_agentgraph(&view);
        rendered.exited(0);
        let line = rendered
            .stdout
            .lines()
            .find(|line| line.contains("rewatched"))
            .unwrap_or_else(|| panic!("no line for the run in:\n{}", rendered.stdout));
        assert!(
            line.contains("ACTIVE") && !line.contains("OBSERVER"),
            "a run whose observer was replaced still reads as unwatched: {line}"
        );
    }
    // And the driver said both halves where a detached run's driver says
    // everything, so an operator following the log reads the replacement rather
    // than inferring it from a graph run id that changed.
    world.until_run_file_holds("rewatched", "driver.log", "started another observer graph");

    world.release("observer.go");
    world.release("turn.go");
    world.release("turn.settle");
}

/// A supervisory read can say **which graph** a settlement record belongs to,
/// after the run has moved past that graph.
///
/// A `graph-settled` names no member, so the graph run `oneagentgraph` stamped it
/// with is the only thing on it that says whose it is. Unless the run names every
/// graph that has watched it, that id has nothing to be held against once
/// `graph_run` has moved on, and the dead observer's records in the store are
/// indistinguishable from a node dispatch's.
///
/// Read the way a supervisor reads: the run's own merged store, joined to the
/// run's own launch record. Nothing here knows a graph run id in advance.
#[test]
fn a_settlement_record_is_still_attributable_to_the_observer_that_wrote_it() {
    // One restart, so the run has had exactly two observers by the time the
    // bound is spent — enough that `graph_run` has moved past the first, which
    // is the state the attribution has to survive.
    let world = World::new("real-observer-attributed").with_env(OBSERVER_RESTARTS_ENV, "1");
    world.write_graphs();
    // The node's turn is held for the whole journey and the observing one is
    // not, so each observer settles while the run it was watching goes on being
    // driven.
    world.script("turn.hold", "hold");

    let path = world.plan(
        "attributed",
        &plan_of("attributed", vec![agent("build", &[])]),
    );
    // Attached, because that is the launch that relays what its observer says
    // into the run's own store — and spawned, because it stays for the run.
    let mut command = world.agentgraph_cmd(&[
        "start",
        &path,
        "--attach",
        "--dag-graph",
        &world.dag_graph(),
    ]);
    let mut launch = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the attached launch starts");

    world.until("the run to be recorded", |world| {
        world.run_file("attributed", "launch.json").is_file()
    });
    let watched = || -> Vec<String> {
        world.run_json("attributed", "launch.json")["observer_runs"]
            .as_array()
            .map(|runs| {
                runs.iter()
                    .filter_map(|run| run.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    world.until("the driver to spend its restart bound", |world| {
        watched().len() == 2
            && world.run_json("attributed", "launch.json")["observer_ending"].is_string()
    });
    let observers = watched();
    let record = world.run_json("attributed", "launch.json");
    assert_eq!(
        record["graph_run"],
        json!(observers[1]),
        "the run addresses a graph that is not the last one it started: {record}"
    );

    // The records of the graph that is *gone*: the run has moved on, and its
    // settlement still has to be readable as this run's observer.
    let of = |run: &str, kind: &str| -> Vec<Value> {
        world
            .journal("attributed")
            .into_iter()
            .filter(|event| event["kind"] == json!(kind) && event["labels"]["run_id"] == json!(run))
            .collect()
    };
    world.until("the first observer's settlement to reach the store", |_| {
        !of(&observers[0], "graph-settled").is_empty()
    });
    let settled = of(&observers[0], "graph-settled");
    assert_eq!(
        settled.len(),
        1,
        "the graph that stopped watching settled more than once: {settled:#?}"
    );
    // What a supervisory read has to be able to say about it, and all of it is
    // on the record itself: which graph wrote it, and that the graph was this
    // run's observer rather than one of its node dispatches.
    assert!(
        settled[0]["labels"]["node"].is_null(),
        "the observer's settlement is labelled as a node dispatch's: {}",
        settled[0]
    );
    assert!(
        observers.contains(
            &settled[0]["labels"]["run_id"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        ),
        "the run does not name the graph that wrote its observer's settlement: {record}"
    );
    // And it ran before it ended: both halves of that graph's life are in the
    // one store an operator reads.
    assert!(
        !of(&observers[0], "graph-started").is_empty(),
        "the store says the observer settled without ever having started"
    );

    // The join discriminates rather than matching everything: the graph
    // dispatching the node is stamped with a run of its own, which this run's
    // record does not name as an observer.
    let dispatched: Vec<String> = world
        .journal("attributed")
        .into_iter()
        .filter(|event| event["labels"]["node"] == json!("build"))
        .filter_map(|event| event["labels"]["run_id"].as_str().map(str::to_string))
        .collect();
    assert!(
        !dispatched.is_empty(),
        "the node was never dispatched, so there is nothing to tell the observer from"
    );
    assert!(
        dispatched.iter().all(|run| !observers.contains(run)),
        "a node dispatch's graph is named as one of this run's observers: {dispatched:?}"
    );

    world.release("turn.go");
    world.release("turn.settle");
    let status = launch.wait().expect("the attached launch exits");
    assert!(status.success(), "the attached launch failed: {status}");
}

/// The other way an observer stops: it finishes, and the run it was watching
/// does not.
///
/// A graph that settles writes an ending into its own run record, which is what
/// this half of the verdict reads — the ordinary case, beside the killed one
/// that never got to write anything.
#[test]
fn a_run_whose_observer_graph_finished_is_reported_unwatched() {
    // Restarting switched off, for the reason the killed journey above gives:
    // this is about the half of the verdict a *settled* graph run answers, and a
    // replacement would answer it instead.
    let world = World::new("real-observer-finished").with_env(OBSERVER_RESTARTS_ENV, "0");
    world.write_graphs();
    // The node's turn is held and the observing one is not, so the graph settles
    // while the run it was watching is still going.
    world.script("turn.hold", "hold");

    let path = world.plan("outlived", &plan_of("outlived", vec![agent("build", &[])]));
    world
        .run_on_agentgraph(&[
            "start",
            &path,
            "--detach",
            "--dag-graph",
            &world.dag_graph(),
        ])
        .exited(0);

    let graph_run = || {
        world.run_json("outlived", "launch.json")["graph_run"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    world.until("the observer graph to be recorded", |_| {
        !graph_run().is_empty()
    });
    world.until("the observer graph to write its ending", |world| {
        std::fs::read_to_string(
            world
                .graph_state()
                .join(graph_run())
                .join(oneagentgraph::run::RECORD_FILE),
        )
        .is_ok_and(|record| record.contains("finished_ms"))
    });

    for view in [vec!["runs"], vec!["status"]] {
        let rendered = world.run_on_agentgraph(&view);
        rendered.exited(0);
        let line = rendered
            .stdout
            .lines()
            .find(|line| line.contains("outlived"))
            .unwrap_or_else(|| panic!("no line for the run in:\n{}", rendered.stdout));
        assert!(
            line.contains("ACTIVE") && line.contains("OBSERVER DEAD"),
            "a run whose observer finished is still reported as watched: {line}"
        );
    }
    world.release("turn.go");
    world.release("turn.settle");
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
            &path,
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

/// A worker **turn** of a detached run can put a question to its manager,
/// through the real `oneagentgraph`.
///
/// One process further in than the journeys in `driver.rs`: the operator's
/// `ask-manager` wrapper runs inside the model turn, so what has to arrive is
/// the run id in *that* process's environment — and the real sibling composing a
/// member's environment per launch is what stands between the dispatch and it.
/// Detached with no dag-scope graph, because that is the shipped default and the
/// shape a long run is launched in.
#[test]
fn a_detached_runs_worker_turn_can_ask_its_manager() {
    let world = World::new("real-detached-run-id");
    world.write_graphs();
    world.script(
        "harness.asks",
        "the worker: this repository has two mains. Which?",
    );
    let path = world.plan("askable", &plan_of("askable", vec![agent("build", &[])]));

    let started = world.run_on(
        world.agentgraph_cmd(&["start", &path, "--detach"]),
        "start --detach with no dag-scope graph",
    );
    started.exited(0);
    let run = started.json()["run_id"]
        .as_str()
        .expect("a detached launch names its run")
        .to_string();

    world.until("the run to settle", |world| {
        world.run_file(&run, "result.json").is_file()
    });
    assert_eq!(
        world.question_for_the_manager_on(world.agentgraph_cmd(&["next", &run]), &run),
        "the worker: this repository has two mains. Which?"
    );
    assert_eq!(
        world.run_json(&run, "result.json")["state"],
        "complete",
        "the run did not settle: {}",
        world.dump()
    );
}
