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
/// handshake until the worker appends its record. An attempt with nothing to carry opens no
/// connection, and its record says it called nothing.
fn every_attempt_landed(world: &World, run: &str) -> bool {
    let opened = store_calls(world)
        .iter()
        .filter(|call| is_call(call, "initialize"))
        .count();
    let records = records(world, run);
    opened_the_store(&records) + 1 == opened
        && records
            .iter()
            .all(|record| record["outcome"] == "projected")
}

/// How many recorded attempts opened the store: every one but those that called nothing.
fn opened_the_store(records: &[Value]) -> usize {
    records
        .iter()
        .filter(|record| record["calls"] != json!({}))
        .count()
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

/// A run's first projection carries its claim as one targeted update per lineage — every
/// lineage, since the baseline the launch seeded records no word — and reads nothing: no
/// project, no page of tasks, no item; a later transition of one node is one update of that node
/// alone. The updated node reaches the board as ever, a label a person put on it included; the
/// node no update named keeps its item byte for byte as a person edited it, a declared field
/// included; and the only thing the worker handed the store for that projection is the update of
/// the named member — no read at all, and no copy. A driver an adoption starts carries only what differs from the file
/// the driver before it left: not the node that finished, whose item already says so.
#[test]
fn a_runs_first_projection_carries_its_claim_by_member_and_a_later_transition_that_node_alone() {
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
    assert_eq!(first["scope"], "members", "{first}");
    assert_eq!(first["whole_because"], Value::Null, "{first}");
    assert_eq!(first["outcome"], "projected", "{first}");
    assert_eq!(
        first["calls"],
        json!({"task-update": 2}),
        "the first projection was other than one targeted update per item it carried: {first}"
    );
    // The claim is the word, and the head each item names; the untitled nodes are titled by id.
    assert_eq!(
        first["updated_fields"],
        json!({"metadata": 2, "status": 2, "title": 2}),
        "{first}"
    );
    assert_eq!(first["project"], project.as_str(), "{first}");
    assert_eq!(first["items"], json!(["aside", "work"]), "{first}");
    assert!(first["actions"].is_object(), "{first}");
    // A local Markdown destination meters nothing, so no update reports a `spent`.
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

    // A person edits the board: a declared field of the node the next update will not name, and
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
    // And a key the engine owns on the one it will name, which that update does not change.
    rewritten(
        &tasks_dir.join("000-work.md"),
        &tasks_dir.join("000-work.md"),
        |front| {
            front
                .get_mut("metadata")
                .and_then(Value::as_object_mut)
                .expect("metadata")
                .insert(
                    "onepipeline.persona".to_owned(),
                    json!("a person's own word"),
                );
        },
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
        opened >= opened_the_store(&later),
        "{} attempts were made over {opened} stores, so one answered from another's reads",
        opened_the_store(&later)
    );
    for record in &later {
        assert_eq!(record["scope"], "members", "{record}");
        assert_eq!(record["whole_because"], Value::Null, "{record}");
        assert_eq!(record["items"], json!(["work"]), "{record}");
        assert_eq!(record["outcome"], "projected", "{record}");
        assert!(record["actions"].is_object(), "{record}");
        assert_eq!(record["calls"], json!({"task-update": 1}), "{record}");
        // A settlement is its word and the engine-owned keys it moved, and nothing else.
        assert_eq!(
            record["updated_fields"],
            json!({"metadata": 1, "status": 1}),
            "{record}"
        );
    }

    assert_eq!(
        std::fs::read(&aside_file).expect("the unnamed item reads"),
        edited,
        "a projection rewrote an item it did not name"
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
    // The person's edit to an engine-owned key stands: the settlement's update named the keys
    // the run changed, and this was not one of them.
    assert_eq!(
        work["item"]["metadata"]["onepipeline.persona"], "a person's own word",
        "an update rewrote an engine-owned key the run had not changed: {work}"
    );

    // What the projection asked the store for: one targeted update of the named member — never
    // the project, a page of tasks, a read of any item, a copy, or the unnamed member.
    let calls = store_calls(&world);
    let member_calls = &calls[asked_before..];
    assert!(
        member_calls.iter().any(|call| is_call(call, "update_task")),
        "no targeted update wrote the transition: {member_calls:?}"
    );
    for call in member_calls {
        let named = call.get(1).map(String::as_str).unwrap_or_default();
        match call[0].as_str() {
            "initialize" | "metering" | "health" => {}
            "update_task" => assert_eq!(
                named,
                native(&work_origin),
                "a projection updated an item other than the member it names: {call:?}"
            ),
            _ => panic!("a projection asked the store for something besides the update: {call:?}"),
        }
        assert_ne!(
            named,
            native(&aside_origin),
            "a projection touched the member it does not name: {call:?}"
        );
    }

    // A second driver compares against the file the first one left, so `work`, whose item
    // already says it is done, is not carried again.
    let recorded_before = records(&world, run).len();
    adopted(&world, run);
    world.until("the adopted driver's first projection", |world| {
        records(world, run).len() > recorded_before
    });
    let adopted_first = &records(&world, run)[recorded_before];
    assert_eq!(adopted_first["scope"], "members", "{adopted_first}");
    assert_eq!(
        adopted_first["whole_because"],
        Value::Null,
        "{adopted_first}"
    );
    assert!(
        adopted_first["items"]
            .as_array()
            .is_some_and(|items| !items.contains(&json!("work"))),
        "the adopted driver carried a lineage the previous driver had landed: {adopted_first}"
    );
    no_record_is_whole_or_reads_a_page_of_tasks(&world, run);
}

/// Every line the run recorded is a member projection whose calls name no page of tasks.
fn no_record_is_whole_or_reads_a_page_of_tasks(world: &World, run: &str) {
    let recorded = records(world, run);
    assert!(!recorded.is_empty(), "the run recorded no projection");
    for record in recorded {
        assert_eq!(record["scope"], "members", "{record}");
        assert!(record["calls"].is_object(), "{record}");
        assert!(
            record["calls"].get("task-list").is_none(),
            "an attempt read the project's page of tasks: {record}"
        );
    }
}

/// Entry 93's block, which is where the landed baseline's file and shape are written down.
fn landed_block() -> Value {
    let record = std::fs::read_to_string(crate::harness::repo_file("docs/contract-divergences.md"))
        .expect("the divergence record ships");
    let entry = record
        .split("\n## ")
        .find(|entry| entry.starts_with("93."))
        .expect("the record still carries entry 93");
    let block = entry
        .split("```json")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("entry 93 carries its json block");
    serde_json::from_str::<Value>(block).expect("entry 93's block is JSON")["landed"].clone()
}

/// The run's landed baseline, as the file holds it.
fn landed(world: &World, run: &str) -> Value {
    let path = world.run_file(run, &in_run_dir(&landed_block()["file"]));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} is not there: {error}; the runs root held:\n{}",
            path.display(),
            world.dump()
        )
    });
    serde_json::from_str(&text).expect("the landed baseline is JSON")
}

