//! A plan spanning two sources: a home project in the plan's own `local-md` source and the
//! member project the store's routed copy files the routed tasks under in a second one.
//!
//! Every journey drives the compiled binary over the real `onetaskgraph` and two real folders
//! of Markdown. The plan source routes one repository to the second source, and where each task
//! lands is decided by the store's own routing — the journey authors the plan in a draft source
//! and copies it with the store's routed project copy, as an operator's tooling would.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// Both sources are the real `local-md` plugin of the real store the binary links, and the
// routing and the member project are that store's own. `harness.rs` carries the full rationale.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::harness::{global, plan_of, project_id, World, STORE_SOURCE};

/// The second source: where the plan source routes the engine repository's tasks.
const MEMBERS: &str = "members";
/// The source a journey authors its plan in before the routed copy files it.
const DRAFT: &str = "draft";
/// The repository the plan source routes to [`MEMBERS`].
const ROUTED: &str = "github.com/owner/engine";
/// The repository that stays in the plan source.
const HOME: &str = "github.com/owner/service";

fn root_of(world: &World, source: &str) -> PathBuf {
    world.root.join(format!("{source}-store"))
}

/// A world whose plan source routes [`ROUTED`] to [`MEMBERS`], with a draft source beside both.
fn a_routed_world(name: &str) -> World {
    let world = World::new(name);
    let mut configured = world;
    for source in [MEMBERS, DRAFT] {
        let root = root_of(&configured, source);
        let upper = source.to_uppercase();
        configured = configured
            .with_env(
                &format!("ONETASKGRAPH_SOURCES__{upper}__PLUGIN"),
                "local-md",
            )
            .with_env(
                &format!("ONETASKGRAPH_SOURCES__{upper}__CONFIG__ROOT"),
                &root.to_string_lossy(),
            );
    }
    let plans = STORE_SOURCE.to_uppercase();
    configured
        .with_env(
            &format!("ONETASKGRAPH_SOURCES__{plans}__ROUTES__0__TO"),
            MEMBERS,
        )
        .with_env(
            &format!("ONETASKGRAPH_SOURCES__{plans}__ROUTES__0__REPOSITORIES"),
            ROUTED,
        )
}

fn on(repo: &str, id: &str, deps: &[&str]) -> Value {
    json!({
        "id": id,
        "repo": repo,
        "persona": "engineer",
        "title": format!("feat: ship {id}"),
        "task": format!("## What\nShip {id}.\n\n## Why\nUsers need it.\n\n## Acceptance criteria\n- {id} is published."),
        "deps": deps,
    })
}

/// Author `plan` in the draft source and file it into the plan source with the store's routed
/// project copy, answering the home project's qualified id.
fn filed(world: &World, name: &str, plan: &Value) -> String {
    world.plan_in(&root_of(world, DRAFT), name, plan);
    // A `local-md` source is a folder that exists, so both destinations are made before the
    // copy builds them.
    for root in [world.store(), root_of(world, MEMBERS)] {
        std::fs::create_dir_all(root).expect("a source root");
    }
    let draft = global(&format!("{DRAFT}:{}", project_id(name)));
    let report = world.store_call(|engine| async move {
        engine
            .copy(&onetaskgraph_core::CopyRequest {
                items: onetaskgraph_core::CopyItems::new(vec![draft]).expect("one item"),
                scope: onetaskgraph_core::CopyScope::Projects { tasks: true },
                destination: onetaskgraph_plugin_api::SourceName::new(STORE_SOURCE)
                    .expect("a source name"),
                match_by: None,
                recreate: false,
                create: false,
                dry_run: false,
            })
            .await
            .unwrap_or_else(|error| panic!("the routed copy files the plan: {error}"))
    });
    let home = report
        .items
        .first()
        .and_then(|item| item.action.destination())
        .expect("the copy reports where the project landed");
    assert_eq!(
        home.source.as_str(),
        STORE_SOURCE,
        "the home is not in the plan source: {report:?}"
    );
    home.to_string()
}

