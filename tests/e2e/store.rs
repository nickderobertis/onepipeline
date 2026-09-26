//! Where a plan comes from: one project of a real `onetaskgraph` store.
//!
//! Every journey in this suite launches from a store, so what is held here is
//! the seam itself — the mapping from a project to the graph a run executes, and
//! the store that mapping is read through. Both are driven for real: the store
//! is a folder of Markdown on this host with no remote system in it, read by the
//! `onetaskgraph` library the binary under test links, configured the way an
//! operator configures it.
//!
//! What an offline store cannot be made to do — refuse, rate-limit, be
//! unreachable, or answer slowly — is arranged at the store's own plugin
//! boundary, by `scripted-source`: a real source serving the same folder through
//! the real `local-md` plugin, answering the one call a journey scripts with the
//! source error a hosted destination answers with.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// `onetaskgraph` is not substituted: every plan below is read out of the real store library
// the binary links, against a real folder of Markdown, and `scripted-source` is a real source
// of that store serving the same folder. `harness.rs` carries the same suppression and the
// full rationale.

use crate::harness::{
    agent, double, lifecycle, plan_of, project_id, renamed, World, REFUSED, RENDEZVOUS_SECONDS_ENV,
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

/// A launch on a host with **no** `onetaskgraph` executable reads its plan, runs it, and
/// projects every settlement onto its board: the store is linked, so nothing is looked up.
///
/// Nothing on the `PATH` answers to `onetaskgraph` — every directory holding one is taken
/// off it — and `ONETASKGRAPH_BIN` names a program that records that it was run and then
/// fails, which is the one way a launch that still spawned a store executable could not go
/// unnoticed: an engine that ran it would fail or leave the record behind, and one that
/// looked it up on the `PATH` would find nothing there to run.
#[cfg(unix)]
#[test]
fn a_launch_with_no_onetaskgraph_executable_reads_runs_and_projects_through_the_linked_store() {
    let world = World::new("store-no-executable");
    let ran = world.root.join("onetaskgraph-was-run");
    let refusing = world.root.join("bin-that-fails").join("onetaskgraph");
    std::fs::create_dir_all(refusing.parent().expect("a directory")).expect("a bin directory");
    onepipeline_testfakes::executable(
        &refusing,
        format!("#!/bin/sh\n: > '{}'\nexit 1\n", ran.display()),
    );
    let name = "no-executable";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![agent("design", &[]), agent("build", &["design"])],
        ),
    );

    let mut start = world.cmd(&["start", &project, "--detach"]);
    let path: Vec<std::path::PathBuf> = std::env::split_paths(
        start
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("the world states a PATH"),
    )
    .filter(|dir| !dir.join("onetaskgraph").exists() && !dir.join("onetaskgraph.exe").exists())
    .collect();
    start
        .env("PATH", std::env::join_paths(path).expect("a PATH"))
        .env("ONETASKGRAPH_BIN", &refusing);
    world.run_on(start, "start --detach").exited(0);
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });

    let result = world.run_json(name, "result.json");
    assert_eq!(result["state"], "complete", "{result}");
    world.until_store("both settlements to reach the board", |world| {
        let tasks = world.store_tasks(&project);
        tasks.len() == 2
            && tasks.iter().all(|task| {
                task["item"]["status"]["category"] == "done"
                    && task["item"]["metadata"]["onepipeline.settlement"].is_object()
            })
    });
    let records =
        std::fs::read_to_string(world.run_file(name, onepipeline::cli::WRITEBACK_PROJECTIONS_FILE))
            .expect("the projection record");
    assert!(
        records
            .lines()
            .all(|line| line.contains("\"outcome\":\"projected\"")),
        "a projection did not land:\n{records}"
    );
    assert!(
        !ran.exists(),
        "the launch ran the program ONETASKGRAPH_BIN names"
    );
}

/// A command whose store is configured by an `onetaskgraph.yaml` alone, run from `dir`: every
/// setting this world's commands carry for their plans source is taken away, so what the
/// command reads its store through is the document it discovers.
fn configured_by_document(
    world: &World,
    dir: &std::path::Path,
    args: &[&str],
) -> std::process::Command {
    let mut command = world.cmd(args);
    command
        .current_dir(dir)
        .env_remove("ONETASKGRAPH_DEFAULT_SOURCES")
        .env_remove(format!(
            "ONETASKGRAPH_SOURCES__{}__PLUGIN",
            crate::harness::STORE_SOURCE.to_uppercase()
        ))
        .env_remove(crate::harness::store_root_env());
    command
}

