//! Where a plan comes from: one project of a real `onetaskgraph` store.
//!
//! Every journey in this suite launches from a store, so what is held here is
//! the seam itself — the mapping from a project to the graph a run executes, and
//! the binary that mapping is read through. Both are driven for real: the store
//! is a folder of Markdown on this host with no remote system in it, and the
//! binary is the one an operator installed, resolved the way the contract says
//! it is resolved.
//!
//! The one double is for the version check, and only for the version check: an
//! install of the wrong version is the one thing a real binary cannot be asked
//! to be. It answers `--version` and refuses every other invocation, so it can
//! never stand in for reading a plan.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. `onetaskgraph` is not among them: every plan below is read out of the
// real store binary against a real folder of Markdown. `harness.rs` carries the same
// suppression and the full rationale.

use crate::harness::{
    agent, double, lifecycle, onetaskgraph_binary, plan_of, renamed, World, REFUSED,
    RENDEZVOUS_SECONDS_ENV, STORE_BINARY_ENV,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// A run launches from a local Markdown project, with no remote system in it at
/// all, and executes the graph that project holds.
///
/// The flow the store exists for: author the plan where you already keep your
/// work, read it back, and run it. `local-md` is not a lesser source here —
/// nothing special-cases a remote one — so a project id of that source launches
/// directly, with no copy into a backend first.
#[test]
fn a_run_launches_from_a_local_markdown_project_and_executes_the_graph_it_holds() {
    let world = World::new("store-localmd");
    let project = world.plan(
        "localmd",
        &json!({
            "schema_version": 3,
            "name": "localmd",
            "concurrency": 4,
            "goal": {"text": "Deliver it from a folder of Markdown"},
            "tasks": [
                {"id": "design", "persona": "engineer", "title": "feat: design it",
                 "task": "## What\nDesign it."},
                {"id": "build", "persona": "engineer", "title": "feat: build it",
                 "task": "## What\nBuild it.", "deps": ["design"]},
            ],
        }),
    );
    assert_eq!(
        project, "plans:localmd-board",
        "the project id a person types"
    );

    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file("localmd", "result.json").is_file()
    });

    // The graph that executed is the project's: both nodes ran, and the one that
    // depends on the other ran after it.
    let result = world.run_json("localmd", "result.json");
    let status = |id: &str| {
        result["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .find(|node| node["id"] == id)
            .unwrap_or_else(|| panic!("{id} is missing from {result}"))["status"]
            .clone()
    };
    assert_eq!(status("design"), "done", "{result}");
    assert_eq!(status("build"), "done", "{result}");

    let dispatched: Vec<String> = world
        .events_of("localmd", "node-dispatched")
        .into_iter()
        .filter_map(|event| event["labels"]["node"].as_str().map(ToOwned::to_owned))
        .collect();
    assert_eq!(
        dispatched,
        ["design", "build"],
        "the dependency edge the project drew did not order the dispatches"
    );

    // And the goal the project stated is the run's, read back through the view a
    // planner reads it in.
    world
        .run(&["goals", "localmd"])
        .exited(0)
        .out_has("Deliver it from a folder of Markdown");
}