/// The home's tasks and its members', each by the node id it carries, read with the store's
/// own members read.
fn plan_tasks(world: &World, home: &str) -> BTreeMap<String, Value> {
    let id = global(home);
    world.store_call(|engine| async move {
        let answer = engine
            .tasks(&onetaskgraph_core::TaskRequest {
                sources: Vec::new(),
                filters: onetaskgraph_core::Filters::default(),
                priorities: Vec::new(),
                metadata: Vec::new(),
                origin: None,
                project: onetaskgraph_core::ProjectSelector::Qualified(id.clone()),
                commented_since: None,
                include_members: true,
                paging: onetaskgraph_core::Paging {
                    limit: std::num::NonZeroU32::new(500).expect("not zero"),
                    token: None,
                },
            })
            .await
            .unwrap_or_else(|error| panic!("the store reads the plan {id}: {error}"));
        assert!(answer.errors.is_empty(), "{:?}", answer.errors);
        assert!(answer.next.is_none(), "one page holds this plan");
        answer
            .items
            .iter()
            .map(|task| {
                let task = serde_json::to_value(task).expect("a task renders");
                (
                    task["item"]["metadata"]["onepipeline.id"]
                        .as_str()
                        .expect("a plan task names its node")
                        .to_owned(),
                    task,
                )
            })
            .collect()
    })
}

/// Every task of `source`, whatever project holds it, by the node id it carries — the whole of
/// what that source holds of any plan.
fn source_nodes(world: &World, source: &str) -> BTreeMap<String, Value> {
    let source = onetaskgraph_plugin_api::SourceName::new(source).expect("a source name");
    world.store_call(|engine| async move {
        let answer = engine
            .tasks(&onetaskgraph_core::TaskRequest {
                sources: vec![source.clone()],
                filters: onetaskgraph_core::Filters::default(),
                priorities: Vec::new(),
                metadata: Vec::new(),
                origin: None,
                project: onetaskgraph_core::ProjectSelector::Any,
                commented_since: None,
                include_members: false,
                paging: onetaskgraph_core::Paging {
                    limit: std::num::NonZeroU32::new(500).expect("not zero"),
                    token: None,
                },
            })
            .await
            .unwrap_or_else(|error| panic!("the store reads {source}: {error}"));
        assert!(answer.errors.is_empty(), "{:?}", answer.errors);
        answer
            .items
            .iter()
            .filter_map(|task| {
                let task = serde_json::to_value(task).expect("a task renders");
                let node = task["item"]["metadata"]["onepipeline.id"]
                    .as_str()?
                    .to_owned();
                Some((node, task))
            })
            .collect()
    })
}

fn words(world: &World, home: &str) -> BTreeMap<String, String> {
    plan_tasks(world, home)
        .into_iter()
        .map(|(node, task)| {
            let word = task["item"]["status"]["name"]
                .as_str()
                .expect("a status word")
                .to_owned();
            (node, word)
        })
        .collect()
}

fn sources(world: &World, home: &str) -> BTreeMap<String, String> {
    plan_tasks(world, home)
        .into_iter()
        .map(|(node, task)| {
            let id = task["id"].as_str().expect("a qualified id");
            let (source, _) = id.split_once(':').expect("a qualified id");
            (node, source.to_owned())
        })
        .collect()
}

fn position(journal: &[Value], kind: &str, node: &str) -> usize {
    journal
        .iter()
        .position(|event| event["kind"] == kind && event["labels"]["node"] == node)
        .unwrap_or_else(|| panic!("the journal holds no {kind} for {node}"))
}

fn result_of(world: &World, run: &str) -> BTreeMap<String, String> {
    let result = world.run_json(run, "result.json");
    assert_eq!(result["run_id"], run, "{result}");
    result["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|node| {
            (
                node["id"].as_str().expect("an id").to_owned(),
                node["status"].as_str().expect("a status").to_owned(),
            )
        })
        .collect()
}

