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
use serde_json::Value;

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