/// Write-back owns exactly what the plan document declares, and preserves everything the
/// plan does not model. Drive that rule through the installed CLI and a real local-md
/// store: a settlement must arrive without renaming the project, without changing a
/// present project body or an absent one, and without dropping a label from the project or
/// from any task in it.
///
/// The destination here is deliberately one where a project's **title is not its native
/// identifier**. On a store where those two coincide, writing the identifier as the title
/// is byte-identical to preserving it, and no assertion could tell a right answer from a
/// wrong one — which is how the rename shipped.
#[test]
fn settlement_preserves_everything_the_plan_does_not_declare() {
    let world = World::new("store-project-content");
    for (name, body) in [
        ("project-with-content", "A person's project description.\n"),
        ("project-without-content", "\n"),
    ] {
        let project = world.plan(
            name,
            &plan_of(name, vec![crate::harness::agent("work", &[])]),
        );
        // The title a person gave the board, which is not the identifier the store holds
        // it under and is not derivable from it.
        let titled = format!("Ship {name}, as a person titled it");
        let identifier = crate::harness::project_id(name);
        let path = world
            .store()
            .join("projects")
            .join(format!("{identifier}.md"));
        let original = std::fs::read_to_string(&path)
            .expect("the authored project document")
            .replacen(
                &format!("title: {}", json!(name)),
                &format!("title: {}", json!(titled)),
                1,
            )
            .replacen(
                "metadata: {",
                &format!(
                    "labels: {}\nmetadata: {{\"authored.note\":\"keep this value\",",
                    json!(["planning", "q3"])
                ),
                1,
            );
        let (front, _) = original
            .split_once("---\n\n")
            .expect("the fixture's front matter delimiter");
        std::fs::write(&path, format!("{front}---\n{body}")).expect("the project body is authored");
        let authored_document = std::fs::read_to_string(&path).expect("the authored document");
        // And a label on the plan's own task, which is the label an operator adds to one
        // issue and which a projection that wrote none would silently delete.
        let task = world
            .store()
            .join("tasks")
            .join(&identifier)
            .join("000-work.md");
        let authored_task = std::fs::read_to_string(&task)
            .expect("the authored task document")
            .replacen(
                "metadata: {",
                &format!("labels: {}\nmetadata: {{", json!(["needs-review"])),
                1,
            );
        std::fs::write(&task, &authored_task).expect("the task label is authored");

        let before = world.store_project(&project)["items"][0]["item"].clone();
        let labels_before = world.store_task_labels(&project);
        let tasks_before = preserved_tasks(&world, &project);
        assert_eq!(
            before["title"], titled,
            "the fixture did not author a title of its own for {project}"
        );
        assert_ne!(
            before["title"], name,
            "the fixture's title is the project's own identifier, which proves nothing"
        );
        assert_eq!(
            before["labels"],
            json!([
                {"id": "planning", "name": "planning", "color": null},
                {"id": "q3", "name": "q3", "color": null},
            ]),
            "the fixture did not author the project's labels for {project}"
        );
        assert_eq!(
            labels_before,
            std::collections::BTreeMap::from([(
                "work".to_owned(),
                json!([{"id": "needs-review", "name": "needs-review", "color": null}])
            )]),
            "the fixture did not author the task's labels for {project}"
        );

        world.run(&["start", &project, "--attach"]).settled();
        world.until_store("the settlement to reach the project", |world| {
            world
                .store_tasks(&project)
                .iter()
                .any(|task| task["item"]["metadata"]["onepipeline.settlement"].is_object())
        });

        let after = world.store_project(&project)["items"][0]["item"].clone();
        assert_eq!(
            after["title"], before["title"],
            "settlement renamed the project for {project}"
        );
        assert_eq!(
            after["labels"], before["labels"],
            "settlement changed the project's labels for {project}"
        );
        assert_eq!(
            world.store_task_labels(&project),
            labels_before,
            "settlement changed a task's labels for {project}"
        );
        assert_eq!(
            after["content"], before["content"],
            "settlement changed authored content for {project}"
        );
        // The complement, as one value: everything the writer does not own, held against
        // what the destination held before it wrote. "Did the new thing arrive?" and "did
        // anything else leave?" are different questions, and a field-by-field presence
        // assertion only ever asked the first.
        assert_eq!(
            preserved(&after),
            preserved(&before),
            "settlement changed something the plan does not declare for {project}"
        );
        assert_eq!(
            preserved_tasks(&world, &project),
            tasks_before,
            "settlement changed something a task's plan does not declare for {project}"
        );
        // And that assertion has teeth: a destination one unowned field lighter has to
        // fail it, or it could never have failed on a deletion.
        assert_ne!(
            preserved(&after),
            preserved(&without(&before, "authored.note")),
            "the complement assertion cannot fail on a deleted field, so it proves nothing"
        );
        let settled_document =
            std::fs::read_to_string(&path).expect("the settled project document");
        let authored_body = authored_document
            .split_once("\n---\n")
            .expect("the authored front matter closes")
            .1;
        let settled_body = settled_document
            .split_once("\n---\n")
            .expect("the settled front matter closes")
            .1;
        assert_eq!(
            settled_body, authored_body,
            "settlement changed the source document body for {project}"
        );
    }
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the edge these three
// need is the crate under test — each drives the compiled binary against its own write-back
// worker — so a project of their own would declare the same dependency and skip nothing.
// Same grounds as the block at
// `a_projection_that_keeps_failing_is_retried_at_a_ceiling_rather_than_abandoned`.
/// A settlement reaches a store at a release this build was **not** written against.
///
/// `onetaskgraph` grows its own machine answers in patch releases — `location` arrived on
/// the project item at 0.2.14 — and this projection reads those answers through types of
/// its own. While those types denied unknown fields, the first field the store added
/// turned every projection into a parse failure; and because write-back is best-effort the
/// run still settled `complete`, so the only evidence was one line on a driver log that a
/// detached run writes where nobody opens it. A consuming host could not adopt any release
/// from 0.2.14 onward without silently losing every settlement projection, which is the
/// worst shape this failure could have taken.
///
/// So the store here is the real one, over the real folder of Markdown, served at a
/// release carrying a field this build has never heard of — on the response, on each item
/// of it, on that item's own body, and on every label it holds. The settlement has to
/// arrive anyway, and everything the plan does not declare has to still be there after it.
#[test]
fn a_settlement_reaches_a_store_whose_answer_grew_a_field_this_build_does_not_know() {
    let world = World::new("store-writeback-grown");
    let name = "writeback-grown";
    let project = world.plan(name, &plan_of(name, vec![agent("work", &[])]));
    let identifier = crate::harness::project_id(name);

    // A board a person has made their own: labels and metadata of their own on the
    // project, a label on the plan's task, and a description. None of it is declared by a
    // plan, so all of it is what the projection has to carry through.
    let document = world
        .store()
        .join("projects")
        .join(format!("{identifier}.md"));
    amend(&document, |front| {
        front.insert("labels".to_owned(), json!(["planning", "q3"]));
        front
            .entry("metadata")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("a project's metadata is a mapping")
            .insert("authored.note".to_owned(), json!("keep this value"));
    });
    let authored = std::fs::read_to_string(&document).expect("the authored project document");
    std::fs::write(
        &document,
        format!("{authored}A person's own description.\n"),
    )
    .expect("the project's description is authored");
    label(
        &world
            .store()
            .join("tasks")
            .join(&identifier)
            .join("000-work.md"),
        &["needs-review"],
    );

    // The release the store is served at. Every read and write below is the real binary's,
    // against that same folder of Markdown — what this adds is the one thing an install on
    // this host cannot be asked to be, an answer from a release later than the one this
    // build was written against.
    world.script(
        "onetaskgraph.delegate",
        &onetaskgraph_binary().to_string_lossy(),
    );
    let grown = json!({"location": {"kind": "board", "url": "https://example.invalid/boards/1"}});
    world.script("onetaskgraph.project-show.grow", &grown.to_string());
    world.script("onetaskgraph.task-list.grow", &grown.to_string());
    let world = world.with_env(
        STORE_BINARY_ENV,
        &double("fake-onetaskgraph").to_string_lossy(),
    );

    // Read through the real binary, which is where the assertions of this journey live:
    // the growth is the run's variable and never the fixture's own state.
    let before = world.store_project(&project)["items"][0]["item"].clone();
    let labels_before = world.store_task_labels(&project);
    let tasks_before = preserved_tasks(&world, &project);
    assert!(
        before["location"].is_null(),
        "the store's own answer already carries the field this journey adds, so nothing \
         here is about a field this build does not know: {before}"
    );
    assert_eq!(
        before["labels"],
        json!([
            {"id": "planning", "name": "planning", "color": null},
            {"id": "q3", "name": "q3", "color": null},
        ]),
        "the fixture authored no project labels, so a label read through a grown shape \
         proves nothing"
    );
    assert_eq!(
        labels_before,
        std::collections::BTreeMap::from([(
            "work".to_owned(),
            json!([{"id": "needs-review", "name": "needs-review", "color": null}])
        )]),
        "the fixture authored no task labels"
    );

    // Detached, so a projection failure would be on the driver's log this journey reads —
    // which is exactly where it went unread.
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    world.until_store("the settlement to reach the project", |world| {
        world
            .store_tasks(&project)
            .iter()
            .any(|task| task["item"]["metadata"]["onepipeline.settlement"].is_object())
    });

    // And this journey could have failed: the bytes the projection actually parsed are the
    // ones the worker kept, and they carry the field. Against a write-back that denies
    // unknown fields, no settlement above ever arrives.
    let answered = world.run_json(name, "writeback-project-show.stdout");
    assert!(
        answered["items"][0]["item"]["location"].is_object(),
        "the answer this projection read carried no field this build does not know, so it \
         could not have told a tolerant write-back from a strict one: {answered}"
    );
    let log = std::fs::read_to_string(world.run_file(name, "driver.log"))
        .expect("the driver log is readable");
    assert!(
        !log.contains("onetaskgraph write-back failed"),
        "a projection failed against a store that had only grown a field: {log}"
    );

    // The field changed nothing about what was written: everything the plan does not
    // declare is still there, and the field itself was not carried onto the destination.
    let after = world.store_project(&project)["items"][0]["item"].clone();
    assert_eq!(
        preserved(&after),
        preserved(&before),
        "settlement changed something the plan does not declare"
    );
    assert_eq!(
        preserved_tasks(&world, &project),
        tasks_before,
        "settlement changed something a task's plan does not declare"
    );
    assert_eq!(
        world.store_task_labels(&project),
        labels_before,
        "settlement changed a task's labels"
    );
    assert_ne!(
        preserved(&after),
        preserved(&without(&before, "authored.note")),
        "the complement assertion cannot fail on a deleted field, so it proves nothing"
    );
    assert!(
        after["location"].is_null(),
        "a field this build does not know was read off one release and written onto \
         another: {after}"
    );
}

/// Tolerating a field this build does not know is not tolerating anything: an answer
/// **missing** a field the projection reads is still refused, and the refusal names it.
///
/// The store is the same real one, served at a release whose project item stopped carrying
/// `labels` — a field the projection reads off the destination and writes back, and one
/// the plan reader never looks at, so what this journey moves is the write-back's own read
/// and nothing else. A projection that read it as absent would write a project with no
/// labels, which is the deletion the strict-destination journey exists to stop.
#[test]
fn a_store_answer_missing_a_field_the_projection_reads_is_still_refused_by_name() {
    let world = World::new("store-writeback-shrunk");
    let name = "writeback-shrunk";
    let project = world.plan(name, &plan_of(name, vec![agent("work", &[])]));
    world.script(
        "onetaskgraph.delegate",
        &onetaskgraph_binary().to_string_lossy(),
    );
    world.script("onetaskgraph.project-show.shrink", "labels\n");
    let world = world.with_env(
        STORE_BINARY_ENV,
        &double("fake-onetaskgraph").to_string_lossy(),
    );

    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    world.until("the projection failure to be reported", |world| {
        std::fs::read_to_string(world.run_file(name, "driver.log"))
            .is_ok_and(|log| log.contains("onetaskgraph write-back failed"))
    });
    let log = std::fs::read_to_string(world.run_file(name, "driver.log"))
        .expect("the driver log is readable");
    assert!(
        log.contains("missing field `labels`"),
        "the refusal does not name the field the store's answer stopped carrying: {log}"
    );

    // And the planner heard what was wrong, rather than only the log a detached run writes
    // where nobody opens it.
    let raised: Vec<Value> = world
        .events_of(name, "planner-surface-queued")
        .into_iter()
        .filter(|event| {
            event["payload"]["message"]
                .as_str()
                .is_some_and(|said| said.contains("did not take this run's projection"))
        })
        .collect();
    assert!(
        raised.iter().any(|event| {
            event["payload"]["message"]
                .as_str()
                .is_some_and(|said| said.contains("missing field `labels`"))
        }),
        "the planner was told a projection failed without being told what was wrong: \
         {raised:?}"
    );

    // Nothing was projected out of what survived: a read this build cannot act on is a
    // refusal, not a projection assembled from the fields that were still there.
    assert!(
        world
            .store_tasks(&project)
            .iter()
            .all(|task| task["item"]["metadata"]["onepipeline.settlement"].is_null()),
        "a settlement was projected out of an answer the projection had refused"
    );

    // The run itself is untouched, because write-back is best-effort and settles nothing.
    let result = world.run_json(name, "result.json");
    assert_eq!(result["state"], "complete", "{result}");
}

/// The same again, for a field the answer carries at the **wrong JSON type**.
///
/// Growth is tolerated because the store owns its own shape; a field this projection reads
/// is not tolerated at whatever type happens to arrive. `labels` is where that matters
/// most: the projection reads the destination's labels and writes them back whole, so a
/// read that took a string for a list would write a project carrying none — and the
/// destination that refuses a label-dropping write is the one this run's every later
/// settlement then stops reaching.
#[test]
fn a_store_answer_carrying_a_field_at_the_wrong_type_is_still_refused_by_name() {
    let world = World::new("store-writeback-retyped");
    let name = "writeback-retyped";
    let project = world.plan(name, &plan_of(name, vec![agent("work", &[])]));
    label(
        &world
            .store()
            .join("projects")
            .join(format!("{}.md", crate::harness::project_id(name))),
        &["planning", "q3"],
    );
    world.script(
        "onetaskgraph.delegate",
        &onetaskgraph_binary().to_string_lossy(),
    );
    // The same field the journey above takes away, at a type it is not: read by the
    // projection, never looked at by the plan reader, so what this moves is the
    // write-back's own read and the launch still reaches a running graph.
    world.script(
        "onetaskgraph.project-show.retype",
        &json!({"labels": "planning, q3"}).to_string(),
    );
    let world = world.with_env(
        STORE_BINARY_ENV,
        &double("fake-onetaskgraph").to_string_lossy(),
    );

    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    world.until("the projection failure to be reported", |world| {
        std::fs::read_to_string(world.run_file(name, "driver.log"))
            .is_ok_and(|log| log.contains("onetaskgraph write-back failed"))
    });

    // The fixture can fail: the bytes the projection actually parsed are the ones the
    // worker kept, and they carry a list answered as a string.
    let answered = world.run_json(name, "writeback-project-show.stdout");
    assert!(
        answered["items"][0]["item"]["labels"].is_string(),
        "the answer this projection read carried `labels` as the list it is, so nothing \
         here is about a field at the wrong type: {answered}"
    );

    let log = std::fs::read_to_string(world.run_file(name, "driver.log"))
        .expect("the driver log is readable");
    // Both halves: which field, and what was wrong with it. `serde_json` on its own says
    // the type it wanted and not where it wanted it, which is one sentence for every field
    // of the answer and tells an operator nothing about which to look at.
    for said in [
        "items[0].item.labels",
        "invalid type: string",
        "expected a sequence",
    ] {
        assert!(
            log.contains(said),
            "the refusal does not say {said:?}: {log}"
        );
    }

    // And the planner heard the same two halves, rather than only the log a detached run
    // writes where nobody opens it.
    let raised: Vec<Value> = world
        .events_of(name, "planner-surface-queued")
        .into_iter()
        .filter(|event| {
            event["payload"]["message"]
                .as_str()
                .is_some_and(|said| said.contains("did not take this run's projection"))
        })
        .collect();
    assert!(
        raised.iter().any(|event| {
            event["payload"]["message"].as_str().is_some_and(|said| {
                said.contains("items[0].item.labels")
                    && said.contains("invalid type: string")
                    && said.contains("expected a sequence")
            })
        }),
        "the planner was told a projection failed without being told which field was \
         answered as what: {raised:?}"
    );

    // Nothing was projected out of what could still be read: a field at a type this build
    // does not read it as is a refusal, not a projection that dropped it.
    assert!(
        world
            .store_tasks(&project)
            .iter()
            .all(|task| task["item"]["metadata"]["onepipeline.settlement"].is_null()),
        "a settlement was projected out of an answer the projection had refused"
    );
    assert_eq!(
        world.store_project(&project)["items"][0]["item"]["labels"],
        json!([
            {"id": "planning", "name": "planning", "color": null},
            {"id": "q3", "name": "q3", "color": null},
        ]),
        "the refused projection reached the destination and dropped its labels"
    );

    // The run itself is untouched, because write-back is best-effort and settles nothing.
    let result = world.run_json(name, "result.json");
    assert_eq!(result["state"], "complete", "{result}");
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// A destination that **refuses** a write whose labels differ from the ones it holds still
/// accepts this projection, run after run.
///
/// That refusal is the shipped `github-projects` rule — *"GitHub issue labels differ from
/// the labels being written"* — and it is why a dropped label is a defect rather than an
/// untidiness: an operator who puts one label on a plan's issue would otherwise stop every
/// later settlement from ever reaching that board, silently, because a failed projection
/// reaches only the driver's own log.
///
/// Nothing offline can reach GitHub, so the destination here is a **real** `onetaskgraph`
/// source carrying that rule: `label-strict-source` speaks the product's own stdio plugin
/// protocol, serves every read and write out of the real `local-md` plugin hosted in the
/// shipped `onetaskgraph-source`, and adds the refusal. The run drives it as its plan's
/// own project, so the projection this build produces is the one that destination judges.
#[test]
fn a_label_strict_destination_accepts_the_settlement_projection() {
    let world = World::new("store-writeback-label-strict");
    let store = world.store();
    let world = world
        // Both sources, exactly as an operator's own configuration would declare them: the
        // board this run launches from is the strict one.
        .with_env("ONETASKGRAPH_DEFAULT_SOURCES", "plans,strict")
        .with_env("ONETASKGRAPH_SOURCES__STRICT__PLUGIN", "subprocess")
        .with_env(
            "ONETASKGRAPH_SOURCES__STRICT__CONFIG__COMMAND",
            &double("label-strict-source").to_string_lossy(),
        )
        .with_env(
            "ONETASKGRAPH_SOURCES__STRICT__CONFIG__SETTINGS__HOST",
            &crate::harness::onetaskgraph_source_binary().to_string_lossy(),
        )
        .with_env(
            "ONETASKGRAPH_SOURCES__STRICT__CONFIG__SETTINGS__ROOT",
            &store.to_string_lossy(),
        );

    let name = "label-strict";
    world.plan(
        name,
        &plan_of(
            name,
            vec![
                crate::harness::agent("work", &[]),
                crate::harness::agent("later", &["work"]),
            ],
        ),
    );
    label(
        &world
            .store()
            .join("projects")
            .join(format!("{}.md", crate::harness::project_id(name))),
        &["roadmap"],
    );
    for file in ["000-work.md", "001-later.md"] {
        label(
            &world
                .store()
                .join("tasks")
                .join(crate::harness::project_id(name))
                .join(file),
            &["needs-review"],
        );
    }
    // The same folder of Markdown, reached through the strict destination rather than
    // directly, which is the project this run launches from.
    let project = format!("strict:{}", crate::harness::project_id(name));
    let before = world.store_project(&project)["items"][0]["item"].clone();
    let labels_before = world.store_task_labels(&project);
    assert_eq!(
        labels_before.len(),
        2,
        "the strict destination did not serve the plan's own tasks"
    );

    // Detached, so the projection's own reports are on the driver's log where this
    // journey can read them — which is exactly where a refusal would have gone unread.
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    world.until_store(
        "both settlements to reach the strict destination",
        |world| {
            let tasks = world.store_tasks(&project);
            tasks.len() == 2
                && tasks
                    .iter()
                    .all(|task| task["item"]["metadata"]["onepipeline.settlement"].is_object())
        },
    );

    // The destination never refused, so the run's own log carries no projection failure at
    // all — the failure mode this journey exists for is a projection that stops arriving.
    let log = std::fs::read_to_string(world.run_file(name, "driver.log"))
        .expect("the driver log is readable");
    assert!(
        !log.contains("onetaskgraph write-back failed"),
        "the strict destination refused a projection: {log}"
    );
    let after = world.store_project(&project)["items"][0]["item"].clone();
    assert_eq!(
        after["labels"], before["labels"],
        "settlement changed the strict destination project's labels"
    );
    assert_eq!(
        after["title"], before["title"],
        "settlement renamed the strict destination project"
    );
    assert_eq!(
        world.store_task_labels(&project),
        labels_before,
        "settlement changed a strict destination task's labels"
    );

    // And the acceptance above means something only if this destination would have
    // refused. A projection of the shape this build used to write — the same project, onto
    // the same destination item, carrying no labels — is one an operator can state as a
    // project of their own and copy across with the shipped verb, and this destination
    // refuses it.
    let dropped = world.root.join("a-projection-that-dropped-them");
    std::fs::create_dir_all(dropped.join("projects")).expect("a folder for the projection");
    std::fs::write(
        dropped.join("projects").join("board.md"),
        format!("---\ntitle: {name}\nmetadata:\n  onetaskgraph.origin: {project}\n---\n"),
    )
    .expect("the projection is authored");
    let refused = world
        .store_cmd(&[
            "project",
            "copy",
            "a-projection:board",
            "--to",
            "strict",
            "--no-tasks",
            "--set",
            "sources.a-projection.plugin=local-md",
            "--set",
            &format!("sources.a-projection.config.root={}", dropped.display()),
        ])
        .output()
        .expect("the real onetaskgraph runs the copy");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success() && said.contains("labels differ from the labels being written"),
        "the destination accepted a projection that dropped its labels, so this journey \
         could not have told a right answer from a wrong one: {said}"
    );
}

/// The reserved keys the write-back owns and overwrites on a projected item.
///
/// Everything else is the complement — what a total replacement has to carry
/// through unchanged — and [`preserved`] is that complement read off a
/// destination item.
fn owned(key: &str) -> bool {
    key.starts_with("onepipeline.") || key.starts_with("onetaskgraph.")
}

/// Everything on a destination item that the writer does **not** own.
fn preserved(item: &Value) -> Value {
    let metadata: serde_json::Map<String, Value> = item["metadata"]
        .as_object()
        .expect("a destination item carries metadata")
        .iter()
        .filter(|(key, _)| !owned(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    json!({
        "title": item["title"],
        "content": item["content"],
        "labels": item["labels"],
        "metadata": metadata,
    })
}

/// The same complement over every task of a project, by node id.
fn preserved_tasks(world: &World, project: &str) -> std::collections::BTreeMap<String, Value> {
    world
        .store_tasks(project)
        .into_iter()
        .map(|task| {
            let metadata: serde_json::Map<String, Value> = task["item"]["metadata"]
                .as_object()
                .expect("a projected task carries metadata")
                .iter()
                .filter(|(key, _)| !owned(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            (
                task["item"]["metadata"]["onepipeline.id"]
                    .as_str()
                    .expect("a projected task names its node")
                    .to_owned(),
                json!({"labels": task["item"]["labels"], "metadata": metadata}),
            )
        })
        .collect()
}

/// The same item with one metadata key deleted: the mutant a complement
/// assertion has to fail on.
fn without(item: &Value, key: &str) -> Value {
    let mut lighter = item.clone();
    lighter["metadata"]
        .as_object_mut()
        .expect("a destination item carries metadata")
        .remove(key)
        .unwrap_or_else(|| panic!("the fixture authored no {key} to delete"));
    lighter
}

fn label(path: &std::path::Path, labels: &[&str]) {
    amend(path, |front| {
        front.insert("labels".to_owned(), json!(labels));
    });
}

fn amend(path: &std::path::Path, edit: impl FnOnce(&mut serde_json::Map<String, Value>)) {
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
    std::fs::write(path, format!("---\n{rendered}---\n{body}")).expect("the document is written");
}

/// Losing the store is a projection failure, never an execution failure. The worker reports
/// it, keeps retrying off the reconcile loop, and catches the board up when it returns.
#[test]
fn an_unreachable_store_is_reported_and_retried_while_the_run_completes_unaffected() {
    let world = World::new("store-writeback-retry");
    world.script("work.wait", "hold");
    let project = world.plan(
        "writeback-retry",
        &plan_of(
            "writeback-retry",
            vec![
                crate::harness::agent("work", &[]),
                crate::harness::agent("later", &["work"]),
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the running state to reach the store", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "work"
                && task["item"]["status"]["category"] == "in-progress"
        })
    });

    let unavailable = world.root.join("plan-store-unavailable");
    renamed(
        &world.store(),
        &unavailable,
        "the store becomes unreachable",
    );
    world
        .run_with_stdin(
            &["reply", "writeback-retry"],
            &json!({
                "version": 1,
                "commands": [{"op": "context", "id": "later", "note": "retry this projection"}]
            })
            .to_string(),
        )
        .exited(0);
    world.until("write-back failure to be reported", |world| {
        std::fs::read_to_string(world.run_file("writeback-retry", "driver.log")).is_ok_and(|log| {
            log.contains("onetaskgraph write-back failed") && log.contains("retrying")
        })
    });
    renamed(&unavailable, &world.store(), "the store returns");
    world.until("write-back recovery to be reported", |world| {
        std::fs::read_to_string(world.run_file("writeback-retry", "driver.log"))
            .is_ok_and(|log| log.contains("onetaskgraph write-back recovered"))
    });
    world.until("the retried edit to reach the store", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"] == "retry this projection"
        })
    });

    // Take it away again for terminal settlement. It remains unreachable until
    // the journal says the graph is complete and the terminal publication has
    // failed, then returns while that failed publication is retryable.
    renamed(
        &world.store(),
        &unavailable,
        "the store becomes unreachable again",
    );

    // The engine remains live and its own journal, not a read from the missing
    // store, still decides what executes. Both nodes settle while the store is
    // absent, which is the graph-completion boundary the terminal projection
    // cannot influence.
    assert_eq!(
        world.events_of("writeback-retry", "node-dispatched").len(),
        1
    );
    world.release("work.go");
    world.until(
        "the graph to settle while the store is unreachable",
        |world| world.events_of("writeback-retry", "node-settled").len() == 2,
    );
    world.until("terminal write-back failure to be reported", |world| {
        std::fs::read_to_string(world.run_file("writeback-retry", "driver.log"))
            .is_ok_and(|log| log.matches("onetaskgraph write-back failed").count() >= 2)
    });
    assert!(
        !world.store().exists(),
        "the terminal failure was only observed after the store became reachable"
    );
    assert_eq!(
        world.events_of("writeback-retry", "node-dispatched").len(),
        2,
        "the outage kept the dependent node from dispatching"
    );

    // Bring the store back inside the worker's bounded closeout window. It must
    // retry the failed terminal publication and project both settlements
    // without revisiting execution.
    renamed(
        &unavailable,
        &world.store(),
        "the store returns after settlement",
    );
    world.until("terminal write-back recovery to be reported", |world| {
        std::fs::read_to_string(world.run_file("writeback-retry", "driver.log"))
            .is_ok_and(|log| log.matches("onetaskgraph write-back recovered").count() >= 2)
    });
    world.until_store(
        "the recovered terminal settlement to reach the store",
        |world| {
            world.store_tasks(&project).iter().all(|task| {
                task["item"]["status"]["category"] == "done"
                    && task["item"]["metadata"]["onepipeline.settlement"].is_object()
            })
        },
    );
    world.until("the recovered run to write its result", |world| {
        world.run_file("writeback-retry", "result.json").is_file()
    });
    let result = world.run_json("writeback-retry", "result.json");
    assert_eq!(result["state"], "complete", "{result}");
    assert!(
        result["nodes"]
            .as_array()
            .expect("result nodes")
            .iter()
            .all(|node| node["status"] == "done"),
        "the outage changed a node's settlement: {result}"
    );
    assert_eq!(
        world.events_of("writeback-retry", "node-dispatched").len(),
        2,
        "recovering terminal write-back changed execution"
    );

    // Each outage reached the planner **once**, not once per retry: the worker retries
    // until the store returns, and a surface for every attempt would bury the first. Held
    // against the driver's own report of the same episodes rather than against a number,
    // so an outage this journey did not arrange still has to agree.
    let log = std::fs::read_to_string(world.run_file("writeback-retry", "driver.log"))
        .expect("the driver log is readable");
    let episodes = log.matches("onetaskgraph write-back failed").count();
    let raised = world
        .events_of("writeback-retry", "planner-surface-queued")
        .into_iter()
        .filter(|event| {
            event["payload"]["message"]
                .as_str()
                .is_some_and(|said| said.contains("did not take this run's projection"))
        })
        .count();
    assert!(
        episodes >= 2,
        "the journey arranged no second outage:\n{log}"
    );
    assert_eq!(
        raised, episodes,
        "the planner heard {raised} times about {episodes} outages:\n{log}"
    );
}

/// A copy refusal happens after the destination task list was read successfully. The
/// write-back worker reports that child failure, retries through the same subprocess
/// boundary, and publishes the snapshot when the real sibling accepts the next copy.
#[test]
fn a_project_copy_refusal_is_reported_retried_and_recovers() {
    let world = World::new("store-writeback-copy-retry");
    world.script("work.wait", "hold");
    let project = world.plan(
        "writeback-copy-retry",
        &plan_of(
            "writeback-copy-retry",
            vec![crate::harness::agent("work", &[])],
        ),
    );
    world.script(
        "onetaskgraph.delegate",
        &onetaskgraph_binary().to_string_lossy(),
    );
    world.script(
        "onetaskgraph.project-copy.refuse.1",
        "the destination refused this copy once\n",
    );
    let world = world.with_env(
        STORE_BINARY_ENV,
        &double("fake-onetaskgraph").to_string_lossy(),
    );

    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the copy refusal and recovery to be reported", |world| {
        std::fs::read_to_string(world.run_file("writeback-copy-retry", "driver.log")).is_ok_and(
            |log| {
                log.contains("the destination refused this copy once")
                    && log.contains("onetaskgraph write-back recovered")
            },
        )
    });
    world.until_store("the retried snapshot to reach the real store", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "work"
                && task["item"]["status"]["category"] == "in-progress"
        })
    });

    world.release("work.go");
    world.until("the run to settle after copy recovery", |world| {
        world
            .run_file("writeback-copy-retry", "result.json")
            .is_file()
    });
}

