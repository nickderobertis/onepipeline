//! What one fixed run spends on the write-back, measured at the store and held to a committed
//! record.
//!
//! The *comparable plan*: one scenario, the same for every build it is taken on, driven
//! through the compiled binary and the real `onetaskgraph` store it links, served through
//! `scripted-source` — the real `local-md` plugin, which logs every call it is handed and
//! meters every request it answers, the way a hosted source meters its own. A launch of five
//! nodes (a chain of three and a parallel pair); every node dispatched and settled `done` but
//! one, which fails and is `retry`-ed to `done`; one `amend`, one `reparent`, one `add`; one
//! transient store outage of three failures that recovers; one rate-limited refusal carrying a
//! wait; a `stop` and an `adopt`; and closeout.
//!
//! What it counts is what the store was asked and what its meter charged, split between the
//! launch's own plan read and the write-back: per store operation — the plugin protocol's own
//! method names, what a hosted source turns into requests — and the meter's summed spend.
//! `tests/golden/writeback-comparable-plan.json` holds the figures this build produces, which
//! the journey fails on the moment one moves, beside the same scenario's figures taken on the
//! engine before the landed baseline (entry 93), which it holds this build to beating.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// The scripted source serves every call out of the real `local-md` plugin and records it, so
// what lands on the board is the real store's own; the scripted refusals and the meter stand
// in only for what an offline store cannot be made to answer. `harness.rs` carries the same
// suppression and the full rationale.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{plan_of, repo_file, World, RENDEZVOUS_SECONDS_ENV};

const RUN: &str = "comparable-plan";

/// The record the figures are held to.
const RECORD: &str = "tests/golden/writeback-comparable-plan.json";

/// What every request the scripted source answers charges: one request, and one point of a
/// `graphql` budget, as a hosted source reports a request it sent.
fn meter() -> Value {
    json!({"requests": 1, "budgets": [
        {"budget": "graphql", "unit": "points", "measured": 1, "modelled": 0}
    ]})
}

fn node(id: &str, deps: &[&str]) -> Value {
    json!({
        "id": id,
        "persona": "engineer",
        "task": format!("## What\nDo {id}.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- {id} is done."),
        "deps": deps,
    })
}

fn reply(world: &World, command: Value) {
    world
        .run_with_stdin(
            &["reply", RUN],
            &json!({"version": 2, "commands": [command]}).to_string(),
        )
        .exited(0);
}

/// Every projection attempt the run has recorded.
fn records(world: &World) -> Vec<Value> {
    std::fs::read_to_string(world.run_file(RUN, "writeback-projections.jsonl"))
        .map(|text| {
            text.lines()
                .map(|line| serde_json::from_str(line).expect("a record line is JSON"))
                .collect()
        })
        .unwrap_or_default()
}

fn settled(world: &World, node: &str, status: &str) -> bool {
    world
        .events_of(RUN, "node-settled")
        .iter()
        .any(|event| event["labels"]["node"] == node && event["payload"]["status"] == status)
}

fn dispatched(world: &World, node: &str) -> usize {
    world
        .events_of(RUN, "node-dispatched")
        .iter()
        .filter(|event| event["labels"]["node"] == node)
        .count()
}

/// Wait until the write-back is at rest: every connection the store was handed but the
/// launch's own read has its attempt recorded, the last attempt landed, and the store has
/// been asked nothing for a while. Nothing here times a single attempt; a step waits for the
/// graph it caused, and then for this.
fn at_rest(world: &World, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let asked = world.store_calls();
        let opened = asked.iter().filter(|call| call[0] == "initialize").count();
        let recorded = records(world);
        let landed = recorded
            .last()
            .is_some_and(|record| record["outcome"] == "projected");
        // An attempt with nothing to carry opens no connection and records that it called
        // nothing; an older engine's line counts no calls at all, and opened one.
        let opened_by_attempts = recorded
            .iter()
            .filter(|record| record["calls"] != json!({}))
            .count();
        if landed && opened_by_attempts + 1 == opened {
            std::thread::sleep(Duration::from_millis(1_500));
            if world.store_calls().len() == asked.len() && records(world).len() == recorded.len() {
                return;
            }
            continue;
        }
        assert!(
            Instant::now() < deadline,
            "the write-back did not come to rest after {what}; the runs root held:\n{}",
            world.dump()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Wait for `ready`, then for the write-back to come to rest.
fn then_at_rest(world: &World, what: &str, ready: impl Fn(&World) -> bool) {
    world.until(what, &ready);
    at_rest(world, what);
}

fn by_method<'a>(calls: impl Iterator<Item = &'a Vec<String>>) -> BTreeMap<String, u64> {
    let mut counted = BTreeMap::new();
    for call in calls {
        *counted.entry(call[0].clone()).or_insert(0) += 1;
    }
    counted
}

fn spent<'a>(charged: impl Iterator<Item = &'a Value>) -> Value {
    let mut requests = 0;
    let mut points = 0;
    for charge in charged {
        requests += charge["spent"]["requests"]
            .as_u64()
            .expect("a request count");
        for budget in charge["spent"]["budgets"].as_array().expect("budgets") {
            points += budget["measured"].as_u64().expect("a figure")
                + budget["modelled"].as_u64().expect("a figure");
        }
    }
    json!({"requests": requests, "graphql_points": points})
}

