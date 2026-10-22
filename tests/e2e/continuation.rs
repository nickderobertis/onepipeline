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
use crate::harness::{agent, held_publication, lifecycle, plan_of, process_ended, World};

/// The host's branch prefix every team journey here runs under.
const PREFIX: &str = "me/";

/// A required check the host reports red.
const RED: &str = "lint completed failure required";

/// Take this host's runs root away, as a host that never held it would find it.
///
/// A run that has settled is waited on until its driver has exited, rather than only until
/// its result is written: the driver records its exit in the run's journal after
/// `result.json`, releases the run, and then seals the run's summary beside it, so a removal
/// racing that closeout finds a file recreated under it. A run still in flight is taken away
/// as it stands, which is the host a journey means when it leaves a dispatch on the store.
fn remove_the_runs_root(world: &World) {
    world.until("every settled run's driver to have exited", |world| {
        std::fs::read_dir(&world.runs).map_or(true, |runs| {
            runs.flatten().all(|run| {
                if !run.path().join("result.json").exists() {
                    return true;
                }
                let exits = world.events_of(&run.file_name().to_string_lossy(), "driver-exited");
                !exits.is_empty()
                    && exits.iter().all(|exit| {
                        exit["payload"]["pid"]
                            .as_u64()
                            .and_then(|pid| u32::try_from(pid).ok())
                            .is_some_and(process_ended)
                    })
            })
        })
    });
    std::fs::remove_dir_all(&world.runs).expect("the runs root is removed");
}

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
    remove_the_runs_root(&world);
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

    remove_the_runs_root(&world);
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
    remove_the_runs_root(world);
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
    cancelled_node_continuation(false);
}

/// A person closing a task after the engine parked it withdraws it from the next graph.
#[test]
fn a_persons_close_after_a_real_engine_park_withdraws_the_task() {
    cancelled_node_continuation(true);
}

