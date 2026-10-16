//! A project continued from its store alone, and the best-effort claim on who drives it.
//!
//! A host holding none of a run's local state — its runs root gone, a fresh `onevcs`
//! state root — launches a project again and carries it on from what the plans store
//! holds: each lineage's `onepipeline.settlement`, a node's open `onepipeline.dispatch`,
//! and the project's `onepipeline.driver`. Entry 119 of `docs/contract-divergences.md`.
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
    // Each published node's item carries the write-back's landing keys beside its settlement,
    // and the project still reads as a plan.
    for id in ["a", "b", "c"] {
        let item = item_metadata(&world, &project, id);
        assert_eq!(
            item["onepipeline.change_url"], stored[id]["change_url"],
            "{item}"
        );
        assert_eq!(item["onepipeline.landing"], "unlanded", "{item}");
    }
    world
        .run_from(&world.project, &["plan", "check", &project])
        .exited(0);

    // The host that ran it is gone: its runs root with it, and the cold host has its own.
    std::fs::remove_dir_all(&world.runs).expect("the runs root is removed");
    let cold = world.on_host("host-b");
    let launched = launch(&cold, run, &project, &[]);
    launched.exited(0);
    // The launch says what it took from the store, and pins nothing it did not.
    launched.err_has(&format!(
        "onepipeline: launch '{run}' continues 3 settled nodes from the store\n"
    ));

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
    let line = "launched on this host from the store: 3 nodes continued, 1 node dispatched";
    cold.run(&["status", run]).exited(0).out_has(line);
    cold.run(&["results", run]).exited(0).out_has(line);
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
    launch(&cold, run, project, &[]).exited(0).err_has(&format!(
        "onepipeline: launch '{run}' continues 1 settled node from the store"
    ));
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

/// A node a manager cancelled while its publication was being retried — its draft already
/// open, its checks red — settles `cancelled` with its branch and that draft, and the board
/// shows it `parked`: a cold host continues it pinned to that branch, and its draft adopted.
#[test]
fn a_cancelled_node_is_continued_pinned_to_its_branch_and_adopts_its_draft() {
    let run = "cancelled";
    let world = team_world("continue-cancelled")
        .with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "2")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    world.script("a.work", "a's change\n");
    world.script("b.work-anew", "b's change");
    world.script("gh.checks.2", RED);
    let b_dispatches = world.rendezvous("b");
    let project = author(
        &world,
        run,
        vec![lifecycle("a", &[]), lifecycle("b", &["a"])],
    );
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    // The first dispatch publishes, its checks red; the retry is held, and cancelled.
    b_dispatches.arrived().release();
    let retry = b_dispatches.arrived();
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "cancel", "id": "b"}]}).to_string(),
        )
        .exited(0);
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    drop(retry);
    let stored = stored_settlements(&world, &project);
    assert_eq!(stored["b"]["status"], "cancelled", "{stored:#?}");
    for field in ["branch", "change_url"] {
        assert!(
            stored["b"][field].is_string(),
            "b stored no {field}: {stored:#?}"
        );
    }
    let item = world
        .store_tasks(&project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == "b")
        .expect("b's item");
    assert_eq!(item["item"]["status"]["name"], "parked", "{item}");
    world.unscript("b.rendezvous");

    // The park is the manager's, and the board carries it: a host continuing the project
    // leaves the node parked until somebody requeues it — and then continues its branch.
    let branch = stored["b"]["branch"]
        .as_str()
        .expect("b's branch")
        .to_owned();
    let created = created_from(&world);
    world.script("gh.checks.2", GREEN);
    std::fs::remove_dir_all(&world.runs).expect("the runs root is removed");
    let cold = world.on_host("host-b");
    let continued = cold.run_from(&cold.project, &["start", &project, "--attach"]);
    assert!(
        cold.run_file(run, "result.json").is_file(),
        "the continuation did not settle: {}",
        continued.stderr
    );
    assert_eq!(
        dispatched(&cold, run),
        Vec::<String>::new(),
        "a parked node was dispatched"
    );
    assert_eq!(settled(&cold, run, "a")["continued"]["source"], "store");
    cold.run_with_stdin(
        &["reply", run],
        &json!({"version": 2, "commands": [{"op": "requeue", "id": "b"}]}).to_string(),
    )
    .exited(0);
    cold.run_from(&cold.project, &["adopt", run]).settled();
    assert_eq!(dispatched(&cold, run), vec!["b".to_owned()]);
    assert_eq!(sessions_opened(&cold, run), vec![branch.clone()]);
    assert_eq!(created_from(&world), created, "a change request was opened");
    let b = settled(&cold, run, "b");
    assert_eq!(b["outcome"], "change-review-draft", "{b}");
    assert_eq!(b["change_url"], stored["b"]["change_url"], "{b}");
    let held = changes_from(&world, &branch);
    assert_eq!(held.len(), 1, "{held:?}");
    assert!(held_as_draft(&world, &held[0]["number"]), "{held:?}");
    drop(cold);
}

/// A node whose stored settlement says `parked` with a branch and a draft is continued the
/// same way.
// llmlint: ignore-block[tests_mirror_real_usage] no build of this crate settles a node
// `parked` — a park is derived, and the dispatch a `cancel` ends settles `cancelled`, which
// the journey above produces through the CLI — so the one way a store holds a `parked`
// settlement is a writer other than this build: the store is external input, and stating the
// document that writer left is the journey's whole subject.
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
// llmlint: ignore-end[tests_mirror_real_usage]

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
    // Its driver is gone, which nothing on this host can prove: the takeover waits out its
    // wait, shortened here through the claim interval.
    let cold = world
        .on_host("host-b")
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
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
    // What it continued, it continued from the run whose claim it took over.
    cold.run(&["status", run]).exited(0).out_has(&format!(
        "launched on this host from the store: 1 node continued from run '{held}', 1 node \
         dispatched"
    ));
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
    // A run of this host nothing drives has let go, which this host's own records prove:
    // the takeover writes nothing first and waits on nothing.
    let taken = world.run_from(
        &world.project,
        &["start", &project, "--attach", "--take-over"],
    );
    taken.exited(0);
    assert!(
        !taken.stderr.contains("--take-over claimed"),
        "{}",
        taken.stderr
    );
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
    let world = World::new("claim-successor")
        .with_env(CLAIM_INTERVAL_ENV, "10")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let first = world.rendezvous("first");
    let after = world.rendezvous("after");
    let closes = launch_detached(
        &world,
        "closes",
        vec![agent("first", &[]), agent("after", &["first"])],
    );
    let mine = first.arrived();
    until_claimed_by(&world, &closes, "closes");
    let successor = world.on_host("host-b");
    let taking = take_over_in_background(&successor, &closes);
    let claimed = until_claimed_on(&world, &closes, "host-b");
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
    // The run taken over finishes its one dispatch and closes out with nothing left to
    // start, and only then does the successor start: with `after`.
    world.until("the first driver to read the takeover", |world| {
        !world.events_of("closes", "driver-superseded").is_empty()
    });
    mine.release();
    world.until("the run taken over to close out", |world| {
        world.run_file("closes", "result.json").is_file()
    });
    launched_over(taking);
    let theirs = after.arrived();
    let now = stored_claim(&world, &closes);
    assert_eq!(
        (&now["run"], &now["host"]),
        (&claimed["run"], &claimed["host"]),
        "the closeout took the claim"
    );
    theirs.release();
    successor.until("the successor to close out", |world| {
        world.run_file("closes", "result.json").is_file()
    });
    world.until("the successor to release its claim", |world| {
        stored_claim(world, &closes) == Value::Null
    });
    drop(third);
    drop(successor);

    // And one stopped while its successor waits on it: the stop leaves the successor's
    // claim, and the successor, which nothing acknowledged, starts once its wait is over.
    let world = World::new("claim-successor-stop");
    let held = world.rendezvous("held");
    let stops = launch_detached(&world, "stops", vec![agent("held", &[])]);
    let mine = held.arrived();
    until_claimed_by(&world, &stops, "stops");
    let successor = world
        .on_host("host-b")
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let taking = take_over_in_background(&successor, &stops);
    let claimed = until_claimed_on(&world, &stops, "host-b");
    world.run(&["stop", "stops"]).exited(0);
    assert_eq!(
        stored_claim(&world, &stops),
        claimed,
        "the stop took the claim"
    );
    drop(mine);
    let said = launched_over(taking);
    assert!(
        said.contains(&format!(
            "has not acknowledged in {}s",
            takeover_wait(5).as_secs()
        )),
        "{said}"
    );
    held.arrived().release();
    successor.until("the successor to close out", |world| {
        world.run_file("stops", "result.json").is_file()
    });
    world.until("the successor to release its claim", |world| {
        stored_claim(world, &stops) == Value::Null
    });
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

    // A dispatch the store still holds is part of what `--fresh` ignores: the node is cut a
    // branch of its own rather than pinned to the stale one.
    restate(
        &world,
        &project,
        "work",
        "onepipeline.dispatch",
        json!({"branch": "stale/work", "base": "main", "opened_at": "2026-10-11T00:00:00.000Z"}),
    );
    let fresh = format!("{run}-3");
    launch(&world, &fresh, &project, &["--fresh"]).exited(0);
    assert_eq!(dispatched(&world, &fresh), vec!["work".to_owned()]);
    let opened = sessions_opened(&world, &fresh);
    assert!(
        !opened.is_empty() && !opened.contains(&"stale/work".to_owned()),
        "--fresh pinned the node to the stored dispatch: {opened:?}"
    );
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

