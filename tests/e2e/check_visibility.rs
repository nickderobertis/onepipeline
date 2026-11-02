//! Check visibility: every failing check on a change a run published reaches the
//! manager's default views once, labelled with whether it blocks the merge; checks
//! still running when the publication returns are followed until they settle, named
//! when the checks timeout stops that, and listed at the run's end — and none of it
//! moves a node, a dependent or a publication.
//!
//! Driven through the compiled binary over real git and a real origin on disk, the
//! publication through the real linked `onevcs`. GitHub stands in at `onevcs`'s
//! `ONEVCS_GH` seam: `fake-gh` answers the publication's watch from its scripts, and a
//! follow read — `onevcs::follow_checks`, the standings query — from `onevcs-testing`'s
//! remote double, seeded per read from `gh.follow.<NUMBER>`.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is the real compiled binary and
// `onevcs` the real linked library over real git; GitHub is substituted at that library's
// own `ONEVCS_GH` override, answering follow reads through `onevcs-testing`'s remote
// double, and `oneagentgraph` at its subprocess boundary — the same substitutions, for the
// same reason, as `harness.rs` records.

use crate::draft_lifecycle::team_repository;
use crate::harness::{agent, lifecycle, plan_of, World};
use serde_json::Value;

/// The publication's watch: the required check green, a check that blocks nothing
/// still running when the required ones settle.
const WATCHED: &str = "lint completed success required\nslow in_progress - advisory";

/// What the follow reads after it answer: still running twice, then red.
const FAILS_ON_THE_THIRD_READ: &str = "lint completed success required\nslow in_progress - \
     advisory\n---\nlint completed success required\nslow in_progress - advisory\n---\nlint \
     completed success required\nslow completed failure advisory";

/// The same, ending green.
const PASSES_ON_THE_THIRD_READ: &str = "lint completed success required\nslow in_progress - \
     advisory\n---\nlint completed success required\nslow in_progress - advisory\n---\nlint \
     completed success required\nslow completed success advisory";

/// The address `fake-gh` gives the second check it reports on `owner/service`.
const SLOW_LOG: &str = "https://github.com/owner/service/actions/runs/1/job/2";

/// The follow interval every journey here runs under, in seconds.
const FOLLOW: &str = "0.2";

/// A world publishing `service` on a team repository whose watch sees [`WATCHED`] and
/// whose follow reads answer `follow`, and whose dependent `after` holds until told.
fn world_following(name: &str, follow: &str) -> World {
    let world = World::new(name).with_env("ONEPIPELINE_CHECK_FOLLOW_SECONDS", FOLLOW);
    team_repository(&world);
    world.script("service.work", "the worker wrote this\n");
    world.script("gh.checks", WATCHED);
    world.script("gh.follow.1", follow);
    world.script("after.wait", "hold");
    world
}

/// Launch `run` detached over `service` and its held dependent `after`.
fn launch(world: &World, run: &str) {
    let path = world.plan(
        run,
        &plan_of(
            run,
            vec![lifecycle("service", &[]), agent("after", &["service"])],
        ),
    );
    world.run_from(&world.project, &["start", path.as_str(), "--detach"]);
    world.until("the dependent to be dispatched", |world| {
        world
            .events_of(run, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "after")
    });
}

/// Let the held dependent finish, and wait for the run to settle.
fn release(world: &World, run: &str) {
    world.script("after.go", "");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
}

