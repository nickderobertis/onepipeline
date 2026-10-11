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
use crate::harness::{agent, lifecycle, plan_of, World};

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

/// The claim the store holds for `project`, once it names `run`: read back off the board,
/// as a host that holds none of the run's state would read it.
fn until_claimed_by(world: &World, project: &str, run: &str) -> Value {
    world.until(
        &format!("the store to name {run} as the project's driver"),
        |world| stored_claim(world, project)["run"] == run,
    );
    stored_claim(world, project)
}

/// The metadata the board's item for node `node` carries.
fn item_metadata(world: &World, project: &str, node: &str) -> Value {
    world
        .store_tasks(project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .map(|task| task["item"]["metadata"].clone())
        .unwrap_or_else(|| panic!("the board holds no item for {node}"))
}

/// A run continuing `a → b` dispatches `b` on its branch, and the board says so while it
/// works — `onepipeline.dispatch` naming the branch and the base, its stale settlement gone,
/// and the run's claim. A host shutdown leaves both on the board. A cold host is refused the
/// claimed project, naming the run that holds it and `--take-over`; taking it over journals
/// the claim it passed, pins `b` to the branch the dispatch key names, and publishes it onto
/// the draft already open from it, opening nothing — and its own settlement removes the key.
#[test]
fn an_in_flight_dispatch_left_on_the_store_is_continued_by_a_host_that_takes_the_project_over() {
    let run = "inflight";
    let (world, project, stored) = a_failed_draft("continue-dispatch", run);
    let branch = stored["b"]["branch"]
        .as_str()
        .expect("b's branch")
        .to_owned();
    world.script("b.wait", "hold");
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let held = format!("{run}-2");
    let claim = until_claimed_by(&world, &project, &held);
    let launched = world.run_json(&held, "launch.json");
    assert_eq!(claim["host"], launched["host"], "{claim}");
    assert_eq!(claim["pid"], launched["pid"], "{claim}");
    assert_eq!(claim["session"], json!(world.session), "{claim}");
    assert!(claim["claimed_at"].is_string(), "{claim}");
    world.until("b's dispatch to reach the board", |world| {
        item_metadata(world, &project, "b")
            .get("onepipeline.dispatch")
            .is_some()
    });
    let b = item_metadata(&world, &project, "b");
    let dispatch = &b["onepipeline.dispatch"];
    assert_eq!(dispatch["branch"], json!(branch), "{dispatch}");
    assert_eq!(dispatch["base"], stored["a"]["branch"], "{dispatch}");
    assert!(dispatch["opened_at"].is_string(), "{dispatch}");
    assert!(
        b.get("onepipeline.settlement").is_none(),
        "the board still holds the settlement b's dispatch superseded: {b}"
    );

    let shutdown = world.run(&["shutdown", &held, "--grace", "1"]);
    assert!(
        [0, 2].contains(&shutdown.code),
        "the shutdown failed: {}\n{}",
        shutdown.stdout,
        shutdown.stderr
    );
    assert_eq!(
        stored_claim(&world, &project),
        claim,
        "the shutdown released the claim"
    );
    assert_eq!(
        item_metadata(&world, &project, "b")["onepipeline.dispatch"],
        *dispatch,
        "the shutdown took b's dispatch off the board"
    );

    world.unscript("b.wait");
    world.script("gh.checks.2", GREEN);
    let created = created_from(&world);
    std::fs::remove_dir_all(&world.runs).expect("the runs root is removed");
    let cold = world.on_host("host-b");
    let refused = cold.run_from(&cold.project, &["start", &project, "--attach"]);
    refused.exited(2);
    assert!(
        refused
            .stderr
            .contains("onepipeline: project driven elsewhere refused:"),
        "{}",
        refused.stderr
    );
    for named in [format!("run '{held}'"), "--take-over".to_owned()] {
        assert!(refused.stderr.contains(&named), "{}", refused.stderr);
    }
    assert!(!refused.stderr.contains("adopt"), "{}", refused.stderr);
    assert!(
        !cold.run_file(run, "launch.json").exists(),
        "the refusal minted a run"
    );

    launch(&cold, run, &project, &["--take-over"]).exited(0);
    let taken = cold.events_of(run, "driver-taken-over");
    assert_eq!(taken.len(), 1, "{taken:?}");
    assert_eq!(taken[0]["payload"]["previous"], claim);
    assert_eq!(dispatched(&cold, run), vec!["b".to_owned()]);
    assert_eq!(sessions_opened(&cold, run), vec![branch.clone()]);
    assert_eq!(created_from(&world), created, "a change request was opened");
    let opened = changes_from(&world, &branch);
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert!(held_as_draft(&world, &opened[0]["number"]), "{opened:?}");
    let b = settled(&cold, run, "b");
    assert_eq!(b["outcome"], "change-review-draft", "{b}");
    let item = item_metadata(&world, &project, "b");
    assert!(
        item.get("onepipeline.dispatch").is_none(),
        "b's settlement left its dispatch on the board: {item}"
    );
    assert_eq!(item["onepipeline.settlement"]["status"], "done", "{item}");
    assert_eq!(stored_claim(&world, &project), Value::Null);
    drop(cold);
}

/// Launch `nodes` as project `name`, detached, and answer the project's id.
fn launch_detached(world: &World, name: &str, nodes: Vec<Value>) -> String {
    let project = world.plan(name, &plan_of(name, nodes));
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    project
}

/// A run's claim is on the store while its driver drives it — naming the run, this host,
/// its launching session and its driver's pid — and is gone once the run closes out, and
/// once a `stop` ends a run before it settles.
#[test]
fn the_claim_is_held_while_a_run_drives_and_released_at_closeout_and_at_stop() {
    let world = World::new("claim-lifecycle");
    world.script("closes.wait", "hold");
    let closes = launch_detached(&world, "closes", vec![agent("closes", &[])]);
    let claim = until_claimed_by(&world, &closes, "closes");
    let launched = world.run_json("closes", "launch.json");
    assert_eq!(claim["host"], launched["host"], "{claim}");
    assert_eq!(claim["pid"], launched["pid"], "{claim}");
    assert_eq!(claim["session"], json!(world.session), "{claim}");
    world.release("closes.go");
    world.until("the run to close out", |world| {
        world.run_file("closes", "result.json").is_file()
    });
    world.until("the closeout to release the claim", |world| {
        stored_claim(world, &closes) == Value::Null
    });

    world.script("stopped.wait", "hold");
    let stopped = launch_detached(&world, "stopped", vec![agent("stopped", &[])]);
    until_claimed_by(&world, &stopped, "stopped");
    world.run(&["stop", "stopped"]).exited(0);
    assert_eq!(
        stored_claim(&world, &stopped),
        Value::Null,
        "the stop left the claim standing"
    );
}

/// A claim naming a run of **this** host refuses a launch whether that run's driver is
/// live or gone, naming the run to `adopt` and `--take-over`; and `--take-over` passes the
/// claim of a run whose driver a shutdown ended, journalling the claim it passed.
#[test]
fn a_claim_of_this_hosts_own_run_refuses_a_launch_naming_adopt_live_or_gone() {
    let world = World::new("claim-this-host");
    world.script("work.wait", "hold");
    let project = launch_detached(&world, "mine", vec![agent("work", &[])]);
    let claim = until_claimed_by(&world, &project, "mine");
    let said = |refused: &crate::harness::Run| {
        refused.exited(2);
        for named in [
            "onepipeline: project driven elsewhere refused:",
            "run 'mine'",
            "`onepipeline adopt mine`",
            "--take-over",
        ] {
            assert!(
                refused.stderr.contains(named),
                "{named}: {}",
                refused.stderr
            );
        }
    };
    // Live: its driver is driving it now.
    said(&world.run_from(&world.project, &["start", &project, "--attach"]));
    assert!(
        !world.run_file("mine-2", "launch.json").exists(),
        "the refusal minted a run"
    );

    // Gone: a shutdown ended its driver and kept the claim, so the run stays adoptable.
    let shutdown = world.run(&["shutdown", "mine", "--grace", "1"]);
    assert!([0, 2].contains(&shutdown.code), "{}", shutdown.stderr);
    assert_eq!(stored_claim(&world, &project), claim);
    said(&world.run_from(&world.project, &["start", &project, "--attach"]));

    world.unscript("work.wait");
    world
        .run_from(
            &world.project,
            &["start", &project, "--attach", "--take-over"],
        )
        .exited(0);
    let taken = world.events_of("mine-2", "driver-taken-over");
    assert_eq!(taken.len(), 1, "{taken:?}");
    assert_eq!(taken[0]["payload"]["previous"], claim);
    assert_eq!(world.run_json("mine-2", "result.json")["state"], "complete");
    assert_eq!(stored_claim(&world, &project), Value::Null);
}

/// A store that cannot be read refuses the launch rather than reading it as unclaimed.
#[test]
fn a_launch_whose_store_cannot_be_read_is_refused_rather_than_read_as_unclaimed() {
    let world = World::new("claim-unreadable").through_scripted_source();
    let project = world.plan("unread", &plan_of("unread", vec![agent("work", &[])]));
    world.store_refuses(
        "get_project",
        &json!({"kind": "unavailable", "message": "the board is down"}),
    );
    let refused = world.run_from(&world.project, &["start", &project, "--attach"]);
    refused.exited(2);
    assert!(
        refused.stderr.contains("the board is down"),
        "{}",
        refused.stderr
    );
    assert!(
        !world.run_file("unread", "launch.json").exists(),
        "the refusal minted a run"
    );
}

/// After a host takes the project over, the run it took it from neither closes out nor
/// stops over the successor's claim: its closeout and a `stop` of it each leave that claim
/// on the store as it was, and a third launch while the successor drives is refused naming
/// the successor's run.
#[test]
fn a_successors_claim_survives_the_closeout_and_the_stop_of_the_run_it_took_over() {
    let world = World::new("claim-successor");
    let first = world.rendezvous("first");
    let closes = launch_detached(&world, "closes", vec![agent("first", &[])]);
    let mine = first.arrived();
    until_claimed_by(&world, &closes, "closes");
    let successor = world.on_host("host-b");
    successor
        .run_from(
            &successor.project,
            &["start", &closes, "--detach", "--take-over"],
        )
        .exited(0);
    let theirs = first.arrived();
    world.until("the successor's claim to reach the store", |world| {
        stored_claim(world, &closes)["host"] == "host-b"
    });
    let claimed = stored_claim(&world, &closes);
    let third = world.on_host("host-c");
    let refused = third.run_from(&third.project, &["start", &closes, "--attach"]);
    refused.exited(2);
    assert!(
        refused
            .stderr
            .contains("project driven elsewhere refused: run 'closes' on host 'host-b'"),
        "{}",
        refused.stderr
    );
    // The run taken over finishes its one node and closes out with nothing left to start.
    mine.release();
    world.until("the run taken over to close out", |world| {
        world.run_file("closes", "result.json").is_file()
    });
    assert_eq!(
        stored_claim(&world, &closes),
        claimed,
        "the closeout took the claim"
    );

    // And one stopped while its successor drives.
    let second = world.rendezvous("second");
    let stops = launch_detached(&world, "stops", vec![agent("second", &[])]);
    let held = second.arrived();
    until_claimed_by(&world, &stops, "stops");
    successor
        .run_from(
            &successor.project,
            &["start", &stops, "--detach", "--take-over"],
        )
        .exited(0);
    let theirs_too = second.arrived();
    world.until("the successor's claim to reach the store", |world| {
        stored_claim(world, &stops)["host"] == "host-b"
    });
    let claimed = stored_claim(&world, &stops);
    world.run(&["stop", "stops"]).exited(0);
    assert_eq!(
        stored_claim(&world, &stops),
        claimed,
        "the stop took the claim"
    );
    drop(held);

    theirs.release();
    theirs_too.release();
    for (run, project) in [("closes", &closes), ("stops", &stops)] {
        successor.until("the successor to close out", |world| {
            world.run_file(run, "result.json").is_file()
        });
        world.until("the successor to release its claim", |world| {
            stored_claim(world, project) == Value::Null
        });
    }
    drop(third);
    drop(successor);
}