fn keys(value: &Value) -> std::collections::BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {value}"))
        .keys()
        .cloned()
        .collect()
}

/// The landed baseline is seeded from the launch's own read before the first projection — the
/// file is there, every item's word still unknown, while that projection's copy is held — and
/// holds exactly entry 93's version-1 shape: only engine-owned `onepipeline.*` keys, and none
/// of a person's label, a key of their own on the task, or the project's title and
/// description, though the board holds all four — and each of those four stands on the board
/// through every write. An update that lands advances it; a refused one advances nothing; and
/// the attempt after the refusal lands the change it lost.
#[test]
fn the_landed_baseline_is_seeded_at_launch_and_advanced_only_by_what_landed() {
    let run = "projections-landed";
    let world = World::new("writeback-projections-landed");
    world.script("work.wait", "hold");
    let project = world.plan(
        run,
        &plan_of(run, vec![agent("work", &[]), agent("later", &["work"])]),
    );
    // What a person put on the board that no plan models.
    // llmlint: ignore-block[tests_mirror_real_usage] a `local-md` destination *is* its folder of
    // Markdown, so a person editing the board edits those files; `onetaskgraph` has no verb that
    // labels an item or retitles a project in place, and `store.rs` authors them the same way.
    let identifier = project_id(run);
    let tasks_dir = world.store().join("tasks").join(&identifier);
    rewritten(
        &tasks_dir.join("000-work.md"),
        &tasks_dir.join("000-work.md"),
        |front| {
            front.insert("labels".to_owned(), json!(["needs-review"]));
            front
                .get_mut("metadata")
                .and_then(Value::as_object_mut)
                .expect("metadata")
                .insert("authored.note".to_owned(), json!("a person's own key"));
        },
    );
    let project_file = world
        .store()
        .join("projects")
        .join(format!("{identifier}.md"));
    rewritten(&project_file, &project_file, |front| {
        front.insert("title".to_owned(), json!("A board a person titled"));
        front.insert("labels".to_owned(), json!(["planning"]));
        // A repository the project names, which the shadow an `add` copies over has to restate.
        front.insert(
            "repositories".to_owned(),
            json!(["github.com/example/board"]),
        );
        let metadata = front
            .get_mut("metadata")
            .and_then(Value::as_object_mut)
            .expect("metadata");
        metadata.insert(
            "authored.owner".to_owned(),
            json!("a person's own project key"),
        );
        // The plan's own name, so the run is still named after it rather than the new title.
        metadata.insert("onepipeline.name".to_owned(), json!(run));
    });
    // llmlint: ignore-end[tests_mirror_real_usage]
    let world = world
        .through_scripted_source()
        .with_env(RENDEZVOUS_SECONDS_ENV, "600");
    let updates = world.store_holds("update_task");
    world.run(&["start", &project, "--detach"]).exited(0);

    // The first projection's first update is held: nothing has landed, and the file is already
    // there, seeded from the launch's read.
    let held = updates.arrived();
    let seeded = landed(&world, run);
    let block = landed_block();
    assert_eq!(keys(&seeded), keys(&block["example"]), "{seeded}");
    assert_eq!(
        seeded["schema_version"], block["schema_version"],
        "{seeded}"
    );
    assert_eq!(seeded["project"], project.as_str(), "{seeded}");
    let item_keys = keys(&block["example"]["items"]["build"]);
    let tasks = world.store_tasks(&project);
    for node in ["work", "later"] {
        let item = &seeded["items"][node];
        assert_eq!(keys(item), item_keys, "{node}: {item}");
        assert_eq!(
            item["status"],
            Value::Null,
            "a seeded item names a word: {item}"
        );
        assert_eq!(
            item["destination"],
            board_task(&tasks, node)["id"],
            "{item}"
        );
        let digest = item["content_sha256"].as_str().expect("a digest");
        assert!(
            digest.len() == 64 && digest.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "{item}"
        );
    }
    assert_eq!(seeded["items"]["later"]["depends_on"], json!(["work"]));
    assert_eq!(
        keys(&seeded["items"]),
        ["later", "work"].map(String::from).into()
    );
    for (key, _) in seeded["project_metadata"].as_object().expect("an object") {
        assert!(key.starts_with("onepipeline."), "{seeded}");
    }
    let text = serde_json::to_string(&seeded).expect("serializes");
    for foreign in [
        "needs-review",
        "authored.note",
        "authored.owner",
        "A board a person titled",
        "labels",
    ] {
        assert!(
            !text.contains(foreign),
            "the landed baseline holds `{foreign}`, which the engine does not own: {text}"
        );
    }

    world.store_stops_holding("update_task");
    held.release();
    drop(updates);
    projected_until(
        &world,
        run,
        &project,
        "the running node to reach the board",
        |tasks| board_word(tasks, "work").as_deref() == Some("in-progress"),
    );
    let advanced = landed(&world, run);
    assert_eq!(
        advanced["items"]["work"]["status"], "in progress",
        "{advanced}"
    );
    assert_eq!(advanced["items"]["later"]["status"], "queued", "{advanced}");
    assert_eq!(
        advanced["items"]["work"]["metadata"]["onepipeline.node"], "work",
        "{advanced}"
    );
    assert!(
        !serde_json::to_string(&advanced)
            .expect("serializes")
            .contains("needs-review"),
        "{advanced}"
    );

    // What no plan models stood through every write: the person's label and key on the item, and
    // the project's title and key, read back unchanged off the board's own files.
    let work_file = std::fs::read_to_string(tasks_dir.join("000-work.md")).expect("the item reads");
    assert!(
        work_file.contains("needs-review") && work_file.contains("a person's own key"),
        "a targeted update took a label or a key no plan models off the item:\n{work_file}"
    );
    let board = std::fs::read_to_string(&project_file).expect("the project reads");
    assert!(
        board.contains("A board a person titled")
            && board.contains("a person's own project key")
            && board.contains("github.com/example/board"),
        "the write-back rewrote the project's title or a key it does not own:\n{board}"
    );

    // An `add` is the one copy, and it lands the project item beside the node it creates: the
    // project is read once for it, so the copy finds the project as the board holds it and
    // writes nothing to it — title, description, labels, repositories and keys read back byte
    // for byte.
    let mark = records(&world, run).len();
    edit(
        &world,
        run,
        json!({"op": "add", "node": agent("extra", &["work"])}),
    );
    projected_until(&world, run, &project, "the added node's item", |tasks| {
        board_word(tasks, "extra").as_deref() == Some("queued")
    });
    assert_eq!(
        std::fs::read_to_string(&project_file).expect("the project reads"),
        board,
        "the copy creating the added node wrote the project item"
    );
    let created: Vec<Value> = records(&world, run)[mark..]
        .iter()
        .filter(|record| record["calls"].get("project-copy").is_some())
        .cloned()
        .collect();
    assert_eq!(created.len(), 1, "{created:?}");
    assert_eq!(created[0]["items"], json!(["extra"]), "{}", created[0]);
    assert_eq!(created[0]["calls"]["project-show"], 1, "{}", created[0]);
    assert_eq!(created[0]["actions"]["created"], 1, "{}", created[0]);
    assert_eq!(created[0]["actions"]["unchanged"], 1, "{}", created[0]);
    assert_eq!(
        landed(&world, run)["items"]["extra"]["destination"],
        board_task(&world.store_tasks(&project), "extra")["id"],
        "the baseline does not record the item the copy created"
    );

    // A refused update lands nothing, and the file says so.
    let path = world.run_file(run, &in_run_dir(&block["file"]));
    let before = std::fs::read(&path).expect("the baseline reads");
    let mark = records(&world, run).len();
    world.store_refuses(
        "update_task",
        &source_refused("the destination declined the write"),
    );
    noted(&world, run, "later", "not landed yet");
    world.until("the refused attempt to be recorded", |world| {
        records(world, run).len() > mark
    });
    assert_eq!(records(&world, run)[mark]["outcome"], "failed");
    assert_eq!(
        std::fs::read(&path).expect("the baseline reads"),
        before,
        "a refused update advanced the landed baseline"
    );
    world.store_stops_refusing("update_task");
    noted(&world, run, "work", "lands with it");
    world.until_store("the lost change to reach the board", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"] == "not landed yet"
        })
    });
    world.until("the baseline to record the lost change", |world| {
        landed(world, run)["items"]["later"]["metadata"]["onepipeline.context"] == "not landed yet"
    });
    no_record_is_whole_or_reads_a_page_of_tasks(&world, run);
    world.release("work.go");
}

