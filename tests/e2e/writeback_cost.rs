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
//! engine before the targeted update (entry 73) — every change a member copy — which it holds
//! this build to beating.

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

fn the_comparable_plan() -> (Value, Vec<Value>, Vec<Vec<Vec<String>>>) {
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
    let mark = records(&world).len();
    reply(
        &world,
        json!({"op": "retry", "id": "d", "node": node("d-2", &[])}),
    );
    then_at_rest(&world, "the retry", |world| dispatched(world, "d-2") == 1);
    // A retry is a targeted update of the root's item: the head's word and lineage keys, the
    // old head's settlement taken off, and the body the replacement restates.
    let retry = the_update_of(&world, mark, "d");
    for field in ["content", "metadata", "status"] {
        assert!(retry["updated_fields"].get(field).is_some(), "{retry}");
    }
    let root = board_item(&world, &project, "d");
    assert_eq!(
        root["item"]["metadata"]["onepipeline.node"], "d-2",
        "{root}"
    );
    assert!(
        root["item"]["metadata"]
            .get("onepipeline.settlement")
            .is_none(),
        "the old head's settlement stayed on the root's item: {root}"
    );

    let mark = records(&world).len();
    reply(
        &world,
        json!({"op": "amend", "id": "c", "text": "## What\nDo c, as amended.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- c is done."}),
    );
    then_at_rest(&world, "the amend", |world| {
        !world.events_of(RUN, "edit-committed").is_empty()
    });
    // An amendment is recorded beside the task on its own engine-owned key, so it is that key
    // alone: the body a retry restates is the one it rewrites.
    assert_eq!(
        the_update_of(&world, mark, "c")["updated_fields"],
        json!({"metadata": 1})
    );
    assert!(
        board_item(&world, &project, "c")["item"]["metadata"]["onepipeline.amendment"]
            .as_str()
            .is_some_and(|text| text.contains("as amended")),
        "the amendment did not reach the item"
    );
    let edits = world.events_of(RUN, "edit-committed").len();
    let mark = records(&world).len();
    reply(&world, json!({"op": "add", "node": node("f", &["e"])}));
    then_at_rest(&world, "the add", |world| {
        world.events_of(RUN, "edit-committed").len() > edits
    });
    // An `add` is the one copy, naming the node it creates alone, beside the one project read
    // that lets the copy land the project item unwritten.
    let created: Vec<Value> = records(&world)[mark..]
        .iter()
        .filter(|record| record["calls"].get("project-copy").is_some())
        .cloned()
        .collect();
    assert_eq!(created.len(), 1, "{created:?}");
    assert_eq!(created[0]["items"], json!(["f"]), "{}", created[0]);
    assert_eq!(created[0]["calls"]["project-show"], 1, "{}", created[0]);
    assert_eq!(created[0]["actions"]["created"], 1, "{}", created[0]);
    let edits = world.events_of(RUN, "edit-committed").len();
    let mark = records(&world).len();
    reply(&world, json!({"op": "reparent", "id": "f", "deps": ["b"]}));
    then_at_rest(&world, "the reparent", |world| {
        world.events_of(RUN, "edit-committed").len() > edits
    });
    // A reparent is the edges alone.
    assert_eq!(
        the_update_of(&world, mark, "f")["updated_fields"],
        json!({"depends-on": 1})
    );

    let failed_before = failures(&world);
    // Each outage is scripted on the first call an attempt makes: the targeted update of the
    // item the settlement changes, as the project read was on the engine before this one.
    world.store_refuses(
        "update_task",
        &json!({"kind": "unavailable", "message": "connection reset by the destination"}),
    );
    go("e");
    world.until("three failed attempts", |world| {
        failures(world) >= failed_before + 3
    });
    world.store_stops_refusing("update_task");
    then_at_rest(&world, "the outage to recover", |world| {
        settled(world, "e", "done")
    });
    assert_eq!(
        failures(&world),
        failed_before + 3,
        "the outage was not three failures"
    );

    world.store_refuses_once(
        "update_task",
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
    // Each attempt that opened the store, as the calls its own connection made: the
    // write-back's connections in order, beside the records of the attempts that opened one.
    let mut connections: Vec<Vec<Vec<String>>> = Vec::new();
    for call in writeback {
        if call[0] == "initialize" {
            connections.push(Vec::new());
        }
        connections
            .last_mut()
            .expect("every write-back call follows its connection's handshake")
            .push(call.clone());
    }
    (figures, recorded, connections)
}

/// The one landed attempt since `mark` that carried `root` alone, as a targeted update.
fn the_update_of(world: &World, mark: usize, root: &str) -> Value {
    let carried: Vec<Value> = records(world)[mark..]
        .iter()
        .filter(|record| record["items"] == json!([root]) && record["outcome"] == "projected")
        .cloned()
        .collect();
    assert_eq!(carried.len(), 1, "{root}: {carried:?}");
    let update = carried[0].clone();
    assert_eq!(update["calls"]["task-update"], 1, "{update}");
    assert!(update["calls"].get("project-copy").is_none(), "{update}");
    update
}

/// The board's item for one lineage, as the real store answers it.
fn board_item(world: &World, project: &str, root: &str) -> Value {
    world
        .store_tasks(project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == root)
        .unwrap_or_else(|| panic!("the board holds no item for {root}"))
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
/// less metered spend than the same scenario took on the engine before the targeted update —
/// with no whole attempt, no attempt whose calls name the project's page of tasks, and a copy
/// only for the one `add`. (The one
/// `query_tasks` the store is asked is inside the copy that creates the `add`ed node: the store
/// looking for a counterpart before it creates one, which is the copy's own work.)
#[test]
fn the_comparable_plan_spends_what_the_committed_record_says() {
    let (measured, recorded, connections) = the_comparable_plan();
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
        "the write-back made {} calls, and the engine before the targeted update made {}",
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
        "the write-back spent {} points, and the engine before the targeted update spent {}",
        points(&measured),
        points(before)
    );
    assert_eq!(measured["writeback"]["whole_attempts"], 0);
    // The one page read the store is asked during the write-back is `Engine::copy`'s own
    // creation lookup — its origin scan for an item it is about to create — so it appears only
    // in an attempt that created an item, at most once per item created, and inside that
    // attempt's copy: never in the reads the write-back makes itself.
    let opened: Vec<&Value> = recorded
        .iter()
        .filter(|record| record["calls"] != json!({}))
        .collect();
    assert_eq!(
        opened.len(),
        connections.len(),
        "an attempt and a connection do not pair"
    );
    let mut lookups = 0;
    for (record, calls) in opened.iter().zip(&connections) {
        let pages = calls.iter().filter(|call| call[0] == "query_tasks").count();
        let created = record["actions"]["created"].as_u64().unwrap_or(0);
        assert!(
            pages as u64 <= created,
            "an attempt that created {created} items read {pages} pages of tasks: {record}"
        );
        if pages > 0 {
            let first_page = calls
                .iter()
                .position(|call| call[0] == "query_tasks")
                .expect("a page");
            let copy_began = calls
                .iter()
                .position(|call| call[0] == "metering")
                .expect("the copy reads the meter before it starts");
            assert!(
                first_page > copy_began,
                "a page of tasks was read before the copy began: {calls:?}"
            );
        }
        lookups += pages;
    }
    assert_eq!(
        measured["writeback"]["calls"]["query_tasks"].as_u64(),
        Some(lookups as u64),
        "a page of tasks was read outside every attempt's copy"
    );
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
        // A copy is made only to create the `add`ed node, and naming only it; the project is
        // read only by that attempt, for its copy; and every other write is a targeted update
        // whose fields the store says it wrote.
        let calls = &record["calls"];
        if calls.get("project-copy").is_some() {
            assert_eq!(record["items"], json!(["f"]), "{record}");
            assert_eq!(calls["project-copy"], 1, "{record}");
        }
        assert!(
            calls.get("project-show").is_none() || calls.get("project-copy").is_some(),
            "an attempt that created nothing read the project: {record}"
        );
        assert_eq!(
            calls.get("task-update").is_some(),
            record.get("updated_fields").is_some(),
            "{record}"
        );
    }
    assert_eq!(
        recorded
            .iter()
            .filter(|record| record["calls"].get("project-copy").is_some()
                && record["outcome"] == "projected")
            .count(),
        1,
        "other than the one `add` was created by a copy"
    );
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
