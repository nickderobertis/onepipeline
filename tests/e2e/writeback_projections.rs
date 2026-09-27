//! What a settlement write-back projection carries, and the record every attempt leaves.
//!
//! A projection used to copy every node of the plan to change one, and nothing on the run
//! said what that cost. What runs here is the real projection through the `onetaskgraph`
//! library the binary under test links, over a local Markdown destination reached through
//! `scripted-source`: a real source of that store serving the same folder through the real
//! `local-md` plugin, which records every call it is handed. What lands on the board is the
//! store's own work; what the source adds is a log of every call, and — for the one journey
//! about the figures a copy reports — a meter on its own writes, the way a hosted source
//! meters its requests. `crates/testfakes/src/bin/scripted-source.rs` says how each is
//! scripted.
//!
//! Entry 73 of `docs/contract-divergences.md` is the source for the record these journeys
//! read, and where the record lives is read out of it rather than restated.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// The scripted source here serves every call out of the real `local-md` plugin and records it,
// so what lands on the board is the real store's own; the scripted refusals and the meter stand
// in only for what an offline store cannot be made to answer. `harness.rs` carries the same
// suppression and the full rationale.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{agent, plan_of, project_id, World, CANCEL_GRACE_ENV, RENDEZVOUS_SECONDS_ENV};

/// Entry 73's block, which is where the record's path is written down.
fn proposed() -> Value {
    let record = std::fs::read_to_string(crate::harness::repo_file("docs/contract-divergences.md"))
        .expect("the divergence record ships");
    let entry = record
        .split("\n## ")
        .find(|entry| entry.starts_with("73."))
        .expect("the record still carries entry 73");
    let block = entry
        .split("```json")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("entry 73 carries the json block these journeys read");
    serde_json::from_str(block).expect("entry 73's block is JSON")
}

/// One of entry 73's `<run dir>/…` paths, relative to the run's directory.
fn in_run_dir(path: &Value) -> String {
    path.as_str()
        .and_then(|path| path.strip_prefix("<run dir>/"))
        .unwrap_or_else(|| panic!("entry 73 names no path in the run's directory: {path}"))
        .to_owned()
}

/// Every projection attempt the run has recorded, in order.
fn records(world: &World, run: &str) -> Vec<Value> {
    let path = world.run_file(run, &in_run_dir(&proposed()["projection"]["record"]));
    let Ok(written) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    written
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("a record line is not JSON ({error}): {line}"))
        })
        .collect()
}

/// Every call the scripted source was handed, in order, as `[method, the id it names]`.
fn store_calls(world: &World) -> Vec<Vec<String>> {
    world.store_calls()
}

fn is_call(call: &[String], method: &str) -> bool {
    call.first().is_some_and(|called| called == method)
}

/// The native half of a qualified id: what the store's plugin protocol names an item by.
fn native(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, native)| native)
}

/// Whether every attempt the worker has made has been recorded and every record landed — so
/// no attempt is in flight and none failed.
///
/// Counted, not timed: each command that reaches the store opens a connection of its own to
/// the scripted source, whose handshake is the first thing it records. One of those was the
/// plan read the launch made; each of the rest is an attempt, which is in flight from its
/// handshake until the worker appends its record.
fn every_attempt_landed(world: &World, run: &str) -> bool {
    let opened = store_calls(world)
        .iter()
        .filter(|call| is_call(call, "initialize"))
        .count();
    let records = records(world, run);
    records.len() + 1 == opened
        && records
            .iter()
            .all(|record| record["outcome"] == "projected")
}

fn board_word(tasks: &[Value], node: &str) -> Option<String> {
    tasks.iter().find_map(|task| {
        (task["item"]["metadata"]["onepipeline.id"] == node)
            .then(|| {
                task["item"]["status"]["category"]
                    .as_str()
                    .map(str::to_owned)
            })
            .flatten()
    })
}

fn board_task<'a>(tasks: &'a [Value], node: &str) -> &'a Value {
    tasks
        .iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .unwrap_or_else(|| panic!("the board holds no item for {node}"))
}

/// A detached run whose every store call goes through the recording source in front of the
/// real store. `held` nodes stay running until the journey releases them.
fn a_run_projecting_through_a_recording_store(
    world: &str,
    run: &str,
    nodes: Vec<Value>,
    held: &[&str],
) -> (World, String) {
    let world = World::new(world);
    for node in held {
        world.script(&format!("{node}.wait"), "hold");
    }
    let project = world.plan(run, &plan_of(run, nodes));
    let world = world
        .through_scripted_source()
        // A held node has to outlast everything the journey does to the board around it, an
        // adoption included; the store journeys hold theirs under the same setting.
        .with_env(RENDEZVOUS_SECONDS_ENV, "600");
    world.run(&["start", &project, "--detach"]).exited(0);
    (world, project)
}

/// Wait until the board holds what `ready` asks and no projection is in flight.
fn projected_until(
    world: &World,
    run: &str,
    project: &str,
    what: &str,
    ready: impl Fn(&[Value]) -> bool,
) {
    world.until_store(what, |world| {
        ready(&world.store_tasks(project)) && every_attempt_landed(world, run)
    });
}

/// Give a run something new to project: a note for one node, which changes that node's
/// projected metadata and nothing else's.
fn noted(world: &World, run: &str, node: &str, text: &str) {
    world
        .run_with_stdin(
            &["reply", run],
            &json!({
                "version": 2,
                "commands": [{"op": "note", "id": node, "addressee": "worker",
                              "text": text, "deliver": "next"}]
            })
            .to_string(),
        )
        .exited(0);
}

/// Rewrite one front-matter field of a destination document, the way a person editing the
/// board's own file does.
fn amend(path: &Path, key: &str, value: Value) {
    rewritten(path, path, |front| {
        front.insert(key.to_owned(), value);
    });
}

/// Rewrite a destination document's front matter with `edit`, from `path` to `into` — the same
/// file for an edit in place, another for a document modelled on this one.
fn rewritten(path: &Path, into: &Path, edit: impl FnOnce(&mut serde_json::Map<String, Value>)) {
    let document = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
    let (front, body) = document
        .strip_prefix("---\n")
        .expect("a store document opens its front matter")
        .split_once("---\n")
        .expect("a store document closes its front matter");
    let mut parsed: serde_json::Map<String, Value> =
        serde_norway::from_str(front).expect("the front matter is YAML");
    edit(&mut parsed);
    let rendered = serde_norway::to_string(&parsed).expect("the front matter renders");
    std::fs::write(into, format!("---\n{rendered}---\n{body}")).expect("the document is written");
}