/// What a hosted destination says when it is refusing for a rate limit, and what this
/// suite's store double is made to say. The words are GitHub's own: that limiter is the
/// one the retry schedule below exists for, and the driver's line has to carry the
/// destination's reason through unchanged for an operator to know which refusal it is.
const RATE_LIMITED: &str = "You have exceeded a secondary rate limit";

/// A detached run whose projections go through the store double, in front of the real
/// store.
///
/// The double delegates every call to the real `onetaskgraph` until [`starts_refusing`]
/// makes it refuse, which is how a destination that begins rate-limiting part-way through
/// a run is arranged — and, because every attempt is a real process this double records,
/// how the intervals the worker actually waits become observable at the destination rather
/// than in the shape of the code.
fn a_run_whose_destination_can_start_refusing(world: &str, run: &str) -> (World, String) {
    let world = World::new(world);
    world.script("work.wait", "hold");
    let project = world.plan(
        run,
        &plan_of(run, vec![agent("work", &[]), agent("later", &["work"])]),
    );
    world.script(
        "onetaskgraph.delegate",
        &onetaskgraph_binary().to_string_lossy(),
    );
    let world = world
        .with_env(
            STORE_BINARY_ENV,
            &double("fake-onetaskgraph").to_string_lossy(),
        )
        // A hold has to outlast the thing the journey is measuring, and what these measure
        // is a schedule in minutes: the default is written to outlast `World::until`'s
        // two-minute deadline, which is shorter than the interval a refusing destination is
        // eventually asked at. Nextest's own `terminate-after` is still the backstop, so a
        // rendezvous nobody releases is ended rather than waited out for ten minutes.
        .with_env(RENDEZVOUS_SECONDS_ENV, "600");
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the running state to reach the store", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "work"
                && task["item"]["status"]["category"] == "in-progress"
        })
    });
    (world, project)
}

fn projections_asked_for(world: &World) -> usize {
    world
        .invocations()
        .iter()
        .filter(|call| {
            call["tool"] == "onetaskgraph"
                && call["args"][0] == "project"
                && call["args"][1] == "show"
        })
        .count()
}

/// Which of the plan's nodes have run, in the order they first were dispatched.
///
/// The dispatches counted would say the same thing, until a held node outlives its
/// rendezvous and is re-asked — which is the double's business rather than the
/// projection's. What these journeys claim is that a refusing destination changed neither
/// which work ran nor what it waited for.
fn dispatched(world: &World, run: &str) -> Vec<String> {
    let mut ran: Vec<String> = Vec::new();
    for event in world.events_of(run, "node-dispatched") {
        if let Some(node) = event["labels"]["node"].as_str() {
            if !ran.iter().any(|already| already == node) {
                ran.push(node.to_owned());
            }
        }
    }
    ran
}

/// An outage this journey arranged: what the driver had already reported before it began,
/// and the reply that gives the worker a snapshot to fail on.
struct Outage {
    /// Read before the destination started refusing, because the streak can begin — and
    /// the line an operator reads can be printed — before the reply that publishes for it
    /// has even exited. A count taken afterwards would be waiting for a second streak that
    /// this outage is never going to produce.
    streaks_before: usize,
    reply: std::process::Child,
}

impl Outage {
    /// The reply that gave the worker something to project, ended.
    ///
    /// Checked rather than dropped: a journey whose reply was refused would be timing a
    /// schedule nothing had been queued for.
    fn replied(mut self) {
        let status = self.reply.wait().expect("the reply ends");
        assert!(
            status.success(),
            "the reply that gave the worker something to project exited {status}"
        );
    }
}

/// The destination starts refusing every read for a rate limit, the way a hosted store does
/// when a limiter takes against it, and the run is given something to project.
///
/// The reply is launched rather than waited for. It is what publishes the snapshot the
/// streak fails on, and the failure it leads to is the moment every interval below is
/// measured from — so a caller that waited here would start observing somewhere after the
/// thing it means to time.
fn starts_refusing(world: &World, run: &str, note: &str) -> Outage {
    let streaks_before = streaks_reported(world, run);
    world.script("onetaskgraph.refuse-reads", RATE_LIMITED);
    let mut reply = world
        .cmd(&["reply", run])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the reply starts");
    let envelope = json!({
        "version": 1,
        "commands": [{"op": "context", "id": "later", "note": note}]
    })
    .to_string();
    let mut stdin = reply.stdin.take().expect("the reply's stdin is piped");
    std::io::Write::write_all(&mut stdin, envelope.as_bytes()).expect("the envelope is written");
    drop(stdin);
    Outage {
        streaks_before,
        reply,
    }
}

fn stops_refusing(world: &World) {
    std::fs::remove_file(world.fakes.join("onetaskgraph.refuse-reads"))
        .expect("the destination stops refusing");
}

/// How many outages reached the planner, in the words the driver raises them in.
fn outages_raised(world: &World, run: &str) -> usize {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| {
            event["payload"]["message"]
                .as_str()
                .is_some_and(|said| said.contains("did not take this run's projection"))
        })
        .count()
}

fn streaks_reported(world: &World, run: &str) -> usize {
    std::fs::read_to_string(world.run_file(run, "driver.log"))
        .map(|log| log.matches("onetaskgraph write-back failed").count())
        .unwrap_or_default()
}

