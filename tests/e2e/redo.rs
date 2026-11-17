//! `start --redo NODE`: a continuing launch dispatches named nodes again although their
//! lineages settled `done`, each on the branch its settlement records, and continues every
//! other lineage as it would without the flag — and `plan writeback-keys`, which names the
//! store keys that write-back owns.
//!
//! Each journey drives the compiled binary over a real `local-md` store, the real linked
//! `onevcs` over real git and a bare origin on disk, and reads what a redo left off the
//! origin, the store and the run's own views.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary over a real `local-md` store, and `onevcs` is the real library over real git and a
// real origin on disk. `oneagentgraph` is substituted at its subprocess boundary so a
// journey states a dispatch's work rather than paying for a model turn. `harness.rs`
// carries the same suppression and the full rationale.

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] each journey cuts real
// branches through the linked `onevcs` over real git, which is what a redo continuing a
// branch has to be proven against; what they exercise is `continuation`, the launch in
// `driver`, the `engine`'s pinned dispatch and the write-back together, which any change
// under `src/` can move, so they belong to this binary for the reason `mod publish_preserve`
// does.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::continuation::{
    changes_from, created_from, dispatched, held_as_draft, launched_over, sessions_opened, settled,
    stored_claim, stored_settlements, team_world, until_claimed_on, CLAIM_INTERVAL_ENV,
    PATIENT_GRACE,
};
use crate::harness::{agent, git, lifecycle, plan_of, Repository, World};

/// A lifecycle node that keeps its branch on the origin rather than landing it.
fn kept(id: &str) -> Value {
    let mut node = lifecycle(id, &[]);
    node["publish"] = json!("preserve");
    node
}

/// Author `nodes` as project `name` and answer its qualified id.
fn author(world: &World, name: &str, nodes: Vec<Value>) -> String {
    world.plan(name, &plan_of(name, nodes))
}

/// A world on a local repository whose every dispatch of `nodes` writes something the one
/// before it did not, so a redo has new work to commit on the branch it continues.
fn world_of(name: &str, nodes: &[&str]) -> (World, Repository) {
    let world = World::new(name);
    let repo = world.repository("local-direct", &[]);
    for id in nodes {
        world.script(&format!("{id}.work-anew"), &format!("{id}'s change"));
    }
    (world, repo)
}

/// Launch `project` attached, wait for run `run` to settle, and answer what was said.
fn launch(world: &World, run: &str, project: &str, extra: &[&str]) -> crate::harness::Run {
    let mut args = vec!["start", project, "--attach"];
    args.extend_from_slice(extra);
    let launched = world.run_from(&world.project, &args);
    assert!(
        world.run_file(run, "result.json").is_file(),
        "the run did not settle:\n{}\n{}",
        launched.stdout,
        launched.stderr
    );
    launched
}

/// Three kept nodes run once to `done`, answering the world, its repository, the project
/// and what the store holds for each.
fn three_kept(name: &str) -> (World, Repository, String, BTreeMap<String, Value>) {
    let (world, repo) = world_of(name, &["first", "second", "third"]);
    let project = author(
        &world,
        name,
        vec![kept("first"), kept("second"), kept("third")],
    );
    launch(&world, name, &project, &[]).exited(0);
    let stored = stored_settlements(&world, &project);
    for id in ["first", "second", "third"] {
        let settlement = &stored[id];
        assert_eq!(settlement["status"], "done", "{id}: {settlement}");
        assert_eq!(settlement["outcome"], "preserved", "{id}: {settlement}");
        assert!(settlement["branch"].is_string(), "{id}: {settlement}");
        assert!(settlement["head"].is_string(), "{id}: {settlement}");
    }
    assert_eq!(
        stored_claim(&world, &project),
        Value::Null,
        "the first run's closeout left its claim standing"
    );
    (world, repo, project, stored)
}