/// How the run's shadow store names a project's folder and a lineage's file: the id's
/// bytes as hex, which is what keeps any id a file name.
fn hex(id: &str) -> String {
    id.bytes().map(|byte| format!("{byte:02x}")).collect()
}

/// The file a `local-md` destination keeps one item in: its id's local half, under the
/// project's tasks folder.
fn item_file(world: &World, task: &Value) -> std::path::PathBuf {
    let id = task["id"].as_str().expect("an item id");
    let local = id
        .split_once(':')
        .map(|(_, local)| local)
        .expect("a qualified id");
    world.store().join("tasks").join(format!("{local}.md"))
}

/// Cancel one node, and wait for the reconciler to commit it.
fn cancelled(world: &World, run: &str, node: &str) {
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "cancel", "id": node}]}).to_string(),
        )
        .exited(0);
    world.until(&format!("the cancel of {node} to settle it"), |world| {
        world.events_of(run, "node-settled").iter().any(|event| {
            event["labels"]["node"] == node && event["payload"]["status"] == "cancelled"
        })
    });
}

fn edit(world: &World, run: &str, command: Value) {
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [command]}).to_string(),
        )
        .exited(0);
}

/// End the run's quiet driver and adopt it, so a second driver projects the same run.
fn adopted(world: &World, run: &str) {
    world.until("the quiet driver to be reported parked", |world| {
        let mut status = world.cmd(&["status", run]);
        status.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
        let out = status.output().expect("the binary runs");
        String::from_utf8_lossy(&out.stdout).contains("PARKED")
    });
    let mut adopt = world.cmd(&["adopt", run, "--detach"]);
    adopt.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    world.run_on(adopt, "adopt --detach").exited(0);
}

/// What a destination says when it declines a write or a read, in the store's own shape.
fn source_refused(message: &str) -> Value {
    json!({"kind": "refused", "message": message})
}

/// A run's first projection carries the whole project and is recorded `whole` / `first`; a
/// later transition of one node carries that node alone. The named node reaches the board as
/// ever, a label a person put on it included; the node the copy did not name keeps its item
/// byte for byte as a person edited it, a declared field included; and the commands the worker
/// handed the store for that projection read the project item and the named member only — no
/// page of tasks, and no read of the unnamed member. A driver an adoption starts projects whole
/// again, as its own first.
#[test]
fn a_runs_first_projection_is_whole_and_a_later_transition_carries_that_node_alone() {
    let run = "projections-members";
    let (world, project) = a_run_projecting_through_a_recording_store(
        "writeback-projections-members",
        run,
        vec![agent("work", &[]), agent("aside", &[])],
        &["work", "aside"],
    );
    projected_until(
        &world,
        run,
        &project,
        "both nodes to reach the board running",
        |tasks| {
            board_word(tasks, "work").as_deref() == Some("in-progress")
                && board_word(tasks, "aside").as_deref() == Some("in-progress")
        },
    );

    let first = &records(&world, run)[0];
    assert_eq!(first["scope"], "whole", "{first}");
    assert_eq!(first["whole_because"], "first", "{first}");
    assert_eq!(first["outcome"], "projected", "{first}");
    assert_eq!(first["project"], project.as_str(), "{first}");
    assert_eq!(first["items"], json!(["aside", "work"]), "{first}");
    assert!(first["actions"].is_object(), "{first}");
    // A local Markdown destination meters nothing, so the report carries no `spent`.
    assert_eq!(first["spent"], Value::Null, "{first}");
    let at = first["at"]
        .as_str()
        .expect("an attempt names when it started");
    assert!(
        at.len() >= 20 && at.as_bytes()[10] == b'T' && at.ends_with('Z'),
        "`at` is not an RFC 3339 UTC time: {at}"
    );
    assert!(first["duration_ms"].is_u64(), "{first}");
    for absent in ["class", "kind", "reason"] {
        assert_eq!(first[absent], Value::Null, "{first}");
    }

    // A person edits the board: a declared field of the node the next copy will not name, and
    // a label on the one it will.
    // llmlint: ignore-block[tests_mirror_real_usage] a `local-md` destination *is* its folder of
    // Markdown, so a person editing the board edits that file; `onetaskgraph` has no verb that
    // retitles or labels an item in place, and `store.rs` edits authored items the same way.
    let identifier = project_id(run);
    let tasks_dir = world.store().join("tasks").join(&identifier);
    let aside_file = tasks_dir.join("001-aside.md");
    amend(
        &aside_file,
        "title",
        json!("A person retitled this on the board"),
    );
    amend(
        &tasks_dir.join("000-work.md"),
        "labels",
        json!(["needs-review"]),
    );
    // llmlint: ignore-end[tests_mirror_real_usage]
    let edited = std::fs::read(&aside_file).expect("the edited item reads");
    let tasks = world.store_tasks(&project);
    let work_origin = board_task(&tasks, "work")["id"]
        .as_str()
        .expect("an item id")
        .to_owned();
    let aside_origin = board_task(&tasks, "aside")["id"]
        .as_str()
        .expect("an item id")
        .to_owned();
    let recorded_before = records(&world, run).len();
    let asked_before = store_calls(&world).len();

    world.release("work.go");
    projected_until(
        &world,
        run,
        &project,
        "the finished node to reach the board",
        |tasks| board_word(tasks, "work").as_deref() == Some("done"),
    );

    let later = records(&world, run)[recorded_before..].to_vec();
    assert!(!later.is_empty(), "the transition was never projected");
    // Each attempt opened a store of its own — the source started afresh, handshake and all —
    // so none of them answered from a read an earlier one made.
    let opened = store_calls(&world)[asked_before..]
        .iter()
        .filter(|call| is_call(call, "initialize"))
        .count();
    assert!(
        opened >= later.len(),
        "{} attempts were made over {opened} stores, so one answered from another's reads",
        later.len()
    );
    for record in &later {
        assert_eq!(record["scope"], "members", "{record}");
        assert_eq!(record["whole_because"], Value::Null, "{record}");
        assert_eq!(record["items"], json!(["work"]), "{record}");
        assert_eq!(record["outcome"], "projected", "{record}");
        assert!(record["actions"].is_object(), "{record}");
    }

    assert_eq!(
        std::fs::read(&aside_file).expect("the unnamed item reads"),
        edited,
        "a member projection rewrote an item it did not name"
    );
    let tasks = world.store_tasks(&project);
    let work = board_task(&tasks, "work");
    assert!(
        work["item"]["metadata"]["onepipeline.settlement"].is_object(),
        "the named node reached the board without its settlement: {work}"
    );
    assert_eq!(
        work["item"]["labels"],
        json!([{"id": "needs-review", "name": "needs-review", "color": null}]),
        "the named node's label did not survive its projection"
    );

    // What the member projection asked the store for: the project item, the named member's own
    // item, and the writes of the copy — never a page of tasks, and never the unnamed member.
    let calls = store_calls(&world);
    let member_calls = &calls[asked_before..];
    assert!(
        member_calls.iter().any(|call| is_call(call, "write_task")),
        "no copy wrote the transition: {member_calls:?}"
    );
    for call in member_calls {
        let named = call.get(1).map(String::as_str).unwrap_or_default();
        match call[0].as_str() {
            "initialize"
            | "get_project"
            | "write_project"
            | "task_dependencies"
            | "metering"
            | "project_dependencies"
            | "health" => {}
            "get_task" | "write_task" => assert_eq!(
                named,
                native(&work_origin),
                "a member projection read or wrote an item other than the member it names: \
                 {call:?}"
            ),
            _ => panic!("a member projection asked the store for something else: {call:?}"),
        }
        assert_ne!(
            named,
            native(&aside_origin),
            "a member projection read the member it does not name: {call:?}"
        );
    }

    // A second driver has landed nothing yet, so its first projection is whole again.
    let recorded_before = records(&world, run).len();
    adopted(&world, run);
    world.until("the adopted driver's first projection", |world| {
        records(world, run).len() > recorded_before
    });
    let adopted_first = &records(&world, run)[recorded_before];
    assert_eq!(adopted_first["scope"], "whole", "{adopted_first}");
    assert_eq!(adopted_first["whole_because"], "first", "{adopted_first}");
}