/// A landed baseline the adopting driver cannot read — here, one holding a key the engine does
/// not own, which only a hand or another tool could have put there — is said on the driver's
/// standard error, naming the file and why, and treated as absent: each lineage is read once by
/// its own id — a lineage whose shadow document cannot be read either, said too, at the task it
/// was launched from — nothing is copied whole, no page of tasks is read, and the file is written again
/// in the shape this build reads once the projection lands.
#[test]
fn a_landed_baseline_the_driver_cannot_read_is_said_and_each_item_read_by_its_id() {
    let run = "projections-unreadable-baseline";
    let (world, project) = a_run_projecting_through_a_recording_store(
        "writeback-projections-unreadable-baseline",
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
    world.run(&["stop", run]).exited(0);
    let path = world.run_file(run, &in_run_dir(&landed_block()["file"]));
    let mut unreadable = landed(&world, run);
    // llmlint: ignore-block[tests_mirror_real_usage] a baseline this build refuses is one
    // nothing this build writes, so the file is edited the way a hand or another tool would.
    unreadable["items"]["work"]["metadata"]["authored.note"] = json!("not the engine's");
    std::fs::write(&path, unreadable.to_string()).expect("the baseline is rewritten");
    // And a shadow document for `later` an older build left — one naming where its item is, which
    // this build no longer writes for an item that exists — that cannot be read: said, and the
    // lineage read at the task it was launched from.
    let later_shadow = world
        .run_file(run, "writeback")
        .join("tasks")
        .join(hex(&project))
        .join(format!("{}.md", hex("later")));
    std::fs::create_dir_all(later_shadow.parent().expect("a folder")).expect("the shadow folder");
    std::fs::write(&later_shadow, "not a shadow document").expect("the document is written");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let mark = records(&world, run).len();
    world.run(&["adopt", run, "--detach"]).exited(0);
    world.until("the adopted driver's first projection", |world| {
        records(world, run).len() > mark && every_attempt_landed(world, run)
    });
    let log = std::fs::read_to_string(world.run_file(run, "driver.log")).expect("the log");
    assert!(
        log.contains(&format!("cannot read {}", path.display()))
            && log.contains("authored.note")
            && log.contains("read from the store once instead"),
        "the driver did not say it could not read the baseline, and why:\n{log}"
    );
    assert!(
        log.contains(&format!(
            "cannot read {} as a shadow document",
            later_shadow.display()
        )),
        "the driver did not say it could not read the shadow document:\n{log}"
    );
    let first = records(&world, run)[mark].clone();
    assert_eq!(first["scope"], "members", "{first}");
    // Both lineages read once by their own id, none read again to be carried, and no project
    // read or copy: each carried lineage is a targeted update of the item the read found.
    assert_eq!(first["calls"]["task-show"], 2, "{first}");
    for absent in ["project-show", "project-copy", "task-list"] {
        assert!(first["calls"].get(absent).is_none(), "{first}");
    }
    let rewritten = landed(&world, run);
    assert!(
        rewritten["items"]["work"]["metadata"]
            .get("authored.note")
            .is_none(),
        "the unreadable baseline was kept: {rewritten}"
    );
    assert_eq!(
        rewritten["items"]["later"]["destination"],
        board_task(&world.store_tasks(&project), "later")["id"],
        "{rewritten}"
    );
    no_record_is_whole_or_reads_a_page_of_tasks(&world, run);
    world.release("work.go");
}

/// A run an older build started whose board already says everything the adopting driver would
/// write — one node done, one parked, nothing left to start — is adopted with each lineage read
/// once by its own id and nothing more: no project read, no copy, and a line naming no items.
/// What the reads found is recorded as landed.
#[test]
fn an_adoption_whose_every_read_matches_reads_each_item_once_and_copies_nothing() {
    let run = "projections-cold-match";
    let world = World::new("writeback-projections-cold-match");
    for node in ["done", "parked"] {
        world.script(&format!("{node}.wait"), "hold");
    }
    // A worker that takes the cancel's ask, so the cancel settles it rather than waiting it out.
    world.script("parked.stops-when-interrupted", "");
    let project = world.plan(
        run,
        &plan_of(run, vec![agent("done", &[]), agent("parked", &[])]),
    );
    let world = world
        .through_scripted_source()
        .with_env(RENDEZVOUS_SECONDS_ENV, "600")
        .with_env(CANCEL_GRACE_ENV, "1");
    world.run(&["start", &project, "--detach"]).exited(0);
    projected_until(&world, run, &project, "both nodes running", |tasks| {
        board_word(tasks, "done").as_deref() == Some("in-progress")
            && board_word(tasks, "parked").as_deref() == Some("in-progress")
    });
    world.release("done.go");
    cancelled(&world, run, "parked");
    // Each with its settlement beside it, which is the last thing either node's item takes:
    // the cancel parks `parked` on the board at once and settles it a projection later, so a
    // board that already reads `parked` can still be one write short of what the adopting
    // driver reads it against. And with no attempt in flight is not enough to say there is
    // none to come: a projection the run has published and not yet opened the store for is
    // invisible to it, and the `stop` below ends that one before it lands.
    projected_until(&world, run, &project, "the board to say it all", |tasks| {
        board_word(tasks, "done").as_deref() == Some("done")
            && board_task(tasks, "done")["item"]["metadata"]["onepipeline.settlement"]["status"]
                == "done"
            && board_task(tasks, "parked")["item"]["status"]["name"] == "parked"
            && board_task(tasks, "parked")["item"]["metadata"]["onepipeline.settlement"]["status"]
                == "cancelled"
    });
    world.run(&["stop", run]).exited(0);
    let path = world.run_file(run, &in_run_dir(&landed_block()["file"]));
    // llmlint: ignore-block[tests_mirror_real_usage] a run directory an older build left holds
    // no landed baseline, and every launch of this build seeds one; removing it is exactly that
    // directory.
    std::fs::remove_file(&path).expect("this build seeded a landed baseline");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let mark = records(&world, run).len();
    world.run(&["adopt", run, "--detach"]).exited(0);
    world.until("the adopted driver's first projection", |world| {
        records(world, run).len() > mark
    });
    let first = records(&world, run)[mark].clone();
    assert_eq!(first["outcome"], "projected", "{first}");
    assert_eq!(first["items"], json!([]), "{first}");
    assert_eq!(
        first["calls"],
        json!({"task-show": 2}),
        "the adopted driver did more than read each item once: {first}"
    );
    assert_eq!(first["actions"], Value::Null, "{first}");
    let recorded = landed(&world, run);
    assert_eq!(
        keys(&recorded["items"]),
        ["done", "parked"].map(String::from).into(),
        "{recorded}"
    );
    assert_eq!(recorded["items"]["done"]["status"], "done", "{recorded}");
    assert_eq!(
        recorded["items"]["parked"]["status"], "parked",
        "{recorded}"
    );
    no_record_is_whole_or_reads_a_page_of_tasks(&world, run);
}

/// A read of one task does not answer its edges, so a lineage a cold adoption reads by its own
/// id that has any is carried even where everything the read did answer matches — and the copy
/// puts back an edge a person took off the board meanwhile — while one without edges that
/// matches is not.
#[test]
fn a_lineage_read_cold_with_edges_is_carried_and_its_edges_put_right() {
    let run = "projections-cold-edges";
    let world = World::new("writeback-projections-cold-edges");
    for node in ["first", "second"] {
        world.script(&format!("{node}.wait"), "hold");
    }
    world.script("second.stops-when-interrupted", "");
    let project = world.plan(
        run,
        &plan_of(run, vec![agent("first", &[]), agent("second", &["first"])]),
    );
    let world = world
        .through_scripted_source()
        .with_env(RENDEZVOUS_SECONDS_ENV, "600")
        .with_env(CANCEL_GRACE_ENV, "1");
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the first node to be dispatched", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    world.release("first.go");
    world.until("the second node to be dispatched", |world| {
        world.events_of(run, "node-dispatched").len() >= 2
    });
    cancelled(&world, run, "second");
    // Each with its settlement beside it, as in the journey above: the cancel's settlement is
    // the last write `second`'s item takes, a projection after the park. A board that read
    // `parked` let the `stop` below land while that projection had opened the store and not
    // yet recorded itself — the stop ends the driver mid-attempt — so the store was opened
    // once more than any record says, and the wait for the adopted driver's projection never
    // ended (Windows, run 36924701088).
    projected_until(&world, run, &project, "the board to say it all", |tasks| {
        board_word(tasks, "first").as_deref() == Some("done")
            && board_task(tasks, "first")["item"]["metadata"]["onepipeline.settlement"]["status"]
                == "done"
            && board_task(tasks, "second")["item"]["status"]["name"] == "parked"
            && board_task(tasks, "second")["item"]["metadata"]["onepipeline.settlement"]["status"]
                == "cancelled"
    });
    world.run(&["stop", run]).exited(0);

    let second = board_task(&world.store_tasks(&project), "second").clone();
    let second_id = second["id"].as_str().expect("an id").to_owned();
    assert!(
        !world.store_deps(&second_id).is_empty(),
        "the fixture drew no edge"
    );
    // llmlint: ignore-block[tests_mirror_real_usage] a run directory an older build left holds
    // no landed baseline, and a person taking an edge off a `local-md` board edits its file;
    // no invocation of this build produces either.
    std::fs::remove_file(world.run_file(run, &in_run_dir(&landed_block()["file"])))
        .expect("this build seeded a landed baseline");
    rewritten(
        &item_file(&world, &second),
        &item_file(&world, &second),
        |front| {
            front.remove("depends_on");
        },
    );
    // llmlint: ignore-end[tests_mirror_real_usage]
    assert!(
        world.store_deps(&second_id).is_empty(),
        "the edge was not taken off the board"
    );

    let mark = records(&world, run).len();
    world.run(&["adopt", run, "--detach"]).exited(0);
    world.until("the adopted driver's first projection", |world| {
        records(world, run).len() > mark && every_attempt_landed(world, run)
    });
    let first = records(&world, run)[mark].clone();
    assert_eq!(
        first["items"],
        json!(["second"]),
        "a lineage with edges read cold was not carried, or one without was: {first}"
    );
    assert_eq!(first["calls"]["task-show"], 2, "{first}");
    assert!(
        !world.store_deps(&second_id).is_empty(),
        "the copy did not put the edge back"
    );
    no_record_is_whole_or_reads_a_page_of_tasks(&world, run);
}

/// The attempt after a failed one carries exactly the lineages whose change had not landed —
/// never the whole project — whatever failed: the store refusing the targeted update, each
/// recorded failed with the store's class and kind and the reason, carrying no report, and the
/// planner told the items the attempt carried. The change the refused attempt lost and one made while it was
/// refused both reach the board on the attempt that recovers, and that attempt carries those
/// two lineages and nothing else. A failure a wait can change is retried on the schedule, and
/// the retry carries the unlanded lineage alone. Across it all, no attempt reads a page of
/// tasks.
#[test]
fn a_projection_after_a_failed_attempt_carries_what_had_not_landed() {
    let run = "projections-after-failure";
    let (world, project) = a_run_projecting_through_a_recording_store(
        "writeback-projections-after-failure",
        run,
        vec![
            agent("work", &[]),
            agent("later", &["work"]),
            agent("aside", &[]),
        ],
        &["work", "aside"],
    );
    projected_until(
        &world,
        run,
        &project,
        "the running nodes to reach the board",
        |tasks| {
            board_word(tasks, "work").as_deref() == Some("in-progress")
                && board_word(tasks, "aside").as_deref() == Some("in-progress")
        },
    );
    let mark = records(&world, run).len();
    let recorded = |count: usize| {
        world.until(&format!("{count} more attempts to be recorded"), |world| {
            records(world, run).len() >= mark + count
        });
        records(&world, run)
    };
    let noted_on = |world: &World, node: &str, text: &str| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == node
                && task["item"]["metadata"]["onepipeline.context"] == text
        })
    };

    world.store_refuses(
        "update_task",
        &source_refused("the item that update names is one the destination no longer holds"),
    );
    noted(&world, run, "later", "refused at the update");
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
        "the surface names other items than the attempt carried: {surface}"
    );
    world.store_stops_refusing("update_task");

    // The graph changes again, on another lineage: the attempt carries that change and the one
    // the refusal lost, and not `aside`, which changed nowhere.
    noted(&world, run, "work", "changed after the refusal");
    let recovered = recorded(2)[mark + 1].clone();
    assert_eq!(recovered["scope"], "members", "{recovered}");
    assert_eq!(recovered["whole_because"], Value::Null, "{recovered}");
    assert_eq!(recovered["items"], json!(["later", "work"]), "{recovered}");
    assert_eq!(recovered["outcome"], "projected", "{recovered}");
    world.until_store(
        "every change the refusal lost to reach the board",
        |world| {
            noted_on(world, "later", "refused at the update")
                && noted_on(world, "work", "changed after the refusal")
        },
    );

    noted(&world, run, "later", "members again");
    let again = recorded(3)[mark + 2].clone();
    assert_eq!(again["scope"], "members", "{again}");
    assert_eq!(again["items"], json!(["later"]), "{again}");
    assert_eq!(again["outcome"], "projected", "{again}");

    // A failure a wait can change is retried on the schedule, and the retry carries what had
    // not landed: the update of the named member failing is recorded failed and `transient`, and
    // the attempt the schedule makes next carries that member again, alone, and lands.
    world.store_refuses_once(
        "update_task",
        &json!({"kind": "unavailable", "message": "the connection was reset"}),
    );
    noted(&world, run, "later", "retried by member");
    let retried = recorded(5);
    let unread = &retried[mark + 3];
    assert_eq!(unread["scope"], "members", "{unread}");
    assert_eq!(unread["items"], json!(["later"]), "{unread}");
    assert_eq!(unread["outcome"], "failed", "{unread}");
    assert_eq!(unread["class"], "transient", "{unread}");
    assert_eq!(unread["kind"], "unavailable", "{unread}");
    assert!(
        unread["reason"].as_str().is_some_and(
            |reason| reason.contains("task-update") && reason.contains("connection was reset")
        ),
        "{unread}"
    );
    let landed = &retried[mark + 4];
    assert_eq!(landed["scope"], "members", "{landed}");
    assert_eq!(landed["items"], json!(["later"]), "{landed}");
    assert_eq!(landed["outcome"], "projected", "{landed}");
    world.until_store("the retried projection to reach the board", |world| {
        noted_on(world, "later", "retried by member")
    });
    assert!(
        !world.fakes.join("store.update_task.refuse.once").exists(),
        "the member's update was never sent, so nothing failed it"
    );
    assert_eq!(
        world.store_asked("query_tasks"),
        1,
        "something beside the launch's own plan read asked for a page of tasks"
    );
    no_record_is_whole_or_reads_a_page_of_tasks(&world, run);
    world.release("aside.go");
}

