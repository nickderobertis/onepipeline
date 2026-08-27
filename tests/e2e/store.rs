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

use crate::harness::{double, onetaskgraph_binary, plan_of, World, REFUSED, STORE_BINARY_ENV};
use serde_json::{json, Value};

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
    assert_eq!(project, "plans:localmd", "the project id a person types");

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
    std::fs::rename(world.store(), &unavailable).expect("the store becomes unreachable");
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
    std::fs::rename(&unavailable, world.store()).expect("the store returns");
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
    std::fs::rename(world.store(), &unavailable).expect("the store becomes unreachable again");

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
    std::fs::rename(&unavailable, world.store()).expect("the store returns after settlement");
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

/// Losing the worker's own command-capture path is handled by the same best-effort
/// boundary as losing the destination: the committed graph keeps running, and the
/// projection catches up after the filesystem recovers.
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
    std::fs::rename(world.store(), &unavailable).expect("the store becomes unreachable");
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

    std::fs::rename(&unavailable, world.store()).expect("the store recovers");
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
    std::fs::rename(world.store(), &unavailable).expect("the store becomes unreachable");
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

#[test]
fn derived_waiting_failed_and_parked_states_use_their_board_categories() {
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
    world.until("every derived category to reach the store", |world| {
        let categories: std::collections::BTreeMap<String, String> = world
            .store_tasks(&project)
            .iter()
            .filter_map(|task| {
                Some((
                    task["item"]["metadata"]["onepipeline.id"]
                        .as_str()?
                        .to_owned(),
                    task["item"]["status"]["category"].as_str()?.to_owned(),
                ))
            })
            .collect();
        let tasks = world.store_tasks(&project);
        categories.get("fails").is_some_and(|value| value == "done")
            && categories
                .get("parked")
                .is_some_and(|value| value == "cancelled")
            && categories
                .get("skipped")
                .is_some_and(|value| value == "cancelled")
            && categories
                .get("approve")
                .is_some_and(|value| value == "todo")
            && categories
                .get("blocked")
                .is_some_and(|value| value == "todo")
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
                    && project["items"][0]["item"]["metadata"]["onepipeline.name"]
                        == "writeback-categories"
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
    let task = world.store().join("tasks/mapping/000-publish.md");
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
    let first = world.store().join("tasks/related/000-first.md");
    let held = std::fs::read_to_string(&first).expect("the first task");
    let held = held.replace(
        "project: related",
        "project: related\ndepends_on:\n  - id: 001-second\n    kind: related",
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
        world.runs.join("native-run").is_dir(),
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