/// The attempt after a failed one is whole and recorded `after-failure`, whatever failed: the
/// store refusing a member copy, and then a whole projection's own page of tasks, each recorded
/// failed with the store's class and kind and the reason, carrying no report. The planner is told
/// the items the copy carried — the one node, for the member copy. Once a whole projection lands,
/// the next change is carried as members again.
#[test]
fn a_projection_after_a_failed_attempt_is_whole() {
    let run = "projections-after-failure";
    let (world, project) = a_run_projecting_through_a_recording_store(
        "writeback-projections-after-failure",
        run,
        vec![agent("work", &[]), agent("later", &["work"])],
        &["work"],
    );
    projected_until(
        &world,
        run,
        &project,
        "the running node to reach the board",
        |tasks| board_word(tasks, "work").as_deref() == Some("in-progress"),
    );
    let mark = records(&world, run).len();
    let recorded = |count: usize| {
        world.until(&format!("{count} more attempts to be recorded"), |world| {
            records(world, run).len() >= mark + count
        });
        records(&world, run)
    };

    world.store_refuses(
        "write_task",
        &source_refused("the item that copy would update is one the destination no longer holds"),
    );
    noted(&world, run, "later", "refused at the copy");
    let member = recorded(1)[mark].clone();
    assert_eq!(member["scope"], "members", "{member}");
    assert_eq!(member["items"], json!(["later"]), "{member}");
    assert_eq!(member["outcome"], "failed", "{member}");
    assert_eq!(member["class"], "refused", "{member}");
    assert_eq!(member["kind"], "refused", "{member}");
    assert!(
        member["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("no longer holds")),
        "{member}"
    );
    assert_eq!(member["actions"], Value::Null, "{member}");
    assert_eq!(member["spent"], Value::Null, "{member}");
    world.until("the refused copy to reach the planner", |world| {
        world
            .events_of(run, "planner-surface-queued")
            .iter()
            .any(|event| {
                event["payload"]["message"]
                    .as_str()
                    .is_some_and(|said| said.contains("did not take this run's projection"))
            })
    });
    let surface = world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .find_map(|event| {
            event["payload"]["message"]
                .as_str()
                .filter(|said| said.contains("did not take this run's projection"))
                .map(str::to_owned)
        })
        .expect("a projection surface");
    assert_eq!(
        surface
            .lines()
            .find_map(|line| line.strip_prefix("items: ")),
        Some("later"),
        "the surface names other items than the copy carried: {surface}"
    );
    world.store_stops_refusing("write_task");

    world.store_refuses(
        "query_tasks",
        &source_refused("source plans refused the request"),
    );
    let asked_before = store_calls(&world).len();
    let board_before = world.store_tasks(&project);
    noted(&world, run, "later", "refused at the page of tasks");
    let whole = recorded(2)[mark + 1].clone();
    assert_eq!(whole["scope"], "whole", "{whole}");
    assert_eq!(whole["whole_because"], "after-failure", "{whole}");
    assert_eq!(whole["items"], json!(["later", "work"]), "{whole}");
    assert_eq!(whole["outcome"], "failed", "{whole}");
    assert_eq!(whole["class"], "refused", "{whole}");
    assert_eq!(whole["kind"], "refused", "{whole}");
    // A destination read that answered in part ends the projection there: nothing was
    // written, and the board holds what it held before.
    let asked = store_calls(&world)[asked_before..].to_vec();
    assert!(
        asked
            .iter()
            .all(|call| !call[0].starts_with("write_") && !call[0].starts_with("set_")),
        "a projection whose page of tasks answered in part wrote to the board: {asked:?}"
    );
    assert_eq!(
        world.store_tasks(&project),
        board_before,
        "a projection whose page of tasks answered in part changed the board"
    );
    world.store_stops_refusing("query_tasks");

    noted(&world, run, "later", "projected whole");
    let landed = recorded(3)[mark + 2].clone();
    assert_eq!(landed["scope"], "whole", "{landed}");
    assert_eq!(landed["whole_because"], "after-failure", "{landed}");
    assert_eq!(landed["outcome"], "projected", "{landed}");
    world.until_store("the whole projection to reach the board", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"] == "projected whole"
        })
    });

    noted(&world, run, "later", "members again");
    let again = recorded(4)[mark + 3].clone();
    assert_eq!(again["scope"], "members", "{again}");
    assert_eq!(again["items"], json!(["later"]), "{again}");
    assert_eq!(again["outcome"], "projected", "{again}");

    // A failure a wait can change is retried on the schedule, and the retry is whole: the read
    // of the named member failing is recorded failed and `transient`, and the attempt the
    // schedule makes next reads the page of tasks rather than that member, and lands.
    world.store_refuses_once(
        "get_task",
        &json!({"kind": "unavailable", "message": "the connection was reset"}),
    );
    noted(&world, run, "later", "retried whole");
    let retried = recorded(6);
    let unread = &retried[mark + 4];
    assert_eq!(unread["scope"], "members", "{unread}");
    assert_eq!(unread["items"], json!(["later"]), "{unread}");
    assert_eq!(unread["outcome"], "failed", "{unread}");
    assert_eq!(unread["class"], "transient", "{unread}");
    assert_eq!(unread["kind"], "unavailable", "{unread}");
    assert!(
        unread["reason"].as_str().is_some_and(
            |reason| reason.contains("task-show") && reason.contains("connection was reset")
        ),
        "{unread}"
    );
    let recovered = &retried[mark + 5];
    assert_eq!(recovered["scope"], "whole", "{recovered}");
    assert_eq!(recovered["whole_because"], "after-failure", "{recovered}");
    assert_eq!(recovered["outcome"], "projected", "{recovered}");
    world.until_store("the retried projection to reach the board", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"] == "retried whole"
        })
    });
    assert!(
        !world.fakes.join("store.get_task.refuse.once").exists(),
        "the member read was never asked, so nothing failed it"
    );
}