/// The store is configured where an operator configures it: the plan is read through the
/// `onetaskgraph.yaml` discovered from the directory a launch runs in, and every write-back
/// after it — the release a `stop` makes, and the claim an adopted driver projects — through
/// the one discovered from the directory the launch record names, whichever directory the
/// verb that makes it was run from. That configuration is read afresh for every attempt, so
/// one that stops loading refuses the projection it was read for, and one put right is what
/// the next attempt reads.
///
/// The directory `stop` and `adopt` run from holds no configuration at all, and no command
/// here carries a store setting in its environment, so a write-back that looked anywhere but
/// the launch record's directory would find no source to write to.
#[test]
fn the_store_is_discovered_from_the_launch_directory_by_the_read_and_by_every_write_back() {
    let world = World::new("store-discovered");
    world.script("work.wait", "hold");
    let name = "discovered";
    let project = world.plan(
        name,
        &plan_of(name, vec![agent("work", &[]), agent("later", &["work"])]),
    );
    let launch = world.root.join("launch-directory");
    std::fs::create_dir_all(&launch).expect("a launch directory");
    std::fs::write(
        launch.join("onetaskgraph.yaml"),
        format!(
            "sources:\n  {}:\n    plugin: local-md\n    config:\n      root: {:?}\n",
            crate::harness::STORE_SOURCE,
            world.store()
        ),
    )
    .expect("the launch directory's store configuration");
    let elsewhere = world.root.join("somewhere-else");
    std::fs::create_dir_all(&elsewhere).expect("a directory with no configuration");

    let start = configured_by_document(&world, &launch, &["start", &project, "--detach"]);
    world.run_on(start, "start --detach").exited(0);
    world.until_store(
        "the claim and the running node to reach the board",
        |world| {
            let words = projected_words(world, &project);
            words.get("work").is_some_and(|word| word == "in progress")
                && words.get("later").is_some_and(|word| word == "queued")
        },
    );

    let stop = configured_by_document(&world, &elsewhere, &["stop", name]);
    world
        .run_on(stop, "stop")
        .exited(0)
        .out_has("\"stopped\":true");
    assert_eq!(
        projected_words(&world, &project)
            .get("later")
            .map(String::as_str),
        Some("todo"),
        "the stop's release did not reach the store the launch directory configures"
    );

    let adopt = configured_by_document(&world, &elsewhere, &["adopt", name, "--detach"]);
    world.run_on(adopt, "adopt --detach").exited(0);
    world.until_store("the adopted driver's claim to reach the board", |world| {
        projected_words(world, &project)
            .get("later")
            .is_some_and(|word| word == "queued")
    });

    // The launch directory's configuration stops loading: the next projection is refused by
    // the store's own reading of it — a document it cannot parse, which no retry changes —
    // and reported once. Put right, the next change to the graph projects again.
    // Each way the store will not run on it: a document it cannot read, one it cannot parse,
    // and a setting it refuses — each its own kind, and none of them retried on a timer.
    let document = launch.join("onetaskgraph.yaml");
    let configured = std::fs::read_to_string(&document).expect("the configuration reads");
    let log = |world: &World| {
        std::fs::read_to_string(world.run_file(name, "driver.log")).unwrap_or_default()
    };
    for (kind, broken) in [
        ("config-read", None),
        ("config-syntax", Some("sources: [this is not a mapping\n")),
        ("config-setting", Some("page_size: never\n")),
    ] {
        match broken {
            Some(text) => std::fs::write(&document, text).expect("the configuration is broken"),
            // A directory where the document is: there, and not a document the store can read.
            None => {
                std::fs::remove_file(&document).expect("the configuration is taken away");
                std::fs::create_dir(&document).expect("a directory stands in its place");
            }
        }
        let reported = log(&world)
            .matches("onetaskgraph write-back failed")
            .count();
        noted(
            &world,
            name,
            "later",
            &format!("projected through a {kind} failure"),
        );
        world.until(&format!("the {kind} failure to be reported"), |world| {
            log(world).matches("onetaskgraph write-back failed").count() > reported
        });
        let said = log(&world)
            .lines()
            .filter(|line| line.contains("onetaskgraph write-back failed"))
            .last()
            .unwrap_or_default()
            .to_owned();
        assert!(
            said.contains("the store's configuration cannot be read")
                && said.contains(&format!("class: refused, kind: {kind}"))
                && said.contains("attempted again when the run's graph next changes"),
            "the line does not report the configuration the store refused: {said}"
        );
        if broken.is_none() {
            std::fs::remove_dir(&document).expect("the directory is taken away");
        }
        std::fs::write(&document, &configured).expect("the configuration is put right");
        let recovered = log(&world)
            .matches("onetaskgraph write-back recovered")
            .count();
        let repaired = format!("projected once the {kind} failure was put right");
        noted(&world, name, "later", &repaired);
        world.until("the projection to recover", |world| {
            log(world)
                .matches("onetaskgraph write-back recovered")
                .count()
                > recovered
        });
        world.until_store("the change after the repair to reach the board", |world| {
            world.store_tasks(&project).iter().any(|task| {
                task["item"]["metadata"]["onepipeline.id"] == "later"
                    && task["item"]["metadata"]["onepipeline.context"] == repaired.as_str()
            })
        });
    }
    world.release("work.go");
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    world.until_store("the settlement to reach the board", |world| {
        let words = projected_words(world, &project);
        words.get("work").is_some_and(|word| word == "done")
            && words.get("later").is_some_and(|word| word == "done")
    });
}

/// A source's credential is read from the variable its own configuration names — out of the
/// engine's own environment, or out of the `secrets.env` that environment names — and handed
/// to the source, exactly as the store's own CLI read it. A launch where nothing defines it
/// is refused as a store that cannot be read, before any run exists.
///
/// The plans source here is reached through `scripted-source` as a `subprocess` source whose
/// configuration names a credential; the variable is named in the `onetaskgraph.yaml` the
/// launch directory holds, beside the settings this world's environment carries, and what
/// the source was handed is read off its own record of the handshake — the name, and a
/// digest of the value it carried.
#[test]
fn a_sources_credential_is_read_from_the_engines_environment_or_its_secrets_file() {
    const TOKEN: &str = "ONEPIPELINE_PLAN_STORE_TOKEN";
    let world = World::new("store-credential").through_scripted_source();
    let launch = world.root.join("launch-directory");
    std::fs::create_dir_all(&launch).expect("a launch directory");
    std::fs::write(
        launch.join("onetaskgraph.yaml"),
        format!(
            "sources:\n  {}:\n    config:\n      secrets: [{TOKEN}]\n",
            crate::harness::STORE_SOURCE
        ),
    )
    .expect("the configuration naming the credential");
    // How many handshakes handed the source the credential holding `value`: read off the
    // source's own record of each handshake, which names each credential beside a digest of
    // the value it arrived with.
    let handed = |world: &World, value: &str| {
        use sha2::Digest;
        let carried = format!("{TOKEN}={:x}", sha2::Sha256::digest(value.as_bytes()));
        world
            .store_calls()
            .iter()
            .filter(|call| call[0] == "initialize")
            .filter(|call| {
                call.get(1)
                    .is_some_and(|handed| handed.split(',').any(|one| one == carried))
            })
            .count()
    };
    // Nothing defines it: the store cannot be built, and the launch says which variable.
    let project = world.plan("unset", &plan_of("unset", vec![agent("work", &[])]));
    let mut start = world.cmd(&["start", &project, "--detach"]);
    start.current_dir(&launch).env_remove(TOKEN);
    world
        .run_on(start, "start --detach")
        .exited(REFUSED)
        .err_has(TOKEN);
    assert!(
        !world.runs.join("unset").exists(),
        "a launch whose store could not be read left a run behind"
    );

    // Defined in the `secrets.env` the engine's environment names.
    let secrets = world.root.join("secrets.env");
    std::fs::write(&secrets, format!("{TOKEN}=from-the-file\n")).expect("a secrets file");
    let project = world.plan("filed", &plan_of("filed", vec![agent("work", &[])]));
    let mut start = world.cmd(&["start", &project, "--detach"]);
    start
        .current_dir(&launch)
        .env_remove(TOKEN)
        .env("ONETASKGRAPH_SECRETS_FILE", &secrets);
    world.run_on(start, "start --detach").exited(0);
    world.until("the run whose token is filed to settle", |world| {
        world.run_file("filed", "result.json").is_file()
    });
    world.until_store("its settlement to reach the board", |world| {
        world
            .store_tasks(&project)
            .iter()
            .all(|task| task["item"]["metadata"]["onepipeline.settlement"].is_object())
    });
    assert!(
        handed(&world, "from-the-file") > 0,
        "the source was never handed the credential the secrets file holds"
    );

    // Exported in the engine's own environment.
    let project = world.plan("exported", &plan_of("exported", vec![agent("work", &[])]));
    let mut start = world.cmd(&["start", &project, "--detach"]);
    start
        .current_dir(&launch)
        .env(TOKEN, "from-the-environment");
    world.run_on(start, "start --detach").exited(0);
    world.until("the run whose token is exported to settle", |world| {
        world.run_file("exported", "result.json").is_file()
    });
    world.until_store("its settlement to reach the board", |world| {
        world
            .store_tasks(&project)
            .iter()
            .all(|task| task["item"]["metadata"]["onepipeline.settlement"].is_object())
    });
    assert!(
        handed(&world, "from-the-environment") > 0,
        "the source was never handed the credential the engine's environment exports"
    );
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
/// protocol, serves every read and write out of the real `local-md` plugin — hosted in its
/// own process, by the `onetaskgraph-core` reference host it links — and adds the refusal.
/// The run drives it as its plan's own project, so the projection this build produces is
/// the one that destination judges — and the only executable this chain resolves is the
/// one cargo built for this suite.
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
    let root = dropped.to_string_lossy().into_owned();
    let refused = world.store_call_with(
        &[
            ("ONETASKGRAPH_SOURCES__A_PROJECTION__PLUGIN", "local-md"),
            ("ONETASKGRAPH_SOURCES__A_PROJECTION__CONFIG__ROOT", &root),
        ],
        |engine| async move {
            engine
                .copy(&onetaskgraph_core::CopyRequest {
                    items: onetaskgraph_core::CopyItems::new(vec![crate::harness::global(
                        "a-projection:board",
                    )])
                    .expect("one item"),
                    scope: onetaskgraph_core::CopyScope::Projects { tasks: false },
                    destination: onetaskgraph_plugin_api::SourceName::new("strict")
                        .expect("a source name"),
                    match_by: None,
                    recreate: false,
                    dry_run: false,
                })
                .await
        },
    );
    let said = match refused {
        Ok(report) => panic!(
            "the destination accepted a projection that dropped its labels, so this journey \
             could not have told a right answer from a wrong one: {report:?}"
        ),
        Err(error) => error.to_string(),
    };
    assert!(
        said.contains("labels differ from the labels being written"),
        "the destination refused the projection for another reason: {said}"
    );
}

