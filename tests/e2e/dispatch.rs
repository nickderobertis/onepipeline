//! The `oneagentgraph` seam, against the real `oneagentgraph`.
//!
//! Every other journey here substitutes that sibling wholesale, which is what
//! let a run report success while the sibling was refusing every dispatch it was
//! sent: the double accepted a `--label` the real CLI reserves. The journeys
//! here close that gap from the other side — the real binary resolves the
//! graph, supervises the member, and stamps the stream, and the only thing
//! standing in is the paid model turn, replaced at that library's own
//! `ONEAGENTGRAPH_ONEHARNESS_BIN` override.

// llmlint: ignore-file[e2e_not_mocked] the one substitution here is the paid model turn,
// at `oneagentgraph`'s own documented binary override — the single genuinely external
// thing a gate cannot run for free. Real `onepipeline` dispatches, real `oneagentgraph`
// resolves the graph and supervises the member, and nothing between them is stubbed.

use crate::harness::{agent, human, plan_of, World};

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

    let started = world.run_real(&["start", &path.to_string_lossy(), "--attach"]);
    started.exited(0).settled();
    let run = started.json()["run_id"]
        .as_str()
        .expect("the launch named its run")
        .to_string();

    // llmlint: ignore-block[tests_mirror_real_usage] what this crate delivers *is* the
    // command line it composes for its sibling: a dispatch that reached `oneagentgraph`
    // with the wrong labels is refused by that CLI and never reaches a view, which is
    // exactly how it went unnoticed. The invocation record is the double's account of a
    // real subprocess execution — the evidence a server-side test takes from the request
    // it received — and every assertion on it here sits beside one on the run's own
    // settled state.
    // A member really ran, twice: the orchestrator that drove the run, and the
    // worker the node was dispatched to. Each was given the prose its own graph
    // was launched with.
    let prompts: Vec<String> = world
        .turns()
        .iter()
        .filter_map(|turn| turn["prompt"].as_str().map(str::to_string))
        .collect();
    assert!(
        prompts.iter().any(|p| p.contains("onepipeline round run")),
        "no member drove the run: {prompts:?}"
    );
    assert!(
        prompts.iter().any(|p| p.contains("Do build.")),
        "the node's own task never reached a member: {prompts:?}"
    );
    // llmlint: ignore-end[tests_mirror_real_usage]

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
        let started = world.run_real(&["start", &path.to_string_lossy(), form]);

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
        .run_real(&["start", &path.to_string_lossy(), "--detach"])
        .exited(0);
    world.until("the driver to be gone", |world| {
        world
            .run_real(&["status", "orphaned"])
            .stdout
            .contains("DRIVER DEAD")
    });

    // The graph the launch record names goes away under it, so the relaunch the
    // adoption performs is refused by the sibling.
    std::fs::remove_file(world.graphs().join("dag-scope.yaml")).expect("the graph is removed");

    let adopted = world.run_real(&["adopt", "orphaned"]);
    adopted.exited(crate::harness::REFUSED);
    adopted.err_has("oneagentgraph");
    assert!(
        world.events_of("orphaned", "driver-adopted").len() == 1,
        "the adoption was recorded more than once: {:?}",
        world.events_of("orphaned", "driver-adopted")
    );
}