/// The record carries exactly what the copy said it did and spent: its `spent` object as the
/// store reported it, and its per-item actions counted. The destination here meters its own
/// requests, as a hosted source does — each request it serves spends one request and three
/// points — so the store reports a `spent` for the copy, and a worker recording it as anything
/// but what the store said fails here, as does one miscounting what the copy did. The copy
/// after the meter is taken away reports what a source that meters nothing reports.
#[test]
fn the_record_carries_exactly_what_the_copy_report_said_it_did_and_spent() {
    let run = "projections-report";
    let (world, project) = a_run_projecting_through_a_recording_store(
        "writeback-projections-report",
        run,
        vec![agent("work", &[]), agent("later", &["work"])],
        &["work"],
    );
    projected_until(
        &world,
        run,
        &project,
        "the running node to reach the board",
        |tasks| board_word(tasks, "work").as_deref() == Some("in-progress"),
    );

    let meter = "store.metering";
    world.script(
        meter,
        &json!({"requests": 1, "budgets": [
            {"budget": "graphql", "unit": "points", "measured": 3, "modelled": 0}
        ]})
        .to_string(),
    );
    let mark = records(&world, run).len();
    let asked_before = store_calls(&world).len();
    noted(&world, run, "later", "counted");
    world.until("the metered copy to be recorded", |world| {
        records(world, run).len() > mark
    });

    let counted = records(&world, run)[mark].clone();
    assert_eq!(counted["outcome"], "projected", "{counted}");
    assert_eq!(counted["scope"], "members", "{counted}");
    // What the metered source served for that copy: every request between the two readings
    // the store takes of its meter, one before the copy and one after, read off the source's
    // own record of calls.
    let calls = store_calls(&world)[asked_before..].to_vec();
    let readings: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| is_call(call, "metering"))
        .map(|(at, _)| at)
        .collect();
    assert!(
        readings.len() >= 2,
        "the store read its meter fewer than twice: {calls:?}"
    );
    let served = (readings[1] - readings[0] - 1) as u64;
    assert!(
        served > 0,
        "the metered copy asked the source nothing: {calls:?}"
    );
    assert_eq!(
        counted["spent"],
        json!({"requests": served, "budgets": [
            {"budget": "graphql", "unit": "points", "amount": 3 * served, "lower_bound": false}
        ]}),
        "{counted}"
    );
    // The project item already read as the source did; the named member was rewritten.
    assert_eq!(
        counted["actions"],
        json!({"created": 0, "updated": 1, "unchanged": 1, "orphaned": 0, "reopened": 0}),
        "{counted}"
    );
    world.until_store("the metered copy to land", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"] == "counted"
        })
    });

    world.unscript(meter);
    noted(&world, run, "later", "reported as it was");
    world.until("the next projection to be recorded", |world| {
        records(world, run).len() > mark + 1
    });
    let real = records(&world, run)[mark + 1].clone();
    assert_eq!(real["spent"], Value::Null, "{real}");
    assert_eq!(
        real["actions"],
        json!({"created": 0, "updated": 1, "unchanged": 1, "orphaned": 0, "reopened": 0}),
        "{real}"
    );
}

/// A run directory an engine that drove the store's binary left behind holds that engine's
/// record of whether its store offered a member copy, `writeback-store.json`, answering that it
/// did not. This build neither reads nor writes that record: the run adopts, the adopted
/// driver's first projection is whole and `first` as every driver's is, and the change after it
/// is carried as members — never whole for `store-lacks-members`, which this build reads on an
/// older line and never gives.
#[test]
fn a_run_directory_holding_an_older_engines_store_record_adopts_and_projects_by_member() {
    let run = "projections-older-record";
    let (world, project) = a_run_projecting_through_a_recording_store(
        "writeback-projections-older-record",
        run,
        vec![agent("work", &[]), agent("later", &["work"])],
        &["work"],
    );
    projected_until(
        &world,
        run,
        &project,
        "the running node to reach the board",
        |tasks| board_word(tasks, "work").as_deref() == Some("in-progress"),
    );
    // llmlint: ignore-block[tests_mirror_real_usage] a record written by **another build** is
    // the input here, and no invocation of this one produces it: this build never writes the
    // file. What is written is exactly what an engine that drove the store's binary left in a
    // run's directory, and everything then asserted is the real compiled binary adopting it —
    // as `writeback_budget.rs` states an older build's launch record.
    let older = in_run_dir(&proposed()["retired"]["record"]);
    let answer = json!({"version": "0.2.29", "members": false});
    std::fs::write(world.run_file(run, &older), answer.to_string())
        .expect("the older engine's record is left in the run directory");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let recorded_before = records(&world, run).len();
    adopted(&world, run);
    world.until("the adopted driver's first projection", |world| {
        records(world, run).len() > recorded_before
    });
    let first = records(&world, run)[recorded_before].clone();
    assert_eq!(first["scope"], "whole", "{first}");
    assert_eq!(first["whole_because"], "first", "{first}");

    let before = records(&world, run).len();
    noted(&world, run, "later", "a change for the adopted driver");
    // The attempt that carries the note: the adopted driver may project its own node's
    // progress around it, each as the members that changed.
    let carrying = |world: &World| {
        records(world, run)[before..]
            .iter()
            .find(|record| record["items"] == json!(["later"]))
            .cloned()
    };
    world.until("the adopted driver's change to be projected", |world| {
        carrying(world).is_some()
    });
    let change = carrying(&world).expect("the change was projected");
    assert_eq!(change["scope"], "members", "{change}");
    assert_eq!(change["outcome"], "projected", "{change}");
    assert!(
        records(&world, run)
            .iter()
            .all(|record| record["whole_because"] != "store-lacks-members"),
        "this build gave a reason only an engine driving the binary gives"
    );
    assert_eq!(
        world.run_json(run, &older),
        answer,
        "this build rewrote the older engine's record"
    );
    world.until_store("the adopted driver's change to reach the board", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"]
                    == "a change for the adopted driver"
        })
    });
}

