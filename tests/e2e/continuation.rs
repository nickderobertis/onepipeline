//! A project continued from its store alone, and the best-effort claim on who drives it.
//!
//! A host holding none of a run's local state — its runs root gone, a fresh `onevcs`
//! state root — launches a project again and carries it on from what the plans store
//! holds: each lineage's `onepipeline.settlement`, a node's open `onepipeline.dispatch`,
//! and the project's `onepipeline.driver`. Entry 116 of `docs/contract-divergences.md`.
//!
//! Each journey drives the compiled binary over a real `local-md` store, the real linked
//! `onevcs` over a real origin on disk, and GitHub at `onevcs`'s own `ONEVCS_GH` seam —
//! the `fake-gh` double every host here shares, as every host shares the one GitHub. A
//! second host is [`World::on_host`]: the same store, origin and GitHub, and nothing else.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is driven as a real compiled
// binary over a real `local-md` store, and `onevcs` is the real library over real git and a
// real origin on disk. `oneagentgraph` is substituted at its subprocess boundary so a
// journey states a dispatch outcome rather than paying for a model turn, and GitHub at
// `onevcs`'s own `ONEVCS_GH` override so a change request's draft state can be read back
// offline. `harness.rs` carries the same suppression and the full rationale.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::draft_lifecycle::{team_repository, GREEN};
use crate::harness::{lifecycle, plan_of, World};

/// The host's branch prefix every team journey here runs under.
const PREFIX: &str = "me/";

/// A required check the host reports red.
const RED: &str = "lint completed failure required";

/// A world on a team repository — `change-open`, approvals required — so a published
/// node settles `change-review-draft` with its change request held as a draft.
fn team_world(name: &str) -> World {
    let world = World::new(name).with_env("ONEVCS_BRANCH_PREFIX", PREFIX);
    team_repository(&world);
    world.script("gh.checks", GREEN);
    world
}

/// Author `nodes` as project `name` on the world's store, one dispatch at a time, and
/// answer the project's qualified id.
fn author(world: &World, name: &str, nodes: Vec<Value>) -> String {
    let mut plan = plan_of(name, nodes);
    plan["concurrency"] = json!(1);
    world.plan(name, &plan)
}

/// Launch `project` attached from `world`'s project directory and wait for its result.
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

/// Each node's `onepipeline.settlement` as the store holds it, by node id.
fn stored_settlements(world: &World, project: &str) -> BTreeMap<String, Value> {
    world
        .store_tasks(project)
        .into_iter()
        .filter_map(|task| {
            let metadata = &task["item"]["metadata"];
            let id = metadata["onepipeline.id"].as_str()?.to_owned();
            let settlement = metadata.get("onepipeline.settlement")?.clone();
            Some((id, settlement))
        })
        .collect()
}

