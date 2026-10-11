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

use crate::draft_lifecycle::{rules, team_repository, GREEN};
use crate::harness::{agent, held_publication, lifecycle, plan_of, World};

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

/// A project whose published node landed carries the landing keys the write-back projects
/// — `onepipeline.landing`, `onepipeline.landing_commit` — and a relaunch reads past them
/// rather than refusing them as node fields, continuing the landed node; `--fresh` runs it
/// again, since nothing it holds is a change request a second run would duplicate.
#[test]
fn a_landed_project_relaunches_continuing_it_and_runs_afresh_with_fresh() {
    let world = World::new("continue-landed");
    world.repository("local-direct", &[]);
    world.script("work.work-anew", "the work");
    let run = "landed";
    let project = author(&world, run, vec![lifecycle("work", &[])]);
    launch(&world, run, &project, &[]);
    let item = item_metadata(&world, &project, "work");
    assert_eq!(item["onepipeline.landing"], "landed", "{item}");
    assert!(item["onepipeline.landing_commit"].is_string(), "{item}");

    let again = format!("{run}-2");
    launch(&world, &again, &project, &[]).exited(0);
    assert_eq!(dispatched(&world, &again), Vec::<String>::new());
    assert_eq!(
        settled(&world, &again, "work")["continued"]["source"],
        "store"
    );

    let fresh = format!("{run}-3");
    launch(&world, &fresh, &project, &["--fresh"]).exited(0);
    assert_eq!(dispatched(&world, &fresh), vec!["work".to_owned()]);
    assert!(settled(&world, &fresh, "work").get("continued").is_none());
    assert_eq!(world.run_json(&fresh, "result.json")["state"], "complete");
}

/// `--fresh` over a project holding a draft nothing has landed is refused, naming the node
/// and its change request, before anything is minted: running it again would open a second.
#[test]
fn fresh_is_refused_while_a_node_holds_a_change_request_nothing_has_landed() {
    let world = team_world("continue-fresh-refused");
    world.script("a.work", "a's change\n");
    let run = "unlanded";
    let project = author(&world, run, vec![lifecycle("a", &[])]);
    launch(&world, run, &project, &[]);
    let url = stored_settlements(&world, &project)["a"]["change_url"]
        .as_str()
        .expect("a's change request")
        .to_owned();
    let refused = world.run_from(&world.project, &["start", &project, "--attach", "--fresh"]);
    refused.exited(2);
    assert!(
        refused.stderr.contains("--fresh refused:")
            && refused.stderr.contains(&format!("'a' ({url})")),
        "{}",
        refused.stderr
    );
    assert!(
        !world.run_file("unlanded-2", "launch.json").exists(),
        "the refusal minted a run"
    );
}

/// The claim the store holds for `project` once the host `host` has written it.
fn until_claimed_on(world: &World, project: &str, host: &str) -> Value {
    world.until(&format!("{host}'s claim to reach the store"), |world| {
        stored_claim(world, project)["host"] == host
    });
    stored_claim(world, project)
}

/// What a superseded driver recorded: the one `driver-superseded`, naming the claim that
/// superseded it, and the non-blocking surface beside it.
fn superseded_by(world: &World, run: &str, claim: &Value) {
    let surfaced = |world: &World| -> Vec<Value> {
        world
            .events_of(run, "planner-surface-queued")
            .into_iter()
            .filter(|event| event["payload"]["kind"] == "driver-superseded")
            .collect()
    };
    // The record first and the surface after it, so a reader holding one may not hold the
    // other yet.
    world.until("the superseded driver to raise its surface", |world| {
        !surfaced(world).is_empty()
    });
    let records = world.events_of(run, "driver-superseded");
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["payload"]["by"], *claim, "{records:?}");
    let surfaced = surfaced(world);
    assert_eq!(surfaced.len(), 1, "{surfaced:?}");
    assert_eq!(surfaced[0]["payload"]["blocking"], false, "{surfaced:?}");
}

/// Every branch the shared host was asked to open a change request from, at most once.
fn one_change_request_per_branch(world: &World) {
    let created = created_from(world);
    let distinct: std::collections::BTreeSet<&String> = created.iter().collect();
    assert_eq!(
        distinct.len(),
        created.len(),
        "a branch got two change requests: {created:?}"
    );
}