/// A running node that is cancelled settles `cancelled` and reads `parked` on the board — the
/// park outranks the settlement, and it is an open word — so within one build nothing closes
/// its item and nothing has to reopen it. What does read closed is an item a board an older
/// build wrote, or a person, closed: `cancelled` on the destination. A retry or a requeue of
/// such a node writes `queued` onto **that same item**, which the store reopens: the attempt
/// that projects the retry carries the lineage root alone, reports `created: 0` and
/// `reopened: 1`, and the board holds one item for the lineage at the id it had before, naming
/// the replacement under `onepipeline.node`; the requeue likewise lands `queued` on its one item
/// with `reopened: 1`; and a cancelled node nobody retries or requeues keeps its one item under
/// the word it had. A held node keeps the run alive through three parks, and under a
/// concurrency of two — that node in one slot, a second held node added into the other — each
/// reopened node is read at `queued` rather than the word its dispatch would move it to.
#[test]
fn a_retry_or_requeue_of_a_cancelled_node_reopens_its_one_item() {
    let run = "projections-reopen";
    let world = World::new("writeback-projections-reopen");
    for node in ["retried", "requeued", "left"] {
        world.script(&format!("{node}.turn-open"), "");
        world.script(&format!("{node}.wait"), "hold");
        world.script(&format!("{node}.stops-when-interrupted"), "");
    }
    for held in ["hog", "filler"] {
        world.script(&format!("{held}.wait"), "hold");
    }
    let mut plan = plan_of(
        run,
        vec![
            agent("hog", &[]),
            agent("retried", &[]),
            agent("requeued", &[]),
            agent("left", &[]),
        ],
    );
    plan["concurrency"] = json!(2);
    let project = world.plan(run, &plan);
    let world = world
        .through_scripted_source()
        .with_env(RENDEZVOUS_SECONDS_ENV, "600")
        .with_env(CANCEL_GRACE_ENV, "1");
    world.run(&["start", &project, "--detach"]).exited(0);

    // One at a time in the slot beside the hog, each stopped when asked and settled cancelled.
    for node in ["retried", "requeued", "left"] {
        world.until(&format!("{node}'s turn to open"), |world| {
            world
                .events_of(run, "turn-started")
                .iter()
                .any(|event| event["labels"]["node"] == node)
        });
        cancelled(&world, run, node);
    }
    // Today's word for a running node the cancel stopped, with the settlement beside it.
    projected_until(
        &world,
        run,
        &project,
        "the three cancellations to reach the board",
        |tasks| {
            ["retried", "requeued", "left"].iter().all(|node| {
                board_word(tasks, node).as_deref() == Some("unknown")
                    && board_task(tasks, node)["item"]["status"]["name"] == "parked"
                    && board_task(tasks, node)["item"]["metadata"]["onepipeline.settlement"]
                        ["status"]
                        == "cancelled"
            })
        },
    );
    let before = world.store_tasks(&project);
    let held_before = |node: &str| board_task(&before, node)["id"].clone();

    // Two of the items are closed on the board — what a board an older build wrote holds
    // for a cancelled node, and what a person closing the card leaves — and one is left as
    // the run wrote it.
    // llmlint: ignore-block[tests_mirror_real_usage] a `local-md` destination *is* its folder
    // of Markdown, so closing an item on the board is writing its file; `onetaskgraph` has no
    // verb that moves one item's status in place.
    for node in ["retried", "requeued"] {
        amend(
            &item_file(&world, board_task(&before, node)),
            "status",
            json!("cancelled"),
        );
    }
    // llmlint: ignore-end[tests_mirror_real_usage]
    world.until_store("the closed items to read cancelled", |world| {
        let tasks = world.store_tasks(&project);
        board_word(&tasks, "retried").as_deref() == Some("cancelled")
            && board_word(&tasks, "requeued").as_deref() == Some("cancelled")
    });

    // A replacement behind the hog is read at `queued` for as long as the hog runs.

    let reopens = |records: &[Value], root: &str| {
        assert!(
            !records.is_empty(),
            "the edit of {root} was never projected"
        );
        for record in records {
            assert_eq!(record["items"], json!([root]), "{record}");
            assert_eq!(record["outcome"], "projected", "{record}");
            assert_eq!(record["actions"]["created"], 0, "{record}");
        }
        records
            .iter()
            .map(|record| record["actions"]["reopened"].as_u64().unwrap_or_default())
            .sum::<u64>()
    };

    let mark = records(&world, run).len();
    edit(
        &world,
        run,
        json!({"op": "retry", "id": "retried", "node": {
            "id": "retried-2", "persona": "engineer", "task": "## What\nRetry it.",
            "deps": ["hog"]
        }}),
    );
    projected_until(
        &world,
        run,
        &project,
        "the retry to reopen the lineage's item",
        |tasks| {
            board_word(tasks, "retried").as_deref() == Some("queued")
                && board_task(tasks, "retried")["item"]["metadata"]["onepipeline.node"]
                    == "retried-2"
        },
    );
    let retried = records(&world, run)[mark..].to_vec();
    assert_eq!(
        reopens(&retried, "retried"),
        1,
        "the retry's projection did not count exactly one reopen: {retried:?}"
    );
    let tasks = world.store_tasks(&project);
    let lineage: Vec<&Value> = tasks
        .iter()
        .filter(|task| task["item"]["metadata"]["onepipeline.id"] == "retried")
        .collect();
    assert_eq!(lineage.len(), 1, "{tasks:?}");
    assert_eq!(lineage[0]["id"], held_before("retried"), "{}", lineage[0]);
    assert_eq!(lineage[0]["item"]["content"], "## What\nRetry it.");
    assert_eq!(
        lineage[0]["item"]["metadata"]["onepipeline.supersedes"],
        json!(["retried"])
    );
    assert!(
        !tasks
            .iter()
            .any(|task| task["item"]["metadata"]["onepipeline.id"] == "retried-2"),
        "the retry minted an item for the replacement: {tasks:?}"
    );

    // A second held node into the free slot, so the requeued node is read at `queued` too.
    edit(
        &world,
        run,
        json!({"op": "add", "node": agent("filler", &[])}),
    );
    projected_until(
        &world,
        run,
        &project,
        "the filler to take the slot",
        |tasks| board_word(tasks, "filler").as_deref() == Some("in-progress"),
    );
    assert_eq!(
        board_word(&world.store_tasks(&project), "requeued").as_deref(),
        Some("cancelled"),
        "a member copy that did not name the closed item rewrote it"
    );
    let mark = records(&world, run).len();
    edit(&world, run, json!({"op": "requeue", "id": "requeued"}));
    projected_until(
        &world,
        run,
        &project,
        "the requeue to reopen the node's item",
        |tasks| board_word(tasks, "requeued").as_deref() == Some("queued"),
    );
    let requeued = records(&world, run)[mark..].to_vec();
    assert_eq!(
        reopens(&requeued, "requeued"),
        1,
        "the requeue's projection did not count exactly one reopen: {requeued:?}"
    );
    let tasks = world.store_tasks(&project);
    assert_eq!(
        board_task(&tasks, "requeued")["id"],
        held_before("requeued")
    );
    assert_eq!(
        board_task(&tasks, "requeued")["item"]["metadata"]["onepipeline.node"],
        "requeued"
    );
    // And the one nobody came back to keeps its item and its word.
    assert_eq!(board_task(&tasks, "left")["id"], held_before("left"));
    assert_eq!(
        board_task(&tasks, "left")["item"]["status"]["name"],
        "parked"
    );
    // Every landed attempt at the current version names the count.
    for record in records(&world, run) {
        assert_eq!(
            record["schema_version"].as_u64(),
            Some(u64::from(
                onepipeline::cli::WRITEBACK_PROJECTIONS_SCHEMA_VERSION
            )),
            "{record}"
        );
        if record["outcome"] == "projected" && record["actions"].is_object() {
            assert!(record["actions"]["reopened"].is_u64(), "{record}");
        }
    }
    world.release("hog.go");
    world.release("filler.go");
}