/// The project's `onepipeline.driver`, as the store holds it: `null` where it holds none.
fn stored_claim(world: &World, project: &str) -> Value {
    world.store_project(project)["items"][0]["item"]["metadata"]
        .get("onepipeline.driver")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Every `gh pr create` the shared host was asked, by head branch.
fn created_from(world: &World) -> Vec<String> {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "gh")
        .filter_map(|call| {
            let args: Vec<&str> = call["args"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            (args.first() == Some(&"pr") && args.get(1) == Some(&"create")).then(|| {
                args.iter()
                    .position(|arg| *arg == "--head")
                    .and_then(|at| args.get(at + 1))
                    .map(|head| (*head).to_owned())
                    .unwrap_or_default()
            })
        })
        .collect()
}

/// Whether the shared host holds change request `number` as a draft.
fn held_as_draft(world: &World, number: &Value) -> bool {
    std::fs::read_to_string(world.fakes.join("gh").join(number.to_string()))
        .is_ok_and(|state| state.trim() == "draft")
}

/// The change requests the shared host holds from `branch`.
fn changes_from(world: &World, branch: &str) -> Vec<Value> {
    world
        .changes_opened()
        .into_iter()
        .filter(|change| change["head"] == branch)
        .collect()
}

/// The newest settlement `node` journalled in `run`.
fn settled(world: &World, run: &str, node: &str) -> Value {
    world
        .events_of(run, "node-settled")
        .into_iter()
        .rev()
        .find(|event| event["labels"]["node"] == node)
        .map(|event| event["payload"].clone())
        .unwrap_or_else(|| panic!("{node} never settled: {:#?}", world.journal(run)))
}

/// `settlement` without the key a continuation adds.
fn as_stored(settlement: &Value) -> Value {
    let mut stored = settlement.clone();
    if let Some(fields) = stored.as_object_mut() {
        fields.remove("continued");
    }
    stored
}

/// The nodes `run` dispatched, in order.
fn dispatched(world: &World, run: &str) -> Vec<String> {
    world
        .events_of(run, "node-dispatched")
        .iter()
        .filter_map(|event| event["labels"]["node"].as_str().map(str::to_owned))
        .collect()
}

/// The branches `run`'s dispatches opened sessions on, each once: the sibling's own
/// `session-opened` and the copy the engine relays beside it describe one session.
fn sessions_opened(world: &World, run: &str) -> Vec<String> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .filter_map(|event| event["payload"]["branch"].as_str().map(str::to_owned))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// A three-node stack drafted on a team identity, read back off the store, and launched
/// again from a cold host: nothing is dispatched, no branch is cut, no change request
/// opened, and every node is settled from what the store held — the same branch, change
/// request, landing, base, head and stack — under `continued`, which `status` and
/// `results` say.
#[test]
fn a_cold_host_continues_a_drafted_stack_from_the_store_and_dispatches_none_of_it() {
    let world = team_world("continue-cold");
    for id in ["a", "b", "c"] {
        world.script(&format!("{id}.work"), &format!("{id}'s change\n"));
    }
    let run = "drafts";
    let project = author(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &["a"]),
            lifecycle("c", &["b"]),
        ],
    );
    launch(&world, run, &project, &[]);
    let stored = stored_settlements(&world, &project);
    for id in ["a", "b", "c"] {
        let settlement = &stored[id];
        assert_eq!(settlement["status"], "done", "{id}: {settlement}");
        assert_eq!(
            settlement["outcome"], "change-review-draft",
            "{id}: {settlement}"
        );
        assert_eq!(settlement["landing"], "unlanded", "{id}: {settlement}");
        for field in ["branch", "change_url", "base", "head"] {
            assert!(
                settlement[field].is_string(),
                "{id} stored no {field}: {settlement}"
            );
        }
    }
    for (id, parent) in [("b", "a"), ("c", "b")] {
        assert_eq!(stored[id]["base"], stored[parent]["branch"], "{stored:#?}");
        assert_eq!(
            stored[id]["stack"]["parents"][0]["branch"], stored[parent]["branch"],
            "{stored:#?}"
        );
    }
    assert_eq!(
        stored_claim(&world, &project),
        Value::Null,
        "the closeout left the claim standing"
    );
    let created = created_from(&world);
    assert_eq!(created.len(), 3, "{created:?}");

    // The host that ran it is gone: its runs root with it, and the cold host has its own.
    std::fs::remove_dir_all(&world.runs).expect("the runs root is removed");
    let cold = world.on_host("host-b");
    let launched = launch(&cold, run, &project, &[]);
    launched.exited(0);

    assert_eq!(
        dispatched(&cold, run),
        Vec::<String>::new(),
        "a continued node was dispatched"
    );
    assert_eq!(
        sessions_opened(&cold, run),
        Vec::<String>::new(),
        "a branch was cut"
    );
    assert_eq!(created_from(&world), created, "a change request was opened");
    for id in ["a", "b", "c"] {
        let continued = settled(&cold, run, id);
        assert_eq!(
            continued["continued"],
            json!({"source": "store", "run": null}),
            "{id}: {continued}"
        );
        assert_eq!(
            as_stored(&continued),
            stored[id],
            "{id} is not what the store held"
        );
    }
    assert_eq!(cold.run_json(run, "result.json")["state"], "complete");

    let status = cold.run(&["status", run]);
    status.exited(0);
    status.out_has("launched on this host from the store: 3 nodes continued, 0 nodes dispatched");
    let results = cold.run(&["results", run]);
    results.exited(0);
    results.out_has("launched on this host from the store: 3 nodes continued, 0 nodes dispatched");
    for id in ["a", "b", "c"] {
        let branch = stored[id]["branch"].as_str().expect("a branch");
        let url = stored[id]["change_url"].as_str().expect("a change request");
        status.out_has(&format!(
            "  {id}: continued from the store on {branch} — {url}"
        ));
        let line = results
            .stdout
            .lines()
            .find(|line| line.trim_start().starts_with(&format!("{id} ")))
            .unwrap_or_else(|| panic!("results names no {id}:\n{}", results.stdout));
        assert!(
            line.contains(&format!("continued from the store on {branch}")) && line.contains(url),
            "{line}"
        );
    }
    // The drafts reviewers are reading are the ones still open, and still drafts.
    for change in world.changes_opened() {
        assert!(held_as_draft(&world, &change["number"]), "{change}");
    }
    drop(cold);
}