/// The intervals the worker waited before each of the next `count` retries of a projection
/// that has started failing.
///
/// Both ends of every interval are read off something the destination or its operator can
/// see, rather than off the shape of the code: the streak's start is the moment the driver
/// printed the line an operator reads, and each retry is the destination's own record of
/// being asked again — the `project show` every attempt opens with, and the only call a
/// refusing destination ever gets that far.
fn retry_intervals(
    world: &World,
    run: &str,
    outage: &Outage,
    count: usize,
    budget: Duration,
) -> Vec<Duration> {
    let deadline = Instant::now() + budget;
    let mut at: Vec<Instant> = Vec::new();
    while streaks_reported(world, run) == outage.streaks_before {
        assert!(
            Instant::now() < deadline,
            "no failing projection was reported within {budget:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    at.push(Instant::now());
    // Seeded at the failure rather than before it, so whatever the attempt that failed had
    // already asked for is behind us and the next thing counted is the retry.
    let mut asked = projections_asked_for(world);
    while at.len() <= count {
        assert!(
            Instant::now() < deadline,
            "the destination was asked {} more times in {budget:?}, not the {count} retries \
             this journey reads its intervals off",
            at.len() - 1
        );
        std::thread::sleep(Duration::from_millis(10));
        let now = projections_asked_for(world);
        if now > asked {
            assert_eq!(
                now,
                asked + 1,
                "{} retries landed inside one observation, so the intervals read off them \
                 are not the ones that were waited",
                now - asked
            );
            asked = now;
            at.push(Instant::now());
        }
    }
    at.windows(2).map(|pair| pair[1] - pair[0]).collect()
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] what these six wait on
// is the schedule itself — a minute-long interval cannot be observed in less than a minute
// — and the edge they need is the crate under test: they drive the compiled `onepipeline`
// binary against its own write-back worker, so a project of their own would declare the
// same dependency and skip nothing. Every e2e journey in this repository lives in this one
// binary for that reason; the single project that was split out,
// `onepipeline-note-journeys`, was split for a test binary it needs rather than a narrower
// edge. Splitting these six would invent a per-topic test project, which is a change to how
// this repository's graph and gate are shaped rather than to what this change does.
/// A destination that keeps refusing is asked further and further apart, rather than four
/// times a second for as long as the run lasts.
///
/// The refusal this schedule exists for is a rate limiter, which every further attempt
/// extends — so retrying at one fixed short interval answers a destination that is already
/// saying no by asking it harder. The first retry stays where it was, though: a projection
/// that fails once and then succeeds still lands without a delay an operator notices.
#[test]
fn a_projection_that_keeps_failing_is_retried_further_and_further_apart() {
    let run = "writeback-backoff";
    let (world, project) =
        a_run_whose_destination_can_start_refusing("store-writeback-backoff", run);

    let outage = starts_refusing(&world, run, "project this through a rate limit");
    let waited = retry_intervals(&world, run, &outage, 4, Duration::from_secs(90));
    outage.replied();

    assert!(
        waited[0] < Duration::from_secs(1),
        "the first retry of a streak came {:?} after the failure, which is a delay an \
         operator watching the board would notice",
        waited[0]
    );
    for pair in waited.windows(2) {
        assert!(
            pair[1] >= pair[0] * 2,
            "the destination was asked again after {:?} and then after {:?}, which is not a \
             schedule that grows: {waited:?}",
            pair[0],
            pair[1]
        );
    }
    assert!(
        waited[3] >= Duration::from_secs(8),
        "four failures in, the destination is still being asked every {:?}: {waited:?}",
        waited[3]
    );

    let log = std::fs::read_to_string(world.run_file(run, "driver.log"))
        .expect("the driver log is readable");
    let said: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("onetaskgraph write-back failed"))
        .collect();
    assert_eq!(
        said.len(),
        1,
        "a streak of failures was reported {} times: {log}",
        said.len()
    );
    let said = said[0];
    assert!(
        said.contains(&project) && said.contains(RATE_LIMITED),
        "the line an operator reads names neither the project nor the reason: {said}"
    );
    assert!(
        said.contains("spacing"),
        "the line an operator reads does not say the attempts are being spaced: {said}"
    );

    assert_eq!(
        dispatched(&world, run),
        ["work"],
        "the outage changed what executed"
    );
}

/// A streak that ends resets the schedule: the projection after a recovery is retried as
/// promptly as the very first one was.
///
/// Without this a run that met one outage would carry a minute-long interval for the rest
/// of its life, so a later isolated failure — the case the quarter-second start is for —
/// would leave the board stale for a minute over a failure that lasted a moment.
#[test]
fn a_projection_that_recovered_is_retried_as_promptly_as_ever_when_it_next_fails() {
    let run = "writeback-backoff-reset";
    let (world, project) =
        a_run_whose_destination_can_start_refusing("store-writeback-backoff-reset", run);

    let outage = starts_refusing(&world, run, "the first outage");
    let waited = retry_intervals(&world, run, &outage, 2, Duration::from_secs(60));
    outage.replied();
    assert!(
        waited[1] > waited[0],
        "the first streak did not grow: {waited:?}"
    );

    stops_refusing(&world);
    world.until("write-back recovery to be reported", |world| {
        std::fs::read_to_string(world.run_file(run, "driver.log"))
            .is_ok_and(|log| log.contains("onetaskgraph write-back recovered"))
    });
    world.until_store("the projection to catch the board up", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"] == "the first outage"
        })
    });

    let outage = starts_refusing(&world, run, "the second outage");
    let again = retry_intervals(&world, run, &outage, 1, Duration::from_secs(60));
    outage.replied();
    assert!(
        again[0] < Duration::from_secs(1),
        "the streak that ended left the next one starting {:?} apart, where the first \
         started {:?} apart",
        again[0],
        waited[0]
    );
}

/// A destination that never returns keeps being asked at the ceiling, rather than being
/// asked at ever-growing intervals until nobody is watching or being abandoned altogether.
///
/// This journey is the suite's slowest on purpose: what it asserts is a schedule measured
/// in minutes, and the only honest evidence for "it stopped growing here, and it kept
/// asking at that" is two consecutive waits of that length actually waited.
#[test]
fn a_projection_that_keeps_failing_is_retried_at_a_ceiling_rather_than_abandoned() {
    let run = "writeback-backoff-ceiling";
    let (world, _project) =
        a_run_whose_destination_can_start_refusing("store-writeback-backoff-ceiling", run);

    let outage = starts_refusing(&world, run, "this destination is not coming back");
    let waited = retry_intervals(&world, run, &outage, 6, Duration::from_secs(300));
    outage.replied();
    let (climbing, ceiling) = waited.split_at(waited.len() - 2);

    for pair in waited[..waited.len() - 1].windows(2) {
        assert!(
            pair[1] >= pair[0],
            "an interval shrank while the destination was still refusing: {waited:?}"
        );
    }
    assert!(
        climbing.windows(2).all(|pair| pair[1] >= pair[0] * 2),
        "the schedule did not climb to its ceiling: {waited:?}"
    );
    assert!(
        ceiling[1] <= ceiling[0].mul_f64(1.5),
        "the interval was still growing after six retries — {:?} then {:?} — so there is no \
         ceiling here, only a slower runaway: {waited:?}",
        ceiling[0],
        ceiling[1]
    );
    let log = std::fs::read_to_string(world.run_file(run, "driver.log"))
        .expect("the driver log is readable");
    assert_eq!(
        log.matches("onetaskgraph write-back failed").count(),
        1,
        "the streak was reported more than once: {log}"
    );

    // The one line an operator ever gets names how far apart to expect the attempts, and
    // that number is only worth reading if it is the interval actually waited: a line
    // promising a minute over a worker asking every twenty seconds sends an operator away
    // from a destination that is still being hammered. So the ceiling this journey measured
    // is held to what the line said, rather than to a constant copied out of the source.
    let said = log
        .lines()
        .find(|line| line.contains("onetaskgraph write-back failed"))
        .expect("the failing streak was reported");
    let promised = said
        .split_once("out to ")
        .and_then(|(_, rest)| rest.split_once(" seconds apart"))
        .and_then(|(seconds, _)| seconds.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| {
            panic!("the line does not say how far apart to expect further attempts: {said}")
        });
    assert!(
        promised >= Duration::from_secs(30),
        "the operator is told to expect another attempt every {promised:?}, which is not the \
         minutes-scale window a hosted rate limiter refuses over"
    );
    for waited_at_the_ceiling in ceiling {
        assert!(
            *waited_at_the_ceiling >= promised && *waited_at_the_ceiling <= promised.mul_f64(1.5),
            "the operator was told to expect another attempt {promised:?} apart, and the \
             destination was actually asked again after {waited_at_the_ceiling:?}: {waited:?}"
        );
    }
    assert_eq!(
        dispatched(&world, run),
        ["work"],
        "the outage changed what executed"
    );
}

/// Settlement never waits on the store, whatever the schedule is doing: a run whose
/// projection is part-way through a long interval settles exactly like one whose
/// projection lands, and the stop that ends the run is honoured rather than waited out.
#[test]
fn a_run_settles_on_time_while_its_projection_is_waiting_out_a_long_interval() {
    let run = "writeback-backoff-settles";
    let (world, _project) =
        a_run_whose_destination_can_start_refusing("store-writeback-backoff-settles", run);

    let outage = starts_refusing(&world, run, "settle without me");
    // Four retries in, the next attempt is a long way off — which is the state this journey
    // needs the graph to complete in.
    let waited = retry_intervals(&world, run, &outage, 4, Duration::from_secs(90));
    outage.replied();
    assert!(
        waited[3] >= Duration::from_secs(8),
        "the outstanding interval is only {:?}, so a run settling inside it proves nothing",
        waited[3]
    );

    world.release("work.go");
    world.until("the graph to settle", |world| {
        world.events_of(run, "node-settled").len() == 2
    });
    let settled = Instant::now();
    world.until("the run to write its result", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert!(
        settled.elapsed() < waited[3],
        "settlement waited {:?} on a projection whose next attempt was {:?} away",
        settled.elapsed(),
        waited[3]
    );

    let result = world.run_json(run, "result.json");
    assert_eq!(result["state"], "complete", "{result}");
    assert!(
        result["nodes"]
            .as_array()
            .expect("result nodes")
            .iter()
            .all(|node| node["status"] == "done"),
        "the refusing destination changed a node's settlement: {result}"
    );
    assert_eq!(
        dispatched(&world, run),
        ["work", "later"],
        "the refusing destination changed what executed"
    );

    // And the planner was told, which is what makes a projection nobody took something
    // anyone can fix. A run that settles under an outage is exactly the run whose record a
    // reader trusts, so the outage reaches the planner from inside the schedule rather than
    // being dropped when the run ends part-way through an interval.
    assert_eq!(
        outages_raised(&world, run),
        1,
        "the planner was told {} times about the outage this run settled under",
        outages_raised(&world, run)
    );
}

/// A closing run's terminal snapshot is projected inside its closeout window, rather than
/// on a schedule written for a run that is still going.
///
/// The spacing exists to stop a refusing destination being asked again and again while a
/// run lasts, and a run that is ending is the opposite case: the window it has left is
/// bounded whatever the store does, so the schedule is suspended inside it. A terminal
/// snapshot left to sit out a minute-long interval inside a two-second window is a board
/// that stays wrong for good, which is the record an operator reads to see what became of
/// their plan.
#[test]
fn a_terminal_projection_is_attempted_at_closeout_rather_than_left_to_a_long_interval() {
    let run = "writeback-backoff-closeout";
    let (world, project) =
        a_run_whose_destination_can_start_refusing("store-writeback-backoff-closeout", run);

    let outage = starts_refusing(&world, run, "come back for the last one");
    let waited = retry_intervals(&world, run, &outage, 4, Duration::from_secs(90));
    outage.replied();

    // The destination returns while the worker is part-way through an interval at least as
    // long as the last one it waited out, so nothing it has already scheduled can reach the
    // store before this run ends.
    stops_refusing(&world);
    world.release("work.go");
    world.until("the graph to settle", |world| {
        world.events_of(run, "node-settled").len() == 2
    });
    let settled = Instant::now();
    world.until_store("the terminal settlement to reach the board", |world| {
        world.store_tasks(&project).iter().all(|task| {
            task["item"]["status"]["category"] == "done"
                && task["item"]["metadata"]["onepipeline.settlement"].is_object()
        })
    });
    assert!(
        settled.elapsed() < waited[3],
        "the terminal projection took {:?} to reach the board, which is the interval the \
         worker was part-way through rather than the closeout window",
        settled.elapsed()
    );
    assert_eq!(
        world.run_json(run, "result.json")["state"],
        "complete",
        "the projection that landed was not a settled run's"
    );
}

/// A stop asked for while a projection is waiting out a long interval answers on the
/// operator's own timescale, not the schedule's, and the destination is asked nothing more
/// for the run that was stopped.
///
/// What this cannot see is the worker's own half of it: `stop` signals the whole process
/// tree, so the driver holding that worker is gone whatever the worker would have done
/// with the request. That half — a worker leaving a wait it was part-way through, watched
/// after its stop was asked for — is
/// `writeback::tests::a_stop_reaches_a_worker_that_is_waiting_out_a_retry_interval`, which
/// holds its process open across the stop precisely because a journey cannot.
#[test]
fn a_stop_during_a_long_retry_interval_is_not_made_to_wait_it_out() {
    let run = "writeback-backoff-stop";
    let (world, _project) =
        a_run_whose_destination_can_start_refusing("store-writeback-backoff-stop", run);

    let outage = starts_refusing(&world, run, "stop me instead");
    let waited = retry_intervals(&world, run, &outage, 4, Duration::from_secs(90));
    outage.replied();

    let stop_requested_at = Instant::now();
    world.run(&["stop", run]).exited(0);
    assert!(
        stop_requested_at.elapsed() < waited[3],
        "the stop took {:?}, which is the schedule's timescale rather than the operator's: \
         the outstanding interval was longer than {:?}",
        stop_requested_at.elapsed(),
        waited[3]
    );
    assert_eq!(world.events_of(run, "run-stopped").len(), 1);

    // A worker woken out of a wait by the stop leaves rather than taking its turn at the
    // destination: the run that was asking is over, so nothing more is asked for it.
    let asked_by_the_stopped_run = projections_asked_for(&world);
    let watched = Instant::now();
    while watched.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            projections_asked_for(&world),
            asked_by_the_stopped_run,
            "the destination was asked again {:?} after the run was stopped",
            watched.elapsed()
        );
    }
    world.release("work.go");
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// Losing the worker's own command-capture path is handled by the same best-effort
/// boundary as losing the destination: the committed graph keeps running, and the
/// projection catches up after the filesystem recovers.
///
/// This fault injection depends on POSIX `File::create` refusing a path occupied by a
/// directory. Windows can open that path as a directory-associated handle instead: the
/// sibling then completes successfully, so there is deliberately no capture failure for
/// this journey to await. The portable sibling-refusal path is covered separately by
/// `a_project_copy_refusal_is_reported_retried_and_recovers`.
#[cfg(not(windows))]
#[test]
fn an_unwritable_writeback_capture_is_reported_retried_and_recovered() {
    let world = World::new("store-writeback-capture-retry");
    world.script("work.wait", "hold");
    let project = world.plan(
        "writeback-capture-retry",
        &plan_of(
            "writeback-capture-retry",
            vec![
                crate::harness::agent("work", &[]),
                crate::harness::agent("later", &["work"]),
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the initial projection to finish", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "work"
                && task["item"]["status"]["category"] == "in-progress"
        })
    });

    let capture = world.run_file("writeback-capture-retry", "writeback-task-list.stdout");
    std::fs::remove_file(&capture).expect("the completed capture is removed");
    std::fs::create_dir(&capture).expect("a directory makes the capture path unwritable");
    world
        .run_with_stdin(
            &["reply", "writeback-capture-retry"],
            &json!({
                "version": 1,
                "commands": [{"op": "context", "id": "later", "note": "capture recovered"}]
            })
            .to_string(),
        )
        .exited(0);
    world.until("the capture failure to be reported", |world| {
        std::fs::read_to_string(world.run_file("writeback-capture-retry", "driver.log")).is_ok_and(
            |log| {
                log.contains("onetaskgraph write-back failed")
                    && log.contains("retrying")
                    && log.contains("Is a directory")
            },
        )
    });

    std::fs::remove_dir(&capture).expect("the capture path becomes writable again");
    world.until("capture recovery to be reported", |world| {
        std::fs::read_to_string(world.run_file("writeback-capture-retry", "driver.log"))
            .is_ok_and(|log| log.contains("onetaskgraph write-back recovered"))
    });
    world.until_store("the committed edit to reach the recovered store", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"] == "capture recovered"
        })
    });

    assert_eq!(
        world
            .events_of("writeback-capture-retry", "edit-committed")
            .len(),
        1,
        "capture failure changed edit validation or journal commitment"
    );
    assert_eq!(
        world
            .events_of("writeback-capture-retry", "node-dispatched")
            .len(),
        1,
        "capture failure changed scheduling"
    );
    world.release("work.go");
    world.until("the unaffected run to complete", |world| {
        world
            .run_file("writeback-capture-retry", "result.json")
            .is_file()
    });
    assert_eq!(
        world.run_json("writeback-capture-retry", "result.json")["state"],
        "complete"
    );
}