/// A board an older build wrote holds one item per attempt: after `--adopt`, the whole
/// projection carries the lineage once, onto the item of the furthest-along attempt, and leaves
/// the root's older item byte for byte. Once this build has written the head onto that item —
/// `onepipeline.id` the root, `onepipeline.node` the attempt — a second whole projection over
/// the same board resolves it the same way and lands, rather than refusing two items for one
/// `onepipeline.id`.
#[test]
fn an_adoption_over_a_board_an_older_build_wrote_reuses_the_furthest_along_item() {
    let run = "projections-older-board";
    let (world, project) = a_run_projecting_through_a_recording_store(
        "writeback-projections-older-board",
        run,
        vec![agent("flaky", &[]), agent("hold", &[])],
        &["flaky", "hold"],
    );
    world.script("flaky-2.wait", "hold");
    projected_until(
        &world,
        run,
        &project,
        "both nodes to reach the board",
        |tasks| {
            board_word(tasks, "flaky").as_deref() == Some("in-progress")
                && board_word(tasks, "hold").as_deref() == Some("in-progress")
        },
    );
    edit(
        &world,
        run,
        json!({"op": "retry", "id": "flaky", "node": {
            "id": "flaky-2", "persona": "engineer", "task": "## What\nAgain."
        }}),
    );
    projected_until(
        &world,
        run,
        &project,
        "the retry to reach the lineage's item",
        |tasks| {
            board_task(tasks, "flaky")["item"]["metadata"]["onepipeline.node"] == "flaky-2"
                && board_word(tasks, "flaky").as_deref() == Some("in-progress")
        },
    );

    // What an older build left: the root's item closed and plain at its own id, and a second
    // item for the attempt, plain at its own id. The destination is a folder of Markdown, so
    // seeding that board is writing its files.
    // llmlint: ignore-block[tests_mirror_real_usage] a `local-md` destination *is* its folder of
    // Markdown, and a board an older build wrote is exactly these files; `onetaskgraph` has no
    // verb that writes an item's reserved metadata in place.
    let tasks = world.store_tasks(&project);
    let root_file = item_file(&world, board_task(&tasks, "flaky"));
    let attempt_file = root_file.with_file_name("older-build-flaky-2.md");
    rewritten(&root_file, &attempt_file, |front| {
        front["status"] = json!("in progress");
        let metadata = front["metadata"].as_object_mut().expect("metadata");
        metadata.insert("onepipeline.id".to_owned(), json!("flaky-2"));
        metadata.remove("onepipeline.node");
        metadata.remove("onepipeline.supersedes");
        metadata.insert(
            "onetaskgraph.origin".to_owned(),
            json!("onepipeline-writeback:older/older-build-flaky-2"),
        );
    });
    rewritten(&root_file, &root_file, |front| {
        front["status"] = json!("cancelled");
        front["title"] = json!("what the older build closed");
        let metadata = front["metadata"].as_object_mut().expect("metadata");
        metadata.remove("onepipeline.node");
        metadata.remove("onepipeline.supersedes");
    });
    // llmlint: ignore-end[tests_mirror_real_usage]
    let older_root = std::fs::read(&root_file).expect("the seeded root item reads");
    // And what that build left in the run's own shadow store, which outlives its driver: a
    // shadow task for the attempt, keyed by the attempt's id. A whole copy carries every
    // document the shadow store holds, so one this build did not write would reach the
    // board as an item of its own.
    let shadow_tasks = world
        .run_file(run, "writeback")
        .join("tasks")
        .join(hex(&project));
    let root_shadow = shadow_tasks.join(format!("{}.md", hex("flaky")));
    let stale_shadow = shadow_tasks.join(format!("{}.md", hex("flaky-2")));
    assert!(
        root_shadow.is_file(),
        "the run keeps its shadow store elsewhere than {}",
        root_shadow.display()
    );
    rewritten(&root_shadow, &stale_shadow, |front| {
        let metadata = front["metadata"].as_object_mut().expect("metadata");
        metadata.insert("onepipeline.id".to_owned(), json!("flaky-2"));
        metadata.remove("onepipeline.node");
        metadata.remove("onepipeline.supersedes");
        metadata.remove("onetaskgraph.origin");
    });
    let seeded = world.store_tasks(&project);
    assert_eq!(
        seeded
            .iter()
            .filter(|task| task["item"]["metadata"]["onepipeline.id"] == "flaky-2")
            .count(),
        1,
        "the board was not seeded with the attempt's own item: {seeded:?}"
    );
    let attempt_id = seeded
        .iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == "flaky-2")
        .map(|task| task["id"].clone())
        .expect("the seeded attempt item");

    // Stopped and then adopted, rather than adopted out from under a live driver: a driver
    // that already knows the lineage's item projects a member copy onto it as it is displaced,
    // and what this journey is about is a driver reading the board cold.
    let reused_by_a_whole_projection = |world: &World, what: &str| {
        // Stopped only once it is quiet. An adopted driver dispatches the head and `hold`
        // again as soon as its whole projection lands, and that dispatch is projected by a
        // member copy of its own; a stop takes no closeout — its signal ends the driver where
        // it stands — so a stop during that copy leaves the double's record of it with no
        // attempt beside it, and nothing after can read every attempt as landed. The
        // adoption's whole projection writes the re-readied nodes `queued`, so `in-progress`
        // on both items is that member copy having landed.
        projected_until(
            world,
            run,
            &project,
            "the driver to have projected the head's dispatch",
            |tasks| {
                tasks
                    .iter()
                    .find(|task| task["id"] == attempt_id)
                    .is_some_and(|task| task["item"]["status"]["category"] == "in-progress")
                    && board_word(tasks, "hold").as_deref() == Some("in-progress")
            },
        );
        let last = records(world, run).pop().expect("the run has projected");
        assert_eq!(
            last["scope"], "members",
            "the stop would end the driver before its dispatch was projected: {last}"
        );
        world.run(&["stop", run]).exited(0);
        let mark = records(world, run).len();
        world.run(&["adopt", run, "--detach"]).exited(0);
        world.until(&format!("{what} to be recorded"), |world| {
            records(world, run).len() > mark && every_attempt_landed(world, run)
        });
        let whole = records(world, run)[mark].clone();
        assert_eq!(whole["scope"], "whole", "{whole}");
        assert_eq!(whole["whole_because"], "first", "{whole}");
        assert_eq!(whole["outcome"], "projected", "{whole}");
        assert_eq!(
            whole["items"],
            json!(["flaky", "hold"]),
            "the whole projection carried other than one item per lineage: {whole}"
        );
        assert_eq!(whole["actions"]["created"], 0, "{whole}");
        // The reused item may still carry the older build's origin when the copy rewrites it,
        // so the store can report it once as the lineage it updated and again as an orphan of
        // that origin: it is one item, rewritten, and counts under `updated` alone.
        assert_eq!(whole["actions"]["orphaned"], 0, "{whole}");
        assert!(
            whole["actions"]["updated"].as_u64() >= Some(1),
            "the reused item was not counted as rewritten: {whole}"
        );
        assert_eq!(
            std::fs::read(&root_file).expect("the root item reads"),
            older_root,
            "the older build's item at the root was rewritten"
        );
        let tasks = world.store_tasks(&project);
        let reused = tasks
            .iter()
            .find(|task| task["id"] == attempt_id)
            .unwrap_or_else(|| panic!("the attempt's item is gone: {tasks:?}"));
        assert_eq!(
            reused["item"]["metadata"]["onepipeline.id"], "flaky",
            "{reused}"
        );
        assert_eq!(
            reused["item"]["metadata"]["onepipeline.node"], "flaky-2",
            "{reused}"
        );
        assert_eq!(
            reused["item"]["metadata"]["onepipeline.supersedes"],
            json!(["flaky"]),
            "{reused}"
        );
        // Open: `queued` where the adopted driver has yet to dispatch the head again,
        // `in-progress` once it has.
        assert!(
            matches!(
                reused["item"]["status"]["category"].as_str(),
                Some("queued" | "in-progress")
            ),
            "{reused}"
        );
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task["item"]["metadata"]["onepipeline.id"] == "flaky")
                .count(),
            2,
            "the older item at the root and the reused head are both meant to stand: {tasks:?}"
        );
        assert!(
            !tasks
                .iter()
                .any(|task| task["item"]["metadata"]["onepipeline.id"] == "flaky-2"),
            "an item still says it is the attempt's own: {tasks:?}"
        );
        assert!(
            !stale_shadow.exists(),
            "the shadow task an older build left for the attempt was carried: {}",
            stale_shadow.display()
        );
    };
    reused_by_a_whole_projection(&world, "the adopted driver's whole projection");
    reused_by_a_whole_projection(&world, "a second whole projection over the rewritten board");
}

