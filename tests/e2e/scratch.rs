//! `ONEPIPELINE_NODE_SCRATCH_DIR`: the directory each node dispatch is given.
//!
//! The promise these journeys hold is in the module documentation of
//! [`onepipeline::executor`] and recorded as divergence 47: an absolute path to a
//! directory that exists and is writable before the dispatch's first turn, unique
//! to that dispatch, and not removed while it runs. The spelling of the path is
//! not promised, so nothing here asserts one.

// llmlint: ignore-file[e2e_not_mocked] the dispatched agent is what reads this variable,
// and the double is what stands in for one: it takes the value out of its own environment
// exactly as an agent would. `harness.rs` carries the same suppression and the rationale.

use crate::harness::{agent, lifecycle, plan_of, World};

/// The scratch directory each of a node's dispatches was given, in dispatch
/// order, read out of the run's own store.
///
/// Every dispatch publishes the value it took out of its environment on the turn
/// it opens, so this is what the dispatches themselves acted on rather than
/// anything a double reported back on the side.
fn given(world: &World, run: &str, node: &str) -> Vec<std::path::PathBuf> {
    world
        .events_of(run, "turn-activity")
        .into_iter()
        .filter(|event| event["labels"]["onepipeline.node"] == node)
        .map(|event| {
            let named = event["payload"]["scratch_dir"].as_str().unwrap_or_else(|| {
                panic!("a dispatch's environment carried no scratch directory: {event}")
            });
            std::path::PathBuf::from(named)
        })
        .collect()
}

/// Assert of one directory everything the promise says about it, except the one
/// thing it does not say: what it is called.
fn kept(world: &World, at: &std::path::Path) {
    assert!(at.is_absolute(), "{} is not an absolute path", at.display());
    assert!(
        at.is_dir(),
        "{} is not a directory that exists\n{}",
        at.display(),
        world.dump()
    );
    // Still holding what its dispatch wrote into it, which is both halves at
    // once: it was writable while that dispatch ran, and nothing took it away
    // afterwards.
    let mut held = std::fs::read_dir(at)
        .unwrap_or_else(|error| panic!("{}: {error}", at.display()))
        .flatten();
    assert!(
        held.next().is_some(),
        "{} no longer holds what its dispatch wrote into it",
        at.display()
    );
}

fn settle(world: &World, name: &str, nodes: Vec<serde_json::Value>) -> String {
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

/// A dispatch is handed an absolute path to a directory of its own that already
/// exists and can be written to.
///
/// A dispatch that has nowhere of its own invents one, and what it invents
/// collides with what every other dispatch on the host invents. Read off the
/// run's store rather than off the double's files: the promise is a property of
/// the *dispatch*, so the evidence is the value the dispatch itself took out of
/// its environment and published, and the directory that value names.
#[test]
fn a_dispatch_is_given_an_absolute_writable_directory_of_its_own() {
    let world = World::new("scratch-given");
    let run = settle(&world, "given", vec![agent("build", &[])]);

    let named = given(&world, &run, "build");
    assert_eq!(named.len(), 1, "{named:?}\n{}", world.dump());
    kept(&world, &named[0]);
}

/// Every dispatch of one node gets a directory of its own, and none of them is
/// taken away.
///
/// The requeue is the case the promise is *for*: a node dispatched again is the
/// one set of dispatches that agree on every name a path could be derived from —
/// same run, same node, same step — so a scratch directory keyed on any of them
/// would hand each attempt the last one's half-written files.
///
/// The requeue here is a real one: the host reports this node's checks red and
/// keeps reporting them red, so its publication fails in a way that leaves the
/// work on its branch and the whole node is dispatched again, to its budget.
#[test]
fn every_dispatch_of_one_node_is_given_its_own_directory_and_none_is_taken_away() {
    let world = World::new("scratch-requeued");
    // `change-auto` is the policy that watches the host's own checks to their
    // conclusion, which is where a red one is observed at all.
    world.repository("change-auto", &[]);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", "llmlint completed failure required");
    let run = settle(&world, "requeued", vec![lifecycle("service", &[])]);

    let dispatched = world.events_of(&run, "node-dispatched").len();
    assert!(
        dispatched > 1,
        "this journey's node was dispatched once, so it compares nothing\n{}",
        world.dump()
    );
    let named = given(&world, &run, "service");
    assert_eq!(
        named.len(),
        dispatched,
        "a dispatch published no scratch directory: {named:?}\n{}",
        world.dump()
    );

    let distinct: std::collections::BTreeSet<&std::path::PathBuf> = named.iter().collect();
    assert_eq!(
        distinct.len(),
        named.len(),
        "a node dispatched again was handed a directory an earlier attempt had \
         been writing into: {named:?}"
    );
    // And every one of them is still there holding what its own dispatch wrote,
    // so no attempt's directory was cleared for the next.
    for at in &named {
        kept(&world, at);
    }
}