/// A plan still reads back after a run of it has settled, and after several.
///
/// Asserted as **which record each node is** and the edges that record holds,
/// both against what the store answered with before the run — not as how many of
/// either there are, which is what the journeys above ask and what cannot tell a
/// record that moved from one that stayed. Two ways a projection loses the
/// destination, and neither shows up as a count: an edge whose far end names the
/// run's own scratch, which no reader of the plan can resolve, and a record
/// written beside the operator's rather than onto it.
///
/// Twice over, because a projection that damaged the destination once would have
/// a second run of the same plan to damage further. The destination's own
/// `onetaskgraph.origin` is out of reach here and divergence 59 says why.
#[test]
fn a_settled_plan_reads_back_holding_the_dependency_edges_it_was_authored_with() {
    let world = World::new("store-settled-readback");
    let project = world.plan(
        "readback",
        &plan_of(
            "readback",
            vec![agent("first", &[]), agent("second", &["first"])],
        ),
    );
    // Keyed by node id and never by the record's own id, so a second record for a
    // node is a changed value here rather than a key nothing compares against.
    let records = |world: &World| -> BTreeMap<String, (String, Vec<Value>)> {
        let tasks = world.store_tasks(&project);
        let held: BTreeMap<String, (String, Vec<Value>)> = tasks
            .iter()
            .map(|task| {
                let id = task["id"]
                    .as_str()
                    .expect("a task has a qualified id")
                    .to_owned();
                let deps = world.store_deps(&id);
                (
                    task["item"]["metadata"]["onepipeline.id"]
                        .as_str()
                        .expect("a task of this plan names its node")
                        .to_owned(),
                    (id, deps),
                )
            })
            .collect();
        assert_eq!(
            held.len(),
            tasks.len(),
            "the plan holds more than one record for a node of it, so a projection wrote \
             beside the operator's records rather than onto them: {tasks:?}"
        );
        held
    };
    let authored = records(&world);
    assert_eq!(
        authored.get("second").map(|(_, deps)| deps.len()),
        Some(1),
        "the fixture authored no dependency edge, so this journey could not tell a \
         preserved one from a rewritten one: {authored:?}"
    );

    for run in ["readback", "readback-2"] {
        world.run(&["start", &project, "--attach"]).exited(0);
        world.until_store(
            "the settlement to reach the plan it was launched from",
            |world| {
                world
                    .store_tasks(&project)
                    .iter()
                    .filter(|task| task["item"]["metadata"]["onepipeline.settlement"].is_object())
                    .count()
                    == 2
            },
        );
        assert_eq!(
            world.run_json(run, "result.json")["state"],
            "complete",
            "the run this readback is about did not settle"
        );

        assert_eq!(
            records(&world),
            authored,
            "settlement rewrote the plan's own records or the edges between them after {run}"
        );
        // Through the engine's own loader, which is how every command that reads a
        // plan reaches one and is not what the store queries above go through.
        world.run(&["plan", "check", &project]).exited(0);
    }
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
/// it off the reconcile loop, and catches the board up on the next change to the graph once
/// the store is back.
///
/// What it does *not* do is ask again on a timer, and that is the store's call rather than
/// this crate's: `onetaskgraph` classes a `local-md` source whose root has gone as a `config`
/// failure, which is `refused`. So the store's return alone attempts nothing, and a terminal
/// projection it refused is not attempted again inside closeout.
#[test]
fn an_unreachable_store_is_reported_and_attempted_again_on_the_next_change_while_the_run_completes_unaffected(
) {
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
                "version": 2,
                "commands": [{"op": "note", "id": "later", "addressee": "worker",
                              "text": "retry this projection", "deliver": "next"}]
            })
            .to_string(),
        )
        .exited(0);
    world.until("write-back refusal to be reported", |world| {
        std::fs::read_to_string(world.run_file("writeback-retry", "driver.log")).is_ok_and(|log| {
            log.contains("onetaskgraph write-back failed") && log.contains("the store refused it")
        })
    });
    renamed(&unavailable, &world.store(), "the store returns");
    noted(
        &world,
        "writeback-retry",
        "later",
        "project this now the store is back",
    );
    world.until("write-back recovery to be reported", |world| {
        std::fs::read_to_string(world.run_file("writeback-retry", "driver.log"))
            .is_ok_and(|log| log.contains("onetaskgraph write-back recovered"))
    });
    world.until("the next edit to reach the store", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"]
                    == "project this now the store is back"
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

    // The terminal projection was refused, so closeout has nothing to wait on and nothing
    // attempts it again: the run settles while the store is still gone, and a settled run's
    // graph does not change again, so the board stays behind what the run recorded.
    world.until("the run to write its result", |world| {
        world.run_file("writeback-retry", "result.json").is_file()
    });
    assert!(
        !world.store().exists(),
        "the run only settled after the store became reachable"
    );
    renamed(
        &unavailable,
        &world.store(),
        "the store returns after settlement",
    );
    let log = std::fs::read_to_string(world.run_file("writeback-retry", "driver.log"))
        .expect("the driver log is readable");
    assert_eq!(
        log.matches("onetaskgraph write-back recovered").count(),
        1,
        "a refused terminal projection was attempted again at closeout:\n{log}"
    );
    assert!(
        world
            .store_tasks(&project)
            .iter()
            .any(|task| task["item"]["status"]["category"] != "done"),
        "the refused terminal settlement reached the store"
    );
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

/// A copy the destination could not take happens after the destination's task list was read
/// successfully. The write-back worker reports that failure — a source that could not be
/// reached, which a wait can change — retries it, and publishes the snapshot when the store
/// accepts the next copy.
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
    world.store_refuses_once(
        "write_task",
        &json!({"kind": "unavailable", "message": "the destination refused this copy once"}),
    );
    let world = world.through_scripted_source();

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

// The failures a hosted destination answers with and an offline store cannot be made to have,
// each in the store's own `SourceError` shape: what `scripted-source` answers the one call a
// journey scripts it to fail, while every other call is the real `local-md` store's own.

/// A source that declined the request: what `github-projects` answers for a status it has
/// disabled.
fn source_refused() -> Value {
    json!({"kind": "refused", "message": "status unknown is disabled for source plans"})
}

/// A source whose configuration it will not run on: what `local-md` answers for a root that
/// has gone.
fn source_misconfigured() -> Value {
    json!({"kind": "config", "message": "source plans: cannot canonicalize root plan-store: \
           No such file or directory (os error 2)"})
}

/// A source that rate-limited the request, in GitHub's own words.
fn rate_limited() -> Value {
    json!({"kind": "rate-limited", "retry_after_seconds": 60, "message": RATE_LIMITED})
}

/// A source that could not be reached at all.
fn source_unreachable() -> Value {
    json!({"kind": "unavailable", "message": UNREACHABLE})
}

/// What an unreachable source says, carried through to the line an operator reads.
const UNREACHABLE: &str = "connection reset by the destination";

/// Give a run something new to project: a note for one node, which changes the graph the
/// write-back worker is handed.
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

/// The one line the driver printed about a failing projection, which has to be the only one.
fn the_line_reported(world: &World, run: &str) -> String {
    let log = std::fs::read_to_string(world.run_file(run, "driver.log"))
        .expect("the driver log is readable");
    let said: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("onetaskgraph write-back failed"))
        .collect();
    assert_eq!(
        said.len(),
        1,
        "one failing projection was reported {} times:\n{log}",
        said.len()
    );
    said[0].to_owned()
}