/// A driver with a node **ready to dispatch** once a host has taken its project over reads
/// the takeover at its next read and dispatches nothing, records why, and ends once the
/// dispatch it had in flight has settled — and only then acknowledges the claim. The host
/// that took over started nothing until then; it continues what that driver settled,
/// dispatches what is left, and publishes the only change request for its branch.
#[test]
fn a_superseded_driver_starts_nothing_once_it_has_read_the_takeover() {
    let interval = 10;
    let world = takeover_world("superseded-ready", interval);
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
    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    world.until("the driver to read the takeover", |world| {
        !world.events_of("ready", "driver-superseded").is_empty()
    });

    // `next` becomes ready in the first driver the moment `gate` settles there.
    mine.release();
    world.until("the superseded driver to end", |world| {
        world.run_file("ready", "result.json").is_file()
    });
    superseded_by(&world, "ready", &claim);
    assert_eq!(dispatched(&world, "ready"), vec!["gate".to_owned()]);
    let said = launched_over(taking);
    assert!(said.contains("acknowledged the takeover"), "{said}");

    successor.until("the successor to settle", |world| {
        world.run_file("ready", "result.json").is_file()
    });
    assert_eq!(dispatched(&successor, "ready"), vec!["next".to_owned()]);
    assert_eq!(
        settled(&successor, "ready", "gate")["continued"]["source"],
        "store"
    );
    assert_eq!(
        settled(&successor, "ready", "next")["outcome"],
        "change-review-draft"
    );
    handed_over(&world, &successor, "ready");
    one_change_request_per_branch(&world);
    assert_eq!(created_from(&world).len(), 1);
    drop(successor);
}

/// A pass that dispatches three nodes asks the store nothing for them: each starts on the
/// lease the driver's launch read gave, so the store is asked for the project not once
/// between the moment they became ready and the moment all three were dispatched.
#[test]
fn a_pass_that_dispatches_three_nodes_reads_the_store_for_none_of_them() {
    let world = World::new("claim-read-none").through_scripted_source();
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
        0,
        "the store was read for the claim to dispatch three nodes"
    );
    for id in ["x", "y", "z"] {
        world.release(&format!("{id}.go"));
    }
    world.until("the run to settle", |world| {
        world.run_file("once", "result.json").is_file()
    });
}

/// A driver whose dispatch is in flight notices a takeover within one claim interval —
/// shortened here through the setting entry 127 names — records it, and acknowledges only
/// once that dispatch has settled unpublished; the host that took over, which started
/// nothing until then, continues the node on its branch, and the shared host is asked for
/// one change request for it.
#[test]
fn a_superseded_driver_notices_within_one_interval_and_publishes_nothing() {
    let interval = 10;
    let world = takeover_world("superseded-interval", interval);
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
    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    let since = std::time::Instant::now();
    world.until("the driver to notice the takeover", |world| {
        !world.events_of("idle", "driver-superseded").is_empty()
    });
    assert!(
        since.elapsed() < std::time::Duration::from_secs(interval + 10),
        "the driver took {:?} to notice",
        since.elapsed()
    );
    superseded_by(&world, "idle", &claim);

    mine.release();
    world.until("the superseded driver to end", |world| {
        world.run_file("idle", "result.json").is_file()
    });
    let withheld = settled(&world, "idle", "l");
    assert_eq!(withheld["status"], "cancelled", "{withheld}");
    assert!(
        withheld["detail"]
            .as_str()
            .is_some_and(|detail| detail.starts_with("not published")),
        "{withheld}"
    );
    let said = launched_over(taking);
    assert!(said.contains("acknowledged the takeover"), "{said}");
    let theirs = l.arrived();
    theirs.release();
    successor.until("the successor to settle", |world| {
        world.run_file("idle", "result.json").is_file()
    });
    assert_eq!(sessions_opened(&successor, "idle"), vec![branch.clone()]);
    assert_eq!(
        settled(&successor, "idle", "l")["outcome"],
        "change-review-draft"
    );
    assert_eq!(changes_from(&world, &branch).len(), 1);
    one_change_request_per_branch(&world);
    drop(successor);
}

/// The setting naming the claim interval every host of a journey is given, in whole seconds.
const CLAIM_INTERVAL_ENV: &str = "ONEPIPELINE_CLAIM_INTERVAL_SECONDS";

/// The failpoint met before each read of the claim.
const READ_GATE: &str = "ONEPIPELINE_TEST_CLAIM_READ_GATE";

/// The failpoint met before a dispatch of the node [`BEGIN_NODE`] names would start.
const BEGIN_GATE: &str = "ONEPIPELINE_TEST_CLAIM_BEGIN_GATE";
const BEGIN_NODE: &str = "ONEPIPELINE_TEST_CLAIM_BEGIN_NODE";

/// A team world whose every host reads the claim every `interval` seconds and gives a
/// cancelled dispatch one second, so a takeover nothing acknowledges waits
/// `2 × interval + 31` seconds: two intervals, a read's twenty-second deadline, that
/// second, and ten seconds' slack.
fn takeover_world(name: &str, interval: u64) -> World {
    team_world(name)
        .with_env(CLAIM_INTERVAL_ENV, &interval.to_string())
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1")
}

/// How long a takeover nothing acknowledges waits in a [`takeover_world`].
fn takeover_wait(interval: u64) -> std::time::Duration {
    std::time::Duration::from_secs(2 * interval + 20 + 1 + 10)
}

/// `host` taking `project` over, in the background: the launch returns only once the driver
/// it displaces has let go, which is what a journey arranges while it waits.
fn take_over_in_background(host: &World, project: &str) -> std::process::Child {
    let mut command = host.cmd(&["start", project, "--detach", "--take-over"]);
    command
        .current_dir(&host.project)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command.spawn().expect("the takeover starts")
}

/// A background takeover's answer: it launched, and what it said on its way.
fn launched_over(taking: std::process::Child) -> String {
    let answered = taking.wait_with_output().expect("the takeover answers");
    let said = String::from_utf8_lossy(&answered.stderr).into_owned();
    assert_eq!(
        answered.status.code(),
        Some(0),
        "the takeover failed:\n{said}"
    );
    said
}