/// One launch of a home project reads its member's tasks beside its own and runs them as one
/// graph: one run, every node of both sources in it, each dispatched only once what it depends
/// on in either source has settled. Before the first dispatch every item not yet dispatched
/// reads `queued` in its own source, and once the run settles each reads its settled word
/// there — and neither source holds an item for a task of the other.
#[test]
fn a_plan_spanning_two_sources_runs_as_one_graph_and_settles_each_task_where_it_lives() {
    let world = a_routed_world("multi-source-run");
    let _service = world.repository("local-direct", &[]);
    let _engine = world.extra_repository("engine");
    for node in ["core", "adopt", "ship"] {
        world.script(&format!("{node}.work"), &format!("{node} wrote this\n"));
    }
    let name = "spanning";
    // `adopt` depends on a home task, and `ship` on the member task: an edge each way.
    let home = filed(
        &world,
        name,
        &plan_of(
            name,
            vec![
                on(HOME, "core", &[]),
                on(ROUTED, "adopt", &["core"]),
                on(HOME, "ship", &["adopt"]),
            ],
        ),
    );
    assert_eq!(
        sources(&world, &home),
        BTreeMap::from([
            ("core".to_owned(), STORE_SOURCE.to_owned()),
            ("adopt".to_owned(), MEMBERS.to_owned()),
            ("ship".to_owned(), STORE_SOURCE.to_owned()),
        ]),
        "the store did not route the plan as the journey needs"
    );

    let core = world.rendezvous("core");
    world.run(&["start", &home, "--detach"]).exited(0);
    let dispatched = core.arrived();
    assert_eq!(world.events_of(name, "node-dispatched").len(), 1);
    let board = words(&world, &home);
    for node in ["adopt", "ship"] {
        assert_eq!(
            board.get(node).map(String::as_str),
            Some("queued"),
            "{node} was not claimed in its own source before the first dispatch: {board:?}"
        );
    }
    dispatched.release();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });

    assert_eq!(
        result_of(&world, name),
        BTreeMap::from([
            ("core".to_owned(), "done".to_owned()),
            ("adopt".to_owned(), "done".to_owned()),
            ("ship".to_owned(), "done".to_owned()),
        ])
    );
    let journal = world.journal(name);
    assert!(
        position(&journal, "node-settled", "core") < position(&journal, "node-dispatched", "adopt"),
        "the member task was dispatched before the home task it depends on settled"
    );
    assert!(
        position(&journal, "node-settled", "adopt") < position(&journal, "node-dispatched", "ship"),
        "the home task was dispatched before the member task it depends on settled"
    );

    world.until_store("every item to read done where it lives", |world| {
        words(world, &home).values().all(|word| word == "done")
    });
    let tasks = plan_tasks(&world, &home);
    for (node, task) in &tasks {
        assert_eq!(
            task["item"]["metadata"]["onepipeline.settlement"]["status"], "done",
            "{node}'s item carries no settlement: {task}"
        );
    }
    let held = |source: &str| -> Vec<String> { source_nodes(&world, source).into_keys().collect() };
    assert_eq!(held(STORE_SOURCE), ["core", "ship"]);
    assert_eq!(held(MEMBERS), ["adopt"]);
}