/// A drafted three-node stack the cold host continues, with a fourth node a planner added
/// on the board depending on its top: only the new node is dispatched, its session is cut
/// from the top node's recorded branch, and its change request is drafted against it.
#[test]
fn a_node_added_on_a_continued_node_is_drafted_on_its_recorded_branch() {
    let world = team_world("continue-added");
    for id in ["a", "b", "c", "d"] {
        world.script(&format!("{id}.work"), &format!("{id}'s change\n"));
    }
    world.script("d.requires-file", "c.md");
    let run = "added";
    let project = author(
        &world,
        run,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &["a"]),
            lifecycle("c", &["b"]),
        ],
    );
    launch(&world, run, &project, &[]);
    let stored = stored_settlements(&world, &project);
    let created = created_from(&world);
    world.add_task(run, "003-d", &lifecycle("d", &["c"]), &[("c", "002-c")]);

    std::fs::remove_dir_all(&world.runs).expect("the runs root is removed");
    let cold = world.on_host("host-b");
    launch(&cold, run, &project, &[]).exited(0);

    assert_eq!(dispatched(&cold, run), vec!["d".to_owned()]);
    let d = settled(&cold, run, "d");
    assert_eq!(d["outcome"], "change-review-draft", "{d}");
    assert_eq!(
        d["base"], stored["c"]["branch"],
        "d was not placed on c's branch: {d}"
    );
    assert_eq!(
        d["stack"]["parents"][0]["branch"], stored["c"]["branch"],
        "{d}"
    );
    let d_branch = d["branch"].as_str().expect("d's branch");
    let mut now = created.clone();
    now.push(d_branch.to_owned());
    assert_eq!(
        created_from(&world),
        now,
        "a change request other than d's was opened"
    );
    let opened = changes_from(&world, d_branch);
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert_eq!(opened[0]["base"], stored["c"]["branch"], "{opened:?}");
    for id in ["a", "b", "c"] {
        assert_eq!(as_stored(&settled(&cold, run, id)), stored[id], "{id}");
    }
    drop(cold);
}

/// `a → b` drafted on a team identity whose host reports `b`'s checks red, publishing once:
/// `a` settles `change-review-draft` and `b` `failed`, holding its branch and its draft.
/// Answers the world, the project and the settlements the store holds.
fn a_failed_draft(name: &str, run: &str) -> (World, String, BTreeMap<String, Value>) {
    let world = team_world(name).with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "1");
    world.script("a.work", "a's change\n");
    world.script("b.work-anew", "b's change");
    // `b`'s change request is the second the host opens; its checks fail.
    world.script("gh.checks.2", RED);
    let project = author(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    launch(&world, run, &project, &[]);
    let stored = stored_settlements(&world, &project);
    assert_eq!(stored["b"]["status"], "failed", "{stored:#?}");
    for field in ["branch", "change_url"] {
        assert!(
            stored["b"][field].is_string(),
            "b stored no {field}: {stored:#?}"
        );
    }
    (world, project, stored)
}