/// The record of a takeover, out of both drivers' journals of `run`: [`in_turn`], and no
/// node dispatched by both.
fn handed_over(displaced: &World, adopter: &World, run: &str) {
    in_turn(displaced, adopter, run);
    let mut both = dispatched(displaced, run);
    both.extend(dispatched(adopter, run));
    let distinct: std::collections::BTreeSet<&String> = both.iter().collect();
    assert_eq!(
        distinct.len(),
        both.len(),
        "a node was dispatched twice: {both:?}"
    );
}

/// Every dispatch the displaced driver started settled before the first one the adopter
/// started — so at no moment did both have a dispatch, or the publication a dispatch makes,
/// in progress — and the displaced driver started none after it read the takeover.
fn in_turn(displaced: &World, adopter: &World, run: &str) {
    let at = |event: &Value| event["ts"].as_str().expect("a record's time").to_owned();
    let node = |event: &Value| event["labels"]["node"].as_str().map(str::to_owned);
    let journal = displaced.journal(run);
    let dispatches: Vec<&Value> = journal
        .iter()
        .filter(|event| event["kind"] == "node-dispatched")
        .collect();
    let settlements: Vec<&Value> = journal
        .iter()
        .filter(|event| event["kind"] == "node-settled")
        .collect();
    let mut ended = String::new();
    for dispatch in &dispatches {
        let settled = settlements
            .iter()
            .filter(|settled| node(settled) == node(dispatch) && at(settled) >= at(dispatch))
            .map(|settled| at(settled))
            .min()
            .unwrap_or_else(|| {
                panic!(
                    "the displaced driver's dispatch of {:?} never settled: {journal:#?}",
                    node(dispatch)
                )
            });
        ended = ended.max(settled);
    }
    if let Some(read) = displaced.events_of(run, "driver-superseded").first() {
        for dispatch in &dispatches {
            assert!(
                at(dispatch) <= at(read),
                "the displaced driver dispatched {:?} after it read the takeover",
                node(dispatch)
            );
        }
    }
    if let Some(began) = adopter
        .events_of(run, "node-dispatched")
        .iter()
        .map(at)
        .min()
    {
        assert!(
            ended <= began,
            "the adopter dispatched at {began}, before the displaced driver's last dispatch \
             settled at {ended}"
        );
    }
}

/// A driver with nothing in progress when a host takes its project over — its last read
/// refused by the board, so it starts nothing on a lease an interval old, and the node
/// that became ready waits — reads the takeover at its next read, and acknowledges it on
/// the claim; the adopter, which started nothing until then, starts on that
/// acknowledgement well inside one claim interval of its claim landing. Each node is
/// dispatched and published once, by one driver.
#[test]
fn a_takeover_of_an_idle_driver_starts_on_its_acknowledgement_within_the_interval() {
    let interval = 30;
    let base = takeover_world("takeover-idle", interval).through_scripted_source();
    base.script("y.work", "y's change\n");
    let successor = base.on_host("host-b");
    let reads = crate::harness::Rendezvous::failpoint();
    let world = base.with_env(READ_GATE, &reads.address());
    let x = world.rendezvous("x");
    let run = "idle";
    let project = author(&world, run, vec![agent("x", &[]), lifecycle("y", &["x"])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    // The read an interval after launch is refused by the board: the lease it would have
    // renewed runs out.
    world.store_refuses_once(
        "get_project",
        &json!({"kind": "unavailable", "message": "the board is down"}),
    );
    reads.arrived().release();
    world.until_run_file_holds(run, "driver.log", "the board is down");
    // x finishes; y is ready, and the driver starts nothing on a lease an interval old.
    mine.release();
    world.until("x to settle", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "x")
    });
    // Its next read, an interval on, finds the driver idle: held there while the takeover
    // lands.
    let read = reads.arrived();
    assert_eq!(dispatched(&world, run), vec!["x".to_owned()]);
    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    let landed = std::time::Instant::now();
    assert!(
        successor.run_file(run, "launch.json").metadata().is_err(),
        "the adopter minted its run before the driver it displaced had let go"
    );
    read.release();
    let said = launched_over(taking);
    assert!(
        landed.elapsed() < std::time::Duration::from_secs(interval),
        "the adopter started {:?} after its claim landed, not on the acknowledgement",
        landed.elapsed()
    );
    assert!(said.contains("acknowledged the takeover"), "{said}");
    superseded_by(&world, run, &claim);
    world.until_run_file_holds(
        run,
        "driver.log",
        "acknowledged run 'idle' on host 'host-b'",
    );

    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(dispatched(&world, run), vec!["x".to_owned()]);
    assert_eq!(dispatched(&successor, run), vec!["y".to_owned()]);
    assert_eq!(
        settled(&successor, run, "x")["continued"]["source"],
        "store"
    );
    handed_over(&world, &successor, run);
    one_change_request_per_branch(&world);
    assert_eq!(created_from(&world).len(), 1);
    drop(successor);
}

/// A takeover that lands while the displaced driver has a dispatch in progress starts
/// nothing until that driver has let go: the dispatch, started on the read its launch
/// made, finishes and starts its publication inside that lease — held at the push past the
/// driver's next read, which finds the takeover — and only once it has finished does the
/// driver acknowledge. The adopter continues what it settled and dispatches only what is
/// left.
#[test]
fn a_takeover_while_the_displaced_driver_dispatches_starts_once_it_has_finished() {
    let interval = 30;
    let world = World::new("takeover-dispatching")
        .with_env("ONEVCS_BRANCH_PREFIX", PREFIX)
        .with_env(CLAIM_INTERVAL_ENV, &interval.to_string())
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let go = world.fakes.join("push.go");
    let held = held_publication(&world, &go);
    world.repository("change-open", &held.argv());
    rules(&world, "change-open", "required", None);
    world.script("gh.checks", GREEN);
    world.script("x.work", "x's change\n");
    world.script("y.work", "y's change\n");
    let successor = world.on_host("host-b");
    let x = world.rendezvous("x");
    let run = "dispatching";
    let project = author(
        &world,
        run,
        vec![lifecycle("x", &[]), lifecycle("y", &["x"])],
    );
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    // The takeover has landed with x in progress; x finishes inside the lease, and its
    // publication starts.
    mine.release();
    world.until("x's publication to be under way", |_| held.holding());
    world.until("the displaced driver to read the takeover", |world| {
        !world.events_of(run, "driver-superseded").is_empty()
    });
    superseded_by(&world, run, &claim);
    assert!(
        successor.run_file(run, "launch.json").metadata().is_err(),
        "the adopter minted its run while the displaced driver was still dispatching"
    );
    held.release();
    let said = launched_over(taking);
    assert!(said.contains("acknowledged the takeover"), "{said}");
    assert_eq!(settled(&world, run, "x")["outcome"], "change-review-draft");
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(dispatched(&world, run), vec!["x".to_owned()]);
    assert_eq!(dispatched(&successor, run), vec!["y".to_owned()]);
    assert_eq!(
        settled(&successor, run, "x")["continued"]["source"],
        "store"
    );
    handed_over(&world, &successor, run);
    one_change_request_per_branch(&world);
    assert_eq!(created_from(&world).len(), 2);
    drop(successor);
    drop(held);
}