/// A live `add` whose node names a repository the home's source routes elsewhere lands in that
/// source, in a member project the store's routed copy creates for it because the plan had none
/// there — and the added task's settlement is written there.
#[test]
fn a_live_add_routed_to_the_second_source_lands_in_a_member_project_created_for_it() {
    let world = a_routed_world("multi-source-add");
    let _service = world.repository("local-direct", &[]);
    let _engine = world.extra_repository("engine");
    for node in ["core", "adopt", "ship"] {
        world.script(&format!("{node}.work"), &format!("{node} wrote this\n"));
    }
    world.script("core.wait", "hold");
    let name = "growing";
    let home = filed(
        &world,
        name,
        &plan_of(
            name,
            vec![on(HOME, "core", &[]), on(HOME, "ship", &["core"])],
        ),
    );
    let read_home = |world: &World| world.store_project(&home)["items"][0]["item"].clone();
    assert!(
        read_home(&world)["metadata"]
            .get("onetaskgraph.members")
            .is_none(),
        "the plan had a member before the add: {}",
        read_home(&world)
    );

    world.run(&["start", &home, "--detach"]).exited(0);
    world.until("the held node to be dispatched", |world| {
        world.events_of(name, "node-dispatched").len() == 1
    });
    world
        .run_with_stdin(
            &["reply", name],
            &json!({"version": 2, "commands": [
                {"op": "add", "node": on(ROUTED, "adopt", &["core"])},
            ]})
            .to_string(),
        )
        .exited(0);
    world.until_store("the added task to land in the second source", |world| {
        words(world, &home).get("adopt").map(String::as_str) == Some("queued")
    });
    // A stop reads the landed baseline back in a process of its own. The added item is in a
    // source the launch read no task from, so only the home's member list, which now names the
    // member the store created, makes it the run's: read whole, it is released where it lives.
    world
        .run(&["stop", name])
        .exited(0)
        .err_lacks("write-back cannot read");
    assert_eq!(
        words(&world, &home).get("adopt").map(String::as_str),
        Some("todo"),
        "the stop did not release the added item in the member's source"
    );
    world.run(&["adopt", name, "--detach"]).exited(0);
    world.release("core.go");
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    assert_eq!(
        result_of(&world, name),
        BTreeMap::from([
            ("core".to_owned(), "done".to_owned()),
            ("ship".to_owned(), "done".to_owned()),
            ("adopt".to_owned(), "done".to_owned()),
        ])
    );

    world.until_store(
        "the added task's settlement to land where it lives",
        |world| words(world, &home).get("adopt").map(String::as_str) == Some("done"),
    );
    let member = read_home(&world)["metadata"]["onetaskgraph.members"].clone();
    let member = member
        .as_array()
        .and_then(|members| members.first())
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the home names no member: {}", read_home(&world)))
        .to_owned();
    assert!(member.starts_with(&format!("{MEMBERS}:")), "{member}");
    assert_eq!(
        world.store_project(&member)["items"][0]["item"]["metadata"]["onetaskgraph.member_of"],
        home.as_str()
    );
    let adopt = &source_nodes(&world, MEMBERS)["adopt"];
    assert_eq!(
        adopt["item"]["project"],
        member.split_once(':').expect("an id").1
    );
    assert_eq!(
        adopt["item"]["metadata"]["onepipeline.settlement"]["status"],
        "done"
    );
    assert!(
        !source_nodes(&world, STORE_SOURCE).contains_key("adopt"),
        "the home's source holds an item for the routed task"
    );
}

/// A stop releases every item of the plan the run never started, in whichever source it lives,
/// and an adopting driver — a process of its own, reading the run's landed baseline back with
/// items in both sources — claims them there again.
#[test]
fn a_stop_releases_and_an_adoption_reclaims_the_unstarted_items_of_both_sources() {
    let world = a_routed_world("multi-source-stop");
    let _service = world.repository("local-direct", &[]);
    let _engine = world.extra_repository("engine");
    world.script("core.wait", "hold");
    let name = "stopping";
    let home = filed(
        &world,
        name,
        &plan_of(
            name,
            vec![
                on(HOME, "core", &[]),
                on(ROUTED, "adopt", &["core"]),
                on(HOME, "ship", &["adopt"]),
            ],
        ),
    );
    world.run(&["start", &home, "--detach"]).exited(0);
    world.until_store("the run's claim to reach both sources", |world| {
        let board = words(world, &home);
        board.get("adopt").map(String::as_str) == Some("queued")
            && board.get("ship").map(String::as_str) == Some("queued")
    });

    // The stop reads the baseline back whole: an item in the member's source is one of this
    // run's, not a file it cannot read.
    world
        .run(&["stop", name])
        .exited(0)
        .err_lacks("write-back cannot read");
    let board = words(&world, &home);
    for node in ["adopt", "ship"] {
        assert_eq!(
            board.get(node).map(String::as_str),
            Some("todo"),
            "{node}, which the stopped run never started, is still claimed: {board:?}"
        );
    }

    world.run(&["adopt", name, "--detach"]).exited(0);
    world.until_store(
        "the adopted driver's claim to reach both sources",
        |world| {
            let board = words(world, &home);
            board.get("adopt").map(String::as_str) == Some("queued")
                && board.get("ship").map(String::as_str) == Some("queued")
        },
    );
    world.run(&["stop", name]).exited(0);
    let held = |source: &str| -> Vec<String> { source_nodes(&world, source).into_keys().collect() };
    assert_eq!(held(STORE_SOURCE), ["core", "ship"]);
    assert_eq!(held(MEMBERS), ["adopt"]);
}

