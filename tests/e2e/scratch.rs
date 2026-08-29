//! The scratch directory every node dispatch is given.
//!
//! A dispatch that has nowhere of its own to write invents somewhere, and what
//! it invents collides — three did on one host in a day, and not one of the
//! three failures read as what it was: two deterministic tiers deadlocked on one
//! lock with both logs frozen, two whole-suite runs wrote into one log and left
//! `SIGTERM` lines that read exactly like test failures, and one worker read
//! another workstream's coverage output as its own.
//!
//! So the engine composes one into every node dispatch's environment, at
//! `ONEPIPELINE_NODE_SCRATCH_DIR`: an absolute path to a directory that exists
//! and is writable before the first turn, unique to that dispatch, and not
//! removed while that dispatch runs. Nothing else is promised — the spelling is
//! not part of the contract and no consumer may derive one path from another —
//! so these journeys assert exactly those properties and never a path shape.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The dispatched agent is what reads this variable, and the double is
// what stands in for one: it takes the value out of its own environment exactly as an
// agent would, writes into the directory, and puts what it saw on the run's own stream.
// `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{agent, plan_of, World};

/// The scratch directory every dispatch of this scenario was given, in the order
/// the dispatches ran.
///
/// Read out of the file each dispatch appends to as it starts, because a dispatch
/// scripted to produce *nothing* publishes no envelope a journey could read the
/// value off — and the two dispatches this file exists to compare are exactly
/// that pair.
fn given(world: &World) -> Vec<(String, std::path::PathBuf)> {
    let log = world.fakes.join("scratch-dirs");
    let text = std::fs::read_to_string(&log)
        .unwrap_or_else(|error| panic!("{}: {error}\n{}", log.display(), world.dump()));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| match line.split_once(' ') {
            Some((key, path)) => (key.to_owned(), std::path::PathBuf::from(path)),
            None => panic!("a dispatch recorded {line:?}, which names no directory"),
        })
        .collect()
}

/// The marker one dispatch wrote into the directory it was given.
///
/// Its presence afterwards is what says the directory was there and writable
/// while that dispatch ran, and that nothing took it away.
fn marker(at: &std::path::Path) -> String {
    let path = at.join("marker");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .trim()
        .to_owned()
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
/// exists and can be written to, and the run's own record carries it.
///
/// Read off the run's store rather than off the double's files: what the engine
/// promises is a property of the *dispatch*, so the evidence is the value the
/// dispatch itself took out of its environment and published, and the directory
/// that value names.
#[test]
fn a_dispatch_is_given_an_absolute_writable_directory_of_its_own() {
    let world = World::new("scratch-given");
    let run = settle(&world, "given", vec![agent("build", &[])]);

    let activity = world
        .events_of(&run, "turn-activity")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the dispatch published no turn\n{}", world.dump()));
    let named = activity["payload"]["scratch_dir"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("the dispatch's environment carried no scratch directory: {activity}")
        })
        .to_owned();
    let at = std::path::Path::new(&named);
    assert!(at.is_absolute(), "{named} is not an absolute path");
    assert!(
        at.is_dir(),
        "{named} is not a directory that exists\n{}",
        world.dump()
    );
    // Written by the dispatch, before its first turn: the directory was there and
    // it was writable at the moment the promise is about.
    assert_eq!(marker(at), "build");

    // And it is the same directory the dispatch recorded on its way in, so the
    // value on the stream is the value the dispatch acted on rather than a second
    // one composed for the record.
    let recorded = given(&world);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].1, at);
}

/// Two dispatches get two directories — including two dispatches of the same
/// node, which is what a retry is — and neither is taken away while its dispatch
/// is running.
///
/// The retry is the case the promise is *for*: a node asked again is the one pair
/// of dispatches that would otherwise agree on every name a path could be derived
/// from — same run, same node, same step — so a scratch directory keyed on any of
/// them would hand the second attempt the first's half-written files.
#[test]
fn two_dispatches_of_one_node_are_given_two_directories_and_neither_is_taken_away() {
    let world = World::new("scratch-retried");
    // A dispatch that produced nothing is asked again: two dispatches of one
    // node, and the second answers.
    world.script("build.silent", "");
    world.script("build.recover-after", "2");
    let run = settle(&world, "retried", vec![agent("build", &[])]);
    assert_eq!(
        world.events_of(&run, "node-dispatched").len(),
        2,
        "this journey's node was not dispatched twice\n{}",
        world.dump()
    );

    let recorded = given(&world);
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    let (first, second) = (&recorded[0].1, &recorded[1].1);
    assert_eq!(recorded[0].0, "build");
    assert_eq!(recorded[1].0, "build");
    assert_ne!(
        first, second,
        "a node asked again was handed the directory its first attempt had been \
         writing into: {recorded:?}"
    );
    // Both are still there, each holding what its own dispatch wrote — so neither
    // was removed while the dispatch it belonged to was running, and the first
    // outlived its own attempt rather than being cleared for the second.
    for at in [first, second] {
        assert!(at.is_dir(), "{} is gone\n{}", at.display(), world.dump());
        assert_eq!(marker(at), "build");
    }
}