/// A takeover that lands while the displaced driver's publication is under way — its push
/// held at the host — starts nothing while it runs: the driver reads the takeover at its
/// next read, starts nothing more, and acknowledges only once the publication has finished.
#[test]
fn a_takeover_while_the_displaced_driver_publishes_starts_once_the_publication_has_finished() {
    let interval = 10;
    let world = World::new("takeover-publishing")
        .with_env("ONEVCS_BRANCH_PREFIX", PREFIX)
        .with_env(CLAIM_INTERVAL_ENV, &interval.to_string())
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let go = world.fakes.join("push.go");
    let held = held_publication(&world, &go);
    world.repository("change-open", &held.argv());
    rules(&world, "change-open", "required", None);
    world.script("gh.checks", GREEN);
    world.script("x.work", "x's change\n");
    world.script("y.work", "y's change\n");
    let successor = world.on_host("host-b");
    let run = "publishing";
    let project = author(
        &world,
        run,
        vec![lifecycle("x", &[]), lifecycle("y", &["x"])],
    );
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    world.until("x's publication to be under way", |_| held.holding());
    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    world.until("the displaced driver to read the takeover", |world| {
        !world.events_of(run, "driver-superseded").is_empty()
    });
    superseded_by(&world, run, &claim);
    // An interval more, and the publication is still under way: nothing is acknowledged,
    // and the adopter has started nothing.
    std::thread::sleep(std::time::Duration::from_secs(interval));
    assert!(
        successor.run_file(run, "launch.json").metadata().is_err(),
        "the adopter minted its run while the displaced driver was publishing"
    );
    held.release();
    let said = launched_over(taking);
    assert!(said.contains("acknowledged the takeover"), "{said}");
    let x = settled(&world, run, "x");
    assert_eq!(x["outcome"], "change-review-draft", "{x}");
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(dispatched(&world, run), vec!["x".to_owned()]);
    assert_eq!(dispatched(&successor, run), vec!["y".to_owned()]);
    assert_eq!(settled(&successor, run, "x")["change_url"], x["change_url"]);
    handed_over(&world, &successor, run);
    one_change_request_per_branch(&world);
    assert_eq!(created_from(&world).len(), 2);
    drop(successor);
    drop(held);
}