/// Write `value` under `key` on the task carrying node `node`, as a driver on another host
/// would have written it.
fn restate(world: &World, project: &str, node: &str, key: &str, value: Value) {
    let task = world
        .store_tasks(project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .unwrap_or_else(|| panic!("the board holds no item for {node}"));
    let id: onetaskgraph_core::GlobalId = task["id"]
        .as_str()
        .expect("an item id")
        .parse()
        .expect("a qualified id");
    let key = onetaskgraph_plugin_api::MetadataKey::new(key.to_owned()).expect("a key");
    world
        .store_call(|engine| async move { engine.set_task_metadata(&id, &key, &value).await })
        .unwrap_or_else(|why| panic!("the store refused the write: {why}"));
}

/// A cold host continues `a` and dispatches `b` pinned to the branch its stored
/// settlement left — no new branch — and `b`'s publication, through the engine's ordinary
/// path, adopts the change request already open from it: the host is asked to open none,
/// and that change request is still the one draft on the branch.
fn the_pinned_node_continues_its_branch_and_its_draft(
    world: &World,
    project: &str,
    run: &str,
    stored: &BTreeMap<String, Value>,
) {
    let branch = stored["b"]["branch"]
        .as_str()
        .expect("b's branch")
        .to_owned();
    let created = created_from(world);
    // The red checks were the reason it failed; a reviewer's fix has turned them green.
    world.script("gh.checks.2", GREEN);
    std::fs::remove_dir_all(&world.runs).expect("the runs root is removed");
    let cold = world.on_host("host-b");
    launch(&cold, run, project, &[]).exited(0);
    assert_eq!(dispatched(&cold, run), vec!["b".to_owned()]);
    assert_eq!(
        sessions_opened(&cold, run),
        vec![branch.clone()],
        "b was not dispatched on the branch it left"
    );
    assert_eq!(created_from(world), created, "a change request was opened");
    let b = settled(&cold, run, "b");
    assert_eq!(b["status"], "done", "{b}");
    assert_eq!(b["outcome"], "change-review-draft", "{b}");
    assert_eq!(b["branch"], json!(branch), "{b}");
    assert_eq!(b["change_url"], stored["b"]["change_url"], "{b}");
    let held = changes_from(world, &branch);
    assert_eq!(held.len(), 1, "{held:?}");
    assert!(held_as_draft(world, &held[0]["number"]), "{held:?}");
    assert_eq!(
        settled(&cold, run, "a")["continued"]["source"],
        "store",
        "a was not continued"
    );
    drop(cold);
}

/// A node its last driver settled `failed` with a branch and a draft is continued on that
/// branch, and its draft adopted.
#[test]
fn a_failed_node_is_continued_pinned_to_its_branch_and_adopts_its_draft() {
    let run = "failed";
    let (world, project, stored) = a_failed_draft("continue-failed", run);
    the_pinned_node_continues_its_branch_and_its_draft(&world, &project, run, &stored);
}

/// A node its last driver settled `cancelled` with a branch and a draft — a dispatch a
/// manager cancelled while its publication was being retried — is continued the same way.
#[test]
fn a_cancelled_node_is_continued_pinned_to_its_branch_and_adopts_its_draft() {
    let run = "cancelled";
    let (world, project, mut stored) = a_failed_draft("continue-cancelled", run);
    let b = stored.get_mut("b").expect("b's settlement");
    b["status"] = json!("cancelled");
    b.as_object_mut().expect("a settlement").remove("outcome");
    restate(&world, &project, "b", "onepipeline.settlement", b.clone());
    the_pinned_node_continues_its_branch_and_its_draft(&world, &project, run, &stored);
}

/// A node its last driver left `parked` with a branch and a draft is continued the same way.
#[test]
fn a_parked_node_is_continued_pinned_to_its_branch_and_adopts_its_draft() {
    let run = "parked";
    let (world, project, mut stored) = a_failed_draft("continue-parked", run);
    let b = stored.get_mut("b").expect("b's settlement");
    b["status"] = json!("parked");
    b.as_object_mut().expect("a settlement").remove("outcome");
    restate(&world, &project, "b", "onepipeline.settlement", b.clone());
    the_pinned_node_continues_its_branch_and_its_draft(&world, &project, run, &stored);
}