/// The record carries exactly what the targeted update said it did and spent: its `spent` object
/// as the store reported it, the fields it wrote, and the item counted. The destination here
/// meters its own requests, as a hosted source does — each request it serves spends one request
/// and three points — so the store reports a `spent` for the update, and a worker recording it
/// as anything but what the store said fails here, as does one miscounting what it wrote. The
/// update after the meter is taken away reports what a source that meters nothing reports.
#[test]
fn the_record_carries_exactly_what_the_update_said_it_wrote_and_spent() {
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
    world.until("the metered update to be recorded", |world| {
        records(world, run).len() > mark
    });

    let counted = records(&world, run)[mark].clone();
    assert_eq!(counted["outcome"], "projected", "{counted}");
    assert_eq!(counted["scope"], "members", "{counted}");
    assert_eq!(counted["calls"], json!({"task-update": 1}), "{counted}");
    // A note is the one engine-owned key it moves.
    assert_eq!(
        counted["updated_fields"],
        json!({"metadata": 1}),
        "{counted}"
    );
    // What the metered source served for that update: every request between the two readings
    // the store takes of its meter, one before the update and one after, read off the source's
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
        "the metered update asked the source nothing: {calls:?}"
    );
    assert_eq!(
        counted["spent"],
        json!({"requests": served, "budgets": [
            {"budget": "graphql", "unit": "points", "amount": 3 * served, "lower_bound": false}
        ]}),
        "{counted}"
    );
    assert_eq!(
        counted["actions"],
        json!({"created": 0, "updated": 1, "unchanged": 0, "orphaned": 0, "reopened": 0}),
        "{counted}"
    );
    world.until_store("the metered update to land", |world| {
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
        json!({"created": 0, "updated": 1, "unchanged": 0, "orphaned": 0, "reopened": 0}),
        "{real}"
    );
    world.until_store("the unmetered update to land", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"] == "reported as it was"
        })
    });

    // A person has already put on the board what the next update names: the store is asked,
    // writes nothing, and says so — the item counted `unchanged`, and no field written.
    let later = board_task(&world.store_tasks(&project), "later").clone();
    // llmlint: ignore-block[tests_mirror_real_usage] a `local-md` destination *is* its folder of
    // Markdown, so a person editing the board edits that file.
    rewritten(
        &item_file(&world, &later),
        &item_file(&world, &later),
        |front| {
            front
                .get_mut("metadata")
                .and_then(Value::as_object_mut)
                .expect("metadata")
                .insert(
                    "onepipeline.context".to_owned(),
                    json!("already on the board"),
                );
        },
    );
    // llmlint: ignore-end[tests_mirror_real_usage]
    noted(&world, run, "later", "already on the board");
    world.until(
        "the update the board already held to be recorded",
        |world| records(world, run).len() > mark + 2,
    );
    let held = records(&world, run)[mark + 2].clone();
    assert_eq!(held["calls"], json!({"task-update": 1}), "{held}");
    assert_eq!(held["updated_fields"], json!({}), "{held}");
    assert_eq!(
        held["actions"],
        json!({"created": 0, "updated": 0, "unchanged": 1, "orphaned": 0, "reopened": 0}),
        "{held}"
    );
}