/// A displaced driver held — at the moment it would start a dispatch — for longer than its
/// lease resumes after the adopter has begun, its dispatch of that node in progress, and
/// starts nothing on the read it took before the hold: it reads the claim again, finds the
/// takeover, and ends with nothing dispatched. The adopter, which nothing acknowledged,
/// started once its wait had passed.
#[test]
fn a_displaced_driver_held_past_its_lease_starts_nothing_when_it_resumes() {
    let interval = 5;
    let (world, successor, project, hold) = held_at_its_first_dispatch("takeover-held", interval);
    let run = "held";
    let x = world.rendezvous("x");
    let taking = take_over_in_background(&successor, &project);
    until_claimed_on(&world, &project, "host-b");
    let said = launched_over(taking);
    assert!(
        said.contains(&format!(
            "has not acknowledged in {}s",
            takeover_wait(interval).as_secs()
        )),
        "{said}"
    );
    // The adopter has begun: its dispatch of x is in progress when the displaced driver,
    // held long past its lease, resumes.
    let theirs = x.arrived();
    hold.release();
    world.until("the displaced driver to end", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert!(
        dispatched(&world, run).is_empty(),
        "the displaced driver dispatched on a lease that had run out"
    );
    let read = world.events_of(run, "driver-superseded");
    assert_eq!(read.len(), 1, "{read:?}");
    assert_eq!(read[0]["payload"]["by"]["host"], "host-b", "{read:?}");
    theirs.release();
    successor.until("the adopter to draft x", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(dispatched(&successor, run), vec!["x".to_owned()]);
    assert_eq!(
        settled(&successor, run, "x")["outcome"],
        "change-review-draft"
    );
    handed_over(&world, &successor, run);
    one_change_request_per_branch(&world);
    assert_eq!(created_from(&world).len(), 1);
    drop(successor);
}

/// A displaced driver held past its lease until the adopter has finished — and so released
/// the claim — reads no claim where its own had landed, and starts nothing on that read:
/// what it would start, the adopter already did.
#[test]
fn a_displaced_driver_resuming_after_its_adopter_finished_starts_nothing() {
    let interval = 5;
    let (world, successor, project, hold) = held_at_its_first_dispatch("takeover-gone", interval);
    let run = "gone";
    let taking = take_over_in_background(&successor, &project);
    until_claimed_on(&world, &project, "host-b");
    launched_over(taking);
    successor.until("the adopter to draft x", |world| {
        world.run_file(run, "result.json").is_file()
    });
    world.until("the adopter to release its claim", |world| {
        stored_claim(world, &project) == Value::Null
    });
    hold.release();
    world.until_run_file_holds(run, "driver.log", "found no claim on");
    assert!(
        dispatched(&world, run).is_empty(),
        "the displaced driver dispatched what its adopter had finished"
    );
    world.run(&["stop", run]).exited(0);
    assert!(dispatched(&world, run).is_empty());
    assert_eq!(dispatched(&successor, run), vec!["x".to_owned()]);
    one_change_request_per_branch(&world);
    assert_eq!(created_from(&world).len(), 1);
    drop(successor);
}

/// A driver of `x` held at its first dispatch's start, by the failpoint there: the world, a
/// second host on it, the project, and the hold.
fn held_at_its_first_dispatch(
    name: &str,
    interval: u64,
) -> (World, World, String, crate::harness::Arrival) {
    let base = takeover_world(name, interval);
    base.script("x.work", "x's change\n");
    let successor = base.on_host("host-b");
    let begins = crate::harness::Rendezvous::failpoint();
    let world = base
        .with_env(BEGIN_GATE, &begins.address())
        .with_env(BEGIN_NODE, "x");
    let run = name.trim_start_matches("takeover-");
    let project = author(&world, run, vec![lifecycle("x", &[])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    // About to start x, on the read its launch made: held there.
    let hold = begins.arrived();
    until_claimed_by(&world, &project, run);
    (world, successor, project, hold)
}

/// A displaced driver that died without acknowledging — a shutdown ended it mid-dispatch,
/// leaving its claim — holds the adopter only until the takeover's wait has passed, by when
/// the lease of a driver that never answered has run out; the adopter then continues the
/// node on the branch its dispatch was on.
#[test]
fn a_takeover_whose_displaced_driver_died_proceeds_once_its_lease_has_run_out() {
    let interval = 5;
    let world = takeover_world("takeover-died", interval);
    world.script("x.work", "x's change\n");
    let x = world.rendezvous("x");
    let run = "died";
    let project = author(&world, run, vec![lifecycle("x", &[])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    let claim = until_claimed_by(&world, &project, run);
    world.until("x's dispatch to reach the board", |world| {
        item_metadata(world, &project, "x")
            .get("onepipeline.dispatch")
            .is_some()
    });
    let branch = item_metadata(&world, &project, "x")["onepipeline.dispatch"]["branch"]
        .as_str()
        .expect("x's branch")
        .to_owned();
    let shutdown = world.run(&["shutdown", run, "--grace", "1"]);
    assert!([0, 2].contains(&shutdown.code), "{}", shutdown.stderr);
    assert_eq!(stored_claim(&world, &project), claim, "the shutdown let go");
    drop(mine);

    let successor = world.on_host("host-b");
    let since = std::time::Instant::now();
    let taking = take_over_in_background(&successor, &project);
    let said = launched_over(taking);
    assert!(
        since.elapsed() >= takeover_wait(interval),
        "the adopter started {:?} after it launched, inside the wait",
        since.elapsed()
    );
    assert!(
        said.contains(&format!(
            "run '{run}' on host '{}' has not acknowledged in {}s",
            claim["host"].as_str().expect("a host"),
            takeover_wait(interval).as_secs()
        )),
        "{said}"
    );
    let theirs = x.arrived();
    theirs.release();
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(sessions_opened(&successor, run), vec![branch.clone()]);
    assert_eq!(
        settled(&successor, run, "x")["outcome"],
        "change-review-draft"
    );
    assert_eq!(changes_from(&world, &branch).len(), 1);
    one_change_request_per_branch(&world);
    drop(successor);
}

/// A takeover whose claim a second takeover replaces while it waits is refused, naming the
/// run that replaced it, and starts nothing; the displaced driver, reading the claim it
/// finds, acknowledges the second once its dispatch has settled, and the second starts on
/// that.
#[test]
fn a_takeover_replaced_while_it_waits_is_refused_and_the_one_that_replaced_it_starts() {
    let interval = 30;
    let base = takeover_world("takeover-replaced", interval);
    base.script("x.work", "x's change\n");
    let first = base.on_host("host-b");
    let second = base.on_host("host-c");
    let reads = crate::harness::Rendezvous::failpoint();
    let world = base.with_env(READ_GATE, &reads.address());
    let x = world.rendezvous("x");
    let run = "replaced";
    let project = author(&world, run, vec![lifecycle("x", &[])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    until_claimed_by(&world, &project, run);
    // The driver's read an interval after launch, held while two takeovers land.
    let read = reads.arrived();
    let refused = take_over_in_background(&first, &project);
    until_claimed_on(&world, &project, "host-b");
    let taking = take_over_in_background(&second, &project);
    let claim = until_claimed_on(&world, &project, "host-c");
    let answered = refused
        .wait_with_output()
        .expect("the first takeover answers");
    let said = String::from_utf8_lossy(&answered.stderr);
    assert_eq!(answered.status.code(), Some(2), "{said}");
    assert!(
        said.contains(&format!(
            "project driven elsewhere refused: run '{run}' on host 'host-c'"
        )),
        "{said}"
    );
    assert!(
        first.run_file(run, "launch.json").metadata().is_err(),
        "the refused takeover minted a run"
    );
    read.release();
    world.until("the driver to read the takeover", |world| {
        !world.events_of(run, "driver-superseded").is_empty()
    });
    superseded_by(&world, run, &claim);
    mine.release();
    let said = launched_over(taking);
    assert!(said.contains("acknowledged the takeover"), "{said}");
    x.arrived().release();
    second.until("the second takeover to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(dispatched(&second, run), vec!["x".to_owned()]);
    assert!(first.run_file(run, "launch.json").metadata().is_err());
    in_turn(&world, &second, run);
    one_change_request_per_branch(&world);
    drop(first);
    drop(second);
}

/// Whether the scripted store still holds the one-shot refusal of `method` a journey scripted:
/// it takes the file away once a call has been refused by it.
fn refusal_pending(world: &World, method: &str) -> bool {
    world
        .fakes
        .join(format!(
            "{}.{method}.refuse.once",
            crate::harness::SCRIPTED_KEY
        ))
        .exists()
}

/// The board's refusal a journey scripts.
fn board_down() -> Value {
    json!({"kind": "unavailable", "message": "the board is down"})
}

/// A takeover whose own claim the board refuses is refused, naming why, before anything is
/// minted: it has displaced nobody.
#[test]
fn a_takeover_whose_claim_the_board_refuses_is_refused_before_anything_is_minted() {
    let world = World::new("takeover-unwritten").through_scripted_source();
    let run = "unwritten";
    let project = world.plan(run, &plan_of(run, vec![agent("work", &[])]));
    let stranger = json!({"run": "elsewhere", "host": "builder", "session": null, "pid": 7,
                          "claimed_at": "2026-10-11T00:00:00.000Z"});
    let key =
        onetaskgraph_plugin_api::MetadataKey::new("onepipeline.driver".to_owned()).expect("a key");
    let id: onetaskgraph_core::GlobalId = project.parse().expect("a qualified project");
    world
        .store_call(|engine| async move { engine.set_project_metadata(&id, &key, &stranger).await })
        .unwrap_or_else(|why| panic!("the store refused the write: {why}"));
    world.store_refuses("set_project_metadata", &board_down());
    let refused = world.run_from(
        &world.project,
        &["start", &project, "--attach", "--take-over"],
    );
    refused.exited(2);
    assert!(
        refused
            .stderr
            .contains("--take-over could not write its claim")
            && refused.stderr.contains("the board is down"),
        "{}",
        refused.stderr
    );
    assert!(
        !world.run_file(run, "launch.json").exists(),
        "the refused takeover minted a run"
    );
    assert_eq!(stored_claim(&world, &project)["run"], "elsewhere");
}

/// A takeover goes on waiting through a poll the board refuses, and a displaced driver
/// that cannot write its acknowledgement says so: the adopter then starts once its wait
/// has passed, by when that driver can start and continue nothing.
#[test]
fn a_takeover_waits_out_a_board_that_refuses_a_poll_and_the_acknowledgement() {
    let interval = 5;
    let base = takeover_world("takeover-unanswered", interval).through_scripted_source();
    let successor = base.on_host("host-b");
    let reads = crate::harness::Rendezvous::failpoint();
    let world = base.with_env(READ_GATE, &reads.address());
    let x = world.rendezvous("x");
    let run = "unanswered";
    let project = author(&world, run, vec![agent("x", &[])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    until_claimed_by(&world, &project, run);
    // The displaced driver's first read, held while the takeover lands and polls.
    let read = reads.arrived();
    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    world.store_refuses_once("get_project", &board_down());
    world.until("the takeover's poll to be refused", |world| {
        !refusal_pending(world, "get_project")
    });
    world.store_refuses_once("set_project_metadata", &board_down());
    read.release();
    world.until("the driver to read the takeover", |world| {
        !world.events_of(run, "driver-superseded").is_empty()
    });
    superseded_by(&world, run, &claim);
    mine.release();
    world.until_run_file_holds(run, "driver.log", "could not acknowledge run 'unanswered'");
    let said = launched_over(taking);
    assert!(
        said.contains("could not read the claim") && said.contains("it goes on waiting"),
        "{said}"
    );
    assert!(
        said.contains(&format!(
            "has not acknowledged in {}s",
            takeover_wait(interval).as_secs()
        )),
        "{said}"
    );
    assert!(!refusal_pending(&world, "set_project_metadata"));
    in_turn(&world, &successor, run);
    drop(successor);
}

/// A driver whose reads of its claim keep failing ends the dispatch it has in progress once
/// its last confirming read has held it two intervals and a read's deadline, says so, and
/// starts nothing more until a read confirms it.
#[test]
fn a_driver_whose_reads_keep_failing_ends_its_dispatch_once_its_lease_lapses() {
    let interval = 5;
    let world = World::new("lease-lapsed")
        .through_scripted_source()
        .with_env(CLAIM_INTERVAL_ENV, &interval.to_string())
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let x = world.rendezvous("x");
    let run = "lapsed";
    let project = launch_detached(&world, run, vec![agent("x", &[]), agent("y", &["x"])]);
    let held = x.arrived();
    until_claimed_by(&world, &project, run);
    let since = std::time::Instant::now();
    world.store_refuses("get_project", &board_down());
    world.until_within(
        std::time::Duration::from_secs(120),
        "the lapsed lease to end x",
        |world| {
            world
                .events_of(run, "node-settled")
                .iter()
                .any(|event| event["labels"]["node"] == "x")
        },
    );
    assert!(
        since.elapsed() >= std::time::Duration::from_secs(2 * interval),
        "x was ended {:?} after the reads began failing, inside the lease",
        since.elapsed()
    );
    assert_eq!(settled(&world, run, "x")["status"], "cancelled");
    world.until_run_file_holds(run, "driver.log", "ends its dispatches in progress — 'x'");
    assert_eq!(dispatched(&world, run), vec!["x".to_owned()]);
    world.store_stops_refusing("get_project");
    world.until("the driver to end", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(dispatched(&world, run), vec!["x".to_owned()]);
    drop(held);
}

/// A publication asked for on a lease an interval old — the read due then refused by the
/// board — waits for the next read, and starts once that read confirms the lease.
#[test]
fn a_publication_asked_for_on_a_stale_lease_waits_for_the_next_read() {
    let interval = 10;
    let world = takeover_world("lease-stale-publication", interval).through_scripted_source();
    world.script("x.work", "x's change\n");
    let x = world.rendezvous("x");
    let run = "stale";
    let project = author(&world, run, vec![lifecycle("x", &[])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    until_claimed_by(&world, &project, run);
    world.store_refuses_once("get_project", &board_down());
    world.until_run_file_holds(run, "driver.log", "the board is down");
    let refused_at = std::time::Instant::now();
    mine.release();
    let published = |world: &World| {
        world
            .journal(run)
            .iter()
            .any(|event| event["kind"] == "change-opened")
    };
    while refused_at.elapsed() < std::time::Duration::from_secs(4) {
        assert!(!published(&world), "x published on a lease an interval old");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    world.until("the next read to clear x's publication", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert!(published(&world));
    assert!(
        refused_at.elapsed() >= std::time::Duration::from_secs(interval / 2),
        "x published {:?} after the refused read, before the next was due",
        refused_at.elapsed()
    );
    assert_eq!(settled(&world, run, "x")["outcome"], "change-review-draft");
    one_change_request_per_branch(&world);
}

/// A running driver reads its claim at most once per claim interval, however many passes it
/// runs: across windows shorter than one interval, a loop woken many times reads at most
/// once, and across three intervals it reads about once each — counted both by the
/// driver's own report and by the store's own record of the project reads it was asked for.
#[test]
fn a_running_driver_reads_its_claim_at_most_once_per_interval() {
    use std::time::Duration;
    let interval = Duration::from_secs(5);
    let world = World::new("claim-reads")
        .through_scripted_source()
        .with_env(crate::harness::LOOP_STATS_ENV, "1")
        .with_env(CLAIM_INTERVAL_ENV, &interval.as_secs().to_string());
    // A dispatch narrating five times a second: a pass for every beat.
    world.script("chatty.wait", "hold");
    world.script("chatty.heartbeat", "200");
    let project = world.plan("reads", &plan_of("reads", vec![agent("chatty", &[])]));
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    until_claimed_by(&world, &project, "reads");
    crate::harness::reporting(&world, "reads");

    let window = |length: Duration| {
        let before = crate::harness::counts(&world, "reads");
        let asked = world.store_asked("get_project");
        std::thread::sleep(length);
        (
            crate::harness::counts(&world, "reads").since(before),
            world.store_asked("get_project") - asked,
        )
    };
    let mut passes = 0;
    let mut inside = Vec::new();
    for _ in 0..3 {
        let (did, asked) = window(interval - Duration::from_millis(500));
        assert!(
            did.claim_reads <= 1,
            "two reads inside one interval: {did:?}"
        );
        assert!(
            asked <= 1,
            "the store was asked {asked} times inside one interval"
        );
        passes += did.passes;
        inside.push((did.claim_reads, asked));
    }
    assert!(
        passes >= 10,
        "the loop ran too few passes for its reads to say anything: {passes}"
    );
    let several: u32 = 3;
    let (did, asked) = window(interval * several);
    assert!(
        did.claim_reads <= u64::from(several) + 1 && did.claim_reads + 1 >= u64::from(several),
        "{} reads across {several} intervals: {did:?}",
        did.claim_reads
    );
    assert!(
        asked <= usize::try_from(several).expect("a count") + 1,
        "the store was asked {asked} times across {several} intervals"
    );
    assert!(did.passes > 2 * did.claim_reads, "{did:?}");
    eprintln!(
        "claim reads (driver's count, store's count) in three windows under one interval: \
         {inside:?} over {passes} passes; across {several} intervals: {} read, the store asked \
         {asked} times, over {} passes",
        did.claim_reads, did.passes
    );
    world.release("chatty.go");
    world.until("the run to settle", |world| {
        world.run_file("reads", "result.json").is_file()
    });
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
        .map(|host| world.on_host(host).with_env(CLAIM_INTERVAL_ENV, "2"))
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
    // Two claim intervals and some: every driver has read the claim at least once since
    // both claims could have reached the store.
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

/// What a store holds for a project is external input: a settlement, a dispatch or a claim
/// that is not one refuses the launch before anything is minted, naming the node or the key
/// and what is wrong, rather than being read as absent.
#[test]
fn a_settlement_dispatch_or_claim_that_is_not_one_refuses_the_launch() {
    let run = "malformed";
    let (world, project, stored) = a_failed_draft("continue-malformed", run);
    let refused = |what: &[&str]| {
        let refused = world.run_from(&world.project, &["start", &project, "--attach"]);
        refused.exited(2);
        for named in what {
            assert!(
                refused.stderr.contains(named),
                "{named}: {}",
                refused.stderr
            );
        }
        assert!(
            !world.run_file(&format!("{run}-2"), "launch.json").exists(),
            "the refusal minted a run"
        );
    };
    let settlement = &stored["b"];
    let with = |edit: &dyn Fn(&mut Value)| {
        let mut changed = settlement.clone();
        edit(&mut changed);
        changed
    };
    let settlements = [
        (with(&|s| s["brnach"] = s["branch"].take()), "`brnach`"),
        (json!("failed"), "not a mapping"),
        (
            with(&|s| s["status"] = json!("finished")),
            "unknown variant",
        ),
        (with(&|s| s["remote"] = json!("teleported")), "`remote`"),
        (with(&|s| s["branch"] = json!("me/b\nme/c")), "`branch`"),
        (with(&|s| s["base"] = json!("")), "`base`"),
        (with(&|s| s["head"] = json!("4b82 5dc6")), "`head`"),
        (with(&|s| s["change_url"] = json!("")), "`change_url`"),
        (
            with(&|s| {
                s["base"] = json!("me/a");
                s["stack"] = json!({"parents": [], "stack_base": null});
            }),
            "no usable parent",
        ),
        (
            with(&|s| {
                s["base"] = json!("me/a");
                s["stack"] = json!({"parents": [{"node": "a", "branch": "me/a", "head": "1f2e"}],
                                    "stack_base": null, "root": "main"});
            }),
            "`root`",
        ),
        (
            with(&|s| {
                s["base"] = json!("me/a");
                s["stack"] = json!({"parents": [{"node": "a", "branch": "me/a", "head": "1f2e",
                                                  "tip": "x"}],
                                    "stack_base": null});
            }),
            "`tip`",
        ),
    ];
    for (bad, says) in settlements {
        restate(&world, &project, "b", "onepipeline.settlement", bad);
        refused(&[
            "node 'b'",
            "`onepipeline.settlement` is not a settlement",
            says,
        ]);
    }
    restate(
        &world,
        &project,
        "b",
        "onepipeline.settlement",
        settlement.clone(),
    );

    let at = "2026-10-11T00:00:00.000Z";
    let dispatches = [
        (json!({"branch": settlement["branch"]}), "missing field"),
        (
            json!({"branch": "", "base": stored["a"]["branch"], "opened_at": at}),
            "not one a session can name",
        ),
        (
            json!({"branch": settlement["branch"], "base": stored["a"]["branch"],
                   "opened_at": "this morning"}),
            "not an RFC 3339 instant",
        ),
        (
            json!({"branch": settlement["branch"], "base": stored["a"]["branch"],
                   "opened_at": at, "host": "builder"}),
            "unknown field",
        ),
    ];
    for (bad, says) in dispatches {
        restate(&world, &project, "b", "onepipeline.dispatch", bad);
        refused(&["node 'b'", "`onepipeline.dispatch` is not a dispatch", says]);
    }
    restate(
        &world,
        &project,
        "b",
        "onepipeline.dispatch",
        json!({"branch": settlement["branch"], "base": stored["a"]["branch"], "opened_at": at}),
    );

    let claim = json!({"run": "elsewhere", "host": "builder", "session": null, "pid": 7,
                       "claimed_at": at});
    let mut leased = claim.clone();
    leased["lease"] = json!(30);
    let mut untimed = claim.clone();
    untimed["claimed_at"] = json!("yesterday");
    let mut unnamed = claim.clone();
    unnamed["host"] = json!("");
    let mut unrun = claim.clone();
    unrun["run"] = json!("else\nwhere");
    let mut unsessioned = claim.clone();
    unsessioned["session"] = json!("two words");
    let mut unacknowledged = claim.clone();
    unacknowledged["acknowledged"] = json!({"run": "older"});
    let mut untimely = claim.clone();
    untimely["acknowledged"] = json!({"run": "older", "host": "builder", "at": "later"});
    let claims = [
        (json!({"run": "elsewhere"}), "missing field"),
        (unnamed, "`host`"),
        (unrun, "`run`"),
        (unsessioned, "`session`"),
        (json!("elsewhere"), "not a mapping"),
        (leased, "`lease`"),
        (untimed, "not an RFC 3339 instant"),
        (unacknowledged, "its `acknowledged`: missing field"),
        (untimely, "not a run, a host and an instant"),
    ];
    for (bad, says) in claims {
        let key = onetaskgraph_plugin_api::MetadataKey::new("onepipeline.driver".to_owned())
            .expect("a key");
        let id: onetaskgraph_core::GlobalId = project.parse().expect("a qualified project");
        world
            .store_call(|engine| async move { engine.set_project_metadata(&id, &key, &bad).await })
            .unwrap_or_else(|why| panic!("the store refused the write: {why}"));
        refused(&["`onepipeline.driver` is not a claim", says]);
    }
    // And `--take-over` passes even a claim that is not one, saying so — and, since a
    // driver may still stand behind it, waits for one to let go, shortened here through the
    // claim interval.
    let world = world
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let taken = world.run_from(
        &world.project,
        &["start", &project, "--attach", "--take-over"],
    );
    assert!(
        taken
            .stderr
            .contains("--take-over passes a claim that is not one"),
        "{}",
        taken.stderr
    );
    assert!(
        taken.stderr.contains(&format!(
            "the driver its claim named has not acknowledged in {}s",
            takeover_wait(5).as_secs()
        )),
        "{}",
        taken.stderr
    );
    assert!(
        world.run_file(&format!("{run}-2"), "result.json").is_file(),
        "the takeover did not run: {}",
        taken.stderr
    );
}

/// The branch a node is pinned to is the latest the store knows it worked on: a dispatch the
/// store still holds opened after the settlement beside it, so its branch wins over the
/// settlement's — and a branch the plan authored on the task wins over both.
#[test]
fn a_dispatchs_branch_wins_over_a_settlements_and_an_authored_branch_over_both() {
    let run = "precedence";
    let (world, project, stored) = a_failed_draft("continue-precedence", run);
    // The branch a failed node preserved is projected as its plan's own `branch`; taken off,
    // what is left to pin it is the store's own record of where it worked.
    restate(&world, &project, "b", "onepipeline.branch", Value::Null);
    restate(
        &world,
        &project,
        "b",
        "onepipeline.dispatch",
        json!({"branch": "me/precedence/b-again", "base": stored["a"]["branch"],
               "opened_at": "2026-10-11T00:00:00.000Z"}),
    );
    world.script("gh.checks.2", GREEN);
    std::fs::remove_dir_all(&world.runs).expect("the runs root is removed");
    let cold = world.on_host("host-b");
    launch(&cold, run, &project, &[])
        .exited(0)
        .err_has(&format!(
            "onepipeline: launch '{run}' continues 1 settled node from the store and pins 1 node \
         to the branch it left: 'b' on me/precedence/b-again\n"
        ));
    assert_eq!(
        sessions_opened(&cold, run),
        vec!["me/precedence/b-again".to_owned()],
        "b was not dispatched on the branch its dispatch named"
    );

    // Back as it was, a dispatch beside the settlement again, and a branch the plan authored.
    restate(
        &world,
        &project,
        "b",
        "onepipeline.settlement",
        stored["b"].clone(),
    );
    restate(
        &world,
        &project,
        "b",
        "onepipeline.dispatch",
        json!({"branch": "me/precedence/b-again", "base": stored["a"]["branch"],
               "opened_at": "2026-10-11T00:00:00.000Z"}),
    );
    restate(
        &world,
        &project,
        "b",
        "onepipeline.branch",
        json!("me/precedence/authored"),
    );
    let again = format!("{run}-2");
    let colder = world.on_host("host-c");
    // The plan's own branch is the plan's, so the launch names no pin of its own.
    launch(&colder, run, &project, &[])
        .exited(0)
        .err_has(&format!(
            "onepipeline: launch '{run}' continues 1 settled node from the store\n"
        ));
    assert_eq!(
        sessions_opened(&colder, run),
        vec!["me/precedence/authored".to_owned()],
        "b was not dispatched on the branch the plan authored"
    );
    assert!(!colder.run_file(&again, "launch.json").exists());
    drop(colder);
    drop(cold);
}

/// A claim read the store does not answer is given up on at its deadline rather than holding
/// the driver, and says why on its log — but it renews no lease: an adoption whose first
/// read is held starts nothing on it, and starts once its next read, an interval after the
/// first began, answers and claims the project.
#[test]
fn an_adoption_whose_claim_read_is_not_answered_starts_nothing_until_a_read_answers() {
    let world = World::new("claim-read-deadline")
        .through_scripted_source()
        .with_env(CLAIM_INTERVAL_ENV, "30");
    world.script("work.wait", "hold");
    let project = launch_detached(&world, "deadline", vec![agent("work", &[])]);
    until_claimed_by(&world, &project, "deadline");
    world.run(&["stop", "deadline"]).exited(0);
    let held = world.store_holds("get_project");
    let since = std::time::Instant::now();
    world.run(&["adopt", "deadline", "--detach"]).exited(0);
    let read = held.arrived();
    world.until_run_file_holds("deadline", "driver.log", "did not answer within 20 seconds");
    assert!(
        since.elapsed() >= std::time::Duration::from_secs(20),
        "the read was given up on {:?} after the adoption began, inside its deadline",
        since.elapsed()
    );
    assert_eq!(
        world.events_of("deadline", "node-dispatched").len(),
        1,
        "the adoption dispatched on a read that did not answer"
    );
    world.store_stops_holding("get_project");
    read.release();
    world.until(
        "the adoption's next read to answer and it to dispatch",
        |world| world.events_of("deadline", "node-dispatched").len() == 2,
    );
    assert!(
        since.elapsed() >= std::time::Duration::from_secs(30),
        "the adoption dispatched {:?} after it began, before its next read was due",
        since.elapsed()
    );
    until_claimed_by(&world, &project, "deadline");
    world.release("work.go");
    world.until("the adopted run to settle", |world| {
        world.run_file("deadline", "result.json").is_file()
    });
}

/// A closeout whose projection is still in flight when its window ends — the store slow to
/// answer the claim's release — does not leave the claim standing: the driver releases it
/// itself, the same read and the same write, before it lets go of the run.
#[test]
fn a_closeout_still_releasing_when_its_window_ends_releases_the_claim_itself() {
    let world = World::new("claim-slow-closeout").through_scripted_source();
    world.script("work.wait", "hold");
    let project = launch_detached(&world, "slowclose", vec![agent("work", &[])]);
    until_claimed_by(&world, &project, "slowclose");
    let writes = world.store_holds("set_project_metadata");
    world.release("work.go");
    // The closeout's own release, held past its window.
    let projection = writes.arrived();
    // The driver's: let through.
    writes.arrived().release();
    world.until("the run to settle", |world| {
        world.run_file("slowclose", "result.json").is_file()
    });
    assert_eq!(
        stored_claim(&world, &project),
        Value::Null,
        "the run let go with its claim still standing"
    );
    world.store_stops_holding("set_project_metadata");
    projection.release();
}

/// A superseded driver whose withheld node's branch nobody has published yet puts that branch
/// on the origin, and its settlement says so — the work is there for the run that took the
/// project over, and no change request was opened for it.
#[test]
fn a_withheld_publication_puts_its_branch_on_the_origin_when_nothing_else_has() {
    let world = takeover_world("superseded-kept", 10);
    world.script("l.work", "l's change\n");
    let l = world.rendezvous("l");
    let project = launch_detached(&world, "kept", vec![lifecycle("l", &[])]);
    let mine = l.arrived();
    until_claimed_by(&world, &project, "kept");
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
    let taking = take_over_in_background(&successor, &project);
    until_claimed_on(&world, &project, "host-b");
    world.until("the first driver to notice the takeover", |world| {
        !world.events_of("kept", "driver-superseded").is_empty()
    });
    mine.release();
    world.until("the superseded driver to end", |world| {
        world.run_file("kept", "result.json").is_file()
    });
    let withheld = settled(&world, "kept", "l");
    assert_eq!(withheld["status"], "cancelled", "{withheld}");
    assert_eq!(withheld["remote"], "pushed", "{withheld}");
    let detail = withheld["detail"].as_str().unwrap_or_default();
    assert!(
        detail.starts_with("not published")
            && detail.contains(&format!("kept on {branch} (pushed)")),
        "{withheld}"
    );
    let origin = world.root.join("origin.git");
    let on_origin = crate::harness::git(&world, &origin, &["rev-parse", &branch]);
    assert_eq!(
        on_origin.trim(),
        withheld["head"].as_str().expect("a head"),
        "the origin does not hold the withheld work"
    );
    assert!(
        changes_from(&world, &branch).is_empty(),
        "a change request was opened"
    );
    // The successor, which started nothing until the first driver had let go, continues
    // `l` from that branch; its own dispatch stays held, publishing nothing here.
    launched_over(taking);
    let theirs = l.arrived();
    successor.until("the successor's session to reach its record", |world| {
        !sessions_opened(world, "kept").is_empty()
    });
    assert_eq!(sessions_opened(&successor, "kept"), vec![branch.clone()]);
    successor.run(&["stop", "kept", "--force"]).exited(0);
    drop(theirs);
    drop(successor);
}

/// A claim the closeout's projection and the driver's own release both fail to land in time
/// is left standing, and the driver says so — the next launch elsewhere is refused over it
/// until it is taken over — rather than the run hanging on its store.
#[test]
fn a_claim_neither_release_can_land_is_left_standing_and_said() {
    let world = World::new("claim-unreleased").through_scripted_source();
    world.script("work.wait", "hold");
    let project = launch_detached(&world, "unreleased", vec![agent("work", &[])]);
    let claim = until_claimed_by(&world, &project, "unreleased");
    let writes = world.store_holds("set_project_metadata");
    world.release("work.go");
    let projection = writes.arrived();
    let release = writes.arrived();
    world.until("the run to settle", |world| {
        world.run_file("unreleased", "result.json").is_file()
    });
    world.until_run_file_holds("unreleased", "driver.log", "could not release its claim on");
    assert_eq!(stored_claim(&world, &project), claim, "the claim moved");
    world.store_stops_holding("set_project_metadata");
    drop(release);
    drop(projection);
}

/// A closeout releasing a claim that is no longer on the store — somebody cleared it — still
/// projects the run's ending, and writes no claim; one finding a claim that is not one leaves
/// it as it stands and writes nothing to the project at all, since it cannot tell whose it is.
#[test]
fn a_closeout_over_a_cleared_or_mangled_claim_writes_no_claim_of_its_own() {
    let world = World::new("claim-cleared-or-mangled");
    let set_claim = |project: &str, value: Value| {
        let key = onetaskgraph_plugin_api::MetadataKey::new("onepipeline.driver".to_owned())
            .expect("a key");
        let id: onetaskgraph_core::GlobalId = project.parse().expect("a qualified project");
        world
            .store_call(
                |engine| async move { engine.set_project_metadata(&id, &key, &value).await },
            )
            .unwrap_or_else(|why| panic!("the store refused the write: {why}"));
    };
    let statuses = |project: &str| -> Vec<Value> {
        world
            .store_tasks(project)
            .iter()
            .map(|task| task["item"]["status"]["name"].clone())
            .collect()
    };

    world.script("cleared.wait", "hold");
    let cleared = launch_detached(&world, "cleared", vec![agent("cleared", &[])]);
    until_claimed_by(&world, &cleared, "cleared");
    set_claim(&cleared, Value::Null);
    world.release("cleared.go");
    world.until("the run to settle", |world| {
        world.run_file("cleared", "result.json").is_file()
    });
    world.until("the closeout to reach the board", |_| {
        statuses(&cleared) == vec![json!("done")]
    });
    assert_eq!(stored_claim(&world, &cleared), Value::Null);

    world.script("mangled.wait", "hold");
    let mangled = launch_detached(&world, "mangled", vec![agent("mangled", &[])]);
    until_claimed_by(&world, &mangled, "mangled");
    let stranger = json!({"run": "mangled", "host": "", "session": null, "pid": 7,
                          "claimed_at": "2026-10-11T00:00:00.000Z"});
    set_claim(&mangled, stranger.clone());
    world.release("mangled.go");
    world.until("the run to settle", |world| {
        world.run_file("mangled", "result.json").is_file()
    });
    let projections =
        std::fs::read_to_string(world.run_file("mangled", "writeback-projections.jsonl"))
            .expect("the run recorded its projections");
    let closeout: Value =
        serde_json::from_str(projections.lines().last().expect("a line")).expect("a record");
    assert_eq!(closeout["items"], json!([]), "{closeout}");
    assert_eq!(closeout["calls"], json!({"project-show": 1}), "{closeout}");
    assert_eq!(
        stored_claim(&world, &mangled),
        stranger,
        "the claim was touched"
    );
}

/// An adoption reads the claim before it claims anything: a run adopted on this host after
/// another host took its project over supersedes at once — it projects nothing onto the
/// store, dispatches nothing, journals `driver-superseded` naming the successor's claim, and
/// lets go with that claim on the store as it was.
#[test]
fn an_adoption_after_a_takeover_supersedes_before_it_projects_or_dispatches() {
    let world = World::new("adopt-superseded");
    world.script("work.wait", "hold");
    let project = launch_detached(
        &world,
        "adopted",
        vec![agent("work", &[]), agent("after", &["work"])],
    );
    let claim = until_claimed_by(&world, &project, "adopted");
    // Its driver ended and its claim kept, so the run is adoptable.
    let shutdown = world.run(&["shutdown", "adopted", "--grace", "1"]);
    assert!([0, 2].contains(&shutdown.code), "{}", shutdown.stderr);
    assert_eq!(stored_claim(&world, &project), claim);

    let successor = world
        .on_host("host-b")
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    successor
        .run_from(
            &successor.project,
            &["start", &project, "--detach", "--take-over"],
        )
        .exited(0);
    // The successor's driver's own claim, which replaced the one its takeover waited under.
    let driving = successor.run_json("adopted", "launch.json")["pid"].clone();
    world.until(
        "the successor's driver's claim to reach the store",
        |world| {
            let claim = stored_claim(world, &project);
            claim["host"] == "host-b" && claim["pid"] == driving
        },
    );
    let claimed = stored_claim(&world, &project);

    let projections = || {
        std::fs::read_to_string(world.run_file("adopted", "writeback-projections.jsonl"))
            .unwrap_or_default()
            .lines()
            .count()
    };
    let projected = projections();
    let dispatched = world.events_of("adopted", "node-dispatched").len();
    let exited = world.events_of("adopted", "driver-exited").len();
    world.run(&["adopt", "adopted", "--detach"]).exited(0);
    world.until("the adopting driver to let go", |world| {
        world.events_of("adopted", "driver-exited").len() > exited
    });

    let superseded = world.events_of("adopted", "driver-superseded");
    assert_eq!(superseded.len(), 1, "{superseded:?}");
    assert_eq!(superseded[0]["payload"]["by"], claimed);
    assert_eq!(
        world.events_of("adopted", "node-dispatched").len(),
        dispatched,
        "the superseded adoption dispatched"
    );
    assert_eq!(
        projections(),
        projected,
        "the superseded adoption projected onto the store"
    );
    assert_eq!(
        stored_claim(&world, &project),
        claimed,
        "the superseded adoption moved the successor's claim"
    );

    world.release("work.go");
    successor.until("the successor to settle", |successor| {
        successor.run_file("adopted", "result.json").is_file()
    });
    drop(successor);
}