fn records(world: &World, run: &str) -> Vec<Value> {
    std::fs::read_to_string(world.run_file(run, "writeback-projections.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("a record line is JSON"))
        .collect()
}

/// A route the plan's source declares is not a member of the plan until the store has created
/// one there. A landed baseline naming an item in a source the plan source routes to, but where
/// the home has no member, is refused when a driver reads it back, and each item is read by its
/// own id instead.
#[test]
fn a_baseline_item_in_a_routed_source_with_no_member_there_is_refused() {
    let world = a_routed_world("multi-source-unvouched");
    let _service = world.repository("local-direct", &[]);
    world.script("core.wait", "hold");
    let name = "unvouched";
    let home = filed(
        &world,
        name,
        &plan_of(
            name,
            vec![on(HOME, "core", &[]), on(HOME, "ship", &["core"])],
        ),
    );
    world.run(&["start", &home, "--detach"]).exited(0);
    world.until_store("the run's claim to reach the board", |world| {
        words(world, &home).get("ship").map(String::as_str) == Some("queued")
    });
    world.run(&["stop", name]).exited(0);
    assert!(
        world.store_project(&home)["items"][0]["item"]["metadata"]
            .get("onetaskgraph.members")
            .is_none(),
        "the plan has a member, so the routed source is not one it lacks"
    );

    let path = world.run_file(name, "writeback-landed.json");
    let stray = format!("{MEMBERS}:{}/001-ship", project_id(name));
    // llmlint: ignore-block[tests_mirror_real_usage] no invocation of this build writes a
    // baseline naming a source the run has no member in, which is the point: a file naming one
    // is what the read-back must refuse, and writing it is the only way to hand the driver one.
    let mut landed: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the baseline")).expect("JSON");
    landed["items"]["ship"]["destination"] = json!(stray);
    std::fs::write(&path, landed.to_string()).expect("the baseline is rewritten");
    // llmlint: ignore-end[tests_mirror_real_usage]

    let mark = records(&world, name).len();
    world.run(&["adopt", name, "--detach"]).exited(0);
    world.until("the adopted driver's first projection", |world| {
        records(world, name).len() > mark
    });
    let log = std::fs::read_to_string(world.run_file(name, "driver.log")).expect("the log");
    assert!(
        log.contains(&format!("cannot read {}", path.display()))
            && log.contains(&format!(
                "`destination` '{stray}' is not an item of this run's destination"
            )),
        "the driver took an item in a source with no member as the run's:\n{log}"
    );
    world.run(&["stop", name]).exited(0);
    assert!(
        source_nodes(&world, MEMBERS).is_empty(),
        "the routed source holds an item of a plan with no member there"
    );
}

/// A run an older build started holds no landed baseline, so an adopting driver reads each
/// lineage once by the id its task was read out of — a member task's in the member's own source,
/// which its task record names — and claims each item where it lives, creating nothing.
#[test]
fn an_adoption_with_no_baseline_reads_a_member_task_by_its_id_in_its_own_source() {
    let world = a_routed_world("multi-source-cold");
    let _service = world.repository("local-direct", &[]);
    let _engine = world.extra_repository("engine");
    world.script("core.wait", "hold");
    let name = "cold";
    let home = filed(
        &world,
        name,
        &plan_of(
            name,
            vec![
                on(HOME, "core", &[]),
                on(ROUTED, "adopt", &["core"]),
                on(HOME, "ship", &["adopt"]),
            ],
        ),
    );
    world.run(&["start", &home, "--detach"]).exited(0);
    world.until_store("the run's claim to reach both sources", |world| {
        let board = words(world, &home);
        board.get("adopt").map(String::as_str) == Some("queued")
            && board.get("ship").map(String::as_str) == Some("queued")
    });
    world.run(&["stop", name]).exited(0);
    assert_eq!(
        words(&world, &home).get("adopt").map(String::as_str),
        Some("todo")
    );
    let plan = world.run_json(name, "plan.json");
    let adopt = plan["tasks"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|node| node["id"] == "adopt")
        .expect("the member's node");
    assert_eq!(adopt["task_record"]["source"], MEMBERS, "{adopt}");

    // llmlint: ignore[tests_mirror_real_usage] a run directory an older build left holds no landed baseline, and no invocation of this build produces one without it: every launch seeds the file. Removing it is exactly that directory, as `writeback_projections` states it.
    std::fs::remove_file(world.run_file(name, "writeback-landed.json")).expect("a seeded baseline");
    let mark = records(&world, name).len();
    world.run(&["adopt", name, "--detach"]).exited(0);
    // The record is appended after the attempt ends its store command, so the board can show
    // the claim before the attempt's record exists: wait for both.
    world.until_store(
        "the adopted driver's claim to reach both sources, and its record",
        |world| {
            let board = words(world, &home);
            board.get("adopt").map(String::as_str) == Some("queued")
                && board.get("ship").map(String::as_str) == Some("queued")
                && records(world, name).len() > mark
        },
    );
    let first = records(&world, name)
        .get(mark)
        .cloned()
        .expect("the adopted driver's first attempt");
    assert_eq!(first["outcome"], "projected", "{first}");
    assert_eq!(first["calls"]["task-show"], 3, "{first}");
    assert!(
        first["calls"].get("project-copy").is_none(),
        "a lineage the run knew an item for was created again: {first}"
    );
    world.run(&["stop", name]).exited(0);
    let held = |source: &str| -> Vec<String> { source_nodes(&world, source).into_keys().collect() };
    assert_eq!(held(STORE_SOURCE), ["core", "ship"]);
    assert_eq!(held(MEMBERS), ["adopt"]);
}

/// A home whose `onetaskgraph.members` breaks the store's rule for the list — not a list, an
/// entry that is no qualified id, a member in the home's own source, two in one source — is a
/// plan this build cannot read: the launch refuses it naming the home and why, and mints no
/// run, rather than running the plan as though it had fewer members.
#[test]
fn a_home_whose_member_list_breaks_the_stores_rule_is_not_launched() {
    let world = a_routed_world("multi-source-malformed");
    let name = "malformed";
    let home = filed(&world, name, &plan_of(name, vec![on(HOME, "core", &[])]));
    let document = world
        .store()
        .join("projects")
        .join(format!("{}.md", project_id(name)));
    let authored = std::fs::read_to_string(&document).expect("the home's document");
    for (members, why) in [
        ("members:board", "is not a list"),
        ("[bare]", "which is not a qualified id"),
        ("[\"plans:other\"]", "a second project in plans"),
        (
            "[\"members:a\", \"members:b\"]",
            "a second project in members",
        ),
    ] {
        // llmlint: ignore-block[tests_mirror_real_usage] a `local-md` home *is* its Markdown
        // document, and a member list a person or another tool wrote wrongly is exactly these
        // bytes; the store's routed writes never produce one, so writing it is the only way to
        // hand a launch one.
        let edited = authored.replacen(
            "metadata:\n",
            &format!("metadata:\n  onetaskgraph.members: {members}\n"),
            1,
        );
        assert_ne!(edited, authored, "the home's document holds no metadata");
        std::fs::write(&document, edited).expect("the home's document is rewritten");
        // llmlint: ignore-end[tests_mirror_real_usage]
        world
            .run(&["start", &home, "--detach"])
            .exited(2)
            .err_has(&home)
            .err_has(why);
        assert!(
            !world.run_file(name, "launch.json").exists(),
            "a run was minted over the member list {members}"
        );
    }
}