/// The surfaces that told the planner a projection failed, by their text.
fn surfaces_raised(world: &World, run: &str) -> Vec<String> {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter_map(|event| event["payload"]["message"].as_str().map(str::to_owned))
        .filter(|said| said.contains("did not take this run's projection"))
        .collect()
}

/// How long a refused projection is watched for another attempt. The schedule every failure
/// used to get asks again a quarter of a second after it, then a second after that, then four,
/// so across this window it would have asked three more times.
const REFUSAL_WINDOW: Duration = Duration::from_secs(8);

/// Nothing asks the store again for `run` across `window`, read off the `project show` every
/// attempt opens with. `midway` runs halfway through, for a journey that puts the store right
/// while it watches.
fn asked_nothing_more(world: &World, run: &str, window: Duration, midway: impl FnOnce()) {
    let asked = projections_asked_for(world);
    let watched = Instant::now();
    let mut midway = Some(midway);
    while watched.elapsed() < window {
        if watched.elapsed() >= window / 2 {
            if let Some(midway) = midway.take() {
                midway();
            }
        }
        assert_eq!(
            projections_asked_for(world),
            asked,
            "the store was asked again {:?} after it refused {run}'s projection",
            watched.elapsed()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] what these five wait on
// is the schedule and its absence — a refused projection not being asked again across a window
// the old schedule retried in, a transient one being asked at intervals that grow, and a run
// settling over refusals — which cannot be observed in less time than the schedule takes. The
// edge they need is the crate under test: they drive the compiled `onepipeline` binary against
// its own write-back worker and the real store, four of them through the store double, exactly
// as the six schedule journeys below do, so a project of their own would declare the same
// dependency and skip nothing. The reason those six record for staying in this binary is these
// five's too.
/// A projection the store **refuses** is reported once and is not asked again on a timer: the
/// store would refuse the same projection the same way, and against a hosted destination every
/// attempt spends its allowance for nothing. It is attempted again when the run's graph next
/// changes, and lands then once what the store refused has been put right — and not when the
/// store is put right alone, which nothing here polls.
#[test]
fn a_projection_the_store_refuses_is_reported_once_and_attempted_again_when_the_graph_changes() {
    let run = "writeback-refused";
    let (world, project) =
        a_run_whose_destination_can_start_refusing("store-writeback-refused", run);

    world.script("store.get_project.absent", "");
    noted(
        &world,
        run,
        "later",
        "project this onto a board that is not there",
    );
    world.until("the refusal to be reported", |world| {
        streaks_reported(world, run) >= 1
    });

    let said = the_line_reported(&world, run);
    for expected in [
        project.as_str(),
        "it is not in the configured sources",
        "class: refused, kind: no-such-item",
        "attempted again when the run's graph next changes",
    ] {
        assert!(
            said.contains(expected),
            "the line an operator reads does not say `{expected}`: {said}"
        );
    }
    assert!(
        !said.contains("retrying"),
        "the line an operator reads says a refused projection is being retried: {said}"
    );

    asked_nothing_more(&world, run, REFUSAL_WINDOW, || stops_refusing(&world));

    let raised = surfaces_raised(&world, run);
    assert_eq!(
        raised.len(),
        1,
        "the planner heard {} times about one refusal: {raised:?}",
        raised.len()
    );
    assert!(
        raised[0]
            .lines()
            .any(|line| line == "class: refused, kind: no-such-item"),
        "the surface does not carry the store's class and kind: {}",
        raised[0]
    );
    assert!(
        raised[0].contains("attempted again when the run's graph next changes")
            && !raised[0].contains("retrying"),
        "the surface does not say when the projection is attempted again: {}",
        raised[0]
    );

    noted(
        &world,
        run,
        "later",
        "project this onto the board that came back",
    );
    world.until("the projection to land once the graph changed", |world| {
        std::fs::read_to_string(world.run_file(run, "driver.log"))
            .is_ok_and(|log| log.contains("onetaskgraph write-back recovered"))
    });
    world.until_store("the changed graph to reach the board", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"]
                    == "project this onto the board that came back"
        })
    });
    assert_eq!(
        streaks_reported(&world, run),
        1,
        "the attempt that landed was reported as a failure"
    );

    world.release("work.go");
    world.until("the run to write its result", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(
        world.run_json(run, "result.json")["state"],
        "complete",
        "the refusal changed how the run settled"
    );
    assert_eq!(
        dispatched(&world, run),
        ["work", "later"],
        "the refusal changed what executed"
    );
}

