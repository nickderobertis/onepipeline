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

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{plan_of, read_jsonl, repo_file, Rendezvous, World, RENDEZVOUS_SECONDS_ENV};

const RUN: &str = "comparable-plan";

const RECORD: &str = "tests/golden/writeback-comparable-plan.json";

/// The call the write-back ends each attempt's command on its retained store with.
const END: &str = "end_command";

/// The driver's private failpoint met before it decides whether to start a dispatch of a
/// node [`BEGIN_NODES`] names — before that admission queues its fence — as
/// `tests/e2e/continuation.rs` names it.
const BEGIN_GATE: &str = "ONEPIPELINE_TEST_CLAIM_BEGIN_GATE";
const BEGIN_NODES: &str = "ONEPIPELINE_TEST_CLAIM_BEGIN_NODE";

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

/// Wait until the write-back is at rest: every command the write-back's store was handed has
/// ended and its attempt is recorded, and the last attempt landed. Nothing here times a single
/// attempt, and nothing here can tell an attempt still to come from none: a step first waits
/// for its own projection to have landed — see [`landed`] — and then for this.
fn at_rest(world: &World, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let asked = world.store_calls();
        let ended = asked.iter().filter(|call| call[0] == END).count();
        let recorded = records(world);
        let landed = recorded
            .last()
            .is_some_and(|record| record["outcome"] == "projected");
        // An attempt with nothing to carry asks the store nothing and records that it called
        // nothing; every other attempt ends its command before it is recorded.
        let called_by_attempts = recorded
            .iter()
            .filter(|record| record["calls"] != json!({}))
            .count();
        // The run's home is the one call a settled run makes after its last attempt's command
        // has ended, on a store of its own: entry 134. The fence's writes are on stores of
        // their own and are no attempt's.
        let fence = fence_of(&world.fakes);
        let between_commands = fence.others.last().is_some_and(|call| call[0] == END)
            || !world.events_of(RUN, "home-status").is_empty();
        if landed && between_commands && called_by_attempts == ended && fence.at_rest(world) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the write-back did not come to rest after {what}; the runs root held:\n{}",
            world.dump()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Wait for a step's graph and its projection, then for the write-back to be at rest.
///
/// `ready` names what the step's last projection landed, and not only the graph it moved: the
/// driver journals a dispatch or a settlement *before* it hands the write-back the snapshot
/// carrying it, so a step that waited on the journal alone could find the write-back at rest
/// on the previous step's attempt, the step's own not yet begun.
fn then_at_rest(world: &World, what: &str, ready: impl Fn(&World) -> bool) {
    world.until(what, &ready);
    at_rest(world, what);
}

/// What the write-back last landed on the project, as the run's landed baseline keeps it
/// (`docs/contract-divergences.md` entry 93): each lineage's item by its root, and the
/// project's engine-owned keys. `Null` until there is one to read.
fn landed(world: &World) -> Value {
    std::fs::read_to_string(world.run_file(RUN, "writeback-landed.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

/// Whether each lineage's item last landed under its word.
fn landed_as(world: &World, words: &[(&str, &str)]) -> bool {
    let landed = landed(world);
    words
        .iter()
        .all(|(root, word)| landed["items"][root]["status"] == *word)
}

/// The driver claim the write-back last landed on the project: `Null` where none stands.
fn landed_claim(world: &World) -> Value {
    landed(world)["project_metadata"]["onepipeline.driver"].clone()
}

/// The metadata key the driver fences the nodes it has under way on.
const PUBLISHING: &str = "onepipeline.publishing";

/// The claim fence's writes, told from every other call the store was handed by the
/// connection each came on.
///
/// The fence is the driver's best-effort projection of what it has under way, written off
/// the reconcile loop on a store of its own whenever that changes (`docs/contract-divergences.md`
/// entry 138). Its requests are the run's like any other, and the figures count them: this is
/// only how a journey knows they have finished. A fence connection is one whose every call is
/// its handshake or a write of [`PUBLISHING`].
struct Fence {
    /// Every call but the fence's, in order.
    others: Vec<Vec<String>>,
    /// The fence's connections, in the order each began, with the value each wrote.
    writes: Vec<(u64, Option<Value>)>,
    /// The value the project's [`PUBLISHING`] was last written to, on any connection: the
    /// write-back clears the fence on its own store at the stop and at closeout.
    last: Option<Value>,
    /// The fence connections that have only made their handshake.
    opening: usize,
}

fn fence_of(fakes: &Path) -> Fence {
    let calls: Vec<Value> = read_jsonl(&fakes.join("invocations.jsonl"))
        .into_iter()
        .filter(|call| call["tool"] == crate::harness::SCRIPTED_KEY)
        .collect();
    let connection = |call: &Value| call["connection"].as_u64().expect("a call's connection");
    let mut fencing: BTreeMap<u64, bool> = BTreeMap::new();
    let mut first: Vec<u64> = Vec::new();
    for call in &calls {
        let on = connection(call);
        if !fencing.contains_key(&on) {
            first.push(on);
        }
        let ok = call["args"][0] == "initialize"
            || (call["args"][0] == "set_project_metadata" && call["key"] == PUBLISHING);
        *fencing.entry(on).or_insert(true) &= ok;
    }
    let wrote = |on: u64| {
        calls
            .iter()
            .find(|call| connection(call) == on && call["args"][0] == "set_project_metadata")
            .map(|call| call["value"].clone())
    };
    let mut others = Vec::new();
    for call in &calls {
        let on = connection(call);
        if !(fencing[&on] && wrote(on).is_some()) {
            others.push(serde_json::from_value(call["args"].clone()).expect("a call's arguments"));
        }
    }
    // A connection that has made only its handshake has not shown what it is: it is the
    // fence's only once it writes, and until then nothing is at rest.
    let opening = first
        .iter()
        .filter(|on| {
            calls
                .iter()
                .filter(|call| connection(call) == **on)
                .all(|call| call["args"][0] == "initialize")
        })
        .count();
    let writes = first
        .into_iter()
        .filter(|on| fencing[on])
        .filter_map(|on| wrote(on).map(|value| (on, Some(value))))
        .collect();
    let last = calls
        .iter()
        .rev()
        .find(|call| call["args"][0] == "set_project_metadata" && call["key"] == PUBLISHING)
        .map(|call| call["value"].clone());
    Fence {
        others,
        writes,
        last,
        opening,
    }
}

impl Fence {
    /// Every fence write the run started has been answered, and no store is still opening.
    fn idle(&self, fakes: &Path) -> bool {
        let answered: Vec<u64> = read_jsonl(&fakes.join("store.answered.jsonl"))
            .iter()
            .filter(|answer| answer["method"] == "set_project_metadata")
            .filter_map(|answer| answer["connection"].as_u64())
            .collect();
        self.opening == 0 && self.writes.iter().all(|(on, _)| answered.contains(on))
    }

    /// The nodes the fence connections' latest write names.
    fn latest(&self) -> BTreeSet<String> {
        self.writes
            .last()
            .map_or_else(BTreeSet::new, |(_, value)| nodes_of(value.as_ref()))
    }

    /// The fence is idle, and the project's fence names exactly what the journal has under
    /// way: no write is still to come.
    fn at_rest(&self, world: &World) -> bool {
        self.idle(&world.fakes) && nodes_of(self.last.as_ref()) == under_way(&world.journal(RUN))
    }
}

/// The nodes a write of [`PUBLISHING`] names: none, for a cleared fence.
fn nodes_of(value: Option<&Value>) -> BTreeSet<String> {
    value
        .and_then(|value| value["nodes"].as_array())
        .into_iter()
        .flatten()
        .map(|node| node.as_str().expect("a fenced node").to_owned())
        .collect()
}

/// The nodes `journal` has dispatched and not settled since.
fn under_way(journal: &[Value]) -> BTreeSet<String> {
    let mut under_way = BTreeSet::new();
    for event in journal {
        let Some(node) = event["labels"]["node"].as_str() else {
            continue;
        };
        if event["kind"] == "node-dispatched" {
            under_way.insert(node.to_owned());
        } else if event["kind"] == "node-settled" {
            under_way.remove(node);
        }
    }
    under_way
}

/// The run, paced so the claim fence never has two snapshots to coalesce.
///
/// The fence worker coalesces the snapshots a driver makes while one of its writes is in
/// flight into the newest (`docs/contract-divergences.md` entry 138), so how many writes a
/// run takes would depend on how fast the store answers. Two things make a snapshot here: an
/// admission, which queues the fence with its node before it dispatches, and a write-back
/// attempt landing a settlement, after which the driver's next pass lets the node go. The
/// journey holds each before it can make one — an admission at the driver's begin failpoint,
/// an attempt at its first targeted update — until the fence's previous write has finished.
/// So every run makes the same fence writes, and the figures count every one of them.
struct Pacer {
    /// The admissions the step under way makes beside a settlement, each as the node and how
    /// many dispatches of it the journal holds once it has: an attempt waits for their fence
    /// writes, so the settlement it lands lets go of its node only after them. A step whose
    /// admissions wait on a projection — a driver's first, carrying its claim — names none.
    admits: Arc<Mutex<Vec<(String, usize)>>>,
}

impl Pacer {
    /// Stand at both holds, through the driver's begin failpoint on `begin`.
    fn start(world: &World, begin: Rendezvous) -> Self {
        let admits: Arc<Mutex<Vec<(String, usize)>>> = Arc::default();
        let fakes = world.fakes.clone();
        let events = world.run_file(RUN, "events.jsonl");
        // An admission goes once the fence is idle and, after a driver's first, once the
        // write its previous admission queued has been made: idle, that write is finished.
        let at = fakes.clone();
        std::thread::spawn(move || {
            let mut released: BTreeMap<u32, usize> = BTreeMap::new();
            loop {
                let arrival = begin.arrived();
                let written = paced(|| {
                    let fence = fence_of(&at);
                    let after = released
                        .get(&arrival.pid)
                        .is_none_or(|before| fence.writes.len() > *before);
                    (after && fence.idle(&at)).then_some(fence.writes.len())
                });
                released.insert(arrival.pid, written);
                arrival.release();
            }
        });
        // An attempt goes once the fence is idle and its latest write names every admission
        // the step makes, so no landing races an admission's write.
        let attempts = world.rendezvous(&format!(
            "{}.update_task.first",
            crate::harness::SCRIPTED_KEY
        ));
        let expected = Arc::clone(&admits);
        std::thread::spawn(move || loop {
            let arrival = attempts.arrived();
            paced(|| {
                let fence = fence_of(&fakes);
                let journal = read_jsonl(&events);
                let latest = fence.latest();
                let admitted = expected
                    .lock()
                    .expect("the step")
                    .iter()
                    .all(|(node, times)| {
                        journal
                            .iter()
                            .filter(|event| {
                                event["kind"] == "node-dispatched"
                                    && event["labels"]["node"] == *node
                            })
                            .count()
                            >= *times
                            && latest.contains(node)
                    });
                (admitted && fence.idle(&fakes)).then_some(())
            });
            arrival.release();
        });
        Self { admits }
    }

    /// The admissions the next step makes.
    fn expect(&self, admits: &[(&str, usize)]) {
        *self.admits.lock().expect("the step") = admits
            .iter()
            .map(|(node, times)| ((*node).to_owned(), *times))
            .collect();
    }
}

/// Poll `ready` until it answers, guarded only against a hang.
fn paced<T>(mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(answer) = ready() {
            return answer;
        }
        assert!(Instant::now() < deadline, "the fence never came to rest");
        std::thread::sleep(Duration::from_millis(20));
    }
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
    // Every admission meets the driver's begin failpoint, which the pacer stands at.
    // llmlint: ignore-block[tests_mirror_real_usage] no CLI input can hold one admission
    // until the fence's previous write has finished, and without that the fence coalesces on
    // the store's timing and the record's figures cannot be the same on every run. The
    // failpoint only orders the run: every figure is a store call the real binary made.
    let begin = Rendezvous::failpoint();
    let world = world
        .through_scripted_source()
        .with_env(RENDEZVOUS_SECONDS_ENV, "600")
        .with_env(BEGIN_GATE, &begin.address())
        .with_env(BEGIN_NODES, "a,b,c,d,e,d-2,f");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let go = |node: &str| world.release(&format!("{node}.go"));
    let pacer = Pacer::start(&world, begin);

    world.run(&["start", &project, "--detach"]).exited(0);
    then_at_rest(&world, "the first dispatches", |world| {
        ["a", "d", "e"]
            .iter()
            .all(|node| dispatched(world, node) == 1)
            && landed_as(
                world,
                &[
                    ("a", "in progress"),
                    ("b", "queued"),
                    ("c", "queued"),
                    ("d", "in progress"),
                    ("e", "in progress"),
                ],
            )
    });

    pacer.expect(&[]);
    go("d");
    then_at_rest(&world, "the failure", |world| {
        settled(world, "d", "failed") && landed_as(world, &[("d", "failed")])
    });
    let mark = records(&world).len();
    pacer.expect(&[("d-2", 1)]);
    reply(
        &world,
        json!({"op": "retry", "id": "d", "node": node("d-2", &[])}),
    );
    then_at_rest(&world, "the retry", |world| {
        dispatched(world, "d-2") == 1
            && landed_as(world, &[("d", "in progress")])
            && landed(world)["items"]["d"]["metadata"]["onepipeline.node"] == "d-2"
    });
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

    pacer.expect(&[]);
    let mark = records(&world).len();
    reply(
        &world,
        json!({"op": "amend", "id": "c", "text": "## What\nDo c, as amended.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- c is done."}),
    );
    then_at_rest(&world, "the amend", |world| {
        !world.events_of(RUN, "edit-committed").is_empty()
            && landed(world)["items"]["c"]["metadata"]["onepipeline.amendment"]
                .as_str()
                .is_some_and(|text| text.contains("as amended"))
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
            && landed_as(world, &[("f", "queued")])
            && landed(world)["items"]["f"]["depends_on"] == json!(["e"])
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
            && landed(world)["items"]["f"]["depends_on"] == json!(["b"])
    });
    // A reparent is the edges alone.
    assert_eq!(
        the_update_of(&world, mark, "f")["updated_fields"],
        json!({"depends-on": 1})
    );

    let failed_before = failures(&world);
    // Each outage is scripted on the first call an attempt makes: the targeted update of the
    // item the settlement changes, as the project read was on the engine before this one.
    pacer.expect(&[]);
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
        settled(world, "e", "done") && landed_as(world, &[("e", "done")])
    });
    assert_eq!(
        failures(&world),
        failed_before + 3,
        "the outage was not three failures"
    );

    pacer.expect(&[("b", 1)]);
    world.store_refuses_once(
        "update_task",
        &json!({"kind": "rate-limited", "retry_after_seconds": 2,
                "message": "API rate limit exceeded"}),
    );
    go("a");
    then_at_rest(&world, "the rate limit to be waited out", |world| {
        settled(world, "a", "done")
            && dispatched(world, "b") == 1
            && landed_as(world, &[("a", "done"), ("b", "in progress")])
    });
    assert_eq!(
        failures(&world),
        failed_before + 4,
        "the rate limit was not one failure"
    );

    let stopped = landed_claim(&world);
    pacer.expect(&[]);
    world.run(&["stop", RUN]).exited(0);
    let recorded = records(&world).len();
    world.run(&["adopt", RUN, "--detach"]).exited(0);
    // The adopting driver's claim, and the two dispatches it made again in progress under it.
    then_at_rest(&world, "the adoption", |world| {
        let claim = landed_claim(world);
        records(world).len() > recorded
            && dispatched(world, "b") == 2
            && dispatched(world, "d-2") == 2
            && claim.is_object()
            && claim != stopped
            && landed_as(
                world,
                &[
                    ("b", "in progress"),
                    ("c", "queued"),
                    ("d", "in progress"),
                    ("f", "queued"),
                ],
            )
    });

    pacer.expect(&[("c", 1), ("f", 1)]);
    go("b");
    then_at_rest(&world, "b", |world| {
        settled(world, "b", "done")
            && dispatched(world, "c") == 1
            && landed_as(
                world,
                &[("b", "done"), ("c", "in progress"), ("f", "in progress")],
            )
    });
    pacer.expect(&[]);
    go("d-2");
    then_at_rest(&world, "d-2", |world| {
        settled(world, "d-2", "done") && landed_as(world, &[("d", "done")])
    });
    go("c");
    then_at_rest(&world, "c", |world| {
        settled(world, "c", "done")
            && dispatched(world, "f") == 1
            && landed_as(world, &[("c", "done")])
    });
    go("f");
    // The closeout's own attempt is the one that releases the run's driver claim.
    then_at_rest(&world, "closeout", |world| {
        world.run_file(RUN, "result.json").is_file()
            && landed_as(world, &[("f", "done")])
            && landed_claim(world).is_null()
    });
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
    // The run's home, marked `done` once the run has settled — the last calls the store is
    // asked, on a store of their own after the write-back's last command ended: entry 134.
    let last_end = writeback
        .iter()
        .rposition(|call| call[0] == END)
        .expect("the write-back ended a command");
    let (writeback, home) = writeback.split_at(last_end + 1);
    let charged: Vec<Value> = std::fs::read_to_string(world.fakes.join("store.charged.jsonl"))
        .expect("the meter charged something")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a charge is JSON"))
        .collect();
    // Every request the launch's read made was answered, so its charges are the first of them.
    let answered = |calls: &[Vec<String>]| {
        calls
            .iter()
            .filter(|call| call[0] != "initialize" && call[0] != "metering")
            .count()
    };
    let launch_charged = answered(launch);
    let home_charges_start = charged.len() - answered(home);
    let recorded = records(&world);
    let figures = json!({
        "launch_read": {
            "calls": by_method(launch.iter()),
            "spent": spent(charged[..launch_charged].iter()),
        },
        "writeback": {
            "calls": by_method(writeback.iter()),
            "calls_total": writeback.len(),
            "spent": spent(charged[launch_charged..home_charges_start].iter()),
            "attempts": recorded.len(),
            "failed_attempts": recorded.iter().filter(|record| record["outcome"] == "failed").count(),
            "whole_attempts": recorded.iter().filter(|record| record["scope"] == "whole").count(),
        },
        "home": {
            "calls": by_method(home.iter()),
            "spent": spent(charged[home_charges_start..].iter()),
        },
    });
    // Each attempt that called the store, as the calls its own command made: the write-back's
    // calls in order, each command closed by its end, beside the records of the attempts that
    // called the store. A store built for an attempt opens its command with the handshake.
    let mut commands: Vec<Vec<Vec<String>>> = vec![Vec::new()];
    for call in writeback {
        commands
            .last_mut()
            .expect("a command is always open")
            .push(call.clone());
        if call[0] == END {
            commands.push(Vec::new());
        }
    }
    assert_eq!(
        commands.pop(),
        Some(Vec::new()),
        "the write-back's last command never ended"
    );
    (figures, recorded, commands)
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
    let (measured, recorded, commands) = the_comparable_plan();
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
    let called: Vec<&Value> = recorded
        .iter()
        .filter(|record| record["calls"] != json!({}))
        .collect();
    assert_eq!(
        called.len(),
        commands.len(),
        "an attempt and a command do not pair"
    );
    let mut lookups = 0;
    for (record, calls) in called.iter().zip(&commands) {
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
    // The closeout carries no item — every node has been projected as it settled — and
    // releases the run's driver claim: one read of the project, which finds the claim still
    // this run's, and one write of it (`docs/contract-divergences.md` entry 119); and clears
    // the claim fence: one read, which finds the fence the paused fence worker last wrote
    // still naming this run, and one write of it (entry 138). Before the claim existed this
    // was the attempt left with nothing to carry.
    let closeout = recorded.last().expect("the closeout was recorded");
    assert_eq!(closeout["items"], json!([]), "{closeout}");
    assert_eq!(
        closeout["calls"],
        json!({"project-show": 2, "project-metadata-set": 2}),
        "{closeout}"
    );
    // An attempt with nothing left to carry asks the store nothing, not even to end a command,
    // and its line names no items, no calls and no report.
    let idle: Vec<&Value> = recorded
        .iter()
        .filter(|record| record["calls"] == json!({}))
        .collect();
    for record in &idle {
        assert_eq!(record["items"], json!([]), "{record}");
        assert_eq!(record["outcome"], "projected", "{record}");
        assert_eq!(record["actions"], Value::Null, "{record}");
    }
    assert_eq!(
        measured["writeback"]["calls"][END].as_u64(),
        Some((recorded.len() - idle.len()) as u64),
        "an attempt that called nothing ended a command, or one that called the store did not"
    );
    for record in &recorded {
        assert_eq!(record["scope"], "members", "{record}");
        assert!(
            record["calls"].is_object() && record["calls"].get("task-list").is_none(),
            "an attempt read the project's page of tasks: {record}"
        );
        // A copy is made only to create the `add`ed node, and naming only it; the project is
        // read only by that attempt, for its copy, and by the two that release the run's
        // driver claim — the stop's and the closeout's — each reading it once and writing the
        // claim beside it, and reading it again to clear the claim fence where it still names
        // this run (entry 138); and every other write is a targeted update whose fields the
        // store says it wrote.
        let calls = &record["calls"];
        if calls.get("project-copy").is_some() {
            assert_eq!(record["items"], json!(["f"]), "{record}");
            assert_eq!(calls["project-copy"], 1, "{record}");
        }
        let releases = [1, 2].iter().any(|times| {
            calls.get("project-show") == Some(&json!(times))
                && calls.get("project-metadata-set") == Some(&json!(times))
        });
        assert!(
            calls.get("project-show").is_none() || calls.get("project-copy").is_some() || releases,
            "an attempt that created nothing and released no claim read the project: {record}"
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