/// A driver with a node **ready to dispatch** when a host took its project over reads the
/// claim at the start of the pass that would dispatch it, dispatches nothing, records why,
/// and ends; the host that took over dispatches it, and publishes the only change request
/// for its branch.
#[test]
fn a_superseded_driver_reads_the_claim_at_the_pass_that_would_dispatch_and_dispatches_nothing() {
    let world = team_world("superseded-ready");
    world.script("next.work", "next's change\n");
    let gate = world.rendezvous("gate");
    let project = launch_detached(
        &world,
        "ready",
        vec![agent("gate", &[]), lifecycle("next", &["gate"])],
    );
    let mine = gate.arrived();
    until_claimed_by(&world, &project, "ready");
    let successor = world.on_host("host-b");
    successor
        .run_from(
            &successor.project,
            &["start", &project, "--detach", "--take-over"],
        )
        .exited(0);
    let theirs = gate.arrived();
    let claim = until_claimed_on(&world, &project, "host-b");

    // `next` becomes ready in the first driver the moment `gate` settles there.
    mine.release();
    world.until("the superseded driver to end", |world| {
        world.run_file("ready", "result.json").is_file()
    });
    superseded_by(&world, "ready", &claim);
    assert_eq!(dispatched(&world, "ready"), vec!["gate".to_owned()]);

    theirs.release();
    successor.until("the successor to settle", |world| {
        world.run_file("ready", "result.json").is_file()
    });
    assert_eq!(
        dispatched(&successor, "ready"),
        vec!["gate".to_owned(), "next".to_owned()]
    );
    assert_eq!(
        settled(&successor, "ready", "next")["outcome"],
        "change-review-draft"
    );
    one_change_request_per_branch(&world);
    assert_eq!(created_from(&world).len(), 1);
    drop(successor);
}

/// A pass that dispatches three nodes reads the project's claim once, for all three: the
/// store is asked for the project once between the moment they became ready and the moment
/// all three were dispatched.
#[test]
fn a_pass_that_dispatches_three_nodes_reads_the_claim_once() {
    let world = World::new("claim-read-once").through_scripted_source();
    for id in ["x", "y", "z"] {
        world.script(&format!("{id}.wait"), "hold");
    }
    let gate = world.rendezvous("gate");
    let mut plan = plan_of(
        "once",
        vec![
            agent("gate", &[]),
            agent("x", &["gate"]),
            agent("y", &["gate"]),
            agent("z", &["gate"]),
        ],
    );
    plan["concurrency"] = json!(3);
    let project = world.plan("once", &plan);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let held = gate.arrived();
    until_claimed_by(&world, &project, "once");
    let before = world.store_asked("get_project");
    held.release();
    world.until("x, y and z to be dispatched", |world| {
        dispatched(world, "once").len() == 4
    });
    assert_eq!(
        world.store_asked("get_project") - before,
        1,
        "the claim was not read exactly once for the pass that dispatched three nodes"
    );
    for id in ["x", "y", "z"] {
        world.release(&format!("{id}.go"));
    }
    world.until("the run to settle", |world| {
        world.run_file("once", "result.json").is_file()
    });
}