/// A member attempt is refused whichever of the calls it makes the store refuses: the read of
/// a member the copy names — refused, or answered with nothing because the item is gone — the
/// copy, or the project read answered as a partial response every source of which refused.
/// Each is reported once under the store's own kind, and none is asked again on a timer. The
/// graph change each scenario makes is a member projection, which reads a named member rather
/// than a page of the project's tasks; a whole projection's page of tasks refused
/// is `writeback_projections::a_projection_after_a_failed_attempt_is_whole`'s.
#[test]
fn a_refusal_of_the_member_read_the_copy_or_the_project_read_stops_the_retry_timer() {
    for (scenario, method, error, kind) in [
        (
            "task-show",
            "get_task",
            Some(source_refused()),
            "class: refused, kind: refused",
        ),
        // The member's own item is not there any more: the read answers nothing, with no
        // failure beside it, which is the store's own reading of an item that is gone.
        (
            "member-gone",
            "get_task",
            None,
            "class: refused, kind: no-such-item",
        ),
        (
            "project-copy",
            "write_task",
            Some(source_refused()),
            "class: refused, kind: refused",
        ),
        (
            "partial",
            "get_project",
            Some(source_misconfigured()),
            "class: refused, kind: config",
        ),
    ] {
        let run = format!("writeback-refused-{scenario}");
        let (world, _project) = a_run_whose_destination_can_start_refusing(
            &format!("store-writeback-refused-{scenario}"),
            &run,
        );
        match &error {
            Some(error) => world.store_refuses(method, error),
            None => world.script(&format!("store.{method}.absent"), ""),
        }
        noted(&world, &run, "later", &format!("refuse this at {scenario}"));
        world.until(&format!("the {scenario} refusal to be reported"), |world| {
            streaks_reported(world, &run) >= 1
        });

        let line = the_line_reported(&world, &run);
        assert!(
            line.contains(kind) && line.contains("the store refused it"),
            "{scenario}: the line does not report the store's refusal as `{kind}`: {line}"
        );
        // The old schedule asked again a quarter of a second in, and a second after that.
        asked_nothing_more(&world, &run, REFUSAL_WINDOW / 2, || {});
        assert_eq!(
            surfaces_raised(&world, &run).len(),
            1,
            "{scenario}: the planner was not told exactly once"
        );
        assert_eq!(
            dispatched(&world, &run),
            ["work"],
            "{scenario}: the refusal changed what executed"
        );
    }
}

/// A failure the store does not refuse keeps today's schedule exactly: a rate limit, and a
/// source that could not be reached — each a failure the store classes `transient`, arrived
/// as its own typed error at the source's own boundary. The first retry stays prompt, the
/// interval grows, and the one line and one surface say it is being retried.
#[test]
fn a_failure_the_store_does_not_refuse_is_retried_on_the_schedule() {
    for (scenario, error, words, kind) in [
        (
            "rate-limited",
            rate_limited(),
            RATE_LIMITED,
            "class: transient, kind: rate-limited",
        ),
        (
            "unreachable",
            source_unreachable(),
            UNREACHABLE,
            "class: transient, kind: unavailable",
        ),
    ] {
        let run = format!("writeback-{scenario}");
        let (world, _project) = a_run_whose_destination_can_start_refusing(
            &format!("store-writeback-{scenario}"),
            &run,
        );

        let outage = starts_refusing_with(&world, &run, &format!("retry this {scenario}"), &error);
        let waited = retry_intervals(&world, &run, &outage, 3, Duration::from_secs(60));
        outage.replied();

        assert!(
            waited[0] < Duration::from_secs(1),
            "{scenario}: the first retry came {:?} after the failure",
            waited[0]
        );
        for pair in waited.windows(2) {
            assert!(
                pair[1] >= pair[0] * 2,
                "{scenario}: the schedule did not grow: {waited:?}"
            );
        }
        let line = the_line_reported(&world, &run);
        assert!(
            line.contains("retrying") && line.contains(words),
            "{scenario}: the line does not report a retried failure: {line}"
        );
        assert!(
            line.contains(kind),
            "{scenario}: the line does not carry the store's `{kind}`: {line}"
        );
        assert_eq!(
            surfaces_raised(&world, &run).len(),
            1,
            "{scenario}: one streak raised other than one surface"
        );
    }
}

