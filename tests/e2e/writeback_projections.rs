//! What a settlement write-back projection carries, and the record every attempt leaves.
//!
//! A projection used to copy every node of the plan to change one, and nothing on the run
//! said what that cost. What runs here is the real projection against the real
//! `onetaskgraph`, at the release this repository's checks install, over a local Markdown
//! destination: every command the worker spawns goes through the store double, which
//! records it and hands it to that real store. What lands on the board is the store's own
//! work; what the double adds is a log of every command it was handed, and — for the one
//! journey about the figures a report carries — a report rewritten to carry figures an
//! offline store never reports. `crates/testfakes/src/bin/fake-onetaskgraph.rs` says how
//! each is scripted.
//!
//! Entry 73 of `docs/contract-divergences.md` is the source for the record these journeys
//! read, and where the record lives is read out of it rather than restated.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// The store double here delegates every command to the real `onetaskgraph` and records it, so
// what lands on the board is the real store's own; the scripted refusals and the one rewritten
// report stand in only for what an offline store cannot be made to answer. `harness.rs` carries
// the same suppression and the full rationale.

use std::path::Path;

use serde_json::{json, Value};

use crate::harness::{
    agent, double, onetaskgraph_binary, plan_of, project_id, World, RENDEZVOUS_SECONDS_ENV,
    STORE_BINARY_ENV,
};

/// Entry 73's block, which is where the record's path and the store's answer's path are
/// written down.
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

/// Every command line the store double was handed, in order.
fn store_calls(world: &World) -> Vec<Vec<String>> {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "onetaskgraph")
        .map(|call| {
            call["args"]
                .as_array()
                .expect("a recorded call names its arguments")
                .iter()
                .map(|arg| arg.as_str().expect("an argument is a string").to_owned())
                .collect()
        })
        .collect()
}

fn is_verb(call: &[String], verb: [&str; 2]) -> bool {
    call.len() >= 2 && call[0] == verb[0] && call[1] == verb[1]
}