/// Every run directory under the world's runs root.
fn run_dirs(world: &World) -> BTreeSet<String> {
    std::fs::read_dir(&world.runs)
        .map(|dirs| {
            dirs.flatten()
                .filter(|dir| dir.path().is_dir())
                .map(|dir| dir.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `ancestor` is `descendant` or one of its ancestors in `repo`.
fn descends(world: &World, repo: &std::path::Path, ancestor: &str, descendant: &str) -> bool {
    git(world, repo, &["rev-list", descendant])
        .lines()
        .any(|commit| commit == ancestor)
}

/// The journey the flag exists for: two of three kept nodes are redone on the branches they
/// settled on — the same name, a head descending from the one each settled at — and the
/// third is continued, by a launch the ended run's released claim admits without
/// `--take-over`. A launch naming nothing afterwards continues all three.
#[test]
fn a_redo_dispatches_each_named_node_again_on_its_settled_branch_and_continues_the_rest() {
    let name = "redo-named";
    let (world, repo, project, before) = three_kept(name);

    let again = format!("{name}-2");
    let launched = launch(
        &world,
        &again,
        &project,
        &["--redo", "first", "--redo", "second"],
    );
    launched.exited(0);
    launched.err_has(&format!(
        "onepipeline: launch '{again}' continues 1 settled node from the store"
    ));

    let mut redone = dispatched(&world, &again);
    redone.sort();
    assert_eq!(redone, vec!["first".to_owned(), "second".to_owned()]);
    let mut branches = sessions_opened(&world, &again);
    branches.sort();
    let mut expected: Vec<String> = ["first", "second"]
        .iter()
        .map(|id| before[*id]["branch"].as_str().expect("a branch").to_owned())
        .collect();
    expected.sort();
    assert_eq!(
        branches, expected,
        "a redone node was not dispatched on the branch it settled on"
    );

    let after = stored_settlements(&world, &project);
    for id in ["first", "second"] {
        let now = settled(&world, &again, id);
        assert!(now.get("continued").is_none(), "{id} was continued: {now}");
        assert_eq!(now["status"], "done", "{id}: {now}");
        assert_eq!(now["branch"], before[id]["branch"], "{id}: {now}");
        let (was, is) = (
            before[id]["head"].as_str().expect("a head"),
            now["head"].as_str().expect("a head"),
        );
        assert_ne!(was, is, "{id}'s redo committed nothing");
        assert!(
            descends(&world, &repo.origin, was, is),
            "{id}'s new head {is} does not descend from {was}"
        );
        assert_eq!(
            after[id], now,
            "{id}'s stored settlement is not the new one"
        );
    }
    let third = settled(&world, &again, "third");
    assert_eq!(third["continued"]["source"], "store", "{third}");
    assert_eq!(after["third"], before["third"]);
    assert_eq!(world.run_json(&again, "result.json")["state"], "complete");
    assert_eq!(
        world.run_json(&again, "launch.json")["redo"],
        json!(["first", "second"])
    );

    // A continuing launch naming nothing, of a project whose last run has ended, is
    // admitted without `--take-over` as the redo was, and dispatches nothing.
    let third_run = format!("{name}-3");
    launch(&world, &third_run, &project, &[]).exited(0);
    assert_eq!(dispatched(&world, &third_run), Vec::<String>::new());
    assert!(world
        .run_json(&third_run, "launch.json")
        .get("redo")
        .is_none());
    assert_eq!(stored_settlements(&world, &project), after);
}

/// Each redo the launch cannot honour is refused before any run exists, naming the node —
/// or the flag it is given beside — and the reason. Every settlement it is refused over is
/// one a real run left: a kept node, a landed one, a direct one with no branch, a dispatch
/// that failed and one a manager cancelled.
#[test]
fn a_redo_that_cannot_be_honoured_is_refused_naming_the_node_and_mints_no_run() {
    let name = "redo-refused";
    let (world, _repo) = world_of(name, &["kept", "landed", "broken", "stopped"]);
    // The cancelled dispatch is ended a second after it is asked to stop.
    let world = world.with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    world.script("broken.fail", "1");
    let stopped = world.rendezvous("stopped");
    let project = author(
        &world,
        name,
        vec![
            kept("kept"),
            lifecycle("landed", &[]),
            agent("direct", &[]),
            kept("broken"),
            kept("stopped"),
        ],
    );
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let held = stopped.arrived();
    world
        .run_with_stdin(
            &["reply", name],
            &json!({"version": 2, "commands": [{"op": "cancel", "id": "stopped"}]}).to_string(),
        )
        .exited(0);
    world.until("the first run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    drop(held);
    world.unscript("stopped.rendezvous");
    let stored = stored_settlements(&world, &project);
    assert_eq!(stored["kept"]["status"], "done", "{stored:#?}");
    assert_eq!(stored["landed"]["landing"], "landed", "{stored:#?}");
    assert_eq!(stored["direct"]["status"], "done", "{stored:#?}");
    assert!(stored["direct"].get("branch").is_none(), "{stored:#?}");
    assert_eq!(stored["broken"]["status"], "failed", "{stored:#?}");
    assert_eq!(stored["stopped"]["status"], "cancelled", "{stored:#?}");
    // A node a planner added on the board since: its lineage holds no settlement at all.
    world.add_task(name, "005-added", &kept("added"), &[]);
    let runs = run_dirs(&world);

    let refused = |redo: &[&str], says: &[&str]| {
        let mut args = vec!["start", &project, "--attach"];
        args.extend_from_slice(redo);
        let run = world.run_from(&world.project, &args);
        run.exited(2);
        for said in says {
            run.err_has(said);
        }
        assert_eq!(run_dirs(&world), runs, "the refusal minted a run");
    };
    refused(
        &["--redo", "nowhere"],
        &[
            "--redo refused: node 'nowhere'",
            "is not a node of the plan",
        ],
    );
    refused(
        &["--redo", "added"],
        &["--redo refused: node 'added'", "holds no settlement"],
    );
    refused(
        &["--redo", "broken"],
        &[
            "--redo refused: node 'broken'",
            "settled `failed`, not `done`",
        ],
    );
    refused(
        &["--redo", "stopped"],
        &[
            "--redo refused: node 'stopped'",
            "settled `cancelled`, not `done`",
        ],
    );
    refused(
        &["--redo", "direct"],
        &[
            "--redo refused: node 'direct'",
            "settled `done` on no branch",
        ],
    );
    refused(
        &["--redo", "landed"],
        &["--redo refused: node 'landed'", "settled `done` and landed"],
    );
    refused(
        &["--redo", "kept", "--fresh"],
        &["--redo refused: --fresh already dispatches every node"],
    );
}

/// A dependent of a redone node is unaffected by the redo: one that settled `done` is
/// continued, and one with no settlement waits on the redone node, as any dependent waits,
/// and dispatches once it has settled again.
#[test]
fn a_redone_nodes_dependents_are_continued_or_wait_on_it_as_without_the_redo() {
    let name = "redo-dependents";
    let (world, _repo) = world_of(name, &["base", "child", "late"]);
    let mut child = kept("child");
    child["deps"] = json!(["base"]);
    let project = author(&world, name, vec![kept("base"), child]);
    launch(&world, name, &project, &[]).exited(0);
    let before = stored_settlements(&world, &project);
    assert_eq!(before["child"]["status"], "done", "{before:#?}");
    let mut late = kept("late");
    late["deps"] = json!(["base"]);
    world.add_task(name, "002-late", &late, &[("base", "000-base")]);

    let again = format!("{name}-2");
    launch(&world, &again, &project, &["--redo", "base"]).exited(0);
    assert_eq!(
        dispatched(&world, &again),
        vec!["base".to_owned(), "late".to_owned()]
    );
    let journal = world.journal(&again);
    let at = |kind: &str, node: &str| {
        journal
            .iter()
            .position(|event| {
                event["kind"] == kind
                    && event["labels"]["node"] == node
                    && event["payload"].get("continued").is_none()
            })
            .unwrap_or_else(|| panic!("no {kind} of {node} in {again}"))
    };
    assert!(
        at("node-settled", "base") < at("node-dispatched", "late"),
        "late was dispatched before base settled again"
    );
    assert_eq!(
        settled(&world, &again, "child")["continued"]["source"],
        "store"
    );
    let after = stored_settlements(&world, &project);
    assert_eq!(
        after["child"], before["child"],
        "the continued dependent moved"
    );
    assert_eq!(after["late"]["status"], "done", "{after:#?}");
    assert_eq!(world.run_json(&again, "result.json")["state"], "complete");
}

/// While a redo run drives the project, a launch with or without `--redo` is refused naming
/// it. A live amendment on the redone node, the branch it is pinned to and its settlement
/// land on its task, and the claim on the project: every key that write-back leaves which
/// the plan did not author is one `plan writeback-keys` names, in its scope.
#[test]
fn a_driven_project_refuses_every_launch_and_writeback_keys_names_what_its_run_wrote() {
    let name = "redo-driven";
    let (world, _repo) = world_of(name, &["first", "second"]);
    let project = author(&world, name, vec![kept("first"), kept("second")]);
    let authored = |world: &World| -> BTreeMap<String, BTreeSet<String>> {
        world
            .store_tasks(&project)
            .into_iter()
            .map(|task| {
                let metadata = task["item"]["metadata"].as_object().expect("metadata");
                let id = metadata["onepipeline.id"]
                    .as_str()
                    .expect("an id")
                    .to_owned();
                (id, metadata.keys().cloned().collect())
            })
            .collect()
    };
    let project_keys = |world: &World| -> BTreeSet<String> {
        world.store_project(&project)["items"][0]["item"]["metadata"]
            .as_object()
            .expect("project metadata")
            .keys()
            .cloned()
            .collect()
    };
    let (task_keys_before, project_keys_before) = (authored(&world), project_keys(&world));
    launch(&world, name, &project, &[]).exited(0);

    world.script("first.wait", "hold");
    let again = format!("{name}-2");
    world
        .run_from(
            &world.project,
            &["start", &project, "--detach", "--redo", "first"],
        )
        .exited(0);
    world.until("the redone node to be running", |world| {
        world
            .run(&["status", &again])
            .stdout
            .contains("first: running")
    });
    assert_eq!(stored_claim(&world, &project)["run"], json!(again));
    let runs = run_dirs(&world);
    for redo in [&[][..], &["--redo", "second"][..]] {
        let mut args = vec!["start", &project, "--attach"];
        args.extend_from_slice(redo);
        let refused = world.run_from(&world.project, &args);
        refused.exited(2);
        refused.err_has("project driven elsewhere refused:");
        refused.err_has(&format!("run '{again}'"));
        assert_eq!(run_dirs(&world), runs, "the refusal minted a run");
    }

    world
        .run_with_stdin(
            &["reply", &again],
            &json!({"version": 2, "commands": [
                {"op": "amend", "id": "first", "text": "Keep the measured row."}
            ]})
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
    world.until("the amendment to be committed", |world| {
        world
            .events_of(&again, "edit-committed")
            .iter()
            .any(|event| event["payload"]["command"]["op"] == "amend")
    });
    world.release("first.go");
    world.until("the redo run to settle", |world| {
        world.run_file(&again, "result.json").is_file()
    });
    assert_eq!(world.run_json(&again, "result.json")["state"], "complete");

    let listed = world.run(&["plan", "writeback-keys", "--json"]);
    listed.exited(0);
    let keys = listed.json();
    let scope = |name: &str| -> BTreeSet<String> {
        keys[name]
            .as_array()
            .unwrap_or_else(|| panic!("no {name} array: {keys}"))
            .iter()
            .map(|key| key.as_str().expect("a key").to_owned())
            .collect()
    };
    let (task, on_project) = (scope("task"), scope("project"));

    let written = authored(&world);
    let first = &written["first"];
    for key in [
        "onepipeline.settlement",
        "onepipeline.branch",
        "onepipeline.amendment",
    ] {
        assert!(
            first.contains(key),
            "the run left no {key} on first: {first:?}"
        );
    }
    for (id, now) in &written {
        let left: Vec<&String> = now.difference(&task_keys_before[id]).collect();
        assert!(!left.is_empty(), "the run wrote nothing onto {id}");
        for key in left {
            assert!(
                task.contains(key),
                "{key} on {id} is not a write-back key: {task:?}"
            );
        }
    }
    let left: Vec<String> = project_keys(&world)
        .difference(&project_keys_before)
        .cloned()
        .collect();
    assert!(
        left.contains(&"onepipeline.driver".to_owned()),
        "the run left no claim key on the project: {left:?}"
    );
    for key in &left {
        assert!(
            on_project.contains(key),
            "{key} on the project is not a write-back key: {on_project:?}"
        );
    }
}

/// `plan writeback-keys` prints exactly the engine's projection-only keys and live-edit
/// fields, each array sorted and once, and the same keys one `<scope> <key>` line each.
#[test]
fn writeback_keys_states_the_projection_only_keys_and_the_live_edit_fields() {
    let world = World::new("writeback-keys");
    let listed = world.run(&["plan", "writeback-keys", "--json"]);
    listed.exited(0);
    assert_eq!(
        listed.json(),
        json!({
            "task": [
                "onepipeline.amendment",
                "onepipeline.branch",
                "onepipeline.change_url",
                "onepipeline.context",
                "onepipeline.dispatch",
                "onepipeline.landing",
                "onepipeline.landing_commit",
                "onepipeline.landing_evidence",
                "onepipeline.node",
                "onepipeline.parked",
                "onepipeline.projected_status",
                "onepipeline.resume",
                "onepipeline.rework",
                "onepipeline.settlement",
                "onepipeline.supersedes",
            ],
            "project": ["onepipeline.driver", "onepipeline.publishing"],
        })
    );
    let lines = world.run(&["plan", "writeback-keys"]);
    lines.exited(0);
    let keys = listed.json();
    let expected: String = ["task", "project"]
        .iter()
        .flat_map(|scope| {
            keys[*scope]
                .as_array()
                .expect("an array")
                .iter()
                .map(move |key| format!("{scope} {}\n", key.as_str().expect("a key")))
        })
        .collect();
    assert_eq!(lines.stdout, expected);
}

/// An `adopt` of a redo run whose driver is gone after a named node settled again replays
/// the run from its journal: that node is not dispatched a third time, and its stored
/// settlement is still the one the redo wrote.
#[test]
fn an_adopted_redo_run_does_not_redo_a_node_that_already_settled_again() {
    let name = "redo-adopted";
    let (world, _repo) = world_of(name, &["first", "second"]);
    let project = author(&world, name, vec![kept("first"), kept("second")]);
    launch(&world, name, &project, &[]).exited(0);
    let before = stored_settlements(&world, &project);

    world.script("second.wait", "hold");
    let again = format!("{name}-2");
    world
        .run_from(
            &world.project,
            &[
                "start", &project, "--detach", "--redo", "first", "--redo", "second",
            ],
        )
        .exited(0);
    // Until first's redo settles, its task carries its open dispatch and no settlement.
    world.until("first's redo to land on the store", |world| {
        stored_settlements(world, &project)
            .get("first")
            .is_some_and(|settlement| settlement["head"] != before["first"]["head"])
    });
    let redone = stored_settlements(&world, &project)["first"].clone();
    assert_eq!(redone["status"], "done", "{redone}");
    let shutdown = world.run(&["shutdown", &again, "--grace", "1"]);
    assert!([0, 2].contains(&shutdown.code), "{}", shutdown.stderr);

    world.unscript("second.wait");
    world.run(&["adopt", &again, "--attach"]).exited(0);
    assert_eq!(world.run_json(&again, "result.json")["state"], "complete");
    let firsts = dispatched(&world, &again)
        .into_iter()
        .filter(|node| node == "first")
        .count();
    assert_eq!(firsts, 1, "the adoption dispatched first again");
    let after = stored_settlements(&world, &project);
    assert_eq!(
        after["first"], redone,
        "the adoption changed first's settlement"
    );
    assert_eq!(after["second"]["status"], "done", "{after:#?}");
    assert_eq!(after["second"]["branch"], before["second"]["branch"]);
}

/// A `done` node whose change request is open as a draft and unlanded is redone on its
/// branch, and its publication adopts that change request: the host is asked to open none,
/// and the draft is still the one change request from the branch.
#[test]
fn a_redone_draft_continues_its_branch_and_adopts_its_open_change_request() {
    let world = team_world("redo-draft");
    world.script("a.work-anew", "a's change");
    let name = "drafted";
    let project = author(&world, name, vec![lifecycle("a", &[])]);
    launch(&world, name, &project, &[]).exited(0);
    let before = stored_settlements(&world, &project)["a"].clone();
    assert_eq!(before["status"], "done", "{before}");
    assert_eq!(before["outcome"], "change-review-draft", "{before}");
    assert_eq!(before["landing"], "unlanded", "{before}");
    let branch = before["branch"].as_str().expect("a branch").to_owned();
    let created = created_from(&world);
    assert_eq!(created, vec![branch.clone()]);

    let again = format!("{name}-2");
    launch(&world, &again, &project, &["--redo", "a"]).exited(0);
    assert_eq!(dispatched(&world, &again), vec!["a".to_owned()]);
    assert_eq!(sessions_opened(&world, &again), vec![branch.clone()]);
    assert_eq!(
        created_from(&world),
        created,
        "a second change request was opened"
    );
    let a = settled(&world, &again, "a");
    assert!(a.get("continued").is_none(), "{a}");
    assert_eq!(a["outcome"], "change-review-draft", "{a}");
    assert_eq!(a["branch"], json!(branch), "{a}");
    assert_eq!(a["change_url"], before["change_url"], "{a}");
    assert_ne!(a["head"], before["head"], "the redo committed nothing: {a}");
    let held = changes_from(&world, &branch);
    assert_eq!(held.len(), 1, "{held:?}");
    assert!(held_as_draft(&world, &held[0]["number"]), "{held:?}");
    assert_eq!(stored_settlements(&world, &project)["a"], a);
    assert_eq!(world.run_json(&again, "result.json")["state"], "complete");
}

/// A `--redo` that takes the project over from a live driver on another host is checked and
/// continued again from the store as it stands once that driver has let go: a node the
/// displaced run settled during the handover is continued rather than dispatched, and the
/// named node is still redone on the branch it settled on.
#[test]
fn a_redo_taking_a_live_project_over_redoes_what_the_store_holds_once_it_is_handed_over() {
    let name = "redo-takeover";
    let (base, _repo) = world_of(name, &["first", "second"]);
    let world = base
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, &PATIENT_GRACE.to_string());
    let project = author(&world, name, vec![kept("first"), kept("second")]);
    launch(&world, name, &project, &[]).exited(0);
    let before = stored_settlements(&world, &project);

    // A planner adds a node, and a continuing run on this host dispatches it and is held.
    let hold = world.rendezvous("hold");
    world.add_task(name, "002-hold", &agent("hold", &[]), &[]);
    let displaced = format!("{name}-2");
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let holding = hold.arrived();
    assert!(
        !stored_settlements(&world, &project).contains_key("hold"),
        "the held node settled before the takeover read the store"
    );

    let successor = world.on_host("host-b");
    let taking = take_over_in_background_redoing(&successor, &project, "first");
    let claim = until_claimed_on(&world, &project, "host-b");
    assert_eq!(claim["host"], "host-b", "{claim}");
    // The displaced run settles its held node and closes out, acknowledging the takeover.
    holding.release();
    let said = launched_over(taking);
    assert!(said.contains("acknowledged the takeover"), "{said}");
    world.until("the displaced run to settle", |world| {
        world.run_file(&displaced, "result.json").is_file()
    });
    assert_eq!(dispatched(&world, &displaced), vec!["hold".to_owned()]);

    successor.until("the adopter to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    assert_eq!(successor.run_json(name, "result.json")["state"], "complete");
    assert_eq!(dispatched(&successor, name), vec!["first".to_owned()]);
    assert_eq!(
        settled(&successor, name, "hold")["continued"]["source"],
        "store",
        "the adopter did not continue what the displaced run settled during the handover"
    );
    assert_eq!(
        settled(&successor, name, "second")["continued"]["source"],
        "store"
    );
    let first = settled(&successor, name, "first");
    assert!(first.get("continued").is_none(), "{first}");
    assert_eq!(first["branch"], before["first"]["branch"], "{first}");
    assert_ne!(first["head"], before["first"]["head"], "{first}");
    assert_eq!(
        successor.run_json(name, "launch.json")["redo"],
        json!(["first"])
    );
    drop(successor);
}

/// `continuation`'s `take_over_in_background`, naming `node` to `--redo`.
fn take_over_in_background_redoing(
    host: &World,
    project: &str,
    node: &str,
) -> onepipeline_testfakes::profile::process::Child {
    let mut command = host.cmd(&["start", project, "--detach", "--take-over", "--redo", node]);
    command
        .current_dir(&host.project)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command.spawn().expect("the takeover starts")
}