/// Closeout attempts a terminal snapshot published after a refusal, because it is a different
/// snapshot, and never asks the store about a refused one again; and a run whose projections
/// the store keeps refusing settles without its closeout waiting on any of them.
#[test]
fn closeout_attempts_what_changed_after_a_refusal_and_never_a_refused_snapshot_again() {
    let run = "writeback-refused-then-put-right";
    let (world, project) =
        a_run_whose_destination_can_start_refusing("store-writeback-refused-then-put-right", run);
    world.script("store.get_project.absent", "");
    noted(&world, run, "later", "refused before the run ends");
    world.until("the refusal to be reported", |world| {
        streaks_reported(world, run) >= 1
    });
    stops_refusing(&world);
    world.release("work.go");
    world.until("the run to write its result", |world| {
        world.run_file(run, "result.json").is_file()
    });
    world.until_store("the terminal settlement to reach the board", |world| {
        world.store_tasks(&project).iter().all(|task| {
            task["item"]["status"]["category"] == "done"
                && task["item"]["metadata"]["onepipeline.settlement"].is_object()
        })
    });
    assert_eq!(
        world.run_json(run, "result.json")["state"],
        "complete",
        "the run did not settle complete"
    );

    let run = "writeback-refused-to-the-end";
    let (world, _project) =
        a_run_whose_destination_can_start_refusing("store-writeback-refused-to-the-end", run);
    let asked_before = projections_asked_for(&world);
    world.script("store.get_project.absent", "");
    noted(&world, run, "later", "refused until the run ends");
    world.until("the refusal to be reported", |world| {
        streaks_reported(world, run) >= 1
    });
    let released = Instant::now();
    world.release("work.go");
    world.until("the run to write its result", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert!(
        released.elapsed() < Duration::from_secs(15),
        "a run whose projections were refused took {:?} to settle",
        released.elapsed()
    );
    let result = world.run_json(run, "result.json");
    assert_eq!(result["state"], "complete", "{result}");
    assert_eq!(
        dispatched(&world, run),
        ["work", "later"],
        "the refusals changed what executed"
    );
    // Every attempt since the store began refusing was a snapshot of its own, reported once:
    // one asked again — on a timer, or inside closeout, where the old schedule asked as fast as
    // the store refused — is an attempt with no line of its own.
    let attempts = projections_asked_for(&world) - asked_before;
    let reported = streaks_reported(&world, run);
    assert!(
        attempts <= reported,
        "the store was asked {attempts} times for {reported} refused snapshots, so a refused \
         one was asked about again"
    );
}

/// The same refusal, answered by the real store with nothing in front of it: the project the
/// run projects onto is taken out of the store, so the project read every attempt opens with
/// answers nothing — which the store's own reading of an empty `show`, and so this engine's,
/// classes `refused` under `no-such-item`.
#[test]
fn a_projection_the_real_store_refuses_is_not_asked_again_until_the_graph_changes() {
    let run = "writeback-refused-real";
    let world = World::new("store-writeback-refused-real");
    world.script("work.wait", "hold");
    let project = world.plan(
        run,
        &plan_of(run, vec![agent("work", &[]), agent("later", &["work"])]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until_store("the running state to reach the store", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "work"
                && task["item"]["status"]["category"] == "in-progress"
        })
    });

    // llmlint: ignore-block[tests_mirror_real_usage] a `local-md` source *is* its folder of
    // Markdown — the file is the interface an operator authors and removes a project through,
    // and `onetaskgraph` has no verb that deletes or creates one (`project` offers list, show,
    // deps and copy). This suite authors every project the same way (`World::plan` writes the
    // file) and takes stores away the same way; the refusal that results is the real binary's.
    let board = world
        .store()
        .join("projects")
        .join(format!("{}.md", project_id(run)));
    let aside = world.root.join("refused-board.md");
    renamed(
        &board,
        &aside,
        "the destination project is taken out of the store",
    );
    // llmlint: ignore-end[tests_mirror_real_usage]
    noted(
        &world,
        run,
        "later",
        "project this onto a board that is not there",
    );
    world.until("the refusal to be reported", |world| {
        streaks_reported(world, run) >= 1
    });

    let said = the_line_reported(&world, run);
    for expected in [
        project.as_str(),
        "class: refused, kind: no-such-item",
        "attempted again when the run's graph next changes",
    ] {
        assert!(
            said.contains(expected),
            "the line an operator reads does not say `{expected}`: {said}"
        );
    }
    assert!(
        !said.contains("retrying"),
        "the line an operator reads says a refused projection is being retried: {said}"
    );

    // Read where an operator reads it: the driver reports every refused attempt with a line of
    // its own and every projection that lands as a recovery, so a store asked again inside the
    // window — on a timer, or because it was put right halfway through — is a second failure
    // line, or a recovery before the graph has changed.
    let watched = Instant::now();
    let mut put_back = false;
    while watched.elapsed() < REFUSAL_WINDOW {
        if !put_back && watched.elapsed() >= REFUSAL_WINDOW / 2 {
            // llmlint: ignore[tests_mirror_real_usage] putting the project back is the same
            // authoring of a `local-md` source's Markdown the block above records the reason for.
            renamed(&aside, &board, "the destination project is put back");
            put_back = true;
        }
        let log = std::fs::read_to_string(world.run_file(run, "driver.log"))
            .expect("the driver log is readable");
        assert!(
            log.matches("onetaskgraph write-back failed").count() == 1
                && !log.contains("onetaskgraph write-back recovered"),
            "the store was asked again {:?} after it refused the projection:\n{log}",
            watched.elapsed()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(put_back, "the destination project was never put back");

    let raised = surfaces_raised(&world, run);
    assert_eq!(
        raised.len(),
        1,
        "the planner heard {} times about one refusal: {raised:?}",
        raised.len()
    );
    assert!(
        raised[0]
            .lines()
            .any(|line| line == "class: refused, kind: no-such-item"),
        "the surface does not carry the store's class and kind: {}",
        raised[0]
    );

    noted(
        &world,
        run,
        "later",
        "project this onto the board that came back",
    );
    world.until("the projection to land once the graph changed", |world| {
        std::fs::read_to_string(world.run_file(run, "driver.log"))
            .is_ok_and(|log| log.contains("onetaskgraph write-back recovered"))
    });
    world.until_store("the changed graph to reach the board", |world| {
        world.store_tasks(&project).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "later"
                && task["item"]["metadata"]["onepipeline.context"]
                    == "project this onto the board that came back"
        })
    });

    world.release("work.go");
    world.until("the run to write its result", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(
        world.run_json(run, "result.json")["state"],
        "complete",
        "the refusal changed how the run settled"
    );
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// What a hosted destination says when it is refusing for a rate limit, and what this
/// suite's scripted source is made to say. The words are GitHub's own: that limiter is the
/// one the retry schedule below exists for, and the driver's line has to carry the
/// destination's reason through unchanged for an operator to know which refusal it is.
const RATE_LIMITED: &str = "You have exceeded a secondary rate limit";

/// A detached run whose store is reached through `scripted-source`, in front of the real
/// `local-md` store.
///
/// The source answers every call from the real store until [`starts_refusing`] makes it
/// refuse, which is how a destination that begins rate-limiting part-way through a run is
/// arranged — and, because every call the run makes is one this source records, how the
/// intervals the worker actually waits become observable at the destination rather than in
/// the shape of the code.
fn a_run_whose_destination_can_start_refusing(world: &str, run: &str) -> (World, String) {
    let world = World::new(world);
    world.script("work.wait", "hold");
    let project = world.plan(
        run,
        &plan_of(run, vec![agent("work", &[]), agent("later", &["work"])]),
    );
    let world = world
        .through_scripted_source()
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

/// How many attempts have asked the store: the project read every attempt opens with.
fn projections_asked_for(world: &World) -> usize {
    world.store_asked("get_project")
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
    starts_refusing_with(world, run, note, &rate_limited())
}

/// The same, refusing with `error`.
fn starts_refusing_with(world: &World, run: &str, note: &str, error: &Value) -> Outage {
    let streaks_before = streaks_reported(world, run);
    world.store_refuses("get_project", error);
    let mut reply = world
        .cmd(&["reply", run])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the reply starts");
    let envelope = json!({
        "version": 2,
        "commands": [{"op": "note", "id": "later", "addressee": "worker",
                      "text": note, "deliver": "next"}]
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

/// The destination answers again: whatever [`starts_refusing`] or an absent board scripted
/// on the project read every attempt opens with is taken away.
fn stops_refusing(world: &World) {
    let mut stopped = false;
    for script in ["store.get_project.refuse", "store.get_project.absent"] {
        if world.fakes.join(script).is_file() {
            world.unscript(script);
            stopped = true;
        }
    }
    assert!(stopped, "the destination was not refusing");
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
    // Each end of an interval is read when a poll notices it rather than when it happened, so
    // a poll that ran late at the start shortens the reading by that lateness: a worker that
    // waited out the full minute has been read as 59.9997s (CI run 34864754127). The slack
    // is a second — far inside the thirty-second floor above — so a worker asking early by
    // any amount an operator would notice still fails here.
    const OBSERVED_WITHIN: Duration = Duration::from_secs(1);
    for waited_at_the_ceiling in ceiling {
        assert!(
            *waited_at_the_ceiling >= promised - OBSERVED_WITHIN
                && *waited_at_the_ceiling <= promised.mul_f64(1.5),
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
    // The nodes settling is not the run settling: the driver writes its result
    // after them, so the document this reads is written strictly later than the
    // wait above returned. Timed before this, so what the interval bound above
    // measures is unchanged.
    world.until("the run to write its result", |world| {
        world.run_file(run, "result.json").is_file()
    });
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

/// Losing the worker's own shadow store is handled by the same best-effort boundary as
/// losing the destination: the committed graph keeps running, and the projection catches up
/// after the filesystem recovers.
///
/// The shadow store is the folder under the run the worker writes a snapshot into before the
/// store copies it onto the board; a file standing where its projects folder belongs is a
/// path no platform will write a document beneath, and a failure no store classed, so it is
/// retried on the schedule.
#[test]
fn an_unwritable_shadow_store_is_reported_retried_and_recovered() {
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

    // llmlint: ignore-block[tests_mirror_real_usage] a shadow store the worker cannot write is
    // a state a host produces on its own — a full disk, a permission change — and not one any
    // verb of this CLI can be asked to make; the journey's subject is what the run does when
    // its own local writes fail, and every claim afterwards is read off the CLI and the board.
    // The folder every attempt, whole or members, writes the shadow project into.
    let shadow = world
        .run_file("writeback-capture-retry", "writeback")
        .join("projects");
    std::fs::remove_dir_all(&shadow).expect("the shadow projects folder is taken away");
    std::fs::write(&shadow, "not a folder").expect("a file makes the shadow store unwritable");
    // llmlint: ignore-end[tests_mirror_real_usage]
    world
        .run_with_stdin(
            &["reply", "writeback-capture-retry"],
            &json!({
                "version": 2,
                "commands": [{"op": "note", "id": "later", "addressee": "worker",
                              "text": "capture recovered", "deliver": "next"}]
            })
            .to_string(),
        )
        .exited(0);
    world.until("the shadow store failure to be reported", |world| {
        std::fs::read_to_string(world.run_file("writeback-capture-retry", "driver.log")).is_ok_and(
            |log| log.contains("onetaskgraph write-back failed") && log.contains("retrying"),
        )
    });

    std::fs::remove_file(&shadow).expect("the shadow store becomes writable again");
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

/// A snapshot the store refused while it was unavailable never reaches the board once it
/// returns, when the graph has since gone back to what the board already holds.
///
/// The store classes a source whose root has gone as `refused`, so the two-edge snapshot is
/// not queued to be asked about again, and the revert publishes the graph last projected,
/// which leaves nothing to attempt. The next change to the graph — `spare` settling — is what
/// projects the graph as it then stands.
#[test]
fn a_reverted_edit_supersedes_the_refused_projection_before_store_recovery() {
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
                    "version": 2,
                    "commands": [{"op": "reparent", "id": "later", "deps": deps}]
                })
                .to_string(),
            )
            .exited(0)
            .out_has("\"applied\"");
    };
    reparent(&["work", "spare"]);
    world.until("the changed projection to be refused", |world| {
        std::fs::read_to_string(world.run_file("writeback-reverted-edit", "driver.log")).is_ok_and(
            |log| {
                log.contains("onetaskgraph write-back failed")
                    && log.contains("the store refused it")
            },
        )
    });
    reparent(&["work"]);
    world.until("both edits to be committed before recovery", |world| {
        world
            .events_of("writeback-reverted-edit", "edit-committed")
            .len()
            == 2
    });

    renamed(&unavailable, &world.store(), "the store recovers");
    world.release("spare.go");
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
        log.contains("onetaskgraph write-back failed") && log.contains("the store refused it"),
        "the run settled without reporting the store's refusal: {log}"
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

/// A project a run retried a node of launches again, reading the lineage head's
/// definition under the root's id — its dependents' edges included.
///
/// The one item a lineage has carries `onepipeline.node` and `onepipeline.supersedes`
/// beside its settlement — and where the plan's own store is the destination, so does the
/// plan's task. Those are the write-back's keys and no node field answers to them, so the
/// reader skips them the way it skips the settlement; before it did, every relaunch of a
/// project a run had written to was refused for a field named `node` nobody authored. A
/// dependent of the retried node keeps its edge onto the lineage's one item, since that is
/// the item its dependency's root holds, and relaunches depending on the root's id.
#[test]
fn a_retried_project_launches_again_reading_the_lineage_head_under_the_roots_id() {
    let first = World::new("store-writeback-relaunch-retried-first");
    first.script("work.fail", "1");
    let run = "writeback-retried";
    let project = first.plan(
        run,
        &plan_of(
            run,
            vec![
                crate::harness::agent("work", &[]),
                crate::harness::agent("ship", &["work"]),
            ],
        ),
    );
    let item_of = |world: &World, node: &str| -> Value {
        world
            .store_tasks(&project)
            .into_iter()
            .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
            .unwrap_or_else(|| panic!("the board holds no item for {node}"))
    };
    let edge_onto = |world: &World, dependent: &str| -> Vec<String> {
        world
            .store_deps(
                item_of(world, dependent)["id"]
                    .as_str()
                    .expect("an item id"),
            )
            .iter()
            .map(|edge| {
                edge["to"]["id"]
                    .as_str()
                    .expect("an edge target")
                    .to_owned()
            })
            .collect()
    };
    let held_before = |world: &World| -> (String, Vec<String>) {
        (
            item_of(world, "work")["id"]
                .as_str()
                .expect("an item id")
                .to_owned(),
            edge_onto(world, "ship"),
        )
    };
    let (work_item, ship_edges) = held_before(&first);
    assert_eq!(
        ship_edges,
        vec![work_item.clone()],
        "the fixture authored no edge"
    );
    first.run(&["start", &project, "--attach"]).settled();
    first
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "retry", "id": "work", "node": {
                "id": "work-2", "persona": "engineer", "task": "## What\nRedo it."
            }}]})
            .to_string(),
        )
        .exited(0);
    first.run(&["adopt", run]).settled();
    first.until(
        "the lineage's head and its dependent to reach the project",
        |world| {
            let tasks = world.store_tasks(&project);
            let settled = |node: &str, head: &str| {
                tasks.iter().any(|task| {
                    task["item"]["metadata"]["onepipeline.id"] == node
                        && task["item"]["metadata"]["onepipeline.node"] == head
                        && task["item"]["metadata"]["onepipeline.settlement"]["status"] == "done"
                })
            };
            settled("work", "work-2") && settled("ship", "ship")
        },
    );
    let tasks = first.store_tasks(&project);
    assert_eq!(
        tasks.len(),
        2,
        "the lineage holds other than one item beside its dependent: {tasks:?}"
    );
    let work = item_of(&first, "work");
    assert_eq!(
        work["id"],
        json!(work_item),
        "the retry moved the lineage's item"
    );
    assert_eq!(
        work["item"]["metadata"]["onepipeline.supersedes"],
        json!(["work"]),
        "{tasks:?}"
    );
    assert_eq!(
        edge_onto(&first, "ship"),
        vec![work_item],
        "the dependent's edge no longer names the lineage's one item"
    );
    assert_eq!(
        item_of(&first, "ship")["item"]["metadata"]["onepipeline.settlement"]["status"],
        "done",
        "the dependent did not run behind the replacement: {tasks:?}"
    );

    let second = World::new("store-writeback-relaunch-retried-second").with_env(
        "ONETASKGRAPH_SOURCES__PLANS__CONFIG__ROOT",
        &first.store().to_string_lossy(),
    );
    second.run(&["start", &project, "--attach"]).settled();
    assert_eq!(second.run_json(run, "result.json")["state"], "complete");
    // What it launched is the head's definition under the root's id: the retried node,
    // named as the plan authored it and titled by that name, carrying the task the retry
    // stated, and its dependent still depending on it by that name.
    let launched = second.run_json(run, "checkpoint.json")["state"]["graph"]["nodes"].clone();
    assert_eq!(
        launched,
        json!([
            {"id": "work", "title": "work", "task": "## What\nRedo it.", "persona": "engineer"},
            {"id": "ship", "title": "ship", "task": "## What\nDo ship.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- ship is done.", "persona": "engineer", "deps": ["work"]},
        ]),
        "{launched}"
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
                "draft": true,
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
/// so the linked store really does hand back continuation tokens — three pages of
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
///
/// The `cancelled` word is a **drop**'s. A node a `retry` superseded settles `cancelled`
/// too, but has no item of its own since entry 80 of `docs/contract-divergences.md`: its
/// lineage's one item reads the replacement's word under the superseded node's id, which
/// is held here beside the five.
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
    // Held open, so a planner can supersede one and drop the other while each is still
    // the run's to act on.
    world.script("superseded.wait", "hold");
    world.script("dropped.wait", "hold");

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
                {"id": "dropped", "persona": "engineer", "task": "## What\nBe dropped."},
                // The planner's own idle, declared rather than raced for: a node nothing
                // dispatches, which is not the same fact as a node something stopped.
                {"id": "idled", "persona": "engineer", "task": "## What\nWait.", "parked": true},
            ],
        }),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the held nodes to be dispatched", |world| {
        let dispatched = world.events_of(name, "node-dispatched");
        ["superseded", "dropped"].iter().all(|node| {
            dispatched
                .iter()
                .any(|event| event["labels"]["node"] == *node)
        })
    });

    // A drop takes a node out of the graph for good, and what became of it is a
    // **cancel**. That is the word a park must not share: a parked node is coming back,
    // and this one is not. A retry takes the superseded node out in the same edit that
    // puts its replacement in, and the replacement is written onto the item the superseded
    // node held.
    world
        .run_with_stdin(
            &["reply", name],
            &json!({
                "version": 2,
                "commands": [
                    {"op": "drop", "id": "dropped", "dependents": "detach"},
                    {"op": "retry", "id": "superseded", "node": {
                        "id": "replacement", "persona": "engineer", "task": "## What\nRetry it."
                    }},
                ],
            })
            .to_string(),
        )
        .exited(0);
    world.release("superseded.go");
    world.release("dropped.go");
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });

    let expected = BTreeMap::from([
        ("finished", "done"),
        ("broke", "failed"),
        ("abandoned", "provider-failed"),
        ("dropped", "cancelled"),
        ("idled", "parked"),
    ]);
    world.until_store("every settlement to reach the board", |world| {
        let projected = projected_words(world, &project);
        expected
            .iter()
            .all(|(node, word)| projected.get(*node).map(String::as_str) == Some(*word))
            && projected.get("superseded").map(String::as_str) == Some("done")
    });
    let projected = projected_words(&world, &project);
    // The superseded node's lineage has one item, at its own id, saying what its
    // replacement settled — and none the replacement's id names.
    let tasks = world.store_tasks(&project);
    let lineage: Vec<&Value> = tasks
        .iter()
        .filter(|task| task["item"]["metadata"]["onepipeline.id"] == "superseded")
        .collect();
    assert_eq!(lineage.len(), 1, "{tasks:?}");
    assert_eq!(
        lineage[0]["item"]["metadata"]["onepipeline.node"], "replacement",
        "{tasks:?}"
    );
    assert!(
        !tasks
            .iter()
            .any(|task| task["item"]["metadata"]["onepipeline.id"] == "replacement"),
        "the replacement reached the board as an item of its own: {tasks:?}"
    );

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
    // A dropped node is out of the graph, so the result names it nowhere; the journal is
    // where the run says what became of it.
    assert!(
        !result["nodes"]
            .as_array()
            .expect("result nodes")
            .iter()
            .any(|entry| entry["id"] == "dropped"),
        "{result}"
    );
    assert!(
        world
            .events_of(name, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "dropped"
                && event["payload"]["status"] == "cancelled"),
        "the drop settled its node under other than cancelled: {}",
        world.dump()
    );

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
    // The reason, on **one** line, carrying what the store said of the source that could not
    // answer: the whole message is five lines, one of them the reason, and the next the class
    // and kind the store gave it. A store's failure can run to several lines of its own, and
    // they are closed up onto that one rather than read as this surface's own advice. A source
    // whose root has gone is the store's `config` failure, which it classes `refused`.
    let reason = message
        .lines()
        .find_map(|line| line.strip_prefix("reason: "))
        .unwrap_or_else(|| panic!("the surface names no reason: {message}"));
    assert_eq!(
        message.lines().count(),
        5,
        "a multi-line refusal was carried onto the surface as it was spelled: {message}"
    );
    assert!(
        reason.contains("could not answer") && reason.contains("cannot canonicalize root"),
        "the surface dropped what the store said rather than carrying it on one line: {reason}"
    );
    assert!(
        message
            .lines()
            .any(|line| line == "class: refused, kind: config"),
        "the surface does not carry the store's class and kind: {message}"
    );
    assert!(
        message.contains("attempted again when the run's graph next changes")
            && !message.contains("retrying"),
        "the surface does not say a refused projection waits for the graph to change: {message}"
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
/// **The writer rests between replacements for as long as the last one took**, so the
/// document is whole for at least half of every cycle however slow the host makes the
/// truncate. A store replaces a document once per projection and moves on; a writer
/// that truncates again the instant it has written is harsher than any store, and on a
/// loaded runner it kept the document empty for longer than the rule is patient.
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
                let replacing = std::time::Instant::now();
                std::fs::write(&rewritten, "").expect("the truncate half of a replacement");
                std::fs::write(&rewritten, &whole).expect("the write half of a replacement");
                // Whole for at least as long as it was not; see above.
                std::thread::sleep(replacing.elapsed());
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

/// A board a **stopped** run left mid-write is the product's state, and the next
/// fixture this world writes is not refused over it.
///
/// `stop` signals the whole tree, and nothing in it handles the signal — so a `project
/// copy` ended between the truncate and the rewrite of one `fs::write` leaves the earlier
/// run's board empty, and no wait settles it. [`World::plan`] once asked the fixture rule
/// of every board in the store, and on the gate it read exactly that board while standing
/// up the journey's *second* run: 5.5 s of asking, then `has no front matter` against a
/// fixture that was sound. So the rule `plan` asks is of what it wrote, and the state is
/// induced here by hand, because a signal landing inside one syscall's window is not a
/// thing to wait for.
// llmlint: ignore-block[tests_mirror_real_usage] the subject is the fixture rule's scope,
// and the state that motivates it cannot be produced on demand: it is a signal landing
// between two halves of one write. The stop is driven through the real binary; the
// half-written board is what that stop leaves when it lands there.
#[test]
fn a_board_a_stopped_run_left_mid_write_is_not_the_next_fixtures_defect() {
    let world = World::new("store-fixture-rule-stopped");
    world.script("work.wait", "hold");
    let project = world.plan("first", &plan_of("first", vec![agent("work", &[])]));
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of("first", "node-dispatched").is_empty()
    });
    world.run(&["stop", "first"]).exited(0);
    let board = world.store().join("projects").join("first-board.md");
    std::fs::write(&board, "").expect("the half a killed copy leaves behind");

    // The next fixture stands up on the rule it is held to, and no other board's.
    world.plan("second", &plan_of("second", vec![agent("work", &[])]));

    // And the rule over the whole store still has its teeth, so a hand-authored store
    // holding that same document is still refused by name.
    let said = crate::harness::undiscriminating(&world.store())
        .unwrap_or_else(|| panic!("an empty project document was accepted as a fixture"));
    assert!(
        said.contains("fixture 'first-board'") && said.contains("has no front matter"),
        "the refusal does not name the empty board: {said}"
    );
    world.release("work.go");
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