/// Whether every copy the worker has run has been recorded, and every record landed — so no
/// attempt is in flight and none failed. Only the worker copies, so the count of copies the
/// double was handed is the count of attempts that reached one.
fn every_attempt_landed(world: &World, run: &str) -> bool {
    let records = records(world, run);
    let copies = store_calls(world)
        .iter()
        .filter(|call| is_verb(call, ["project", "copy"]))
        .count();
    !records.is_empty()
        && records.len() == copies
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

/// A detached run whose every store command goes through the recording double in front of the
/// real store. `held` nodes stay running until the journey releases them, and `version` is
/// what the store says it is where a journey needs an older one.
fn a_run_projecting_through_a_recording_store(
    world: &str,
    run: &str,
    nodes: Vec<Value>,
    held: &[&str],
    version: Option<&str>,
) -> (World, String) {
    let world = World::new(world);
    for node in held {
        world.script(&format!("{node}.wait"), "hold");
    }
    let project = world.plan(run, &plan_of(run, nodes));
    world.script(
        "onetaskgraph.delegate",
        &onetaskgraph_binary().to_string_lossy(),
    );
    if let Some(version) = version {
        world.script("onetaskgraph.version", version);
    }
    let world = world
        .with_env(
            STORE_BINARY_ENV,
            &double("fake-onetaskgraph").to_string_lossy(),
        )
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
    let document = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
    let (front, body) = document
        .strip_prefix("---\n")
        .expect("a store document opens its front matter")
        .split_once("---\n")
        .expect("a store document closes its front matter");
    let mut parsed: serde_json::Map<String, Value> =
        serde_norway::from_str(front).expect("the front matter is YAML");
    parsed.insert(key.to_owned(), value);
    let rendered = serde_norway::to_string(&parsed).expect("the front matter renders");
    std::fs::write(path, format!("---\n{rendered}---\n{body}")).expect("the document is written");
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

/// Script the double to refuse `script` the way the store does, failure document and all.
fn refusing(world: &World, script: &str, said: &str, document: &str) {
    world.script(&format!("{script}.stdout"), document);
    world.script(&format!("{script}.exit"), "1");
    world.script(script, said);
}

fn stops_refusing(world: &World, script: &str) {
    for file in [
        script.to_owned(),
        format!("{script}.stdout"),
        format!("{script}.exit"),
    ] {
        std::fs::remove_file(world.fakes.join(&file))
            .unwrap_or_else(|error| panic!("{file} is taken away: {error}"));
    }
}

const STALE_ORIGIN_SAID: &str =
    "onetaskgraph: onepipeline-writeback:board/later was copied from a destination item that \
     destination no longer holds";
const STALE_ORIGIN: &str = r#"{"failure":{"class":"refused","kind":"stale-origin","source":null,"message":"a destination item that destination no longer holds","retry_after_seconds":null}}"#;
const SOURCE_REFUSED_SAID: &str = "onetaskgraph: source plans refused the request";
const SOURCE_REFUSED: &str = r#"{"failure":{"class":"refused","kind":"refused","source":"plans","message":"source plans refused the request","retry_after_seconds":null}}"#;

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
        None,
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

    // The shadow task the copy names for `work`, read off the whole copy's own command line
    // and the shadow store it pointed the copy at.
    let calls = store_calls(&world);
    let whole_copy = calls[..asked_before]
        .iter()
        .find(|call| is_verb(call, ["project", "copy"]))
        .expect("the first projection ran a copy");
    let shadow_project = &whole_copy[2];
    let shadow_root = whole_copy
        .iter()
        .find_map(|arg| {
            arg.strip_prefix(&format!(
                "sources.{}.config.root=",
                shadow_project.split(':').next().unwrap_or_default()
            ))
        })
        .expect("the copy names its shadow store's root");
    let shadow_file = shadow_project
        .split_once(':')
        .map(|(_, file)| file)
        .expect("a qualified shadow project");
    let work_member = std::fs::read_dir(Path::new(shadow_root).join("tasks").join(shadow_file))
        .expect("the shadow store holds the project's tasks")
        .filter_map(Result::ok)
        .find(|entry| {
            std::fs::read_to_string(entry.path())
                .is_ok_and(|document| document.contains("onepipeline.id: work"))
        })
        .and_then(|entry| {
            entry
                .path()
                .file_stem()
                .map(|stem| format!("{shadow_project}/{}", stem.to_string_lossy()))
        })
        .expect("the shadow store holds work's task");

    let member_calls = &calls[asked_before..];
    assert!(
        member_calls
            .iter()
            .any(|call| is_verb(call, ["project", "copy"])),
        "no copy was run for the transition"
    );
    for call in member_calls {
        if is_verb(call, ["project", "show"]) {
            assert_eq!(call, &["project", "show", project.as_str(), "--json"]);
        } else if is_verb(call, ["task", "show"]) {
            assert_eq!(
                call,
                &["task", "show", work_origin.as_str(), "--json"],
                "a member projection read an item other than the member it names"
            );
        } else if is_verb(call, ["project", "copy"]) {
            let named: Vec<&String> = call
                .windows(2)
                .filter(|pair| pair[0] == "--member")
                .map(|pair| &pair[1])
                .collect();
            assert_eq!(
                named,
                [&work_member],
                "the copy named other members: {call:?}"
            );
            assert!(!call.iter().any(|arg| arg == "--no-tasks"), "{call:?}");
        } else {
            panic!("a member projection asked the store for something else: {call:?}");
        }
        assert!(
            !call.iter().any(|arg| arg == &aside_origin),
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
        None,
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

    refusing(
        &world,
        "onetaskgraph.project-copy.refuse",
        STALE_ORIGIN_SAID,
        STALE_ORIGIN,
    );
    noted(&world, run, "later", "refused at the copy");
    let member = recorded(1)[mark].clone();
    assert_eq!(member["scope"], "members", "{member}");
    assert_eq!(member["items"], json!(["later"]), "{member}");
    assert_eq!(member["outcome"], "failed", "{member}");
    assert_eq!(member["class"], "refused", "{member}");
    assert_eq!(member["kind"], "stale-origin", "{member}");
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
    stops_refusing(&world, "onetaskgraph.project-copy.refuse");

    refusing(
        &world,
        "onetaskgraph.task-list.refuse",
        SOURCE_REFUSED_SAID,
        SOURCE_REFUSED,
    );
    noted(&world, run, "later", "refused at the page of tasks");
    let whole = recorded(2)[mark + 1].clone();
    assert_eq!(whole["scope"], "whole", "{whole}");
    assert_eq!(whole["whole_because"], "after-failure", "{whole}");
    assert_eq!(whole["items"], json!(["later", "work"]), "{whole}");
    assert_eq!(whole["outcome"], "failed", "{whole}");
    assert_eq!(whole["class"], "refused", "{whole}");
    assert_eq!(whole["kind"], "refused", "{whole}");
    stops_refusing(&world, "onetaskgraph.task-list.refuse");

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
}

/// Against a store that reports a version older than the first offering a member copy, every
/// projection is whole and recorded `store-lacks-members`, and no copy naming a member is ever
/// attempted. The store is asked nothing to find out: the answer is decided once, before the
/// first projection, and kept in the run's directory — so no projection asks the version again,
/// and a driver an adoption starts keeps the run's answer even from a store that now reports a
/// release that offers one.
#[test]
fn a_store_older_than_the_member_copy_is_projected_whole_and_decided_once_per_run() {
    let run = "projections-older-store";
    let (world, project) = a_run_projecting_through_a_recording_store(
        "writeback-projections-older-store",
        run,
        vec![agent("work", &[]), agent("later", &["work"])],
        &["work"],
        Some("onetaskgraph 0.2.29\n"),
    );
    projected_until(
        &world,
        run,
        &project,
        "the running node to reach the board",
        |tasks| board_word(tasks, "work").as_deref() == Some("in-progress"),
    );
    let answer_file = in_run_dir(&proposed()["detection"]["record"]);
    let answer = world.run_json(run, &answer_file);
    assert_eq!(answer, json!({"version": "0.2.29", "members": false}));
    let versions_asked = |world: &World| {
        store_calls(world)
            .iter()
            .filter(|call| call == &&["--version"])
            .count()
    };
    let asked_once = versions_asked(&world);

    for note in ["one change", "another change"] {
        let before = records(&world, run).len();
        noted(&world, run, "later", note);
        world.until(&format!("`{note}` to be projected"), |world| {
            records(world, run).len() > before && every_attempt_landed(world, run)
        });
    }
    assert_eq!(
        versions_asked(&world),
        asked_once,
        "a projection asked the store its version again"
    );

    let recorded_before = records(&world, run).len();
    world.script("onetaskgraph.version", "onetaskgraph 0.2.30\n");
    adopted(&world, run);
    world.until("the adopted driver's first projection", |world| {
        records(world, run).len() > recorded_before
    });
    let before = records(&world, run).len();
    noted(&world, run, "later", "a change for the adopted driver");
    world.until("the adopted driver's change to be projected", |world| {
        records(world, run).len() > before
    });

    let all = records(&world, run);
    assert!(all.len() >= 5, "{all:?}");
    for record in &all {
        assert_eq!(record["scope"], "whole", "{record}");
        assert_eq!(record["whole_because"], "store-lacks-members", "{record}");
        assert_eq!(record["items"], json!(["later", "work"]), "{record}");
    }
    assert!(
        !store_calls(&world)
            .iter()
            .flatten()
            .any(|arg| arg == "--member"),
        "a copy naming a member was attempted against a store that offers none"
    );
    assert_eq!(
        world.run_json(run, &answer_file),
        answer,
        "the adopted driver decided the run's answer again"
    );
}

/// The record carries exactly what the copy report said: its `spent` object verbatim and its
/// per-item actions counted. The store double hands the worker the real store's report for one
/// copy rewritten to carry known figures — the copy itself is still the real store's work — so
/// a worker recording `spent` or `actions` as anything but what the report said fails here. The
/// copy after it reports what the real store did.
#[test]
fn the_record_carries_exactly_what_the_copy_report_said_it_did_and_spent() {
    let run = "projections-report";
    let (world, project) = a_run_projecting_through_a_recording_store(
        "writeback-projections-report",
        run,
        vec![agent("work", &[]), agent("later", &["work"])],
        &["work"],
        None,
    );
    projected_until(
        &world,
        run,
        &project,
        "the running node to reach the board",
        |tasks| board_word(tasks, "work").as_deref() == Some("in-progress"),
    );

    let spent = json!({
        "requests": 7,
        "budgets": [
            {"budget": "graphql", "unit": "points", "amount": 12, "lower_bound": false},
            {"budget": "rest", "unit": "requests", "amount": 2, "lower_bound": true},
        ],
    });
    let mut items = Vec::new();
    for (action, count) in [
        ("created", 2),
        ("updated", 3),
        ("unchanged", 5),
        ("orphaned", 1),
    ] {
        for n in 0..count {
            items.push(json!({
                "source": format!("elsewhere:board/{action}-{n}"),
                "action": action,
                "destination": format!("plans:elsewhere/{action}-{n}"),
            }));
        }
    }
    let rewrite = "onetaskgraph.project-copy.rewrite";
    world.script(
        rewrite,
        &json!({"items": items, "spent": spent}).to_string(),
    );
    let mark = records(&world, run).len();
    noted(&world, run, "later", "counted");
    world.until("the rewritten report to be recorded", |world| {
        !world.fakes.join(rewrite).exists() && records(world, run).len() > mark
    });

    let counted = records(&world, run)[mark].clone();
    assert_eq!(counted["outcome"], "projected", "{counted}");
    assert_eq!(counted["scope"], "members", "{counted}");
    assert_eq!(counted["spent"], spent, "{counted}");
    assert_eq!(
        counted["actions"],
        json!({"created": 2, "updated": 3, "unchanged": 5, "orphaned": 1}),
        "{counted}"
    );
    world.until_store("the copy the report was rewritten for to land", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"] == "counted"
        })
    });

    noted(&world, run, "later", "reported as it was");
    world.until("the next projection to be recorded", |world| {
        records(world, run).len() > mark + 1
    });
    let real = records(&world, run)[mark + 1].clone();
    assert_eq!(real["spent"], Value::Null, "{real}");
    assert_eq!(
        real["actions"],
        json!({"created": 0, "updated": 1, "unchanged": 1, "orphaned": 0}),
        "{real}"
    );
}