/// Run the comparable plan and take its figures, with the run's projection record beside them.
fn the_comparable_plan() -> (Value, Vec<Value>) {
    let world = World::new("writeback-comparable-plan");
    for held in ["a", "b", "c", "d", "e", "d-2", "f"] {
        world.script(&format!("{held}.wait"), "hold");
    }
    world.script("d.fail", "1");
    let project = world.plan(
        RUN,
        &plan_of(
            RUN,
            vec![
                node("a", &[]),
                node("b", &["a"]),
                node("c", &["b"]),
                node("d", &[]),
                node("e", &[]),
            ],
        ),
    );
    world.script("store.metering", &meter().to_string());
    let world = world
        .through_scripted_source()
        .with_env(RENDEZVOUS_SECONDS_ENV, "600");
    let go = |node: &str| world.release(&format!("{node}.go"));

    world.run(&["start", &project, "--detach"]).exited(0);
    then_at_rest(&world, "the first dispatches", |world| {
        ["a", "d", "e"]
            .iter()
            .all(|node| dispatched(world, node) == 1)
    });

    go("d");
    then_at_rest(&world, "the failure", |world| settled(world, "d", "failed"));
    reply(
        &world,
        json!({"op": "retry", "id": "d", "node": node("d-2", &[])}),
    );
    then_at_rest(&world, "the retry", |world| dispatched(world, "d-2") == 1);

    reply(
        &world,
        json!({"op": "amend", "id": "c", "text": "## What\nDo c, as amended.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- c is done."}),
    );
    then_at_rest(&world, "the amend", |world| {
        !world.events_of(RUN, "edit-committed").is_empty()
    });
    let edits = world.events_of(RUN, "edit-committed").len();
    reply(&world, json!({"op": "add", "node": node("f", &["e"])}));
    then_at_rest(&world, "the add", |world| {
        world.events_of(RUN, "edit-committed").len() > edits
    });
    let edits = world.events_of(RUN, "edit-committed").len();
    reply(&world, json!({"op": "reparent", "id": "f", "deps": ["b"]}));
    then_at_rest(&world, "the reparent", |world| {
        world.events_of(RUN, "edit-committed").len() > edits
    });

    let failed_before = failures(&world);
    world.store_refuses(
        "get_project",
        &json!({"kind": "unavailable", "message": "connection reset by the destination"}),
    );
    go("e");
    world.until("three failed attempts", |world| {
        failures(world) >= failed_before + 3
    });
    world.store_stops_refusing("get_project");
    then_at_rest(&world, "the outage to recover", |world| {
        settled(world, "e", "done")
    });
    assert_eq!(
        failures(&world),
        failed_before + 3,
        "the outage was not three failures"
    );

    world.store_refuses_once(
        "get_project",
        &json!({"kind": "rate-limited", "retry_after_seconds": 2,
                "message": "API rate limit exceeded"}),
    );
    go("a");
    then_at_rest(&world, "the rate limit to be waited out", |world| {
        settled(world, "a", "done") && dispatched(world, "b") == 1
    });
    assert_eq!(
        failures(&world),
        failed_before + 4,
        "the rate limit was not one failure"
    );

    world.run(&["stop", RUN]).exited(0);
    let recorded = records(&world).len();
    world.run(&["adopt", RUN, "--detach"]).exited(0);
    then_at_rest(&world, "the adoption", |world| {
        records(world).len() > recorded
            && dispatched(world, "b") == 2
            && dispatched(world, "d-2") == 2
    });

    go("b");
    then_at_rest(&world, "b", |world| {
        settled(world, "b", "done") && dispatched(world, "c") == 1
    });
    go("d-2");
    then_at_rest(&world, "d-2", |world| settled(world, "d-2", "done"));
    go("c");
    then_at_rest(&world, "c", |world| {
        settled(world, "c", "done") && dispatched(world, "f") == 1
    });
    go("f");
    world.until("the run to settle", |world| {
        world.run_file(RUN, "result.json").is_file()
    });
    at_rest(&world, "closeout");
    assert_eq!(world.run_json(RUN, "result.json")["state"], "complete");

    // The launch's own read is the first connection the store was handed, up to the next.
    let calls = world.store_calls();
    let second = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call[0] == "initialize")
        .nth(1)
        .map(|(at, _)| at)
        .expect("the write-back opened the store");
    let (launch, writeback) = calls.split_at(second);
    let charged: Vec<Value> = std::fs::read_to_string(world.fakes.join("store.charged.jsonl"))
        .expect("the meter charged something")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a charge is JSON"))
        .collect();
    // Every request the launch's read made was answered, so its charges are the first of them.
    let launch_charged = launch
        .iter()
        .filter(|call| call[0] != "initialize" && call[0] != "metering")
        .count();
    let recorded = records(&world);
    let figures = json!({
        "launch_read": {
            "calls": by_method(launch.iter()),
            "spent": spent(charged[..launch_charged].iter()),
        },
        "writeback": {
            "calls": by_method(writeback.iter()),
            "calls_total": writeback.len(),
            "spent": spent(charged[launch_charged..].iter()),
            "attempts": recorded.len(),
            "failed_attempts": recorded.iter().filter(|record| record["outcome"] == "failed").count(),
            "whole_attempts": recorded.iter().filter(|record| record["scope"] == "whole").count(),
        },
    });
    (figures, recorded)
}