/// A run directory an engine that drove the store's binary left behind holds that engine's
/// record of whether its store offered a member copy, `writeback-store.json`, answering that it
/// did not. This build neither reads nor writes that record: the run adopts, the adopted
/// driver's first projection carries its members as every driver's does, and the change after
/// it is carried as members — never whole for `store-lacks-members`, which this build reads on
/// an older line and never gives.
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
    assert_eq!(first["scope"], "members", "{first}");
    assert_eq!(first["whole_because"], Value::Null, "{first}");

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
/// its item and nothing has to reopen it. What does read closed is an item a person closed:
/// `cancelled` on the destination. A retry or a requeue of such a node writes `queued` onto
/// **that same item**, which the store reopens: the attempt that projects the retry carries the
/// lineage root alone as one targeted update, reports `created: 0`, and the board holds one item
/// for the lineage at the id it had before, naming the replacement under `onepipeline.node`; the
/// requeue likewise lands `queued` on its one item. `reopened` counts off the word the run knew
/// the item by, since no attempt reads an item the landed baseline holds: `0` for those two,
/// whose close the run never saw, and `1` for a card a person closed that a driver adopting the
/// run with no baseline reads by its id and writes back open. A cancelled node nobody retries
/// or requeues keeps its one item under the word it had. A held node keeps the run alive through three parks, and under a
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
            "id": "retried-2", "persona": "engineer", "task": "## What\nRetry it.\n\n## Acceptance criteria\n- It is done.",
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
        0,
        "the retry of a card only a person closed counted a close the run never saw: {retried:?}"
    );
    let tasks = world.store_tasks(&project);
    let lineage: Vec<&Value> = tasks
        .iter()
        .filter(|task| task["item"]["metadata"]["onepipeline.id"] == "retried")
        .collect();
    assert_eq!(lineage.len(), 1, "{tasks:?}");
    assert_eq!(lineage[0]["id"], held_before("retried"), "{}", lineage[0]);
    assert_eq!(
        lineage[0]["item"]["content"],
        "## What\nRetry it.\n\n## Acceptance criteria\n- It is done."
    );
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
        0,
        "the requeue of a card only a person closed counted a close the run never saw: \
         {requeued:?}"
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
    // A close the run reads is a close it counts. A person closes the one nobody came back to,
    // and a driver adopting the run with no landed baseline — as a run an older build started
    // has none — reads that item by its id, finds it `cancelled`, and writes it back open.
    // llmlint: ignore-block[tests_mirror_real_usage] closing an item on a `local-md` board is
    // writing its file, and a run directory with no landed baseline is one an older build left;
    // no invocation of this build produces either.
    amend(
        &item_file(&world, board_task(&tasks, "left")),
        "status",
        json!("cancelled"),
    );
    world.run(&["stop", run]).exited(0);
    std::fs::remove_file(world.run_file(run, &in_run_dir(&landed_block()["file"])))
        .expect("this build seeded a landed baseline");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let mark = records(&world, run).len();
    world.run(&["adopt", run, "--detach"]).exited(0);
    projected_until(
        &world,
        run,
        &project,
        "the adopting driver to write the closed card open",
        |tasks| board_task(tasks, "left")["item"]["status"]["name"] == "parked",
    );
    let adopted = records(&world, run)[mark..].to_vec();
    assert_eq!(
        adopted
            .iter()
            .map(|record| record["actions"]["reopened"].as_u64().unwrap_or_default())
            .sum::<u64>(),
        1,
        "the card read closed and written open was not counted exactly once: {adopted:?}"
    );
    assert_eq!(
        board_task(&world.store_tasks(&project), "left")["id"],
        held_before("left")
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

/// A run an older build started holds no landed baseline, and its board one item per attempt:
/// after `--adopt`, each lineage is read once by its own id — the furthest-along attempt's item
/// the previous driver's shadow documents recorded — and the projection carries the lineage
/// once, by member, onto that item, and leaves the root's older item byte for byte. No attempt
/// is whole and none reads a page of tasks. Once this build has written the head onto that item
/// and recorded where it landed, a second adoption carries the difference from that record and
/// lands onto the same item, rather than refusing two items for one `onepipeline.id`.
#[test]
fn an_adoption_of_a_run_an_older_build_started_reads_the_furthest_along_item_by_its_id() {
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
            "id": "flaky-2", "persona": "engineer", "task": "## What\nAgain.\n\n## Acceptance criteria\n- It is done."
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
    // And what that build left in the run's own directory, which outlives its driver: no landed
    // baseline, which that build never wrote, and a shadow task for the attempt, keyed by the
    // attempt's id and naming the item the attempt landed on — which is how the adopting driver
    // finds that item without reading the board's page of tasks. This build writes a shadow task
    // only for an item it creates, so the older build's is written here as that build wrote it.
    let shadow_tasks = world
        .run_file(run, "writeback")
        .join("tasks")
        .join(hex(&project));
    let stale_shadow = shadow_tasks.join(format!("{}.md", hex("flaky-2")));
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
    let landed_file = world.run_file(run, "writeback-landed.json");

    // Stopped and then adopted, rather than adopted out from under a live driver: a driver
    // that already knows the lineage's item projects a member copy onto it as it is displaced,
    // and what this journey is about is a driver reading the board cold.
    let reused_by_a_member_projection = |world: &World, what: &str, cold: bool| {
        // Stopped only once it is quiet. An adopted driver dispatches the head and `hold`
        // again as soon as its first projection lands, and that dispatch is projected by a
        // copy of its own; a stop takes no closeout — its signal ends the driver where it
        // stands — so a stop during that copy leaves the double's record of it with no attempt
        // beside it, and nothing after can read every attempt as landed. The adoption's first
        // projection writes the re-readied nodes `queued`, so `in-progress` on both items is
        // the copy after it having landed.
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
        if cold {
            // llmlint: ignore-block[tests_mirror_real_usage] a run directory an older build left
            // holds no landed baseline, and no invocation of this build produces one without it:
            // every launch seeds the file. Removing it is exactly that directory.
            std::fs::remove_file(&landed_file).expect("this build seeded a landed baseline");
            // Written once the driver has stopped, as the older build's driver left it.
            std::fs::create_dir_all(&shadow_tasks).expect("the shadow folder");
            std::fs::write(
                &stale_shadow,
                format!(
                    "---\n{}---\n## What\nAgain.\n",
                    serde_norway::to_string(&json!({
                        "title": "flaky-2",
                        "status": "in progress",
                        "metadata": {"onepipeline.id": "flaky-2", "onetaskgraph.origin": attempt_id},
                    }))
                    .expect("the front matter renders")
                ),
            )
            .expect("the older build's shadow task is written");
            // llmlint: ignore-end[tests_mirror_real_usage]
        }
        let mark = records(world, run).len();
        world.run(&["adopt", run, "--detach"]).exited(0);
        world.until(&format!("{what} to be recorded"), |world| {
            records(world, run).len() > mark && every_attempt_landed(world, run)
        });
        let first = records(world, run)[mark].clone();
        assert_eq!(first["scope"], "members", "{first}");
        assert_eq!(first["whole_because"], Value::Null, "{first}");
        assert_eq!(first["outcome"], "projected", "{first}");
        assert_eq!(
            first["items"],
            json!(["flaky", "hold"]),
            "the projection carried other than one item per lineage: {first}"
        );
        if cold {
            // Each lineage read once, by its own id, then one targeted update of each item the
            // reads found — no project read and no copy.
            assert_eq!(
                first["calls"],
                json!({"task-show": 2, "task-update": 2}),
                "{first}"
            );
        }
        assert_eq!(first["actions"]["created"], 0, "{first}");
        // The reused item is updated in place, never counted as an orphan of the older origin.
        assert_eq!(first["actions"]["orphaned"], 0, "{first}");
        assert!(
            first["actions"]["updated"].as_u64() >= Some(1),
            "the reused item was not counted as rewritten: {first}"
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
    };
    reused_by_a_member_projection(&world, "the adopted driver's first projection", true);
    assert!(
        landed_file.is_file(),
        "the adopted driver recorded nothing of what it landed"
    );
    let recorded: Value =
        serde_json::from_str(&std::fs::read_to_string(&landed_file).expect("the baseline reads"))
            .expect("the baseline is JSON");
    assert_eq!(
        recorded["items"]["flaky"]["destination"], attempt_id,
        "the baseline does not record the furthest-along item as the lineage's: {recorded}"
    );
    reused_by_a_member_projection(&world, "a second adoption over the rewritten board", false);
    no_record_is_whole_or_reads_a_page_of_tasks(&world, run);
}

/// How many nodes each run of the concurrency journey below adds, one at a time.
///
/// A creation is the one write the shadow store takes, so each add is one rewrite of the
/// project's shadow document and the added node's for the reader to race.
const CONCURRENT_ADDS: usize = 6;

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
// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] this uses the same compiled
// binary and store fixture as its peer write-back journeys. The only separate test edge is for
// conversational note journeys; moving this filesystem race there loses this module's
// crateSource coverage when the projection code changes.
/// Two runs each `add` nodes one at a time — every creation rewriting its run's shadow project
/// document and writing the added node's, and taking the previous one out — while a reader lists
/// and parses both shadow stores at rate, as the `local-md` source a copy reads them through
/// does: no document it reads is ever torn.
#[test]
fn overlapping_projections_never_show_a_reader_a_torn_shadow_document() {
    let world = World::new("writeback-concurrent-shadow").through_scripted_source();
    let runs = ["left", "right"];
    let shadows: Vec<PathBuf> = runs
        .iter()
        .map(|run| {
            let store = world.store_apart(run);
            let hold = format!("{run}-hold");
            world.script(&format!("{hold}.wait"), "hold");
            let project = world.plan_in(&store, run, &plan_of(run, vec![agent(&hold, &[])]));
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

    let mut passes = 0_usize;
    let mut observed_changes = 0_usize;
    let mut last: Vec<Vec<Value>> = vec![Vec::new(); runs.len()];
    let mut read_until = |what: &str, done: &dyn Fn() -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            passes += 1;
            for (nth, shadow) in shadows.iter().enumerate() {
                let mut seen = Vec::new();
                for document in shadow_documents(shadow) {
                    match shadow_document(&document) {
                        Ok(read) => seen.push(read),
                        Err(torn) => panic!(
                            "a reader caught a shadow document half-written, {passes} passes \
                             and {observed_changes} changes in: {torn}"
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
            if passes.is_multiple_of(50) {
                if done() {
                    return;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "{what} did not happen; the runs root held:\n{}",
                    world.dump()
                );
            }
        }
    };
    let created = |run: &str| {
        records(&world, run)
            .iter()
            .filter(|record| record["outcome"] == "projected")
            .filter_map(|record| record["actions"]["created"].as_u64())
            .sum::<u64>()
    };
    for nth in 0..CONCURRENT_ADDS {
        for run in runs {
            edit(
                &world,
                run,
                json!({"op": "add", "node": agent(&format!("{run}{nth}"), &[])}),
            );
        }
        read_until(&format!("add {nth} to be created on both boards"), &|| {
            runs.iter().all(|run| created(run) > nth as u64)
        });
    }

    // Both halves, because either alone is passable by a journey that raced nothing: a
    // reader that read across no rewrite saw one settled state, and a run that published
    // nothing gave it none to read.
    assert!(
        observed_changes >= 2 * CONCURRENT_ADDS,
        "the reader never overlapped the projections it is about: {observed_changes} changes read \
         across {passes} passes"
    );
    for run in runs {
        assert!(
            records(&world, run)
                .iter()
                .filter(|record| record["calls"].get("project-copy").is_some())
                .count()
                >= CONCURRENT_ADDS,
            "{run} published no board while the reader was reading it"
        );
        world.release(&format!("{run}-hold.go"));
    }
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
// llmlint: ignore-end[tests_mirror_real_usage]