/// Returning to the last successful graph is still a new projection when a different
/// snapshot superseded it while the store was unavailable.
#[test]
fn a_reverted_edit_supersedes_the_failed_projection_before_store_recovery() {
    let world = World::new("store-writeback-reverted-edit");
    world.script("work.wait", "hold");
    world.script("spare.wait", "hold");
    let project = world.plan(
        "writeback-reverted-edit",
        &plan_of(
            "writeback-reverted-edit",
            vec![
                crate::harness::agent("work", &[]),
                crate::harness::agent("spare", &[]),
                crate::harness::agent("later", &["work"]),
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until_store("the original edge to reach the store", |world| {
        world
            .store_tasks(&project)
            .into_iter()
            .find(|task| task["item"]["metadata"]["onepipeline.id"] == "later")
            .is_some_and(|task| {
                world
                    .store_deps(task["id"].as_str().expect("later has a qualified id"))
                    .len()
                    == 1
            })
    });

    let unavailable = world.root.join("plan-store-reverted-edit-unavailable");
    renamed(
        &world.store(),
        &unavailable,
        "the store becomes unreachable",
    );
    let reparent = |deps: &[&str]| {
        world
            .run_with_stdin(
                &["reply", "writeback-reverted-edit"],
                &json!({
                    "version": 1,
                    "commands": [{"op": "reparent", "id": "later", "deps": deps}]
                })
                .to_string(),
            )
            .exited(0)
            .out_has("\"applied\"");
    };
    reparent(&["work", "spare"]);
    world.until("the changed projection to fail", |world| {
        std::fs::read_to_string(world.run_file("writeback-reverted-edit", "driver.log"))
            .is_ok_and(|log| log.contains("onetaskgraph write-back failed"))
    });
    reparent(&["work"]);
    world.until("both edits to be committed before recovery", |world| {
        world
            .events_of("writeback-reverted-edit", "edit-committed")
            .len()
            == 2
    });

    renamed(&unavailable, &world.store(), "the store recovers");
    world.until("write-back to report recovery", |world| {
        std::fs::read_to_string(world.run_file("writeback-reverted-edit", "driver.log"))
            .is_ok_and(|log| log.contains("onetaskgraph write-back recovered"))
    });
    let recovered_later = world
        .store_tasks(&project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == "later")
        .expect("the recovered store still has later");
    assert_eq!(
        world
            .store_deps(
                recovered_later["id"]
                    .as_str()
                    .expect("later has a qualified id")
            )
            .len(),
        1,
        "recovery published the superseded two-edge snapshot after its one-edge replacement was committed"
    );
    world.until_store("the reverted edge to supersede the failed edit", |world| {
        world
            .store_tasks(&project)
            .into_iter()
            .find(|task| task["item"]["metadata"]["onepipeline.id"] == "later")
            .is_some_and(|task| {
                world
                    .store_deps(task["id"].as_str().expect("later has a qualified id"))
                    .len()
                    == 1
            })
    });

    world.release("work.go");
    world.release("spare.go");
    world.until("the unchanged run to settle", |world| {
        world
            .run_file("writeback-reverted-edit", "result.json")
            .is_file()
    });
    assert_eq!(
        world.run_json("writeback-reverted-edit", "result.json")["state"],
        "complete"
    );
}

/// A store that remains unavailable cannot hold terminal run settlement past the write-back
/// closeout bound. This drives the installed CLI and real local-md sibling, then observes the
/// user's result file while the store is still absent.
#[test]
fn a_terminal_writeback_outage_expires_without_holding_run_settlement() {
    let world = World::new("store-writeback-closeout-expiry");
    world.script("work.wait", "hold");
    let project = world.plan(
        "writeback-closeout-expiry",
        &plan_of(
            "writeback-closeout-expiry",
            vec![crate::harness::agent("work", &[])],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the work to be dispatched", |world| {
        world
            .events_of("writeback-closeout-expiry", "node-dispatched")
            .len()
            == 1
    });

    let unavailable = world.root.join("plan-store-unavailable-through-closeout");
    renamed(
        &world.store(),
        &unavailable,
        "the store becomes unreachable",
    );
    let released = std::time::Instant::now();
    world.release("work.go");
    world.until(
        "the run to settle after write-back closeout expires",
        |world| {
            world
                .run_file("writeback-closeout-expiry", "result.json")
                .is_file()
        },
    );

    assert!(
        released.elapsed() < std::time::Duration::from_secs(15),
        "the unavailable store held closeout for {:?}",
        released.elapsed()
    );
    assert!(
        !world.store().exists(),
        "the run only settled after the store became reachable"
    );
    let result = world.run_json("writeback-closeout-expiry", "result.json");
    assert_eq!(result["state"], "complete", "{result}");
    assert_eq!(result["nodes"][0]["status"], "done", "{result}");
    let log = std::fs::read_to_string(world.run_file("writeback-closeout-expiry", "driver.log"))
        .expect("the driver log is readable");
    assert!(
        log.contains("onetaskgraph write-back failed") && log.contains("retrying"),
        "the run settled without reporting the store outage: {log}"
    );
}

#[test]
fn a_settled_project_launches_again_from_its_projected_metadata() {
    let first = World::new("store-writeback-relaunch-first");
    let project = first.plan(
        "writeback-relaunch",
        &plan_of(
            "writeback-relaunch",
            vec![crate::harness::agent("work", &[])],
        ),
    );
    first.run(&["start", &project, "--attach"]).settled();
    first.until("the settlement to reach the project", |world| {
        world
            .store_tasks(&project)
            .iter()
            .any(|task| task["item"]["metadata"]["onepipeline.settlement"].is_object())
    });

    let second = World::new("store-writeback-relaunch-second").with_env(
        "ONETASKGRAPH_SOURCES__PLANS__CONFIG__ROOT",
        &first.store().to_string_lossy(),
    );
    second.run(&["start", &project, "--attach"]).settled();
    assert_eq!(
        second.run_json("writeback-relaunch", "result.json")["state"],
        "complete"
    );
}

/// A state a node *derives* rather than settles into reaches the board under the same
/// vocabulary a settlement does.
///
/// Held beside `every_settlement_reaches_the_board_under_its_own_word`, which drives the
/// five settlements: these are the states nothing dispatched — a node a failed dependency
/// made unsafe, a node the plan declared parked, a ready human action, and the node it
/// holds — and the words for them must be as distinct as the settled ones. `todo` and
/// `in progress` are still onetaskgraph's own categories, so those are asserted as
/// categories; the rest are names that vocabulary has none of.
#[test]
fn derived_waiting_failed_and_parked_states_reach_the_board_under_their_own_words() {
    let world = World::new("store-writeback-categories");
    let local = world.repository("local-direct", &[]);
    let local = local.checkout.to_string_lossy().into_owned();
    world.script("fails.fail", "1");
    let project = world.plan(
        "writeback-categories",
        &json!({
            "schema_version": 3,
            "name": "writeback-categories",
            "concurrency": 2,
            "goal": {"text": "Keep the board current"},
            "tasks": [
                {"id": "fails", "persona": "engineer", "task": "## What\nFail."},
                {"id": "skipped", "persona": "engineer", "task": "## What\nWait.", "deps": ["fails"]},
                {"id": "parked", "persona": "engineer", "task": "## What\nWait.", "parked": true},
                {"id": "approve", "kind": "human", "task": "Approve it."},
                {"id": "blocked", "persona": "engineer", "task": "## What\nWait.", "deps": ["approve"]},
                {"id": "cross", "persona": "engineer", "task": "## What\nWait.", "parked": true, "deps": ["run:missing#up"]},
                {"id": "hosted", "persona": "engineer", "task": "## What\nWait.\n\nKeep this body.", "parked": true, "repo": "github.com/owner/service", "title": "test: hosted", "max_turns": 7, "context": "carry this note"},
                {"id": "local", "persona": "engineer", "task": "## What\nWait.", "parked": true, "repo": local, "title": "test: local"}
            ]
        }),
    );
    let original_ids: std::collections::BTreeMap<String, String> = world
        .store_tasks(&project)
        .into_iter()
        .filter_map(|task| {
            Some((
                task["item"]["metadata"]["onepipeline.id"]
                    .as_str()?
                    .to_owned(),
                task["id"].as_str()?.to_owned(),
            ))
        })
        .collect();
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("every derived state to reach the store", |world| {
        let words = projected_words(world, &project);
        let says = |node: &str, word: &str| words.get(node).is_some_and(|held| held == word);
        let tasks = world.store_tasks(&project);
        // A node that failed is not `done`: that word used to cover both, and the run
        // reading it could not tell work that merged from work that was thrown away.
        says("fails", "failed")
            && says("parked", "parked")
            && says("skipped", "skipped")
            && says("approve", "todo")
            && says("blocked", "todo")
            && tasks.iter().all(|task| {
                task["item"]["metadata"]["onepipeline.id"] != "approve"
                    || task["item"]["status"]["category"] == "todo"
            })
            && tasks.iter().any(|task| {
                task["item"]["metadata"]["onepipeline.id"] == "cross"
                    && task["item"]["metadata"]["onepipeline.deps"] == json!(["run:missing#up"])
            })
            && tasks.iter().any(|task| {
                task["item"]["metadata"]["onepipeline.id"] == "hosted"
                    && task["item"]["repositories"] == json!(["github.com/owner/service"])
                    && task["id"] == original_ids["hosted"]
                    && task["item"]["title"] == "test: hosted"
                    && task["item"]["content"]
                        .as_str()
                        .is_some_and(|body| body.contains("Keep this body."))
                    && task["item"]["metadata"]["onepipeline.persona"] == "engineer"
                    && task["item"]["metadata"]["onepipeline.max_turns"] == 7
                    && task["item"]["metadata"]["onepipeline.context"] == "carry this note"
            })
            && tasks.iter().any(|task| {
                task["item"]["metadata"]["onepipeline.id"] == "local"
                    && task["item"]["metadata"]["onepipeline.repo"] == local
            })
            && {
                let project = world.store_project(&project);
                project["items"][0]["item"]["metadata"]["onepipeline.schema_version"] == 3
                    && project["items"][0]["item"]["metadata"]["onepipeline.concurrency"] == 2
                    && project["items"][0]["item"]["metadata"]["onepipeline.goal"]["text"]
                        == "Keep the board current"
                    // `name` supplied the native project title; it was not authored as
                    // metadata, so write-back must not materialise a second copy.
                    && project["items"][0]["item"]["metadata"]["onepipeline.name"].is_null()
            }
    });
}

/// A project produces the same graph a plan document of the same content
/// produced.
///
/// Field by field, against the run's own record of the plan it is executing: the
/// node fields, the dependency edges, the plan-level settings, and the lifecycle
/// node's title. The node is `parked`, so the graph is complete and nothing is
/// dispatched — what this journey is about is the mapping, not the work.
#[test]
fn a_project_reads_as_the_plan_document_of_the_same_content() {
    let world = World::new("store-mapping");
    // A real identity for the lifecycle node to name, registered the way an
    // operator registers one: `onevcs` is asked about a repository's live
    // holders before a run is minted, so a node naming an identity this host
    // does not have never reaches the mapping at all.
    world.repository("local-direct", &[]);
    let document = json!({
        "schema_version": 3,
        "name": "mapping",
        "concurrency": 2,
        "goal": {"text": "Prove the mapping"},
        "tasks": [
            {
                "id": "publish",
                "repo": "github.com/owner/service",
                "repo_type": "team",
                "workflow": "remote",
                "merge_policy": "change-auto",
                "base_branch": "main",
                "branch": "topic/publish",
                "title": "feat: publish it",
                "body": "## What\nIt publishes.",
                "persona": "engineer",
                "task": "## What\nPublish it.\n\n## Why\nUsers need it.",
                "max_turns": 12,
                "context": "the fixture moved",
                "executor": "local",
                "parked": true,
            },
            {
                "id": "audit",
                "kind": "human",
                "task": "Approve the publication.",
                "deps": ["publish"],
                "parked": true,
            },
        ],
    });
    let project = world.plan("mapping", &document);
    let task = world.store().join("tasks/mapping-board/000-publish.md");
    let held = std::fs::read_to_string(&task).expect("the task document");
    let held = held.replacen(
        r#"repositories: ["github.com/owner/service"]"#,
        r#"repositories: ["github.com/owner/service","github.com/owner/ignored"]"#,
        1,
    );
    assert!(
        held.contains("github.com/owner/ignored"),
        "the multi-repository fixture was not installed"
    );
    std::fs::write(&task, held).expect("the task names a second repository");
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file("mapping", "result.json").is_file()
    });

    // The nodes come back in the store's own order rather than the order a
    // document listed them in, which is no difference at all: the schema says
    // the nodes are in no particular order and `deps` is what orders them. So
    // both sides are read by id before they are compared.
    let by_id = |plan: &Value| {
        let mut plan = plan.clone();
        plan["tasks"]
            .as_array_mut()
            .expect("nodes")
            .sort_by_key(|node| node["id"].as_str().unwrap_or_default().to_owned());
        plan
    };
    assert_eq!(
        by_id(&world.run_json("mapping", "plan.json")),
        by_id(&document),
        "the project read as a different plan than the document of the same content"
    );
}

/// A project bigger than one page of the store is read to its end.
///
/// A store pages, and a plan is the whole graph or it is not a plan: a launch
/// that read the first page alone would execute a prefix of the project and
/// never say which nodes it left out. The world's own `page_size` is turned down
/// so the real binary really does hand back continuation tokens — three pages of
/// tasks, and a page of edges behind each of them — rather than fitting the
/// project into one response by accident.
#[test]
fn a_project_larger_than_one_page_is_read_to_its_end() {
    let world = World::new("store-paged").with_env("ONETASKGRAPH_PAGE_SIZE", "2");
    let nodes: Vec<Value> = (0..5)
        .map(|nth| {
            let id = format!("node-{nth}");
            // A chain, so every node also has an edge to page through, and so
            // the last of them can only run if the first four were read.
            let deps: Vec<String> = (nth > 0)
                .then(|| format!("node-{}", nth - 1))
                .into_iter()
                .collect();
            json!({
                "id": id,
                "persona": "engineer",
                "title": format!("feat: ship {id}"),
                "task": format!("## What\nShip {id}."),
                "deps": deps,
            })
        })
        .collect();
    let project = world.plan("paged", &plan_of("paged", nodes));

    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file("paged", "result.json").is_file()
    });

    let result = world.run_json("paged", "result.json");
    let settled: Vec<String> = result["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter(|node| node["status"] == "done")
        .filter_map(|node| node["id"].as_str().map(ToOwned::to_owned))
        .collect();
    assert_eq!(
        settled.len(),
        5,
        "the pages past the first did not reach the graph that executed: {result}"
    );
    // The chain the edges drew survived the paging too: every node ran after the
    // one it depends on.
    let dispatched: Vec<String> = world
        .events_of("paged", "node-dispatched")
        .into_iter()
        .filter_map(|event| event["labels"]["node"].as_str().map(ToOwned::to_owned))
        .collect();
    assert_eq!(
        dispatched,
        (0..5).map(|nth| format!("node-{nth}")).collect::<Vec<_>>(),
        "a dependency edge on a later page did not order its node"
    );
}