/// A driver whose node's **publication** falls in the pass after a takeover reads the claim
/// at the start of that pass and publishes nothing — the node settles `cancelled`, its work
/// kept rather than published — while a publication the pass before the takeover cleared,
/// already under way at the host, completes and adopts the change request open from its
/// branch. The host that took over continues the in-flight node on its branch; the shared
/// host is asked for one change request per branch across both drivers.
#[test]
fn a_superseded_driver_publishes_nothing_after_the_check_but_finishes_a_publication_under_way() {
    let world = World::new("superseded-publication")
        .with_env("ONEVCS_BRANCH_PREFIX", PREFIX)
        .with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "1");
    let go = world.fakes.join("push.go");
    let held = held_publication(&world, &go);
    world.repository("change-open", &held.argv());
    rules(&world, "change-open", "required", None);
    world.script("gh.checks", GREEN);
    // `m` drafts once with its checks red, so the run under test continues it on its branch.
    world.script("gh.checks.1", RED);
    world.script("m.work-anew", "m's change");
    world.script("l.work", "l's change\n");
    held.release();
    let run = "pub";
    let project = author(&world, run, vec![lifecycle("m", &[])]);
    launch(&world, run, &project, &[]);
    let stored = stored_settlements(&world, &project);
    assert_eq!(stored["m"]["status"], "failed", "{stored:#?}");
    let m_branch = stored["m"]["branch"]
        .as_str()
        .expect("m's branch")
        .to_owned();
    world.script("gh.checks.1", GREEN);
    world.add_task(run, "001-l", &lifecycle("l", &[]), &[]);
    let mut plan = world.store_project(&project)["items"][0]["item"]["metadata"].clone();
    plan["onepipeline.concurrency"] = json!(2);
    let key = onetaskgraph_plugin_api::MetadataKey::new("onepipeline.concurrency".to_owned())
        .expect("a key");
    let id: onetaskgraph_core::GlobalId = project.parse().expect("a qualified project");
    world
        .store_call(|engine| async move { engine.set_project_metadata(&id, &key, &json!(2)).await })
        .unwrap_or_else(|why| panic!("the store refused the write: {why}"));
    // Every push from this host is held again until the journey lets it through.
    for marker in [go.clone(), go.with_extension("go.holding")] {
        let _ = std::fs::remove_file(marker);
    }
    let m = world.rendezvous("m");
    let l = world.rendezvous("l");
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let driving = format!("{run}-2");
    m.arrived().release();
    let mine_l = l.arrived();
    // `m`'s publication was cleared and is at the host, held mid-push.
    world.until("m's publication to be under way", |_| held.holding());
    world.until("both dispatches to reach the board", |world| {
        ["m", "l"].iter().all(|node| {
            item_metadata(world, &project, node)
                .get("onepipeline.dispatch")
                .is_some()
        })
    });
    let l_branch = item_metadata(&world, &project, "l")["onepipeline.dispatch"]["branch"]
        .as_str()
        .expect("l's branch")
        .to_owned();
    let created = created_from(&world);

    let successor = world.on_host("host-b");
    successor
        .run_from(
            &successor.project,
            &["start", &project, "--detach", "--take-over"],
        )
        .exited(0);
    let theirs_m = m.arrived();
    let theirs_l = l.arrived();
    let claim = until_claimed_on(&world, &project, "host-b");
    // The successor continues `l` on the branch the board named, and drafts it.
    theirs_l.release();
    successor.until("the successor to draft l", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "l")
    });
    assert_eq!(settled(&successor, run, "l")["branch"], json!(l_branch));
    assert_eq!(
        settled(&successor, run, "l")["outcome"],
        "change-review-draft"
    );

    // The publication under way completes, onto the draft already open from `m`'s branch.
    held.release();
    world.until("m's publication to complete", |world| {
        world
            .events_of(&driving, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "m")
    });
    let m_settled = settled(&world, &driving, "m");
    assert_eq!(m_settled["outcome"], "change-review-draft", "{m_settled}");
    assert_eq!(
        m_settled["change_url"], stored["m"]["change_url"],
        "{m_settled}"
    );

    // And `l`'s, which falls in the pass after the takeover, is never made.
    mine_l.release();
    world.until("the superseded driver to end", |world| {
        world.run_file(&driving, "result.json").is_file()
    });
    superseded_by(&world, &driving, &claim);
    let l_settled = settled(&world, &driving, "l");
    assert_eq!(l_settled["status"], "cancelled", "{l_settled}");
    assert!(
        l_settled["detail"]
            .as_str()
            .is_some_and(|detail| detail.starts_with("not published")),
        "{l_settled}"
    );
    let mut now = created.clone();
    now.push(l_branch.clone());
    assert_eq!(created_from(&world), now, "{:?}", world.changes_opened());
    one_change_request_per_branch(&world);
    let drafts = changes_from(&world, &m_branch);
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    assert!(held_as_draft(&world, &drafts[0]["number"]), "{drafts:?}");

    theirs_m.release();
    successor.until("the successor to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(sessions_opened(&successor, run), {
        let mut both = vec![m_branch.clone(), l_branch.clone()];
        both.sort();
        both
    });
    one_change_request_per_branch(&world);
    drop(successor);
    drop(held);
}

/// A driver with nothing to dispatch or publish notices a takeover within one idle check —
/// shortened here through the test seam entry 116 names — records it, and ends once its
/// dispatch in flight has settled unpublished; the host that took over continues that node
/// on its branch, and the shared host is asked for one change request for it.
#[test]
fn an_idle_superseded_driver_notices_within_one_idle_check_and_publishes_nothing() {
    let world = team_world("superseded-idle").with_env("ONEPIPELINE_IDLE_CLAIM_CHECK_SECONDS", "2");
    world.script("l.work", "l's change\n");
    let l = world.rendezvous("l");
    let project = launch_detached(&world, "idle", vec![lifecycle("l", &[])]);
    let mine = l.arrived();
    until_claimed_by(&world, &project, "idle");
    world.until("l's dispatch to reach the board", |world| {
        item_metadata(world, &project, "l")
            .get("onepipeline.dispatch")
            .is_some()
    });
    let branch = item_metadata(&world, &project, "l")["onepipeline.dispatch"]["branch"]
        .as_str()
        .expect("l's branch")
        .to_owned();
    let successor = world.on_host("host-b");
    successor
        .run_from(
            &successor.project,
            &["start", &project, "--detach", "--take-over"],
        )
        .exited(0);
    let theirs = l.arrived();
    let claim = until_claimed_on(&world, &project, "host-b");
    let since = std::time::Instant::now();
    world.until("the idle driver to notice the takeover", |world| {
        !world.events_of("idle", "driver-superseded").is_empty()
    });
    assert!(
        since.elapsed() < std::time::Duration::from_secs(2 * 2 + 10),
        "the idle driver took {:?} to notice",
        since.elapsed()
    );
    superseded_by(&world, "idle", &claim);

    theirs.release();
    successor.until("the successor to settle", |world| {
        world.run_file("idle", "result.json").is_file()
    });
    assert_eq!(sessions_opened(&successor, "idle"), vec![branch.clone()]);
    assert_eq!(
        settled(&successor, "idle", "l")["outcome"],
        "change-review-draft"
    );

    mine.release();
    world.until("the superseded driver to end", |world| {
        world.run_file("idle", "result.json").is_file()
    });
    assert_eq!(settled(&world, "idle", "l")["status"], "cancelled");
    assert_eq!(changes_from(&world, &branch).len(), 1);
    one_change_request_per_branch(&world);
    drop(successor);
}