fn cancelled_node_continuation(withdrawn: bool) {
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
    assert_eq!(
        item["item"]["metadata"]["onepipeline.projected_status"], "parked",
        "{item}"
    );
    if withdrawn {
        crate::withdrawn::withdraw(&world, &project, "b");
    }
    world.unscript("b.rendezvous");

    // The park is the manager's, and the board carries it: a host continuing the project
    // leaves the node parked until somebody requeues it — and then continues its branch.
    let branch = stored["b"]["branch"]
        .as_str()
        .expect("b's branch")
        .to_owned();
    let created = created_from(&world);
    world.script("gh.checks.2", GREEN);
    remove_the_runs_root(&world);
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
    if withdrawn {
        let result = cold.run_json(run, "result.json");
        assert_eq!(
            result["nodes"].as_array().expect("nodes").len(),
            1,
            "{result}"
        );
        assert_eq!(result["nodes"][0]["id"], "a", "{result}");
        assert_eq!(
            crate::withdrawn::stored_status(&cold, &project, "b")["category"],
            "cancelled"
        );
        return;
    }
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
    // The terms its claim states, which a host taking the project over waits out.
    let world = world
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
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
    assert_eq!(
        claim["lease"],
        json!({"interval_seconds": 5, "cancel_grace_seconds": 1}),
        "{claim}"
    );
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
    remove_the_runs_root(&world);
    // Its driver is gone, which nothing on this host can prove: the takeover waits out the
    // wait its claim states, shortened there through that host's cancel grace.
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
/// on the store, acknowledged — the run has nothing left in progress — and a third launch
/// while the successor waits is refused naming the successor's run.
#[test]
fn a_successors_claim_survives_the_closeout_and_the_stop_of_the_run_it_took_over() {
    let world = World::new("claim-successor")
        .with_env(CLAIM_INTERVAL_ENV, "10")
        .with_env(crate::harness::CANCEL_GRACE_ENV, &PATIENT_GRACE.to_string());
    let first = world.rendezvous("first");
    let closes = launch_detached(&world, "closes", vec![agent("first", &[])]);
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
    // The run taken over finishes its one dispatch, which publishes nothing, and closes out:
    // its release finds the successor's claim and acknowledges it, and only then does the
    // successor start.
    mine.release();
    world.until("the run taken over to close out", |world| {
        world.run_file("closes", "result.json").is_file()
    });
    let said = launched_over(taking);
    assert!(said.contains("acknowledged the takeover"), "{said}");
    let now = stored_claim(&world, &closes);
    assert_eq!(
        (&now["run"], &now["host"]),
        (&claimed["run"], &claimed["host"]),
        "the closeout took the claim"
    );
    successor.until("the successor to close out", |world| {
        world.run_file("closes", "result.json").is_file()
    });
    assert!(dispatched(&successor, "closes").is_empty());
    world.until("the successor to release its claim", |world| {
        stored_claim(world, &closes) == Value::Null
    });
    drop(third);
    drop(successor);

    // And one stopped while its successor waits on it: the stop leaves the successor's
    // claim, acknowledging it — the stopped run has nothing left running — and the
    // successor starts on that rather than waiting out the wait the stopped run's claim
    // states.
    let world = World::new("claim-successor-stop")
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, &PATIENT_GRACE.to_string());
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
    let since = std::time::Instant::now();
    world.run(&["stop", "stops"]).exited(0);
    let mut left = stored_claim(&world, &stops);
    let acknowledged = left
        .as_object_mut()
        .and_then(|claim| claim.remove("acknowledged"))
        .unwrap_or(Value::Null);
    assert_eq!(left, claimed, "the stop took the claim");
    assert_eq!(acknowledged["run"], "stops", "{acknowledged}");
    drop(mine);
    let said = launched_over(taking);
    assert!(
        said.contains("run 'stops' on host") && said.contains("acknowledged the takeover"),
        "{said}"
    );
    assert!(
        since.elapsed() < takeover_wait(PATIENT_GRACE),
        "the successor waited {:?} on a run the stop had acknowledged it over",
        since.elapsed()
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

/// A pass that dispatches three nodes asks the store nothing for them: no dispatch reads the
/// claim, so the store is asked for the project not once between the moment they became
/// ready and the moment all three were dispatched.
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

/// A takeover of a live driver mid-dispatch hands the project over safely. The displaced
/// driver reads nothing while its dispatch runs; once that dispatch has finished, the read
/// before its publication finds the takeover, so it withholds the publication — its branch
/// kept — starts nothing more, and acknowledges once nothing of its own is in progress. The
/// adopter, which started nothing until then, continues the node on its branch, drafts the
/// only change request from it, and drafts what is left; no publication of the two overlaps.
#[test]
fn a_driver_taken_over_mid_dispatch_withholds_its_publication_and_hands_over() {
    let world = patient_world("takeover-mid-dispatch");
    world.script("x.work", "x's change\n");
    world.script("y.work", "y's change\n");
    let x = world.rendezvous("x");
    let run = "middispatch";
    let project = author(
        &world,
        run,
        vec![lifecycle("x", &[]), lifecycle("y", &["x"])],
    );
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    until_claimed_by(&world, &project, run);
    world.until("x's dispatch to reach the board", |world| {
        item_metadata(world, &project, "x")
            .get("onepipeline.dispatch")
            .is_some()
    });
    let branch = item_metadata(&world, &project, "x")["onepipeline.dispatch"]["branch"]
        .as_str()
        .expect("x's branch")
        .to_owned();
    let successor = world.on_host("host-b");
    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    // While x runs the displaced driver reads nothing, so it has not noticed, and the
    // adopter, waiting on it, has minted nothing.
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(
        world.events_of(run, "driver-superseded").is_empty(),
        "the displaced driver read its claim while it was dispatching"
    );
    assert!(
        successor.run_file(run, "launch.json").metadata().is_err(),
        "the adopter minted its run while the displaced driver was dispatching"
    );

    mine.release();
    world.until("the displaced driver to end", |world| {
        world.run_file(run, "result.json").is_file()
    });
    superseded_by(&world, run, &claim);
    let withheld = settled(&world, run, "x");
    assert_eq!(withheld["status"], "cancelled", "{withheld}");
    assert!(
        withheld["detail"]
            .as_str()
            .is_some_and(|detail| detail.starts_with("not published")),
        "{withheld}"
    );
    assert_eq!(dispatched(&world, run), vec!["x".to_owned()]);
    let said = launched_over(taking);
    assert!(said.contains("acknowledged the takeover"), "{said}");

    x.arrived().release();
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert!(
        sessions_opened(&successor, run).contains(&branch),
        "the adopter did not continue x on its branch"
    );
    for node in ["x", "y"] {
        assert_eq!(
            settled(&successor, run, node)["outcome"],
            "change-review-draft",
            "{node}"
        );
    }
    assert_eq!(changes_from(&world, &branch).len(), 1);
    in_turn(&world, &successor, run);
    one_change_request_per_branch(&world);
    assert_eq!(created_from(&world).len(), 2);
    drop(successor);
}

/// The setting naming the claim interval every host of a journey is given, in whole seconds:
/// what paces a takeover's reads of the claim while it waits, and nothing a running driver
/// does.
const CLAIM_INTERVAL_ENV: &str = "ONEPIPELINE_CLAIM_INTERVAL_SECONDS";

/// A team world whose every host has a takeover read the claim every tenth of `interval`
/// seconds, at least one apart, and gives a cancelled dispatch one second, so a takeover
/// nothing acknowledges waits [`takeover_wait`]`(1)`: that second and ten seconds' slack.
fn takeover_world(name: &str, interval: u64) -> World {
    team_world(name)
        .with_env(CLAIM_INTERVAL_ENV, &interval.to_string())
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1")
}

/// A displaced host's cancel grace long enough that a takeover of it waits out none of a
/// journey: it starts on that host's acknowledgement.
const PATIENT_GRACE: u64 = 120;

/// A [`team_world`] whose every host gives a cancelled dispatch [`PATIENT_GRACE`] seconds,
/// so a takeover of one of its drivers starts only on that driver's acknowledgement.
fn patient_world(name: &str) -> World {
    team_world(name)
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, &PATIENT_GRACE.to_string())
}

/// How long a takeover nothing acknowledges waits over a displaced host given a cancel
/// grace of `grace` seconds: that grace, and ten seconds' slack.
fn takeover_wait(grace: u64) -> std::time::Duration {
    std::time::Duration::from_secs(grace + 10)
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

/// A takeover of a driver whose dispatch has nothing to publish starts on the acknowledgement
/// that driver's closeout writes: it reads no claim while it runs, and its release, finding
/// the adopter's claim, acknowledges it rather than releasing — well inside the wait the
/// displaced claim states. The adopter launches from the project origin the store names when
/// it starts, not the one the displaced driver launched from, and continues what that driver
/// settled rather than dispatching it again.
#[test]
fn a_takeover_of_a_driver_with_nothing_to_publish_starts_on_the_acknowledgement_its_closeout_writes(
) {
    let base = patient_world("takeover-idle").through_scripted_source();
    let successor = base.on_host("host-b");
    let x = base.rendezvous("x");
    let run = "idle";
    let project = author(&base, run, vec![agent("x", &[])]);
    let authoring = base.store_apart("authoring");
    let world = base
        .with_env("ONETASKGRAPH_SOURCES__AUTHORING__PLUGIN", "local-md")
        .with_env(
            "ONETASKGRAPH_SOURCES__AUTHORING__CONFIG__ROOT",
            authoring.to_str().unwrap(),
        );
    let origin = |name: &str| {
        let mut plan = world.store_project(&project)["items"][0]["item"].clone();
        // Use the launched plan's current metadata so copying the replacement origin
        // preserves its active claim and the driver's projection.
        let file = authoring.join("projects").join(format!("{name}.md"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        plan["title"] = json!("idle");
        std::fs::write(
            &file,
            format!(
                "---\n{}---\n",
                serde_saphyr::to_string(&json!({
                    "title": plan["title"], "metadata": plan["metadata"]
                }))
                .unwrap()
            ),
        )
        .unwrap();
        world.copy_project_metadata(&format!("authoring:{name}"), crate::harness::STORE_SOURCE);
    };
    origin("before");
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    until_claimed_by(&world, &project, run);
    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    // The successor has read the old origin and written its claim, but cannot yet launch.
    origin("after");
    assert!(
        successor.run_file(run, "launch.json").metadata().is_err(),
        "the adopter minted its run before the driver it displaced had let go"
    );
    let landed = std::time::Instant::now();
    mine.release();
    let said = launched_over(taking);
    assert!(
        landed.elapsed() < takeover_wait(PATIENT_GRACE),
        "the adopter started {:?} after its claim landed, not on the acknowledgement",
        landed.elapsed()
    );
    assert!(said.contains("acknowledged the takeover"), "{said}");
    world.until_run_file_holds(run, "driver.log", "acknowledged the run that took it over");
    assert_eq!(
        world.run_json(run, "launch.json")["project_origin"],
        "authoring:before"
    );
    assert_eq!(
        successor.run_json(run, "launch.json")["project_origin"],
        "authoring:after"
    );
    assert_eq!(
        successor.run(&["status", run, "--json"]).json()["group"],
        "authoring:after"
    );
    assert!(
        world.events_of(run, "driver-superseded").is_empty(),
        "the displaced driver read its claim while it was dispatching"
    );
    assert_eq!(claim["host"], "host-b", "{claim}");

    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(dispatched(&world, run), vec!["x".to_owned()]);
    assert!(dispatched(&successor, run).is_empty());
    assert_eq!(
        settled(&successor, run, "x")["continued"]["source"],
        "store"
    );
    handed_over(&world, &successor, run);
    drop(successor);
}

/// The project's `onepipeline.publishing`, as the store holds it: `null` where it holds none.
fn stored_publishing(world: &World, project: &str) -> Value {
    world.store_project(project)["items"][0]["item"]["metadata"]
        .get("onepipeline.publishing")
        .cloned()
        .unwrap_or(Value::Null)
}

/// A publication the displaced driver has under way — its push held at the host — for
/// longer than the **whole** takeover wait holds the takeover with it: the driver named it
/// on the project before it started, so the adopter, finding it named once the wait has
/// passed, mints nothing and says what it waits on, and starts only once the publication has
/// finished. The wait is the displaced driver's own — its claim states a longer cancel grace
/// than the adopter was given, so the adopter's own terms would have had it start while the
/// publication was still held — and no dispatch or publication of the two drivers overlapped.
#[test]
fn a_publication_held_past_the_whole_takeover_wait_holds_the_takeover_until_it_has_finished() {
    // The displaced host's terms, which its claim states: a wait of 12 + 10.
    let (interval, grace) = (8, 12);
    let theirs = takeover_wait(grace);
    let world = World::new("takeover-fenced")
        .with_env("ONEVCS_BRANCH_PREFIX", PREFIX)
        .with_env(CLAIM_INTERVAL_ENV, &interval.to_string())
        .with_env(crate::harness::CANCEL_GRACE_ENV, &grace.to_string());
    let go = world.fakes.join("push.go");
    let held = held_publication(&world, &go);
    world.repository("change-open", &held.argv());
    rules(&world, "change-open", "required", None);
    world.script("gh.checks", GREEN);
    world.script("x.work", "x's change\n");
    // The adopter's own terms wait 1 + 10: shorter than the displaced driver's.
    let successor = world
        .on_host("host-b")
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let run = "fenced";
    let project = author(&world, run, vec![lifecycle("x", &[])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    world.until("x's publication to be under way", |_| held.holding());
    // Named on the project before it started, by the driver that started it.
    let fence = stored_publishing(&world, &project);
    assert_eq!(fence["run"], run, "{fence}");
    assert_eq!(fence["nodes"], json!(["x"]), "{fence}");
    let displaced = stored_claim(&world, &project);
    assert_eq!(
        displaced["lease"],
        json!({"interval_seconds": interval, "cancel_grace_seconds": grace}),
        "{displaced}"
    );

    let taking = take_over_in_background(&successor, &project);
    until_claimed_on(&world, &project, "host-b");
    let landed = std::time::Instant::now();
    // Held past the whole wait — the displaced driver's, longer than the adopter's own — and
    // a few polls more: the adopter has minted nothing, and the publication is still held.
    std::thread::sleep(theirs + std::time::Duration::from_secs(8));
    assert!(
        held.holding(),
        "the publication was ended while it was held"
    );
    assert!(
        successor.run_file(run, "launch.json").metadata().is_err(),
        "the adopter minted its run {:?} after its claim landed, while the displaced \
         driver's publication was still under way",
        landed.elapsed()
    );
    assert_eq!(stored_publishing(&world, &project)["nodes"], json!(["x"]));
    held.release();
    let said = launched_over(taking);
    assert!(
        said.contains(&format!(
            "has not acknowledged in {}s, and its publication of 'x' has been under way since",
            theirs.as_secs()
        )),
        "{said}"
    );
    let x = settled(&world, run, "x");
    assert_eq!(x["outcome"], "change-review-draft", "{x}");
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(dispatched(&world, run), vec!["x".to_owned()]);
    assert!(dispatched(&successor, run).is_empty());
    assert_eq!(settled(&successor, run, "x")["change_url"], x["change_url"]);
    handed_over(&world, &successor, run);
    one_change_request_per_branch(&world);
    assert_eq!(created_from(&world).len(), 1);
    // Each driver cleared what it named once its publications had settled.
    assert_eq!(stored_publishing(&world, &project), Value::Null);
    drop(successor);
    drop(held);
}

/// How a journey keeps the displaced driver's settlement of `x` off the store.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OffTheStore {
    /// Its projection is held, and let through inside the displaced driver's one bounded
    /// wait for it.
    Held,
    /// Its projection is held past that wait, and let through after it.
    HeldPastTheWait,
}

/// A publication that settles while a takeover is past its whole wait leaves its fence
/// standing until its settlement is on the store. The displaced driver has read the takeover
/// before `z`'s publication — withholding it — while `x`'s was already under way; its
/// projection of `x`'s settlement is held off the store, and the adopter's read inside that
/// hold finds the fence and starts nothing; its read once the projection has landed finds `x`
/// settled, so it continues `x` and dispatches `z` alone. A fence cleared ahead of the
/// settlement would have had the read inside the hold find neither, and dispatch `x` again.
#[test]
fn a_takeover_reading_before_a_settled_publication_has_landed_waits_until_it_has() {
    a_takeover_waits_for_a_settlement_off_the_store("takeover-unlanded", OffTheStore::Held);
}

/// The same with the settlement's projection held past the superseded driver's one bounded
/// wait for it: past that wait the driver neither clears its fence nor acknowledges, and the
/// adopter starts nothing, until the projection has landed.
#[test]
fn a_settlement_held_past_the_displaced_drivers_wait_holds_the_takeover_until_it_lands() {
    a_takeover_waits_for_a_settlement_off_the_store(
        "takeover-unlanded-late",
        OffTheStore::HeldPastTheWait,
    );
}

fn a_takeover_waits_for_a_settlement_off_the_store(name: &str, off: OffTheStore) {
    let world = World::new(name)
        .with_env("ONEVCS_BRANCH_PREFIX", PREFIX)
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let go = world.fakes.join("push.go");
    let held = held_publication(&world, &go);
    world.repository("change-open", &held.argv());
    rules(&world, "change-open", "required", None);
    world.script("gh.checks", GREEN);
    world.script("x.work", "x's change\n");
    world.script("z.work", "z's change\n");
    let z = world.rendezvous("z");
    // Each read the adopter makes once its wait has passed is held until this journey lets
    // it go.
    let polls = crate::harness::Rendezvous::failpoint();
    let successor = world
        .on_host("host-b")
        .with_env("ONEPIPELINE_TEST_TAKEOVER_PAST_WAIT_GATE", &polls.address());
    // The displaced driver's first projection carrying `x`'s settlement is held before it
    // reaches the store.
    let projecting = crate::harness::Rendezvous::failpoint();
    let world = world
        .with_env("ONEPIPELINE_TEST_PROJECTION_SETTLED_NODE", "x")
        .with_env(
            "ONEPIPELINE_TEST_PROJECTION_SETTLED_GATE",
            &projecting.address(),
        );
    let run = "unlanded";
    let mut plan = plan_of(run, vec![lifecycle("x", &[]), lifecycle("z", &[])]);
    plan["concurrency"] = json!(2);
    let project = world.plan(run, &plan);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = z.arrived();
    world.until("x's publication to be under way", |_| held.holding());
    assert_eq!(stored_publishing(&world, &project)["nodes"], json!(["x"]));

    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    // `z` finishes, and the read before its publication finds the takeover: the driver is
    // superseded with `x`'s publication still under way.
    mine.release();
    world.until("the displaced driver to read the takeover", |world| {
        !world.events_of(run, "driver-superseded").is_empty()
    });
    superseded_by(&world, run, &claim);
    // The adopter is past its whole wait, and held before its next read.
    let past = polls.arrived();
    held.release();
    // `x`'s publication has settled, and its settlement is about to be projected.
    let projection = projecting.arrived();
    if off == OffTheStore::HeldPastTheWait {
        // Past the driver's one wait for the settlement, the driver says it is still owed,
        // where one that had given up on it would have cleared its fence or acknowledged
        // instead.
        world.until(
            "the displaced driver's wait for x's settlement to pass",
            |world| {
                std::fs::read_to_string(world.run_file(run, "driver.log"))
                    .is_ok_and(|log| log.contains("has not reached the store; it acknowledges"))
                    || stored_publishing(world, &project).is_null()
                    || stored_claim(world, &project).get("acknowledged").is_some()
            },
        );
    }
    assert_eq!(
        stored_publishing(&world, &project)["nodes"],
        json!(["x"]),
        "the fence was cleared before x's settlement reached the store"
    );
    assert!(
        stored_claim(&world, &project).get("acknowledged").is_none(),
        "the takeover was acknowledged before x's settlement reached the store"
    );
    // The adopter reads before the settlement has landed, finds the fence, and reads again
    // rather than starting.
    past.release();
    let again = polls.arrived();
    assert!(
        successor.run_file(run, "launch.json").metadata().is_err(),
        "the adopter minted its run before x's settlement had reached the store"
    );
    assert!(
        dispatched(&successor, run).is_empty(),
        "the adopter dispatched before x's settlement had reached the store"
    );
    // Once the settlement has landed the fence goes, and the adopter's next read starts it.
    projection.release();
    world.until(
        "the fence to be cleared once x's settlement has landed",
        |world| stored_publishing(world, &project).is_null(),
    );
    again.release();
    // Every later read past the wait is let through.
    std::thread::spawn(move || loop {
        polls.arrived().release();
    });
    let said = launched_over(taking);
    assert!(
        said.contains("and its publication of 'x' has been under way since"),
        "{said}"
    );
    z.arrived().release();
    let x = settled(&world, run, "x");
    assert_eq!(x["outcome"], "change-review-draft", "{x}");
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    let mut theirs = dispatched(&world, run);
    theirs.sort();
    assert_eq!(theirs, vec!["x".to_owned(), "z".to_owned()]);
    assert_eq!(dispatched(&successor, run), vec!["z".to_owned()]);
    assert_eq!(
        settled(&successor, run, "x")["continued"]["source"],
        "store"
    );
    assert_eq!(settled(&successor, run, "x")["change_url"], x["change_url"]);
    in_turn(&world, &successor, run);
    one_change_request_per_branch(&world);
    assert_eq!(created_from(&world).len(), 2);
    assert_eq!(stored_publishing(&world, &project), Value::Null);
    drop(successor);
    drop(held);
}

/// Write `value` as the project's `onepipeline.publishing`, as somebody other than a driver
/// would.
fn restate_publishing(world: &World, project: &str, value: Value) {
    let key = onetaskgraph_plugin_api::MetadataKey::new("onepipeline.publishing".to_owned())
        .expect("a key");
    let id: onetaskgraph_core::GlobalId = project.parse().expect("a qualified project");
    world
        .store_call(|engine| async move { engine.set_project_metadata(&id, &key, &value).await })
        .unwrap_or_else(|why| panic!("the store refused the write: {why}"));
}

/// A publication whose fence the board refuses to take starts nothing: the driver says so,
/// writes the fence again a few seconds later, and publishes once that write lands — one
/// change request — and clears it once the publication has settled.
///
/// Nothing reads the claim on a timer, so nothing but the retry wakes the driver to write it
/// again: the retry is due on its own clock, not at once — and a pass something else wakes
/// inside that window, here a surface queued on the run's channel, writes nothing.
#[test]
fn a_publication_whose_fence_the_board_refuses_starts_once_a_later_write_lands() {
    let world = takeover_world("fence-refused", 60).through_scripted_source();
    world.script("x.work", "x's change\n");
    let x = world.rendezvous("x");
    let run = "fence-refused";
    let project = author(&world, run, vec![lifecycle("x", &[])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    until_claimed_by(&world, &project, run);
    // The next project write the board is asked for is the fence x's publication needs.
    world.store_refuses_once("set_project_metadata", &board_down());
    mine.release();
    world.until_run_file_holds(
        run,
        "driver.log",
        "could not write its publications under way",
    );
    let refused_at = std::time::Instant::now();
    let refused = world.store_asked("set_project_metadata");
    world
        .run(&[
            "surface",
            run,
            "--kind",
            "check-in",
            "--message",
            "a pass inside the fence's retry",
        ])
        .exited(0);
    world.until("the fence to be written again", |world| {
        world.store_asked("set_project_metadata") > refused
    });
    let retried_after = refused_at.elapsed();
    assert!(
        retried_after >= std::time::Duration::from_secs(3),
        "the fence was written again {retried_after:?} after the refusal, before its retry was due"
    );
    assert!(
        retried_after < std::time::Duration::from_secs(30),
        "the fence was written again only {retried_after:?} after the refusal"
    );
    world.until("the driver to end", |world| {
        world.run_file(run, "result.json").is_file()
    });
    let x = settled(&world, run, "x");
    assert_eq!(x["outcome"], "change-review-draft", "{x}");
    assert_eq!(created_from(&world).len(), 1);
    assert_eq!(stored_publishing(&world, &project), Value::Null);
}

/// A takeover past its wait that finds the project's `onepipeline.publishing` holding
/// something that is not a driver's publications cannot tell whether the displaced driver
/// — here one a shutdown ended — has one under way, so it starts nothing and says why;
/// once that is cleared, it starts.
#[test]
fn a_takeover_finding_a_fence_it_cannot_read_waits_until_it_is_cleared() {
    a_takeover_waits_until_a_fence_it_cannot_read_is_cleared("fence-unreadable", |_| {
        json!("x is publishing")
    });
}

/// The same for a fence shaped as a driver's publications that names none: the displaced
/// run and its host, publishing no node, which no driver writes.
#[test]
fn a_takeover_finding_a_fence_naming_no_publication_waits_until_it_is_cleared() {
    a_takeover_waits_until_a_fence_it_cannot_read_is_cleared("fence-no-nodes", |claim| {
        assert!(
            claim["run"].is_string()
                && claim["host"].is_string()
                && claim["claimed_at"].is_string(),
            "{claim}"
        );
        json!({
            "run": claim["run"],
            "host": claim["host"],
            "nodes": [],
            "at": claim["claimed_at"],
        })
    });
}

/// Store the fence `stored` makes of the displaced driver's claim once a shutdown has
/// ended that driver, and show a takeover starting nothing until it is cleared.
fn a_takeover_waits_until_a_fence_it_cannot_read_is_cleared(
    name: &str,
    stored: impl FnOnce(&Value) -> Value,
) {
    let world = takeover_world(name, 5);
    world.script("x.work", "x's change\n");
    let x = world.rendezvous("x");
    let run = "unreadable";
    let project = author(&world, run, vec![lifecycle("x", &[])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    until_claimed_by(&world, &project, run);
    let shutdown = world.run(&["shutdown", run, "--grace", "1"]);
    assert!([0, 2].contains(&shutdown.code), "{}", shutdown.stderr);
    drop(mine);
    let fence = stored(&stored_claim(&world, &project));
    restate_publishing(&world, &project, fence);

    let successor = world.on_host("host-b");
    let taking = take_over_in_background(&successor, &project);
    until_claimed_on(&world, &project, "host-b");
    std::thread::sleep(takeover_wait(1) + std::time::Duration::from_secs(5));
    assert!(
        successor.run_file(run, "launch.json").metadata().is_err(),
        "the adopter minted its run past a fence it could not read"
    );
    restate_publishing(&world, &project, Value::Null);
    let said = launched_over(taking);
    assert!(
        said.contains("is not a driver's publications") && said.contains("may be under way"),
        "{said}"
    );
    assert!(
        said.contains("and none of its publications is under way"),
        "{said}"
    );
    x.arrived().release();
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(
        settled(&successor, run, "x")["outcome"],
        "change-review-draft"
    );
    one_change_request_per_branch(&world);
    drop(successor);
}

/// A superseded driver whose read before it clears its fence the board refuses leaves the
/// fence for that pass, says so, and still acknowledges once its publication has settled;
/// the adopter starts on that, and its own fence replaces the one left standing.
///
/// The driver reads the takeover before `z`'s publication, withholding it, while `x`'s was
/// already under way: it is `x`'s settling that leaves it a fence to clear.
#[test]
fn a_superseded_driver_whose_read_before_clearing_its_fence_fails_still_hands_over() {
    let world = World::new("fence-clear-unread")
        .through_scripted_source()
        .with_env("ONEVCS_BRANCH_PREFIX", PREFIX)
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, &PATIENT_GRACE.to_string());
    let go = world.fakes.join("push.go");
    let held = held_publication(&world, &go);
    world.repository("change-open", &held.argv());
    rules(&world, "change-open", "required", None);
    world.script("gh.checks", GREEN);
    world.script("x.work", "x's change\n");
    world.script("z.work", "z's change\n");
    let z = world.rendezvous("z");
    let successor = world.on_host("host-b");
    let run = "clear-unread";
    let mut plan = plan_of(run, vec![lifecycle("x", &[]), lifecycle("z", &[])]);
    plan["concurrency"] = json!(2);
    let project = world.plan(run, &plan);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = z.arrived();
    world.until("x's publication to be under way", |_| held.holding());
    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    mine.release();
    world.until("the displaced driver to read the takeover", |world| {
        !world.events_of(run, "driver-superseded").is_empty()
    });
    superseded_by(&world, run, &claim);
    world.store_refuses("get_project", &board_down());
    held.release();
    world.until_run_file_holds(run, "driver.log", "could not read its publications");
    world.store_stops_refusing("get_project");
    let said = launched_over(taking);
    assert!(said.contains("acknowledged the takeover"), "{said}");
    z.arrived().release();
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    let mut theirs = dispatched(&world, run);
    theirs.sort();
    assert_eq!(theirs, vec!["x".to_owned(), "z".to_owned()]);
    assert_eq!(dispatched(&successor, run), vec!["z".to_owned()]);
    in_turn(&world, &successor, run);
    one_change_request_per_branch(&world);
    assert_eq!(stored_publishing(&world, &project), Value::Null);
    drop(successor);
    drop(held);
}

/// A claim stating no lease — as a build that kept none wrote it — names a driver whose
/// fence this build cannot vouch for, so its takeover starts only on that driver's
/// acknowledgement: well past the wait the adopter's own terms would give, while the
/// displaced driver's dispatch is still running, it has minted nothing; once the dispatch
/// has settled the driver closes out, acknowledging, and the adopter starts on that.
#[test]
fn a_takeover_over_a_claim_stating_no_lease_starts_only_on_an_acknowledgement() {
    let world = World::new("takeover-unleased")
        .with_env(CLAIM_INTERVAL_ENV, "30")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let x = world.rendezvous("x");
    let run = "unleased";
    let project = launch_detached(&world, run, vec![agent("x", &[])]);
    let mine = x.arrived();
    let mut claim = until_claimed_by(&world, &project, run);
    claim
        .as_object_mut()
        .expect("a claim")
        .remove("lease")
        .expect("this build's claim states its lease");
    let key =
        onetaskgraph_plugin_api::MetadataKey::new("onepipeline.driver".to_owned()).expect("a key");
    let id: onetaskgraph_core::GlobalId = project.parse().expect("a qualified project");
    let written = claim.clone();
    world
        .store_call(|engine| async move { engine.set_project_metadata(&id, &key, &written).await })
        .unwrap_or_else(|why| panic!("the store refused the write: {why}"));

    let successor = world
        .on_host("host-b")
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let taking = take_over_in_background(&successor, &project);
    until_claimed_on(&world, &project, "host-b");
    std::thread::sleep(takeover_wait(1) + std::time::Duration::from_secs(5));
    assert!(
        successor.run_file(run, "launch.json").metadata().is_err(),
        "the adopter minted its run over a claim stating no lease, which nothing acknowledged"
    );
    mine.release();
    let said = launched_over(taking);
    assert!(
        said.contains(&format!(
            "run '{run}' on host '{}' states no lease, so no wait outlasts it",
            claim["host"].as_str().expect("a host")
        )),
        "{said}"
    );
    assert!(said.contains("acknowledged the takeover"), "{said}");
    assert!(!said.contains("has not acknowledged in"), "{said}");
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(dispatched(&world, run), vec!["x".to_owned()]);
    assert!(dispatched(&successor, run).is_empty());
    handed_over(&world, &successor, run);
    drop(successor);
}

/// A live displaced driver that has not read the takeover by the time its wait has passed
/// may still start a dispatch after the adopter has begun — no dispatch reads the claim —
/// but that dispatch publishes nothing: the read before its publication finds the adopter's
/// claim, so the driver withholds it with its branch kept, stops, and acknowledges. The
/// adopter's publication of that node is the only change request opened.
#[test]
fn a_displaced_driver_that_has_not_read_the_takeover_publishes_nothing_it_starts_after_it() {
    let world = takeover_world("takeover-unread", 5);
    world.script("y.work", "y's change\n");
    let x = world.rendezvous("x");
    let y = world.rendezvous("y");
    let run = "unread";
    let project = author(&world, run, vec![agent("x", &[]), lifecycle("y", &["x"])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let theirs_x = x.arrived();
    until_claimed_by(&world, &project, run);
    let successor = world.on_host("host-b");
    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    // Nothing of the displaced driver's is fenced, so the adopter starts once its wait has
    // passed, while the displaced driver's x still runs.
    let said = launched_over(taking);
    assert!(
        said.contains(&format!(
            "has not acknowledged in {}s, and none of its publications is under way",
            takeover_wait(1).as_secs()
        )),
        "{said}"
    );
    let adopters_x = x.arrived();
    let began = successor
        .events_of(run, "node-dispatched")
        .first()
        .map(|event| event["ts"].as_str().expect("a record's time").to_owned())
        .expect("the adopter dispatched x");

    // The displaced driver goes on: x finishes, and it starts y after the adopter began.
    theirs_x.release();
    let theirs_y = y.arrived();
    let started = world
        .events_of(run, "node-dispatched")
        .into_iter()
        .find(|event| event["labels"]["node"] == "y")
        .map(|event| event["ts"].as_str().expect("a record's time").to_owned())
        .expect("the displaced driver dispatched y");
    assert!(
        started >= began,
        "the displaced driver dispatched y at {started}, before the adopter began at {began}"
    );
    theirs_y.release();
    world.until("the displaced driver to end", |world| {
        world.run_file(run, "result.json").is_file()
    });
    // By then the adopter's driver has written its own claim over the takeover's.
    let read = world.events_of(run, "driver-superseded");
    assert_eq!(read.len(), 1, "{read:?}");
    let by = &read[0]["payload"]["by"];
    assert_eq!(
        (&by["run"], &by["host"]),
        (&claim["run"], &claim["host"]),
        "{by}"
    );
    let withheld = settled(&world, run, "y");
    assert_eq!(withheld["status"], "cancelled", "{withheld}");
    assert!(
        withheld["detail"]
            .as_str()
            .is_some_and(|detail| detail.starts_with("not published")),
        "{withheld}"
    );
    assert!(
        created_from(&world).is_empty(),
        "the displaced driver opened a change request"
    );

    adopters_x.release();
    y.arrived().release();
    successor.until("the adopter to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(
        settled(&successor, run, "y")["outcome"],
        "change-review-draft"
    );
    one_change_request_per_branch(&world);
    assert_eq!(created_from(&world).len(), 1);
    drop(successor);
}

/// A displaced driver that died without acknowledging — a shutdown ended it mid-dispatch,
/// leaving its claim — holds the adopter only until the takeover's wait has passed with no
/// publication of it under way; the adopter then continues the node on the branch its
/// dispatch was on.
#[test]
fn a_takeover_whose_displaced_driver_died_proceeds_once_its_wait_has_passed() {
    let world = takeover_world("takeover-died", 5);
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
    // A claim somebody clears while it waits is no answer: it goes on waiting.
    until_claimed_on(&world, &project, "host-b");
    let key =
        onetaskgraph_plugin_api::MetadataKey::new("onepipeline.driver".to_owned()).expect("a key");
    let id: onetaskgraph_core::GlobalId = project.parse().expect("a qualified project");
    world
        .store_call(
            |engine| async move { engine.set_project_metadata(&id, &key, &Value::Null).await },
        )
        .unwrap_or_else(|why| panic!("the store refused the write: {why}"));
    let said = launched_over(taking);
    assert!(
        since.elapsed() >= takeover_wait(1),
        "the adopter started {:?} after it launched, inside the wait",
        since.elapsed()
    );
    assert!(
        said.contains(&format!(
            "run '{run}' on host '{}' has not acknowledged in {}s, and none of its \
             publications is under way",
            claim["host"].as_str().expect("a host"),
            takeover_wait(1).as_secs()
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
/// finds before its publication, withholds it, acknowledges the second, and the second
/// starts on that.
#[test]
fn a_takeover_replaced_while_it_waits_is_refused_and_the_one_that_replaced_it_starts() {
    let world = patient_world("takeover-replaced");
    world.script("x.work", "x's change\n");
    let first = world.on_host("host-b");
    let second = world.on_host("host-c");
    let x = world.rendezvous("x");
    let run = "replaced";
    let project = author(&world, run, vec![lifecycle("x", &[])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    until_claimed_by(&world, &project, run);
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
    mine.release();
    world.until("the driver to read the takeover", |world| {
        !world.events_of(run, "driver-superseded").is_empty()
    });
    superseded_by(&world, run, &claim);
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
/// has passed with none of that driver's publications under way.
#[test]
fn a_takeover_waits_out_a_board_that_refuses_a_poll_and_the_acknowledgement() {
    // A displaced host's grace long enough that its withheld publication has settled well
    // inside the wait.
    let grace = 15;
    let base = takeover_world("takeover-unanswered", 5)
        .through_scripted_source()
        .with_env(crate::harness::CANCEL_GRACE_ENV, &grace.to_string());
    base.script("x.work", "x's change\n");
    let successor = base
        .on_host("host-b")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    // llmlint: ignore-block[tests_mirror_real_usage] the property is the displaced driver's
    // acknowledgement failing, which only a board refusing that one write shows, and no
    // user-facing input places a refusal on it: the failpoint holds the projection the
    // acknowledgement waits for while the refusal is arranged. It only orders the real binary
    // over the real store, origin and GitHub double, and every outcome is asserted on the
    // binary's own records.
    let projecting = crate::harness::Rendezvous::failpoint();
    let world = base
        .with_env("ONEPIPELINE_TEST_PROJECTION_SETTLED_NODE", "x")
        .with_env(
            "ONEPIPELINE_TEST_PROJECTION_SETTLED_GATE",
            &projecting.address(),
        );
    // llmlint: ignore-end[tests_mirror_real_usage]
    let x = world.rendezvous("x");
    let run = "unanswered";
    let project = author(&world, run, vec![lifecycle("x", &[])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let mine = x.arrived();
    until_claimed_by(&world, &project, run);
    let taking = take_over_in_background(&successor, &project);
    let claim = until_claimed_on(&world, &project, "host-b");
    world.store_refuses_once("get_project", &board_down());
    world.until("the takeover's poll to be refused", |world| {
        !refusal_pending(world, "get_project")
    });
    // x finishes, the read before its publication finds the takeover, and x is withheld;
    // its settlement's projection, which the acknowledgement waits on, is held while the
    // board is set to refuse that acknowledgement.
    mine.release();
    let projection = projecting.arrived();
    superseded_by(&world, run, &claim);
    world.until("the fence to be cleared", |world| {
        stored_publishing(world, &project).is_null()
    });
    world.store_refuses_once("set_project_metadata", &board_down());
    projection.release();
    world.until_run_file_holds(run, "driver.log", "could not acknowledge run 'unanswered'");
    let said = launched_over(taking);
    assert!(
        said.contains("could not read the claim") && said.contains("it goes on waiting"),
        "{said}"
    );
    assert!(
        said.contains(&format!(
            "has not acknowledged in {}s, and none of its publications is under way",
            takeover_wait(grace).as_secs()
        )),
        "{said}"
    );
    assert!(!refusal_pending(&world, "set_project_metadata"));
    in_turn(&world, &successor, run);
    drop(successor);
}

/// How the store answers a driver's reads of its claim, from a moment a dispatch is in
/// flight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ClaimAnswer {
    /// Every read is refused: the board is down.
    Fails,
    /// Every read is held past its twenty-second deadline.
    TimesOut,
    /// Every read is refused as rate limited.
    RateLimited,
    /// Every read answers, and finds no claim on the project.
    NoClaim,
}

/// A claim read that fails ends no dispatch: with `x` in flight for longer than the lapse the
/// claim's old lease had — two of the intervals the setting still names and a read's deadline
/// — `x` finishes, the next dispatch starts, and only its publication waits, until a read
/// answers; while it waits, another dispatch starts and settles.
#[test]
fn a_claim_read_that_fails_ends_no_dispatch_and_holds_only_its_publication() {
    a_claim_read_answering("claim-read-fails", ClaimAnswer::Fails);
}

/// The same for a claim read the store holds past its deadline.
#[test]
fn a_claim_read_that_times_out_ends_no_dispatch_and_holds_only_its_publication() {
    a_claim_read_answering("claim-read-times-out", ClaimAnswer::TimesOut);
}

/// The same for a claim read the store refuses as rate limited.
#[test]
fn a_rate_limited_claim_read_ends_no_dispatch_and_holds_only_its_publication() {
    a_claim_read_answering("claim-read-rate-limited", ClaimAnswer::RateLimited);
}

/// A claim read that finds no claim — as one whose board update never landed would — ends
/// no dispatch either, and lets the publication it was made for go ahead.
#[test]
fn a_claim_read_finding_no_claim_ends_no_dispatch_and_publishes() {
    a_claim_read_answering("claim-read-no-claim", ClaimAnswer::NoClaim);
}

/// Drive `answer` on a run of `x` and then `y`, which publishes, beside `w` and then `z`, from
/// the moment `x` is in flight: `w` is held until `y`'s publication is waiting, so `z` starts
/// while it waits.
fn a_claim_read_answering(name: &str, answer: ClaimAnswer) {
    use std::time::Duration;
    // The interval the claim's old lease ran on: it lapsed at two of them and a read's
    // twenty-second deadline, which `x` is held well past.
    let interval = 2;
    let lapsed = Duration::from_secs(2 * interval + 20);
    let world = takeover_world(name, interval).through_scripted_source();
    world.script("y.work", "y's change\n");
    let x = world.rendezvous("x");
    let w = world.rendezvous("w");
    let run = "answered";
    let mut plan = plan_of(
        run,
        vec![
            agent("x", &[]),
            lifecycle("y", &["x"]),
            agent("w", &[]),
            agent("z", &["w"]),
        ],
    );
    plan["concurrency"] = json!(3);
    let project = world.plan(run, &plan);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let in_flight = x.arrived();
    let beside = w.arrived();
    until_claimed_by(&world, &project, run);
    let held = match answer {
        ClaimAnswer::Fails => {
            world.store_refuses("get_project", &board_down());
            None
        }
        ClaimAnswer::TimesOut => Some(world.store_holds("get_project")),
        ClaimAnswer::RateLimited => {
            world.store_refuses(
                "get_project",
                &json!({"kind": "rate-limited", "retry_after_seconds": 3,
                        "message": "You have exceeded a secondary rate limit"}),
            );
            None
        }
        ClaimAnswer::NoClaim => {
            let key = onetaskgraph_plugin_api::MetadataKey::new("onepipeline.driver".to_owned())
                .expect("a key");
            let id: onetaskgraph_core::GlobalId = project.parse().expect("a qualified project");
            world
                .store_call(|engine| async move {
                    engine.set_project_metadata(&id, &key, &Value::Null).await
                })
                .unwrap_or_else(|why| panic!("the store refused the write: {why}"));
            None
        }
    };
    // `x` runs well past the old lapse, and nothing ends it.
    std::thread::sleep(lapsed + Duration::from_secs(6));
    let ended = |world: &World| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "x")
    };
    assert!(!ended(&world), "{answer:?}: x was ended while it ran");
    in_flight.release();
    world.until("x to settle", ended);
    assert_eq!(settled(&world, run, "x")["status"], "done", "{answer:?}");
    world.until("y to be dispatched", |world| {
        dispatched(world, run).contains(&"y".to_owned())
    });
    let published = |world: &World| {
        world
            .journal(run)
            .iter()
            .any(|event| event["kind"] == "change-opened")
    };
    // While y's publication waits, w finishes and z starts and settles beside it.
    let z_settled = |world: &World| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "z")
    };
    match (answer, held) {
        (ClaimAnswer::NoClaim, _) => {
            beside.release();
            world.until("z to settle", z_settled);
        }
        (ClaimAnswer::TimesOut, Some(held)) => {
            // y's publication waits on the read that does not answer, and on no other.
            let read = held.arrived();
            beside.release();
            world.until("z to settle", z_settled);
            world.until_run_file_holds(run, "driver.log", "did not answer within 20 seconds");
            assert!(
                !published(&world),
                "y published on a read that did not answer"
            );
            world.store_stops_holding("get_project");
            read.release();
        }
        (_, _) => {
            world.until_run_file_holds(run, "driver.log", "before it publishes");
            beside.release();
            world.until("z to settle", z_settled);
            assert!(
                !published(&world),
                "{answer:?}: y published on a read that did not answer"
            );
            world.store_stops_refusing("get_project");
        }
    }
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert!(published(&world), "{answer:?}");
    let mut started = dispatched(&world, run);
    started.sort();
    assert_eq!(started, ["w", "x", "y", "z"], "{answer:?}");
    assert_eq!(settled(&world, run, "z")["status"], "done", "{answer:?}");
    assert_eq!(
        settled(&world, run, "y")["outcome"],
        "change-review-draft",
        "{answer:?}"
    );
    assert!(
        world.events_of(run, "driver-superseded").is_empty(),
        "{answer:?}"
    );
    let log = std::fs::read_to_string(world.run_file(run, "driver.log")).unwrap_or_default();
    assert!(
        !log.contains("ends its dispatches in progress"),
        "{answer:?}: {log}"
    );
    one_change_request_per_branch(&world);
}

/// A running driver reads its claim only immediately before a publication, however long it
/// runs and however many passes it makes: across a dispatch held for ten of the intervals
/// it once read on, woken five times a second, it reads it not once — counted both by the
/// driver's own report and by the store's own record of the project reads it was asked for
/// — and once the dispatch has finished it reads it once, before that dispatch's
/// publication.
#[test]
fn a_running_driver_reads_its_claim_only_immediately_before_a_publication() {
    use std::time::Duration;
    let interval = 1;
    let world = takeover_world("claim-reads", interval)
        .through_scripted_source()
        .with_env(crate::harness::LOOP_STATS_ENV, "1");
    world.script("chatty.work", "chatty's change\n");
    // A dispatch narrating five times a second: a pass for every beat.
    world.script("chatty.heartbeat", "200");
    world.script("chatty.wait", "hold");
    let run = "reads";
    let project = author(&world, run, vec![lifecycle("chatty", &[])]);
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    until_claimed_by(&world, &project, run);
    world.until("chatty to be dispatched", |world| {
        dispatched(world, run) == vec!["chatty".to_owned()]
    });
    crate::harness::reporting(&world, run);

    let before = crate::harness::counts(&world, run);
    let asked = world.store_asked("get_project");
    std::thread::sleep(Duration::from_secs(10 * interval));
    let did = crate::harness::counts(&world, run).since(before);
    let store = world.store_asked("get_project") - asked;
    assert_eq!(did.claim_reads, 0, "the driver read its claim: {did:?}");
    assert_eq!(
        store, 0,
        "the store was asked for the project {store} times"
    );
    assert!(
        did.passes >= 10,
        "the loop ran too few passes for its reads to say anything: {did:?}"
    );
    eprintln!(
        "claim reads across ten of the old intervals: {} by the driver's count, {store} by the \
         store's, over {} passes",
        did.claim_reads, did.passes
    );

    world.release("chatty.go");
    world.until("the publication to be read for", |world| {
        world
            .journal(run)
            .iter()
            .any(|event| event["kind"] == "change-opened")
    });
    world.until("the driver to report its read", |world| {
        crate::harness::counts(world, run).claim_reads > before.claim_reads
    });
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    let all = crate::harness::counts(&world, run).since(before);
    assert_eq!(
        all.claim_reads, 1,
        "the driver read its claim other than once before its publication: {all:?}"
    );
    assert_eq!(
        settled(&world, run, "chatty")["outcome"],
        "change-review-draft"
    );
}

/// Two cold hosts continue one project whose nodes hold open drafts, launched at the same
/// moment over the one store: at least one launches, neither opens a change request, and
/// each draft is still the only one for its branch and still a draft. The claim is not
/// atomic, so both may drive; neither publishes, so neither reads the claim again until it
/// lets go, and the store's claim is then nobody's or one of theirs.
#[test]
fn two_cold_hosts_launching_one_project_at_once_leave_every_draft_alone() {
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
    // A node a planner added that keeps a driver driving, so both are live at once.
    world.add_task(run, "003-hold", &agent("hold", &[]), &[]);
    world.script("hold.wait", "hold");
    remove_the_runs_root(&world);

    let hosts: Vec<World> = ["host-b", "host-c"]
        .into_iter()
        .map(|host| world.on_host(host))
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
    world.release("hold.go");
    for (host, launched) in hosts.iter().zip(&launched) {
        if *launched {
            host.until("each driver to end", |host| {
                host.run_file(run, "result.json").is_file()
            });
        }
    }
    let claim = stored_claim(&world, &project);
    assert!(
        claim.is_null() || ["host-b", "host-c"].contains(&claim["host"].as_str().unwrap_or("")),
        "the store's claim is neither racing host's: {claim}"
    );
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
    let mut unshaped = claim.clone();
    unshaped["acknowledged"] = json!("yes");
    let mut overlong = claim.clone();
    overlong["acknowledged"] = json!({"run": "older", "host": "builder", "at": at, "pid": 9});
    let mut nameless = claim.clone();
    nameless["acknowledged"] = json!({"run": "", "host": "builder", "at": at});
    let mut instant = claim.clone();
    instant["lease"] = json!({"interval_seconds": 0, "cancel_grace_seconds": 1});
    let mut endless = claim.clone();
    endless["lease"] = json!({"interval_seconds": 86_401, "cancel_grace_seconds": 1});
    let mut graceless = claim.clone();
    graceless["lease"] = json!({"interval_seconds": 5, "cancel_grace_seconds": 0});
    let mut padded = claim.clone();
    padded["lease"] = json!({"interval_seconds": 5, "cancel_grace_seconds": 1, "slack": 9});
    let claims = [
        (json!({"run": "elsewhere"}), "missing field"),
        (unnamed, "`host`"),
        (unrun, "`run`"),
        (unsessioned, "`session`"),
        (json!("elsewhere"), "not a mapping"),
        (leased, "its `lease` is 30, not a mapping"),
        (instant, "expected a nonzero"),
        (endless, "a 86401s interval"),
        (graceless, "expected a nonzero"),
        (padded, "its `lease` carries `slack`"),
        (untimed, "not an RFC 3339 instant"),
        (unacknowledged, "its `acknowledged`: missing field"),
        (untimely, "not a run, a host and an instant"),
        (unshaped, "its `acknowledged`: invalid type"),
        (overlong, "its `acknowledged`: unknown field"),
        (nameless, "not a run, a host and an instant"),
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
    // driver may still stand behind it whose lease nothing states, starts only once one
    // acknowledges: well past the wait a claim stating this host's terms would be given, it
    // has minted nothing.
    let world = world
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
    let taking = take_over_in_background(&world, &project);
    let host = world.run_json(run, "launch.json")["host"].clone();
    world.until("the takeover's claim to reach the store", |world| {
        stored_claim(world, &project)["host"] == host
    });
    std::thread::sleep(takeover_wait(5) + std::time::Duration::from_secs(5));
    assert!(
        !world.run_file(&format!("{run}-2"), "launch.json").exists(),
        "the takeover minted a run over a claim stating no lease, which nothing acknowledged"
    );
    let mut taking = taking;
    taking.kill().expect("the waiting takeover is ended");
    let said =
        String::from_utf8_lossy(&taking.wait_with_output().expect("it ends").stderr).into_owned();
    assert!(
        said.contains("--take-over passes a claim that is not one"),
        "{said}"
    );
    assert!(
        said.contains(
            "the driver its claim named states no lease, so no wait outlasts it: it starts once \
             that driver has acknowledged"
        ),
        "{said}"
    );
    assert!(!said.contains("has not acknowledged in"), "{said}");
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
    remove_the_runs_root(&world);
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
/// the driver, and says why on its log — and holds nothing else: an adoption whose first
/// read is held goes on driving once it has given up on it, and reads the claim no more.
#[test]
fn an_adoption_whose_claim_read_is_not_answered_goes_on_driving() {
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
    world.until_run_file_holds("deadline", "driver.log", "it goes on driving");
    world.until(
        "the adoption to dispatch past its unanswered read",
        |world| world.events_of("deadline", "node-dispatched").len() == 2,
    );
    assert!(
        since.elapsed() < std::time::Duration::from_secs(30),
        "the adoption dispatched {:?} after it began, waiting for a read",
        since.elapsed()
    );
    let asked = world.store_asked("get_project");
    world.store_stops_holding("get_project");
    read.release();
    until_claimed_by(&world, &project, "deadline");
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert_eq!(
        world.store_asked("get_project"),
        asked,
        "the adoption read its claim again"
    );
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
    let world = patient_world("superseded-kept");
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
    // l finishes, and the read before its publication finds the takeover.
    mine.release();
    world.until("the first driver to notice the takeover", |world| {
        !world.events_of("kept", "driver-superseded").is_empty()
    });
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

/// An adoption reads the claim on its first pass, before it claims anything: a run adopted
/// on this host after another host took its project over supersedes at once — it projects nothing onto the
/// store, dispatches nothing, journals `driver-superseded` naming the successor's claim, and
/// lets go with that claim on the store as it was.
#[test]
fn an_adoption_after_a_takeover_supersedes_before_it_projects_or_dispatches() {
    // The terms its claim states are the ones the takeover waits out.
    let world = World::new("adopt-superseded")
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, "1");
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