/// A launch that names something that is not a qualified project id is refused,
/// and told what one looks like.
///
/// A bare id names nothing a store can answer for — a store may hold several
/// sources and a native id is only unique within one — so it is refused where a
/// person typed it, before the binary is asked anything.
#[test]
fn a_project_id_that_is_not_qualified_is_refused_and_told_what_one_looks_like() {
    let world = World::new("store-unqualified");
    for typed in [
        "ship-the-widget",
        ":ship",
        "plans:",
        "Plan Store:ship",
        "plan_store:ship",
    ] {
        world
            .run(&["start", typed, "--detach"])
            .exited(REFUSED)
            .err_has(typed)
            .err_has("<source>:<native>");
    }
    assert!(
        world.runs.read_dir().expect("a runs root").next().is_none(),
        "a launch refused for its project id left a run directory behind"
    );
}

/// A launch names the project it came from, and the run's journal is still this
/// crate's.
///
/// The store holds the plan's **definition**; what the run executes is the graph
/// projected from its own journal. So the launch record names where the plan came
/// from and the run's state is read from the ledger, exactly as it was.
#[test]
fn the_launch_record_names_the_project_and_the_run_still_projects_from_its_journal() {
    let world = World::new("store-record");
    let project = world.plan(
        "record",
        &plan_of("record", vec![crate::harness::agent("build", &[])]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file("record", "result.json").is_file()
    });

    assert_eq!(
        world.run_json("record", "launch.json")["project"],
        project,
        "the launch record does not name the project the plan came from"
    );
    // The journal is untouched by any of this: the run's own store still carries
    // the records the engine wrote, and `status` folds them.
    assert!(
        world.run_file("record", "events.jsonl").is_file(),
        "the run journal moved"
    );
    world.run(&["status", "record"]).exited(0).out_has("record");
}

/// A store that answers something this build will not act on refuses the launch.
///
/// Five endings, and none of them is reachable through a correct install: the
/// double below is scripted to answer badly on purpose, which is the only way to
/// reach the far side of "the store said something unusable". Every one of them
/// is a **refusal** — the double never stands in for a plan that reads — and
/// every one of them names the command it came from, because a launch that
/// stopped without saying which query answered badly would leave an operator
/// with a store to search by hand.
#[test]
fn a_store_that_answers_badly_refuses_the_launch_and_names_the_query() {
    type Script<'a> = &'a [(&'a str, &'a str)];
    type BadAnswer<'a> = (&'a str, Script<'a>, &'a str);
    let cases: &[BadAnswer<'_>] = &[
        // A query that ran and failed, after the version check passed.
        (
            "exits",
            &[(
                "onetaskgraph.refuse-reads",
                "this store cannot be reached\n",
            )],
            "this store cannot be reached",
        ),
        // An answer that is not the JSON this build reads.
        (
            "malformed",
            &[("onetaskgraph.project-show", "not json at all\n")],
            "answered with something this build cannot read",
        ),
        // A `show` answering with nothing, and with several. A `show` addresses
        // one item, so both are a store this build cannot read a plan out of
        // rather than a set to take the first of.
        (
            "nothing",
            &[("onetaskgraph.project-show", r#"{"items":[],"next":null}"#)],
            "names nothing in the configured sources",
        ),
        (
            "several",
            &[(
                "onetaskgraph.project-show",
                r#"{"items":[{"id":"plans:mine","item":{"title":"T","metadata":{}}},
                   {"id":"plans:mine","item":{"title":"T","metadata":{}}}],"next":null}"#,
            )],
            "answered with more than one item",
        ),
        (
            "show-next-page",
            &[(
                "onetaskgraph.project-show",
                r#"{"items":[{"id":"plans:mine","item":{"title":"T","metadata":{}}}],
                   "next":"another"}"#,
            )],
            "answer claims another page",
        ),
        // A `show` answering with an item nobody asked for.
        (
            "elsewhere",
            &[(
                "onetaskgraph.project-show",
                r#"{"items":[{"id":"plans:other","item":{"title":"T","metadata":{}}}],"next":null}"#,
            )],
            "answered with 'plans:other'",
        ),
        // A task list carrying an item of another source, which `--project`
        // said it would not.
        (
            "foreign",
            &[
                (
                    "onetaskgraph.project-show",
                    r#"{"items":[{"id":"plans:mine","item":{"title":"T","metadata":
                       {"onepipeline.schema_version":3}}}],"next":null}"#,
                ),
                (
                    "onetaskgraph.task-list",
                    r#"{"items":[{"id":"elsewhere:build","item":{"title":"B","content":"t",
                       "project":"mine","metadata":{"onepipeline.id":"build"}}}],"next":null}"#,
                ),
            ],
            "which is an item of another source",
        ),
        // The store's wire type promises normalized origins. Validate that
        // third-party output at this boundary rather than letting an arbitrary
        // string become the repository a lifecycle node acts on.
        (
            "repository",
            &[
                (
                    "onetaskgraph.project-show",
                    r#"{"items":[{"id":"plans:mine","item":{"title":"T","metadata":
                       {"onepipeline.schema_version":3}}}],"next":null}"#,
                ),
                (
                    "onetaskgraph.task-list",
                    r#"{"items":[{"id":"plans:build","item":{"title":"B","content":"t",
                       "project":"mine","repositories":["https://github.com/acme/widget"],
                       "metadata":{"onepipeline.id":"build"}}}],"next":null}"#,
                ),
            ],
            "is not a normalized repository origin",
        ),
        // A dependency edge whose far end is a project. The store draws edges at
        // both levels and across them; a plan node is a task, so a far end that
        // is not one is refused rather than read as a node.
        (
            "project-end",
            &[
                (
                    "onetaskgraph.project-show",
                    r#"{"items":[{"id":"plans:mine","item":{"title":"T","metadata":
                       {"onepipeline.schema_version":3}}}],"next":null}"#,
                ),
                (
                    "onetaskgraph.task-list",
                    r#"{"items":[{"id":"plans:build","item":{"title":"B","content":"t",
                       "project":"mine","metadata":{"onepipeline.id":"build"}}}],"next":null}"#,
                ),
                (
                    "onetaskgraph.task-deps",
                    r#"{"items":[{"from":{"id":"plans:build","kind":"task"},
                       "to":{"id":"plans:other","kind":"project"},"kind":"blocks"}],"next":null}"#,
                ),
            ],
            "which is a project and not a node of a plan",
        ),
        // The command asks for a task's dependencies, so an edge claiming its
        // matching id is a project is still a malformed answer.
        (
            "project-near-end",
            &[
                (
                    "onetaskgraph.project-show",
                    r#"{"items":[{"id":"plans:mine","item":{"title":"T","metadata":
                       {"onepipeline.schema_version":3}}}],"next":null}"#,
                ),
                (
                    "onetaskgraph.task-list",
                    r#"{"items":[{"id":"plans:build","item":{"title":"B","content":"t",
                       "project":"mine","metadata":{"onepipeline.id":"build"}}}],"next":null}"#,
                ),
                (
                    "onetaskgraph.task-deps",
                    r#"{"items":[{"from":{"id":"plans:build","kind":"project"},
                       "to":{"id":"plans:other","kind":"task"},"kind":"blocks"}],"next":null}"#,
                ),
            ],
            "answered with an edge from a project",
        ),
        (
            "unknown-edge-kind",
            &[
                (
                    "onetaskgraph.project-show",
                    r#"{"items":[{"id":"plans:mine","item":{"title":"T","metadata":
                       {"onepipeline.schema_version":3}}}],"next":null}"#,
                ),
                (
                    "onetaskgraph.task-list",
                    r#"{"items":[{"id":"plans:build","item":{"title":"B","content":"t",
                       "project":"mine","metadata":{"onepipeline.id":"build"}}}],"next":null}"#,
                ),
                (
                    "onetaskgraph.task-deps",
                    r#"{"items":[{"from":{"id":"plans:build","kind":"task"},
                       "to":{"id":"plans:other","kind":"task"},"kind":"orders"}],"next":null}"#,
                ),
            ],
            "unknown variant `orders`",
        ),
        // A walk handed a token that is no token: it names no next page and ends
        // nothing, so the walk would ask for the same page for ever.
        (
            "emptytoken",
            &[
                (
                    "onetaskgraph.project-show",
                    r#"{"items":[{"id":"plans:mine","item":{"title":"T","metadata":
                       {"onepipeline.schema_version":3}}}],"next":null}"#,
                ),
                ("onetaskgraph.task-list", r#"{"items":[],"next":""}"#),
            ],
            "does not advance the walk",
        ),
        // A walk that cycles: two tokens handed back for ever. Read naively this
        // is a launch that never returns and never says why.
        (
            "cycle",
            &[
                (
                    "onetaskgraph.project-show",
                    r#"{"items":[{"id":"plans:mine","item":{"title":"T","metadata":
                       {"onepipeline.schema_version":3}}}],"next":null}"#,
                ),
                ("onetaskgraph.task-list", r#"{"items":[],"next":"first"}"#),
                (
                    "onetaskgraph.task-list.2",
                    r#"{"items":[],"next":"second"}"#,
                ),
                ("onetaskgraph.task-list.3", r#"{"items":[],"next":"first"}"#),
            ],
            "does not advance the walk",
        ),
    ];

    for (name, scripted, expected) in cases {
        let world = World::new(&format!("store-bad-{name}")).with_env(
            STORE_BINARY_ENV,
            &double("fake-onetaskgraph").to_string_lossy(),
        );
        world.script("onetaskgraph.version", "onetaskgraph 0.1.0\n");
        for (file, body) in *scripted {
            world.script(file, body);
        }
        world
            .run(&["start", "plans:mine", "--detach"])
            .exited(REFUSED)
            .err_has(expected);
        assert!(
            world.runs.read_dir().expect("a runs root").next().is_none(),
            "a launch refused for its store left a run directory behind"
        );
    }
}

/// Related links are visible in the store but are not plan ordering edges.
#[test]
fn a_related_task_link_does_not_become_a_plan_dependency() {
    let world = World::new("store-related-edge");
    let project = world.plan(
        "related",
        &json!({
            "schema_version": 3,
            "name": "related",
            "tasks": [
                crate::harness::agent("first", &[]),
                crate::harness::agent("second", &[]),
            ],
        }),
    );
    let first = world.store().join("tasks/related-board/000-first.md");
    let held = std::fs::read_to_string(&first).expect("the first task");
    let held = held.replace(
        "project: related-board",
        "project: related-board\ndepends_on:\n  - id: 001-second\n    kind: related",
    );
    std::fs::write(first, held).expect("the real task carries a related edge");

    world.run(&["start", &project, "--detach"]).exited(0);
    let plan = world.run_json("related", "plan.json");
    let first = plan["tasks"]
        .as_array()
        .expect("tasks")
        .iter()
        .find(|node| node["id"] == "first")
        .expect("first node");
    assert!(
        first.get("deps").is_none(),
        "the related link became a plan dependency: {first}"
    );
}