/// How many nodes each run of the concurrency journey below carries.
///
/// A full phase replaces this many tasks and the project item. The readiness wait
/// uses the count to ensure both stores have projected before reading them.
const CONCURRENT_NODES: usize = 12;

/// The body each run's project carries in the concurrency journey below, which its
/// projected project document has to restate whole.
///
/// Long enough that a document published by writing in place could be read with its
/// front matter closed and its body cut short.
fn concurrent_project_body() -> String {
    (0..512)
        .map(|line| format!("Project body line {line:04} of the concurrency journey."))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `path`'s text, re-read past the refusal Windows gives an open that lands while a
/// rename is replacing the file: that open observed no document, whole or torn. The
/// rename finishes in microseconds, so the deadline is only a backstop.
fn read_published(path: &Path) -> std::io::Result<String> {
    let backstop = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match std::fs::read_to_string(path) {
            Err(denied)
                if cfg!(windows)
                    && denied.kind() == std::io::ErrorKind::PermissionDenied
                    && std::time::Instant::now() < backstop =>
            {
                std::thread::yield_now();
            }
            read => return read,
        }
    }
}

/// A shadow document, parsed, or why it is not a whole one.
///
/// Its body is compared as `local-md` reads it: less that format's own framing, which is at
/// most one line break under the front matter and one ending the file.
fn shadow_document(path: &Path) -> Result<Value, String> {
    let text = match read_published(path) {
        Ok(text) => text,
        // Absent is not torn: the projection removes a shadow task no snapshot wrote, and
        // a listing taken a moment before the removal names a file that has since gone.
        // That one kind and no other — every other refusal is this reader failing rather
        // than the store changing under it, and taken for absence it would let the journey
        // pass having read nothing at all.
        Err(gone) if gone.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Null),
        Err(refused) => return Err(format!("{} could not be read: {refused}", path.display())),
    };
    let (front, body) = text
        .strip_prefix("---\n")
        .ok_or_else(|| format!("{} opens no front matter: {text:?}", path.display()))?
        .split_once("---\n")
        .ok_or_else(|| format!("{} closes no front matter: {text:?}", path.display()))?;
    let parsed: Value = serde_norway::from_str(front)
        .map_err(|error| format!("{} is not YAML ({error}): {text:?}", path.display()))?;
    let body = body.strip_prefix('\n').unwrap_or(body);
    let body = body.strip_suffix('\n').unwrap_or(body);
    if parsed.get("title").is_none() {
        return Err(format!(
            "{} carries no title, so it is not a whole document: {text:?}",
            path.display()
        ));
    }
    if path.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new("projects")) {
        if body != concurrent_project_body() {
            return Err(format!(
                "{} has a partial project body of {} bytes",
                path.display(),
                body.len()
            ));
        }
    } else {
        let id = parsed["metadata"]["onepipeline.id"]
            .as_str()
            .ok_or_else(|| format!("{} carries no task id: {text:?}", path.display()))?;
        let expected = agent(id, &[])["task"].as_str().unwrap().to_owned();
        if body != expected {
            return Err(format!(
                "{} has a partial task body: {body:?}",
                path.display()
            ));
        }
    }
    Ok(parsed)
}

