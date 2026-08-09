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