fn failures(world: &World) -> usize {
    records(world)
        .iter()
        .filter(|record| record["outcome"] == "failed")
        .count()
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the comparable plan is one
// whole run — thirteen steps, an outage served on the retry schedule, a rate limit's own wait —
// and its edge is the crate under test: it drives the compiled `onepipeline` binary against its
// own write-back worker, exactly as `writeback_projections.rs`' journeys do and for the reason
// recorded where this module is declared.
/// The comparable plan's figures are the ones the record commits, and fewer store calls and
/// less metered spend than the same scenario took on the engine before the landed baseline —
/// with no whole attempt, and no attempt whose calls name the project's page of tasks. (The one
/// `query_tasks` the store is asked is inside the copy that creates the `add`ed node: the store
/// looking for a counterpart before it creates one, which is the copy's own work.)
#[test]
fn the_comparable_plan_spends_what_the_committed_record_says() {
    let (measured, recorded) = the_comparable_plan();
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(repo_file(RECORD)).expect("the comparable plan's record ships"),
    )
    .expect("the record is JSON");
    assert_eq!(
        measured,
        record["after"]["figures"],
        "the comparable plan's figures moved; measured:\n{}",
        serde_json::to_string_pretty(&measured).expect("serializes")
    );
    let before = &record["before"]["figures"];
    let total = |figures: &Value| {
        figures["writeback"]["calls_total"]
            .as_u64()
            .expect("a total")
    };
    assert!(
        total(&measured) < total(before),
        "the write-back made {} calls, and the engine before the baseline made {}",
        total(&measured),
        total(before)
    );
    let points = |figures: &Value| {
        figures["writeback"]["spent"]["graphql_points"]
            .as_u64()
            .expect("points")
    };
    assert!(
        points(&measured) < points(before),
        "the write-back spent {} points, and the engine before the baseline spent {}",
        points(&measured),
        points(before)
    );
    assert_eq!(measured["writeback"]["whole_attempts"], 0);
    // An attempt with nothing left to carry asks the store nothing: it opens no connection, and
    // its line names no items, no calls and no report.
    let idle: Vec<&Value> = recorded
        .iter()
        .filter(|record| record["calls"] == json!({}))
        .collect();
    assert!(
        !idle.is_empty(),
        "no attempt was left with nothing to carry"
    );
    for record in &idle {
        assert_eq!(record["items"], json!([]), "{record}");
        assert_eq!(record["outcome"], "projected", "{record}");
        assert_eq!(record["actions"], Value::Null, "{record}");
    }
    assert_eq!(
        measured["writeback"]["calls"]["initialize"].as_u64(),
        Some((recorded.len() - idle.len()) as u64),
        "an attempt that called nothing opened the store"
    );
    for record in &recorded {
        assert_eq!(record["scope"], "members", "{record}");
        assert!(
            record["calls"].is_object() && record["calls"].get("task-list").is_none(),
            "an attempt read the project's page of tasks: {record}"
        );
    }
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