/// Every `.md` document under `dir`, in path order, or a panic naming what it could not
/// list.
///
/// `.md` and nothing else, because that is what a `local-md` source lists: an atomic write
/// leaves a temporary beside its destination until the rename publishes it, and a reader
/// that took one of those for a document would be reading a file nobody published.
///
/// A directory that is not there is a state of a live store — the projection writes one
/// per project and takes it away with the project — so it is stepped over. Anything else
/// the host refuses is this listing failing, and it says so rather than returning a
/// shorter list, which would read here as a store with fewer documents in it.
fn shadow_documents(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let entries = match std::fs::read_dir(&next) {
            Ok(entries) => entries,
            Err(gone) if gone.kind() == std::io::ErrorKind::NotFound => continue,
            Err(refused) => panic!("{} could not be listed: {refused}", next.display()),
        };
        for entry in entries {
            let entry = entry.unwrap_or_else(|refused| {
                panic!(
                    "{} holds an entry that could not be read: {refused}",
                    next.display()
                )
            });
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("md") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

// llmlint: ignore-block[tests_mirror_real_usage] no CLI output reports a torn file, and a
// window microseconds wide is caught only by reading at rate, as `local-md` itself reads.
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] Measured at 7.5s
// within this module's 36.2s, this uses the same compiled binary and store fixture as
// its six peer write-back journeys. The only separate test edge is for conversational
// note journeys; moving this filesystem race there loses this module's crateSource
// coverage when the projection code changes.
#[test]
fn overlapping_projections_never_show_a_reader_a_torn_shadow_document() {
    let world = World::new("writeback-concurrent-shadow").through_scripted_source();
    // Each run's first copy, held at its first write: the store is started once per attempt, so
    // this holds the two runs' first attempts and no later one.
    let copies = world.store_holds("write_task");
    let runs = ["left", "right"];
    let shadows: Vec<PathBuf> = runs
        .iter()
        .map(|run| {
            let store = world.store_apart(run);
            let nodes = (0..CONCURRENT_NODES)
                .map(|n| agent(&format!("{run}{n}"), &[]))
                .collect();
            let project = world.plan_in(&store, run, &plan_of(run, nodes));
            let fixture = store
                .join("projects")
                .join(format!("{}.md", project_id(run)));
            let written = std::fs::read_to_string(&fixture).expect("the project fixture reads");
            std::fs::write(&fixture, written + &concurrent_project_body())
                .expect("the project fixture takes a body");
            world
                .run_in(&store, &["start", &project, "--detach"])
                .exited(0);
            world.run_file(run, "writeback")
        })
        .collect();

    let left_copy = copies.arrived();
    let right_copy = copies.arrived();
    assert_ne!(left_copy.pid, right_copy.pid, "the same copy arrived twice");
    world.store_stops_holding("write_task");
    left_copy.release();
    right_copy.release();

    // Cache the paths after the first projection so each pass spends its time
    // reading documents during the remaining write-back phases.
    world.until("both runs to project a shadow store", |_| {
        shadows
            .iter()
            .all(|shadow| shadow_documents(shadow).len() > CONCURRENT_NODES)
    });
    let watched: Vec<Vec<PathBuf>> = shadows
        .iter()
        .map(|shadow| shadow_documents(shadow))
        .collect();

    let mut passes = 0_usize;
    let mut observed_changes = 0_usize;
    let mut last: Vec<Vec<Value>> = vec![Vec::new(); runs.len()];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        passes += 1;
        for (nth, documents) in watched.iter().enumerate() {
            let mut seen = Vec::with_capacity(documents.len());
            for document in documents {
                match shadow_document(document) {
                    Ok(read) => seen.push(read),
                    Err(torn) => panic!(
                        "a reader caught a shadow document half-written, {passes} passes and \
                         {observed_changes} changes in: {torn}"
                    ),
                }
            }
            if seen != last[nth] {
                observed_changes += 1;
                last[nth] = seen;
            }
        }
        // Asked every so often rather than every pass: two stats between two reads of one
        // document is the same interval spent elsewhere the listing was.
        if passes.is_multiple_of(200) {
            if runs
                .iter()
                .all(|run| world.run_file(run, "result.json").is_file())
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the two runs did not settle; the runs root held:\n{}",
                world.dump()
            );
        }
    }

    // Both halves, because either alone is passable by a journey that raced nothing: a
    // reader that read across no rewrite saw one settled state, and a run that published
    // nothing gave it none to read.
    assert!(
        observed_changes >= 10,
        "the reader never overlapped the projections it is about: {observed_changes} changes read \
         across {passes} passes"
    );
    for run in runs {
        assert!(
            records(&world, run).len() >= 2,
            "{run} published no board while the reader was reading it"
        );
    }
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
// llmlint: ignore-end[tests_mirror_real_usage]