/// Every follow read the host answered, in order.
fn follow_reads(world: &World) -> Vec<Value> {
    std::fs::read_to_string(world.fakes.join("gh").join("follow-reads.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("a follow read record"))
        .collect()
}

/// One node of a run's result: its status, outcome and landing.
fn settlement(world: &World, run: &str, id: &str) -> (Value, Value, Value) {
    let result = world.run_json(run, "result.json");
    let node = result["nodes"]
        .as_array()
        .expect("the result lists nodes")
        .iter()
        .find(|node| node["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("{id} is missing from {result}"));
    (
        node["status"].clone(),
        node["outcome"].clone(),
        node["landing"].clone(),
    )
}

/// The `check-failed` surfaces a run raised, as `planner-surface-queued` records.
fn raised(world: &World, run: &str, kind: &str) -> Vec<Value> {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == kind)
        .collect()
}

/// The journey the follow exists for: a check that blocks nothing is still running
/// when the required checks settle, and fails two follow reads later. The manager is
/// told once — on a non-blocking `check-failed` surface, a `check-failed` record, and
/// the `change-check` line every default view prints — and the node settles exactly as
/// it does when that check passes, unretried, its dependent dispatched; following
/// stops at the read that answered nothing outstanding.
#[test]
fn a_late_failing_check_that_blocks_nothing_is_shown_once_and_moves_nothing() {
    let world = world_following("late-red", FAILS_ON_THE_THIRD_READ);
    let run = "latered";
    launch(&world, run);
    world.until("the late failure to be raised", |world| {
        !world.events_of(run, "check-failed").is_empty()
    });

    // A run watch armed with no `--until` returns on the surface.
    let watched = world.run(&["watch", run, "--timeout", "60", "--tick-interval", "0"]);
    watched.out_has("check-failed");
    let surfaces = raised(&world, run, "check-failed");
    assert_eq!(surfaces.len(), 1, "{}", world.dump());
    let message = surfaces[0]["payload"]["message"]
        .as_str()
        .expect("a message");
    assert!(!message.contains('\n'), "{message}");
    assert_eq!(surfaces[0]["payload"]["blocking"], false);
    for named in [
        "check 'slow' failed on https://github.com/owner/service/pull/1",
        "(node 'service', head ",
        "not required — does not block",
        SLOW_LOG,
        "Nothing is held, failed or retried for it.",
    ] {
        assert!(message.contains(named), "{named:?} is not in {message}");
    }

    // One record, carrying the check whole.
    let records = world.events_of(run, "check-failed");
    assert_eq!(records.len(), 1, "{}", world.dump());
    let payload = &records[0]["payload"];
    assert_eq!(payload["node"], "service");
    assert_eq!(payload["change"], "https://github.com/owner/service/pull/1");
    assert_eq!(payload["check"], "slow");
    assert_eq!(payload["state"], "failed");
    assert_eq!(payload["required"], false);
    assert_eq!(payload["requirement"], "not required — does not block");
    assert_eq!(payload["url"], SLOW_LOG);
    assert_eq!(
        payload["head"].as_str().map(str::len),
        Some(40),
        "{payload}"
    );

    // The planner profile — what `monitor` and `next` read by default — shows the
    // check's line, stamped `attention`.
    let line = format!(
        "change-check 'slow' failed (not required — does not block) [attention] log: {SLOW_LOG}"
    );
    world.run(&["monitor", run]).exited(0).out_has(&line);
    let next = world.run(&["next", run]);
    next.exited(0);
    let read: Value = serde_json::from_str(next.stdout.trim()).expect("next prints one object");
    assert!(
        read["check_lines"]
            .as_array()
            .is_some_and(|lines| lines.iter().any(|said| *said == Value::from(line.clone()))),
        "{read}"
    );
    assert!(
        read["events"]
            .as_array()
            .is_some_and(|events| events.iter().any(|event| {
                event["kind"] == "change-check"
                    && event["payload"]["name"] == "slow"
                    && event["labels"]["significance"] == "attention"
            })),
        "{read}"
    );

    // Following stopped at the read that answered nothing outstanding.
    let reads = follow_reads(&world).len();
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert_eq!(follow_reads(&world).len(), reads, "a read after the last");
    assert_eq!(reads, 3, "{:?}", follow_reads(&world));

    release(&world, run);
    let red = settlement(&world, run, "service");
    assert_eq!(settlement(&world, run, "after").0, "done");
    let dispatched = world
        .events_of(run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "service")
        .count();
    assert_eq!(dispatched, 1, "the node was retried: {}", world.dump());

    // Exactly what the same journey settles at when the check passes.
    let green_world = world_following("late-green", PASSES_ON_THE_THIRD_READ);
    let green = "lategreen";
    launch(&green_world, green);
    green_world.until("the follow to finish", |world| {
        follow_reads(world).len() >= 3
    });
    release(&green_world, green);
    assert_eq!(red, settlement(&green_world, green, "service"));
    assert!(green_world.events_of(green, "check-failed").is_empty());
}