/// Two cold hosts continue one project whose nodes hold open drafts, launched at the same
/// moment over the one store: neither opens a change request, each draft is still the only
/// one for its branch and still a draft, and once each driver has made its next claim check
/// each has been refused, ended superseded, or is the one driver left — exactly one.
#[test]
fn two_cold_hosts_launching_one_project_at_once_leave_one_driver_and_every_draft_alone() {
    let world = team_world("continue-race");
    for id in ["a", "b", "c"] {
        world.script(&format!("{id}.work"), &format!("{id}'s change\n"));
    }
    let run = "race";
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
    let drafts = world.changes_opened();
    let created = created_from(&world);
    // A node a planner added that keeps a driver driving, so each has a claim to check.
    world.add_task(run, "003-hold", &agent("hold", &[]), &[]);
    world.script("hold.wait", "hold");
    std::fs::remove_dir_all(&world.runs).expect("the runs root is removed");

    let hosts: Vec<World> = ["host-b", "host-c"]
        .into_iter()
        .map(|host| {
            world
                .on_host(host)
                .with_env("ONEPIPELINE_IDLE_CLAIM_CHECK_SECONDS", "2")
        })
        .collect();
    let launches: Vec<std::process::Child> = hosts
        .iter()
        .map(|host| {
            let mut command = host.cmd(&["start", &project, "--detach"]);
            command
                .current_dir(&host.project)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            command.spawn().expect("the launch starts")
        })
        .collect();
    let answered: Vec<std::process::Output> = launches
        .into_iter()
        .map(|child| child.wait_with_output().expect("the launch answers"))
        .collect();
    let launched: Vec<bool> = answered
        .iter()
        .map(|output| match output.status.code() {
            Some(0) => true,
            Some(2) => {
                let said = String::from_utf8_lossy(&output.stderr);
                assert!(said.contains("project driven elsewhere refused:"), "{said}");
                false
            }
            other => panic!(
                "a launch exited {other:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        })
        .collect();
    assert!(
        launched.iter().any(|launched| *launched),
        "both launches were refused"
    );
    // Two idle checks and some: every driver has read the claim at least once since both
    // claims could have reached the store.
    std::thread::sleep(std::time::Duration::from_secs(6));
    let left: Vec<&World> = hosts
        .iter()
        .zip(&launched)
        .filter(|(_, launched)| **launched)
        .map(|(host, _)| host)
        .filter(|host| host.events_of(run, "driver-superseded").is_empty())
        .collect();
    assert_eq!(left.len(), 1, "not exactly one driver is left driving");
    let claim = stored_claim(&world, &project);
    let host_of = |host: &World| {
        host.environment
            .iter()
            .find(|(key, _)| key == "HOSTNAME")
            .map(|(_, name)| json!(name))
            .expect("a host world names its host")
    };
    assert_eq!(
        claim["host"],
        host_of(left[0]),
        "the store's claim is not the driver left's"
    );

    world.release("hold.go");
    for (host, launched) in hosts.iter().zip(&launched) {
        if *launched {
            host.until("each driver to end", |host| {
                host.run_file(run, "result.json").is_file()
            });
        }
    }
    assert_eq!(created_from(&world), created, "a change request was opened");
    for draft in &drafts {
        let head = draft["head"].as_str().expect("a head");
        assert_eq!(changes_from(&world, head).len(), 1, "{head}");
        assert!(held_as_draft(&world, &draft["number"]), "{draft}");
    }
}