/// With no reserved or usable project name, the native project id names the run.
#[test]
fn a_project_without_a_usable_name_mints_the_run_id_from_its_native_id() {
    let world = World::new("store-native-run-id");
    let project = world.plan(
        "native-run",
        &serde_json::json!({
            "schema_version": 3,
            "name": "",
            "tasks": [crate::harness::agent("build", &[])],
        }),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    assert!(
        world.runs.join("native-run-board").is_dir(),
        "the project's native id did not name the run"
    );
}

/// When the override is empty, launch resolves `onetaskgraph` on `PATH`.
#[test]
fn onetaskgraph_resolves_by_executable_name_when_the_override_is_empty() {
    let binary = crate::harness::onetaskgraph_binary();
    let path = std::env::join_paths(
        std::iter::once(
            binary
                .parent()
                .expect("the binary has a directory")
                .to_path_buf(),
        )
        .chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .expect("a PATH");
    let world = World::new("store-path-binary")
        .with_env(STORE_BINARY_ENV, "")
        .with_env("PATH", &path.to_string_lossy());
    let project = world.plan(
        "path-binary",
        &plan_of("path-binary", vec![crate::harness::agent("build", &[])]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    assert!(world.runs.join("path-binary").is_dir());
}

/// An executable path is an OS path, not necessarily Unicode.
#[cfg(unix)]
#[test]
fn onetaskgraph_resolves_a_non_unicode_executable_path_from_the_override() {
    use std::os::unix::ffi::OsStringExt;

    let world = World::new("store-non-unicode-binary");
    let project = world.plan(
        "non-unicode-binary",
        &plan_of(
            "non-unicode-binary",
            vec![crate::harness::agent("build", &[])],
        ),
    );
    let alias = world
        .root
        .join(std::ffi::OsString::from_vec(b"onetaskgraph-\xff".to_vec()));
    if let Err(error) = std::os::unix::fs::symlink(crate::harness::onetaskgraph_binary(), &alias) {
        #[cfg(target_os = "macos")]
        if error.raw_os_error() == Some(libc::EILSEQ) {
            eprintln!(
                "macOS refused the non-Unicode executable alias with EILSEQ; override resolution \
                 for a non-Unicode executable path is unproven on this platform"
            );
            return;
        }
        panic!("a non-Unicode executable alias: {error}");
    }

    let output = world
        .cmd(&["start", &project, "--detach"])
        .env(STORE_BINARY_ENV, &alias)
        .output()
        .expect("onepipeline runs");
    assert!(
        output.status.success(),
        "non-Unicode executable path was refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(world.runs.join("non-unicode-binary").is_dir());
}

/// An absent `onetaskgraph` refuses the launch, and nothing is started for it.
///
/// The path resolved, the minimum this build needs, and how to install one — all
/// three, because "not found" alone leaves the one actionable thing unsaid.
#[test]
fn an_absent_onetaskgraph_refuses_the_launch_and_starts_nothing() {
    let world = World::new("store-absent");
    let missing = world.root.join("no-such-onetaskgraph");
    let world = world.with_env(STORE_BINARY_ENV, &missing.to_string_lossy());
    let project = world.plan(
        "absent",
        &plan_of("absent", vec![crate::harness::agent("build", &[])]),
    );

    world
        .run(&["start", &project, "--detach"])
        .exited(REFUSED)
        .err_has("no-such-onetaskgraph")
        .err_has("0.1.0 or newer")
        .err_has("cargo install onetaskgraph");
    assert!(
        !world.runs.join("absent").exists(),
        "a launch refused for its store left a run directory behind"
    );
    assert!(
        world.events_of("absent", "node-dispatched").is_empty(),
        "a launch refused for its store dispatched a node"
    );
}

/// An `onetaskgraph` below the minimum refuses the launch, naming the version it
/// found and the one it needs.
///
/// The two numbers together, because either alone leaves an operator guessing:
/// what is installed, and what has to be.
#[test]
fn an_onetaskgraph_below_the_minimum_refuses_the_launch_naming_both_versions() {
    let world = World::new("store-stale").with_env(
        STORE_BINARY_ENV,
        &double("fake-onetaskgraph").to_string_lossy(),
    );
    let project = world.plan(
        "stale",
        &plan_of("stale", vec![crate::harness::agent("build", &[])]),
    );
    world.script("onetaskgraph.version", "onetaskgraph 0.0.9\n");

    world
        .run(&["start", &project, "--detach"])
        .exited(REFUSED)
        .err_has("is version 0.0.9")
        .err_has("0.1.0 or newer")
        .err_has("cargo install onetaskgraph");
    assert!(
        !world.runs.join("stale").exists(),
        "a launch refused for a stale store left a run directory behind"
    );
}

/// An `onetaskgraph` that cannot say what it is refuses the launch too.
///
/// Two endings, and both are an install this build cannot read a plan through: a
/// binary that refuses `--version`, and one that answers with something that is
/// not a version at all. Neither is allowed to become a run that fails on its
/// first node.
#[test]
fn an_onetaskgraph_that_cannot_report_a_version_refuses_the_launch() {
    let world = World::new("store-unusable").with_env(
        STORE_BINARY_ENV,
        &double("fake-onetaskgraph").to_string_lossy(),
    );
    let project = world.plan(
        "unusable",
        &plan_of("unusable", vec![crate::harness::agent("build", &[])]),
    );

    world.script("onetaskgraph.refuse", "this install is broken\n");
    world
        .run(&["start", &project, "--detach"])
        .exited(REFUSED)
        .err_has("refused `--version`")
        .err_has("this install is broken")
        .err_has("0.1.0 or newer");

    std::fs::remove_file(world.fakes.join("onetaskgraph.refuse")).expect("the refusal is cleared");
    world.script("onetaskgraph.version", "something that is not a version\n");
    world
        .run(&["start", &project, "--detach"])
        .exited(REFUSED)
        .err_has("reported no version this build can read")
        .err_has("0.1.0 or newer");

    world.script("onetaskgraph.version", "onetaskgraph 0.1.1-01\n");
    world
        .run(&["start", &project, "--detach"])
        .exited(REFUSED)
        .err_has("reported no version this build can read")
        .err_has("0.1.1-01");

    world.script("onetaskgraph.version-invalid-utf8", "invalid");
    world
        .run(&["start", &project, "--detach"])
        .exited(REFUSED)
        .err_has("reported a version that is not UTF-8");

    assert!(
        !world.runs.join("unusable").exists(),
        "a launch refused for an unusable store left a run directory behind"
    );
}

/// Every settlement reaches the board under a word of its own.
///
/// The reading this replaces. A **failed** node was projected `done` — the same word a
/// merged node gets, with only a nested `outcome: task-failed` to disagree — so a manager
/// reviewing a board saw one word for work that merged and work that was thrown away. And
/// `cancelled` covered a park as well as a stop, though only one of the two is coming
/// back.
///
/// So five settlements are driven through one run and read back off the real store, and
/// the assertion is that no two of them share a word. That is a property a mapping which
/// collapsed any pair could not have, and it is what the previous one could not state:
/// `done` for both a merge and a failure passed every assertion anybody had written.
#[test]
fn every_settlement_reaches_the_board_under_its_own_word() {
    let world = World::new("store-settlement-words");
    // A task its agent failed, and a dispatch its provider killed. Both settle the same
    // status and mean opposite things, which is the pair the nested outcome used to be the
    // only record of.
    world.script("broke.fail", "1");
    world.script(
        "abandoned.died-as",
        "provider-failure quota the subscription behind this member is exhausted",
    );
    // Held open, so a planner can supersede it while it is still the run's to act on.
    world.script("superseded.wait", "hold");

    let name = "settlement-words";
    let project = world.plan(
        name,
        &json!({
            "schema_version": 3,
            "name": name,
            "concurrency": 4,
            "goal": {"text": "Deliver settlement-words"},
            "tasks": [
                {"id": "finished", "persona": "engineer", "task": "## What\nFinish."},
                {"id": "broke", "persona": "engineer", "task": "## What\nFail."},
                {"id": "abandoned", "persona": "engineer", "task": "## What\nLose the provider."},
                {"id": "superseded", "persona": "engineer", "task": "## What\nBe retried."},
                // The planner's own idle, declared rather than raced for: a node nothing
                // dispatches, which is not the same fact as a node something stopped.
                {"id": "idled", "persona": "engineer", "task": "## What\nWait.", "parked": true},
            ],
        }),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the held node to be dispatched", |world| {
        world
            .events_of(name, "node-dispatched")
            .iter()
            .any(|event| event["labels"]["node"] == "superseded")
    });

    // A retry takes the node out of the graph and puts its replacement in the same edit,
    // and what became of the node it replaced is a **cancel**. That is the word a park
    // must not share: a parked node is coming back, and this one is not.
    world
        .run_with_stdin(
            &["reply", name],
            &json!({
                "version": 1,
                "commands": [{"op": "retry", "id": "superseded", "node": {
                    "id": "replacement", "persona": "engineer", "task": "## What\nRetry it."
                }}],
            })
            .to_string(),
        )
        .exited(0);
    world.release("superseded.go");
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });

    let expected = BTreeMap::from([
        ("finished", "done"),
        ("broke", "failed"),
        ("abandoned", "provider-failed"),
        ("superseded", "cancelled"),
        ("idled", "parked"),
    ]);
    world.until_store("every settlement to reach the board", |world| {
        let projected = projected_words(world, &project);
        expected
            .iter()
            .all(|(node, word)| projected.get(*node).map(String::as_str) == Some(*word))
    });
    let projected = projected_words(&world, &project);

    // The run's own record, so the board is being held against what actually happened
    // rather than against a second statement of the same guess.
    let result = world.run_json(name, "result.json");
    let settled = |node: &str| {
        result["nodes"]
            .as_array()
            .expect("result nodes")
            .iter()
            .find(|entry| entry["id"] == node)
            .unwrap_or_else(|| panic!("{node} is missing from {result}"))
            .clone()
    };
    assert_eq!(settled("finished")["status"], "done", "{result}");
    assert_eq!(settled("broke")["status"], "failed", "{result}");
    assert_eq!(settled("abandoned")["status"], "failed", "{result}");
    assert_eq!(
        settled("abandoned")["outcome"],
        "provider-failed",
        "{result}"
    );
    assert_eq!(settled("superseded")["status"], "cancelled", "{result}");
    assert_eq!(settled("idled")["status"], "parked", "{result}");

    for (node, word) in &expected {
        assert_eq!(
            projected.get(*node).map(String::as_str),
            Some(*word),
            "the board says {node} is {:?}, and the run says it is {word}",
            projected.get(*node)
        );
    }
    // And no two of them share one, which is the assertion the old projection failed.
    let mut words: Vec<&String> = expected
        .keys()
        .filter_map(|node| projected.get(*node))
        .collect();
    words.sort();
    words.dedup();
    assert_eq!(
        words.len(),
        expected.len(),
        "two settlements reached the board as one word: {projected:?}"
    );
}

/// The word each node reached the board under, by node id, read back through the real
/// store binary: the status **name** rather than its normalised category.
fn projected_words(world: &World, project: &str) -> BTreeMap<String, String> {
    world
        .store_tasks(project)
        .into_iter()
        .filter_map(|task| {
            Some((
                task["item"]["metadata"]["onepipeline.id"]
                    .as_str()?
                    .to_owned(),
                task["item"]["status"]["name"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// A node that published a change is **closed** on the board, and says what closed it —
/// whether that change landed or is still open for a person.
///
/// A status alone cannot say whether the work reached anybody: a change request left open
/// for review settles a node exactly as a merge does, so a reader closing work on the word
/// `done` would close it on a change nobody has merged. Both halves are driven here — the
/// commit a merge landed at, and the URL an open change is read in — and beside them a
/// node with no change of its own, which claims neither.
#[test]
fn a_published_node_is_closed_carrying_the_change_that_closed_it_landed_or_not() {
    for (policy, landing, key) in [
        ("local-direct", "landed", "onepipeline.landing_commit"),
        ("change-open", "unlanded", "onepipeline.change_url"),
    ] {
        let world = World::new(&format!("store-landing-{policy}"));
        let repo = world.repository(policy, &[]);
        world.script("service.work", "the worker wrote this\n");
        if policy == "change-open" {
            world.script("gh.opened", "");
        } else {
            world.script("gh.merged", "");
        }

        let name = "landing";
        let project = world.plan(
            name,
            // Two nodes, and deliberately of different shapes: one publishes a change and
            // one has none of its own to publish. A fixture holding only the first could
            // not tell "records what closed it" from "records this on everything".
            &plan_of(name, vec![lifecycle("service", &[]), agent("note", &[])]),
        );
        world.run(&["start", &project, "--attach"]).settled();
        world.until_store("the settlement to reach the board", |world| {
            projected_words(world, &project)
                .get("service")
                .is_some_and(|word: &String| word == "done")
        });

        let task = |node: &str| {
            world
                .store_tasks(&project)
                .into_iter()
                .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
                .unwrap_or_else(|| panic!("{node} did not reach the board"))
        };
        let service = task("service");
        // Closed, in the store's own normalised vocabulary — which is what a board filters
        // and reports on, and what a `todo` left standing would have contradicted.
        assert_eq!(
            service["item"]["status"]["category"], "done",
            "a node that settled is still open on the board: {service}"
        );
        assert_eq!(
            service["item"]["metadata"]["onepipeline.landing"], landing,
            "the board does not say whether the change reached its base: {service}"
        );
        let recorded = service["item"]["metadata"][key]
            .as_str()
            .unwrap_or_else(|| {
                panic!("the board names no {key} for the change that closed it: {service}")
            })
            .to_owned();
        let result = world.run_json(name, "result.json");
        let node = result["nodes"]
            .as_array()
            .expect("result nodes")
            .iter()
            .find(|entry| entry["id"] == "service")
            .expect("the lifecycle node settled")
            .clone();
        assert_eq!(node["landing"], landing, "{result}");
        match key {
            // The commit the change reached its base at, held against the origin the merge
            // actually reached rather than against the run's own second statement of it.
            "onepipeline.landing_commit" => assert_eq!(
                recorded,
                crate::harness::git(&world, &repo.checkout, &["rev-parse", "origin/main"]).trim(),
                "the commit the board names is not the one the base branch is at"
            ),
            _ => assert_eq!(
                recorded,
                node["change_url"].as_str().unwrap_or_default(),
                "the change the board names is not the one the run recorded"
            ),
        }

        // And the node with no change of its own claims none, rather than an empty value a
        // reader would have to interpret.
        let note = task("note");
        for absent in [
            "onepipeline.landing",
            "onepipeline.landing_commit",
            "onepipeline.change_url",
        ] {
            assert_eq!(
                note["item"]["metadata"].get(absent),
                None,
                "a node with no change of its own was recorded as having {absent}: {note}"
            );
        }
    }
}

/// A projection that fails reaches the planner, and changes nothing about the run.
///
/// Both halves, because either one alone is the defect: a surface naming the project, the
/// items and the reason, and a run whose settlement is exactly what it would have been.
#[test]
fn a_projection_that_fails_raises_a_planner_surface_and_settles_the_run_unchanged() {
    let world = World::new("store-writeback-surface");
    world.script("work.wait", "hold");
    let name = "writeback-surface";
    let project = world.plan(
        name,
        &plan_of(name, vec![agent("work", &[]), agent("later", &["work"])]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the running state to reach the store", |world| {
        projected_words(world, &project)
            .get("work")
            .is_some_and(|word| word == "in progress")
    });

    // The store goes, and stays gone through settlement: this is the run whose *terminal*
    // projection fails, which is exactly the run a person then reads as the record.
    let unavailable = world.root.join("plan-store-gone");
    renamed(
        &world.store(),
        &unavailable,
        "the store becomes unreachable",
    );
    world.release("work.go");
    world.until("the run to settle without its store", |world| {
        world.run_file(name, "result.json").is_file()
    });

    let raised: Vec<Value> = world
        .events_of(name, "planner-surface-queued")
        .into_iter()
        .filter(|event| {
            event["payload"]["message"]
                .as_str()
                .is_some_and(|said| said.contains("did not take this run's projection"))
        })
        .collect();
    let said = raised.first().unwrap_or_else(|| {
        panic!(
            "a projection that failed outright reached nobody but the driver's log:\n{}",
            std::fs::read_to_string(world.run_file(name, "driver.log")).unwrap_or_default()
        )
    })["payload"]
        .clone();
    assert_eq!(said["kind"], "finding", "{said}");
    assert_eq!(
        said["blocking"],
        json!(false),
        "a board that is behind held the run's frontier back: {said}"
    );
    let message = said["message"].as_str().expect("a surface carries text");
    assert!(
        message.contains(&project),
        "the surface does not name the project: {message}"
    );
    for item in ["work", "later"] {
        assert!(
            message.contains(item),
            "the surface does not name the item {item}: {message}"
        );
    }
    // The reason, on **one** line. What the sibling refused with is several lines of its
    // own and the last of them is the `next:` it ends with, which read as this surface's
    // own advice — so the whole message is four lines, one of which is the reason.
    let reason = message
        .lines()
        .find_map(|line| line.strip_prefix("reason: "))
        .unwrap_or_else(|| panic!("the surface names no reason: {message}"));
    assert_eq!(
        message.lines().count(),
        4,
        "a multi-line refusal was carried onto the surface as it was spelled: {message}"
    );
    assert!(
        reason.contains("next:"),
        "the surface dropped what the store said rather than carrying it on one line: {reason}"
    );

    // And it is on the queue the planner actually reads, not only in the journal.
    world
        .run(&["next", name])
        .exited(0)
        .out_has("did not take this run's projection");

    // The run itself is untouched: nothing was settled, scheduled or failed on this.
    let result = world.run_json(name, "result.json");
    assert_eq!(result["state"], "complete", "{result}");
    assert!(
        result["nodes"]
            .as_array()
            .expect("result nodes")
            .iter()
            .all(|node| node["status"] == "done"),
        "the failed projection changed a node's settlement: {result}"
    );
    assert_eq!(
        world.events_of(name, "node-dispatched").len(),
        2,
        "the failed projection changed scheduling"
    );
}

/// The rule every store fixture in this suite is built to, held against fixtures that
/// break it.
///
/// A check that passed a one-project, id-equals-title fixture would be the thing it exists
/// to prevent, so each degenerate shape is stated here and the check has to name it.
// llmlint: ignore-block[tests_mirror_real_usage] there is no user-facing command behind
// this and deliberately so: what it holds is the rule the *fixtures* of this suite are
// built to, and the only way to prove that rule can fail is to hand it a fixture that
// breaks it. Driving the CLI here would exercise the projection this rule exists to make
// checkable, which every journey above already does.
#[test]
fn a_store_fixture_that_could_not_tell_a_right_answer_from_a_wrong_one_is_refused() {
    let world = World::new("store-fixture-rule");
    // Every store this suite builds, built the way every journey builds one.
    world.plan("sound", &plan_of("sound", vec![agent("work", &[])]));
    assert_eq!(
        crate::harness::undiscriminating(&world.store()),
        None,
        "the store fixture every journey here is built from does not meet its own rule"
    );
    // And the store this repository ships, which a run launches from byte for byte.
    assert_eq!(
        crate::harness::undiscriminating(&crate::harness::repo_file("examples/plan-store")),
        None,
        "the shipped example store could not tell a right answer from a wrong one"
    );

    let named = |store: &std::path::Path, fixture: &str, property: &str| {
        let said = crate::harness::undiscriminating(store).unwrap_or_else(|| {
            panic!("a fixture whose {property} could prove nothing was accepted")
        });
        assert!(
            said.contains(&format!("fixture '{fixture}'")),
            "the refusal does not name the fixture: {said}"
        );
        assert!(
            said.contains(property),
            "the refusal does not name the missing property: {said}"
        );
    };

    // One project, so an ignored project filter is indistinguishable from an honoured one.
    let lone = world.root.join("one-project-store");
    author_project(&lone, "board", "A person's own board");
    named(&lone, "one-project-store", "holds 1 project");

    // Two projects, one of which is titled with the identifier the store holds it under.
    let identical = world.root.join("id-equals-title-store");
    author_project(&identical, "board", "board");
    author_project(
        &identical,
        "another-board",
        "A board this run must not touch",
    );
    named(&identical, "board", "identifier is its own title");

    // Not a store at all.
    named(
        &world.root.join("nothing-here"),
        "nothing-here",
        "its projects directory could not be read",
    );
}

// llmlint: ignore-end[tests_mirror_real_usage]

/// The rule reads a store **live runs are writing to**, and a document caught between
/// the two halves of a replacement is not a fixture that could prove nothing.
///
/// A run projects its nodes back onto the store it launched from by handing
/// `onetaskgraph` a `project copy`, and that store's writer replaces a document in
/// place — `fs::write`, which truncates the file and then writes it again. Every
/// journey here that launches a second run against a store an earlier run of the same
/// journey is still settling therefore reads its project documents while something
/// else is halfway through rewriting one, and read once, that lands as
/// `has no front matter` against a fixture that is perfectly sound.
///
/// So the writer's own two halves are what this drives: a thread that alternates the
/// empty file the truncate leaves behind with the whole document that follows it,
/// while the rule is asked over and over. The rule has to answer for the document,
/// not for the instant it was asked in.
///
/// **Measured over this tree, at 440 runs — 88 000 asks — of which none refused.** 400
/// of them by invoking the compiled `e2e` binary on this test's module-qualified name,
/// and 40 through `cargo nextest run -E`, whose `binary(e2e) and test(...)` filterset
/// matches this name by substring; each run counted only where the runner itself
/// reported that one test ran and passed. Its teeth were re-checked over that same tree
/// rather than remembered: `settled_title` cut down to a single read called this sound
/// fixture a defect 189 of 200 times and failed the journey on its own assertion. The 340-run
/// figure the cadence commit carries was taken before `harness::renamed` changed this
/// file and `harness.rs`, so it is honest about that tree and does not cover this one.
///
/// **Count what the runner reports ran, never exit codes.** This test's libtest name
/// carries its module, so `--exact` over the bare function name matches nothing, runs
/// nothing, and *exits 0* — a shell loop counting exit codes reads 200 iterations of
/// that as 200 passes, which is how a reliability figure with nothing behind it gets
/// reported. Either qualify the name with `store::` or filter by substring, as the
/// nextest filterset above does.
// llmlint: ignore-block[tests_mirror_real_usage] the subject is the fixture rule itself
// and the only way to show it can be asked mid-write is to write underneath it. The
// projection this stands in for is driven for real by the journeys above.
#[test]
fn a_project_document_being_rewritten_underneath_the_rule_is_not_a_fixture_defect() {
    let world = World::new("store-fixture-rule-mid-write");
    world.plan("sound", &plan_of("sound", vec![agent("work", &[])]));
    let store = world.store();

    // The document an earlier run of the same journey is projecting onto, and the two
    // states the store's writer leaves it in: empty, then whole.
    let rewritten = store.join("projects").join("sound-board.md");
    let whole = std::fs::read_to_string(&rewritten).expect("the fixture project is readable");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writing = {
        let (rewritten, whole, stop) = (rewritten.clone(), whole.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                std::fs::write(&rewritten, "").expect("the truncate half of a replacement");
                std::fs::write(&rewritten, &whole).expect("the write half of a replacement");
            }
        })
    };

    let asked: Vec<String> = (0..200)
        .filter_map(|_| crate::harness::undiscriminating(&store))
        .collect();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    writing.join().expect("the writer ends");

    // The count and the distinct reasons: 200 copies of one reason says nothing 1 does
    // not, and the failure is read in a CI log.
    let distinct: std::collections::BTreeSet<&String> = asked.iter().collect();
    assert!(
        asked.is_empty(),
        "a sound fixture was called a defect {} of 200 times because its store was \
         mid-write: {distinct:?}",
        asked.len()
    );
    // And the rule still has its teeth over the store it was just asked about.
    assert_eq!(
        crate::harness::undiscriminating(&store),
        None,
        "the store did not survive being rewritten"
    );
}

// llmlint: ignore-end[tests_mirror_real_usage]

/// One project document, written straight rather than through [`World::plan`]: what is
/// being checked here is the rule, so the fixture has to be able to break it.
fn author_project(store: &std::path::Path, identifier: &str, title: &str) {
    let projects = store.join("projects");
    std::fs::create_dir_all(&projects).expect("a store fixture directory");
    std::fs::write(
        projects.join(format!("{identifier}.md")),
        format!("---\ntitle: {}\n---\n\n", json!(title)),
    )
    .expect("the fixture project is authored");
}

/// No assertion in these journeys is a bare presence assertion over metadata the writer
/// added: a presence assertion passes whatever the projection wrote, so it cannot fail on
/// a deletion.
///
/// Waiting for a projection to arrive is a different thing and stays allowed — a predicate
/// handed to `until_store` is a condition to wait on, and the assertions follow it — so
/// what is checked is the assertion macros alone.
// llmlint: ignore-block[tests_mirror_real_usage] the subject here is this file's own
// assertions, which is the only place the defect lives: an assertion that passes whatever
// the projection wrote cannot be caught by running anything, because it passes. Nothing
// about the crate under test is asserted, and nothing about it could be.
#[test]
fn no_write_back_assertion_is_a_bare_presence_check_over_projected_metadata() {
    // Both files that assert against a projection, so a journey moved between them is
    // still read.
    let bare: Vec<String> = [include_str!("store.rs"), include_str!("live_edit.rs")]
        .into_iter()
        .flat_map(bare_presence_assertions)
        .collect();
    assert!(
        bare.is_empty(),
        "these assertions pass whatever the projection wrote, so they cannot fail on a \
         deletion: {bare:#?}"
    );

    // And the reading has teeth: a planted one has to be found, or an empty answer above
    // means nothing. Assembled rather than written out, because this file is one of the
    // two the reading above is over — a literal one here would be found there.
    let planted = bare_presence_assertions(&format!(
        "{macro_name}(\n    task[\"metadata\"][\"onepipeline.settlement\"].is_object(),\n    \
         \"it arrived\"\n);",
        macro_name = ["assert", "!"].concat(),
    ));
    assert_eq!(
        planted.len(),
        1,
        "the reading does not find a bare presence assertion, so it proves nothing: \
         {planted:#?}"
    );
}

/// Every assertion in one source that asks only whether metadata the writer added is
/// *there*.
fn bare_presence_assertions(source: &str) -> Vec<String> {
    assertions(source)
        .into_iter()
        .filter(|invocation| {
            invocation.contains("onepipeline.")
                && ["is_object()", "is_string()", "is_array()", "is_some()"]
                    .iter()
                    .any(|shape| invocation.contains(shape))
        })
        .map(|invocation| invocation.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// Every `assert` macro invocation in a source file, as its own text.
///
/// Braces and parentheses are matched rather than lines counted: an assertion over a
/// projection spans several lines, and a line-at-a-time reading would miss the one this
/// exists to find.
fn assertions(source: &str) -> Vec<String> {
    let bytes: Vec<char> = source.chars().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let rest: String = bytes[at..].iter().take(12).collect();
        let opened = ["assert!(", "assert_eq!(", "assert_ne!("]
            .iter()
            .find(|macro_name| rest.starts_with(**macro_name))
            .copied();
        let Some(opened) = opened else {
            at += 1;
            continue;
        };
        let mut depth = 0usize;
        let mut end = at + opened.len() - 1;
        while end < bytes.len() {
            match bytes[end] {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            end += 1;
        }
        found.push(bytes[at..=end.min(bytes.len() - 1)].iter().collect());
        at = end + 1;
    }
    found
}
// llmlint: ignore-end[tests_mirror_real_usage]
