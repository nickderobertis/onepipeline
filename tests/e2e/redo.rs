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

/// A redone node's dependents follow it: one that settled `done` is redone with it, recorded
/// as its dependent, and one with no settlement waits on it, as any dependent waits, and
/// dispatches once it has settled again.
#[test]
fn a_redone_nodes_done_dependents_are_redone_with_it_and_the_rest_wait_on_it() {
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
    launch(&world, &again, &project, &["--redo", "base"])
        .exited(0)
        .err_has("redoes node 'child', as a done dependent of redone node 'base'");
    assert_eq!(
        dispatched(&world, &again),
        vec!["base".to_owned(), "child".to_owned(), "late".to_owned()]
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
    for dependent in ["child", "late"] {
        assert!(
            at("node-settled", "base") < at("node-dispatched", dependent),
            "{dependent} was dispatched before base settled again"
        );
    }
    assert_eq!(
        world.run_json(&again, "launch.json")["redone"],
        json!([
            {"node": "base", "reason": "named"},
            {"node": "child", "reason": "dependent", "of": "base"},
        ])
    );
    let after = stored_settlements(&world, &project);
    assert_eq!(
        after["child"]["branch"], before["child"]["branch"],
        "{after:#?}"
    );
    assert_ne!(
        after["child"]["head"], before["child"]["head"],
        "{after:#?}"
    );
    assert_eq!(after["late"]["status"], "done", "{after:#?}");
    assert_eq!(world.run_json(&again, "result.json")["state"], "complete");
}

/// The store task of `node` in `project`, as the store answers it.
fn task_of(world: &World, project: &str, node: &str) -> Value {
    world
        .store_tasks(project)
        .into_iter()
        .find(|task| task["item"]["metadata"]["onepipeline.id"] == node)
        .unwrap_or_else(|| panic!("{project} holds no task of node {node}"))
}

/// Write `update` to `node`'s task in `project` through the store's own engine, as a person or
/// a host tool editing the board would.
fn update_task(
    world: &World,
    project: &str,
    node: &str,
    update: onetaskgraph_plugin_api::TaskUpdate,
) {
    let id = crate::harness::global(
        task_of(world, project, node)["id"]
            .as_str()
            .expect("a task id"),
    );
    world
        .store_call(|engine| async move { engine.update_task(&id, &update).await })
        .unwrap_or_else(|why| panic!("the store takes the edit of {node}: {why}"));
}

/// Revise what `node`'s task asks, by adding a sentence to its body.
fn revise(world: &World, project: &str, node: &str, sentence: &str) {
    let task = task_of(world, project, node);
    let body = task["item"]["content"].as_str().unwrap_or_default();
    update_task(
        world,
        project,
        node,
        onetaskgraph_plugin_api::TaskUpdate {
            content: Some(format!("{body}\n\n{sentence}")),
            ..onetaskgraph_plugin_api::TaskUpdate::default()
        },
    );
}

fn metadata_key(key: &str) -> onetaskgraph_plugin_api::MetadataKey {
    onetaskgraph_plugin_api::MetadataKey::new(key).expect("a metadata key")
}

/// Whether `run`'s projection record holds a landed attempt, started no earlier than `since`,
/// that wrote metadata on `node`'s item: the write-back appends each attempt's line once the
/// store has answered it.
fn projected_metadata_since(world: &World, run: &str, node: &str, since: &str) -> bool {
    let since = crate::harness::epoch_millis(since);
    std::fs::read_to_string(world.run_file(run, "writeback-projections.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|attempt| {
            attempt["outcome"] == "projected"
                && attempt["at"]
                    .as_str()
                    .is_some_and(|at| crate::harness::epoch_millis(at) >= since)
                && attempt["items"]
                    .as_array()
                    .is_some_and(|items| items.contains(&json!(node)))
                && attempt["updated_fields"].get("metadata").is_some()
        })
}

/// Each `(node, reason, of)` the run's launch record says it redid, in its order.
fn redone(world: &World, run: &str) -> Vec<Value> {
    world.run_json(run, "launch.json")["redone"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The redo of changed work, end to end: a lifecycle chain `a → b → c` each kept on a branch
/// as a draft nothing has landed, a direct node `d` on `a`, and a lifecycle node `l` in another
/// repository whose work landed. A launch with no edit — and one after a run-written key, an
/// attestation and a copy link land on tasks — redoes nothing; once `a`'s and `l`'s tasks are
/// revised, `--keep-changed` redoes nothing either, and a launch without it redoes `a` on its
/// branch and `b`, `c` and `d` as its dependents — `d` afresh — leaves `l` settled and says why, and records on each new settlement the digest of
/// the task it was dispatched with. A settlement stripped of its digest counts as unchanged,
/// `--redo b` redoes `b` and `c` and continues `a`, and a task revised while its node runs is
/// redone by the next launch.
#[test]
fn a_launch_redoes_each_node_whose_task_changed_and_its_done_dependents() {
    let name = "redo-changed";
    let base = team_world(name);
    for id in ["a", "b", "c", "d", "l"] {
        base.script(&format!("{id}.work-anew"), &format!("{id}'s change"));
    }
    // A second repository whose work lands: the team's own opens drafts nothing merges.
    base.extra_repository("engine");
    std::fs::write(
        base.onevcs_home().join("rules.yml"),
        "version: 3\nrules:\n  - match: {name: engine}\n    publication: local-direct\n    \
         approvals: none\ndefault:\n  publication: change-open\n  approvals: required\n",
    )
    .expect("the rules file is written");
    let origin = base.root.join("origin.git");
    // A second `local-md` source a task is copied into, so its copy link is a real one.
    let elsewhere = base.store_apart("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("the second source's root");
    let world = base
        .with_env("ONETASKGRAPH_SOURCES__ELSEWHERE__PLUGIN", "local-md")
        .with_env(
            "ONETASKGRAPH_SOURCES__ELSEWHERE__CONFIG__ROOT",
            elsewhere.to_str().expect("a path"),
        );
    let mut landed = lifecycle("l", &[]);
    landed["repo"] = json!("engine");
    let project = author(
        &world,
        name,
        vec![
            lifecycle("a", &[]),
            lifecycle("b", &["a"]),
            lifecycle("c", &["b"]),
            agent("d", &["a"]),
            landed,
        ],
    );
    launch(&world, name, &project, &[]).exited(0);
    let first = stored_settlements(&world, &project);
    for id in ["a", "b", "c", "d", "l"] {
        assert_eq!(first[id]["status"], "done", "{id}: {first:#?}");
        let digest = first[id]["task_digest"]
            .as_str()
            .unwrap_or_else(|| panic!("{id}'s settlement records no task digest: {first:#?}"));
        assert!(
            digest.starts_with("sha256:") && digest.len() == 71,
            "{id}: {digest}"
        );
        assert_eq!(
            json!(digest),
            world.run_json(name, "launch.json")["task_digests"][id],
            "{id}'s settlement is not the digest of the task it was dispatched with"
        );
    }
    assert_eq!(first["l"]["landing"], "landed", "{first:#?}");
    for id in ["a", "b", "c"] {
        assert_eq!(first[id]["landing"], "unlanded", "{id}: {first:#?}");
        assert!(first[id]["branch"].is_string(), "{id}: {first:#?}");
    }
    assert!(first["d"].get("branch").is_none(), "{first:#?}");

    // Nothing edited: the write-back's own keys, written by the run just ended, change nothing.
    let unedited = format!("{name}-2");
    launch(&world, &unedited, &project, &[]).exited(0);
    assert_eq!(dispatched(&world, &unedited), Vec::<String>::new());
    assert!(redone(&world, &unedited).is_empty());

    // A run-written key, an attestation and a copy link: none is a change to a task.
    update_task(
        &world,
        &project,
        "b",
        onetaskgraph_plugin_api::TaskUpdate {
            metadata_set: [(
                metadata_key("onepipeline.amendment"),
                json!("Keep the export format stable."),
            )]
            .into_iter()
            .collect(),
            ..onetaskgraph_plugin_api::TaskUpdate::default()
        },
    );
    let attested = crate::harness::global(
        task_of(&world, &project, "c")["id"]
            .as_str()
            .expect("an id"),
    );
    world
        .store_call(|engine| async move {
            engine
                .attest(
                    onetaskgraph_plugin_api::MetadataRecord::Task,
                    &onetaskgraph_core::AttestRequest {
                        id: attested,
                        kind: onetaskgraph_core::attestation::AttestationKind::new("review")
                            .expect("a kind"),
                        by: onetaskgraph_core::attestation::AttestationLabel::new("by", "reviewer")
                            .expect("a label"),
                        bar: onetaskgraph_core::attestation::AttestationLabel::new("bar", "bar-1")
                            .expect("a label"),
                        excludes: Vec::new(),
                        note: None,
                        if_version: None,
                    },
                )
                .await
        })
        .expect("the store records the attestation");
    let copied = crate::harness::global(
        task_of(&world, &project, "d")["id"]
            .as_str()
            .expect("an id"),
    );
    world.store_call(|engine| async move {
        engine
            .copy(&onetaskgraph_core::CopyRequest {
                items: onetaskgraph_core::CopyItems::new(vec![copied]).expect("one item"),
                scope: onetaskgraph_core::CopyScope::Tasks,
                destination: onetaskgraph_plugin_api::SourceName::new("elsewhere")
                    .expect("a source"),
                status_policy: onetaskgraph_core::StatusPolicy::Source,
                target_task: None,
                match_by: None,
                recreate: false,
                create: true,
                dry_run: false,
                comments: false,
                no_link: false,
                expect: None,
                statuses: Vec::new(),
                conditions: onetaskgraph_core::CopyConditions::default(),
            })
            .await
            .expect("the store copies the task")
    });
    let linked = task_of(&world, &project, "d");
    assert!(
        linked["item"]["metadata"]
            .get("onetaskgraph.copies")
            .is_some(),
        "the copy left no link on its source: {linked}"
    );
    let bystanders = format!("{name}-3");
    launch(&world, &bystanders, &project, &[]).exited(0);
    assert_eq!(dispatched(&world, &bystanders), Vec::<String>::new());

    // `a`'s and `l`'s tasks are revised: kept with `--keep-changed`, and without it `a` is
    // redone with its dependents while `l`, whose work landed, stays settled.
    revise(&world, &project, "a", "Also cover the export.");
    revise(&world, &project, "l", "Also cover the engine.");
    let keeping = format!("{name}-4");
    launch(&world, &keeping, &project, &["--keep-changed"]).exited(0);
    assert_eq!(dispatched(&world, &keeping), Vec::<String>::new());
    assert_eq!(
        world.run_json(&keeping, "launch.json")["keep_changed"],
        json!(true)
    );
    world
        .run_from(
            &world.project,
            &["start", &project, "--keep-changed", "--fresh"],
        )
        .exited(2)
        .err_has("--keep-changed refused: --fresh");
    // A config declaring a schema before the key's is refused by the key's name.
    let early = world.root.join("keep-changed-early.yaml");
    std::fs::write(&early, "schema_version: 14\nkeep_changed: true\n")
        .expect("the launch config is written");
    world
        .run_from(
            &world.project,
            &[
                "start",
                &project,
                "--launch-config",
                &early.to_string_lossy(),
            ],
        )
        .exited(2)
        .err_has("`keep_changed` is a schema 15 key");
    // The launch config's `keep_changed` says the same, and is refused beside `--fresh` too.
    let config = world.root.join("keep-changed.yaml");
    std::fs::write(&config, "schema_version: 15\nkeep_changed: true\n")
        .expect("the launch config is written");
    let config = config.to_string_lossy().into_owned();
    let configured = format!("{name}-5");
    launch(&world, &configured, &project, &["--launch-config", &config]).exited(0);
    assert_eq!(dispatched(&world, &configured), Vec::<String>::new());
    assert_eq!(
        world.run_json(&configured, "launch.json")["keep_changed"],
        json!(true)
    );
    world
        .run_from(
            &world.project,
            &["start", &project, "--launch-config", &config, "--fresh"],
        )
        .exited(2)
        .err_has("--keep-changed refused: --fresh");

    let before = stored_settlements(&world, &project);
    let changed = format!("{name}-6");
    let launched = launch(&world, &changed, &project, &[]);
    launched
        .exited(0)
        .err_has(&format!(
            "launch '{changed}' redoes node 'a', as its task changed since it settled"
        ))
        .err_has("redoes node 'b', as a done dependent of redone node 'a'")
        .err_has("redoes node 'd', as a done dependent of redone node 'a'")
        .err_has(
            "leaves node 'l' settled rather than redoing it as its task changed since it \
             settled: its work landed",
        );
    let mut redispatched = dispatched(&world, &changed);
    redispatched.sort();
    assert_eq!(redispatched, ["a", "b", "c", "d"]);
    assert_eq!(
        redone(&world, &changed),
        [
            json!({"node": "a", "reason": "changed"}),
            json!({"node": "b", "reason": "dependent", "of": "a"}),
            json!({"node": "c", "reason": "dependent", "of": "a"}),
            json!({"node": "d", "reason": "dependent", "of": "a"}),
        ]
    );
    // The lifecycle nodes continue the branches they settled on; the direct node has none.
    let mut branches = sessions_opened(&world, &changed);
    branches.sort();
    let mut expected: Vec<String> = ["a", "b", "c"]
        .iter()
        .map(|id| before[*id]["branch"].as_str().expect("a branch").to_owned())
        .collect();
    expected.sort();
    assert_eq!(branches, expected);
    let after = stored_settlements(&world, &project);
    let digests = world.run_json(&changed, "launch.json")["task_digests"].clone();
    for id in ["a", "b", "c", "d"] {
        assert_eq!(after[id]["status"], "done", "{id}: {after:#?}");
        assert_eq!(
            after[id]["task_digest"], digests[id],
            "{id}'s new settlement is not the digest of the task it was dispatched with"
        );
        assert_eq!(
            settled(&world, &changed, id)["task_digest"],
            digests[id],
            "{id}"
        );
    }
    for id in ["a", "b", "c"] {
        let (was, is) = (
            before[id]["head"].as_str().expect("a head"),
            after[id]["head"].as_str().expect("a head"),
        );
        assert_ne!(was, is, "{id}'s redo committed nothing");
        assert!(descends(&world, &origin, was, is), "{id}: {is} from {was}");
    }
    assert_ne!(after["a"]["task_digest"], before["a"]["task_digest"]);
    assert_eq!(after["l"], before["l"], "the landed node was redone");
    assert_eq!(
        settled(&world, &changed, "l")["continued"]["source"],
        "store"
    );

    // A settlement recording no digest was written before digests were: unchanged, however
    // its task reads now.
    // llmlint: ignore-block[tests_mirror_real_usage] the settlement an earlier build stored, with
    // no `task_digest` — no verb of this build writes one without it, and the store is where such
    // a record reaches a launch; the launch that reads it is the compiled CLI's own.
    let mut stripped = after["b"].clone();
    stripped
        .as_object_mut()
        .expect("a settlement")
        .remove("task_digest");
    update_task(
        &world,
        &project,
        "b",
        onetaskgraph_plugin_api::TaskUpdate {
            metadata_set: [(metadata_key("onepipeline.settlement"), stripped)]
                .into_iter()
                .collect(),
            ..onetaskgraph_plugin_api::TaskUpdate::default()
        },
    );
    // llmlint: ignore-end[tests_mirror_real_usage]
    revise(&world, &project, "b", "Name the columns.");
    let stripped_run = format!("{name}-7");
    launch(&world, &stripped_run, &project, &[]).exited(0);
    assert_eq!(dispatched(&world, &stripped_run), Vec::<String>::new());

    // `--redo b` redoes `b` and its dependent `c`, and continues `a`.
    let named = format!("{name}-8");
    launch(&world, &named, &project, &["--redo", "b"]).exited(0);
    let mut redispatched = dispatched(&world, &named);
    redispatched.sort();
    assert_eq!(redispatched, ["b", "c"]);
    assert_eq!(settled(&world, &named, "a")["continued"]["source"], "store");

    // A task revised while its node runs: the settlement records the task as dispatched, so
    // the next launch redoes it.
    let held = world.rendezvous("c");
    let during = format!("{name}-9");
    world
        .run_from(
            &world.project,
            &["start", &project, "--detach", "--redo", "c"],
        )
        .exited(0);
    let running = held.arrived();
    // Revised only once the run's write-back of the dispatch is recorded — the attempt that
    // put the dispatch on `c`'s task appended to the run's projection record, which the
    // write-back does after the store has answered — so the edit lands on the task as that
    // write-back left it rather than beside it.
    world.until("the dispatch's write-back to be recorded", |world| {
        task_of(world, &project, "c")["item"]["metadata"]["onepipeline.dispatch"]["opened_at"]
            .as_str()
            .is_some_and(|opened| projected_metadata_since(world, &during, "c", opened))
    });
    revise(&world, &project, "c", "Also index the export.");
    drop(running);
    // And the next launch only once this run's driver has recorded its exit, which it writes
    // after its closeout's write-back: a launch beside that write-back would read the store as
    // it moved.
    world.until("the run's driver to record its exit", |world| {
        !world.events_of(&during, "driver-exited").is_empty()
    });
    world.unscript("c.rendezvous");
    let next = format!("{name}-10");
    launch(&world, &next, &project, &[])
        .exited(0)
        .err_has("redoes node 'c', as its task changed since it settled");
    assert_eq!(dispatched(&world, &next), ["c"]);
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

/// Every path-valued key the plan loader resolves — a node's `agent_graph` and a step's, made
/// absolute against the launch directory by `driver::resolve_node_graphs` — authored relative,
/// on a direct node and on a lifecycle node's steps. The write-back after the launch leaves each
/// stored value byte-identical to what the plan authored, so every task's content digest is the
/// one the launch read, and an unchanged relaunch dispatches nothing.
#[test]
fn the_writeback_leaves_every_resolved_path_as_the_plan_authored_it() {
    let name = "redo-authored-paths";
    let (world, _repo) = world_of(name, &["direct", "stepped.implement"]);
    let graphs = world.project.join("graphs");
    std::fs::create_dir_all(&graphs).expect("the graphs directory");
    for file in ["planner.yaml", "stepped.yaml", "review.yaml"] {
        std::fs::copy(
            crate::harness::repo_file("graphs/node-scope.yaml"),
            graphs.join(file),
        )
        .expect("a graph the plan names relatively");
    }
    let mut direct = agent("direct", &[]);
    direct["agent_graph"] = json!("graphs/planner.yaml");
    let mut stepped = lifecycle("stepped", &[]);
    stepped["publish"] = json!("preserve");
    let fields = stepped.as_object_mut().expect("a node");
    fields.remove("persona");
    fields.remove("task");
    stepped["agent_graph"] = json!("./graphs/stepped.yaml");
    stepped["steps"] = json!([
        {"id": "implement", "persona": "engineer",
         "task": "## What\nimplement\n\n## Acceptance criteria\n- It is done."},
        {"id": "review", "persona": "reviewer",
         "task": "## What\nreview\n\n## Acceptance criteria\n- It is done.",
         "deps": ["implement"], "agent_graph": "graphs/review.yaml"},
    ]);
    let project = author(&world, name, vec![direct, stepped]);
    let authored = |node: &str| task_of(&world, &project, node)["item"]["metadata"].clone();
    let before: BTreeMap<&str, Value> = ["direct", "stepped"]
        .into_iter()
        .map(|node| (node, authored(node)))
        .collect();
    assert_eq!(
        before["direct"]["onepipeline.agent_graph"],
        "graphs/planner.yaml"
    );

    launch(&world, name, &project, &[]).exited(0);
    // The launch kept each value it resolved beside the spelling the plan authored.
    let record = world.run_json(name, "launch.json");
    assert_eq!(
        record["authored"]["direct"]["/agent_graph"]["authored"], "graphs/planner.yaml",
        "{record}"
    );
    assert_eq!(
        record["authored"]["stepped"]["/steps/1/agent_graph"]["authored"], "graphs/review.yaml",
        "{record}"
    );
    for node in ["direct", "stepped"] {
        let after = authored(node);
        assert_eq!(after["onepipeline.settlement"]["status"], "done", "{after}");
        for key in ["onepipeline.agent_graph", "onepipeline.steps"] {
            assert_eq!(
                serde_json::to_string(&after[key]).expect("a value"),
                serde_json::to_string(&before[node][key]).expect("a value"),
                "{node}'s {key} is not the value its plan authored"
            );
        }
    }

    let again = format!("{name}-2");
    launch(&world, &again, &project, &[]).exited(0);
    assert_eq!(
        world.run_json(&again, "launch.json")["task_digests"],
        record["task_digests"],
        "the write-back changed a task's content digest"
    );
    assert_eq!(dispatched(&world, &again), Vec::<String>::new());
    assert!(redone(&world, &again).is_empty());
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

/// An `adopt` replays the redo its launch recorded and detects no changed work of its own:
/// a launch redoes `first`, whose task changed, and `second` as its done dependent, and its
/// driver is shut down with `second` dispatched once `first` has settled again. `first`'s task
/// is then revised a second time and `third`'s for the first time, and the adoption dispatches
/// `second` alone — the one redo its launch recorded and had not finished — leaving `first` and
/// `third` as they stood, with the launch's record of what it redid unchanged.
#[test]
fn an_adoption_replays_the_redo_its_launch_recorded_and_reads_no_later_task_edit() {
    let name = "redo-replayed";
    let (world, _repo) = world_of(name, &["first", "second", "third"]);
    let mut second = kept("second");
    second["deps"] = json!(["first"]);
    let project = author(&world, name, vec![kept("first"), second, kept("third")]);
    launch(&world, name, &project, &[]).exited(0);
    let before = stored_settlements(&world, &project);

    revise(&world, &project, "first", "Also cover the export.");
    world.script("second.wait", "hold");
    let again = format!("{name}-2");
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    world.until("first's redo to land on the store", |world| {
        stored_settlements(world, &project)
            .get("first")
            .is_some_and(|settlement| settlement["head"] != before["first"]["head"])
    });
    world.until("second's redo to be dispatched", |world| {
        dispatched(world, &again).contains(&"second".to_owned())
    });
    let recorded = redone(&world, &again);
    assert_eq!(
        recorded,
        [
            json!({"node": "first", "reason": "changed"}),
            json!({"node": "second", "reason": "dependent", "of": "first"}),
        ]
    );
    let shutdown = world.run(&["shutdown", &again, "--grace", "1"]);
    assert!([0, 2].contains(&shutdown.code), "{}", shutdown.stderr);
    let redone_first = stored_settlements(&world, &project)["first"].clone();

    revise(&world, &project, "first", "And the import.");
    revise(&world, &project, "third", "Also cover the archive.");
    world.unscript("second.wait");
    let adopted = world.run(&["adopt", &again, "--attach"]);
    adopted.exited(0);
    assert!(
        !adopted.stderr.contains("redoes node"),
        "the adoption decided a redo of its own: {}",
        adopted.stderr
    );
    assert_eq!(world.run_json(&again, "result.json")["state"], "complete");
    assert_eq!(
        dispatched(&world, &again),
        ["first", "second", "second"],
        "the adoption dispatched other than the second its launch had not finished"
    );
    assert_eq!(redone(&world, &again), recorded);
    let after = stored_settlements(&world, &project);
    assert_eq!(
        after["first"], redone_first,
        "the adoption redid first on its later edit"
    );
    assert_eq!(after["third"], before["third"], "the adoption redid third");
    assert_eq!(after["second"]["status"], "done", "{after:#?}");
    assert_eq!(after["second"]["branch"], before["second"]["branch"]);
    assert_ne!(after["second"]["head"], before["second"]["head"]);
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

/// A `--redo` the first read admits is held to the store as it stands once the displaced
/// driver has let go: a planner removes the named node from the board during the handover,
/// so the takeover is refused naming it, and dispatches nothing.
#[test]
fn a_redo_the_handover_leaves_unredoable_is_refused_and_dispatches_nothing() {
    let name = "redo-takeover-refused";
    let (base, _repo) = world_of(name, &["first", "second"]);
    let world = base
        .with_env(CLAIM_INTERVAL_ENV, "5")
        .with_env(crate::harness::CANCEL_GRACE_ENV, &PATIENT_GRACE.to_string());
    let project = author(&world, name, vec![kept("first"), kept("second")]);
    launch(&world, name, &project, &[]).exited(0);

    // A planner adds a node, and a continuing run on this host dispatches it and is held.
    let hold = world.rendezvous("hold");
    world.add_task(name, "002-hold", &agent("hold", &[]), &[]);
    let displaced = format!("{name}-2");
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let holding = hold.arrived();
    assert_eq!(
        stored_settlements(&world, &project)["first"]["status"],
        "done"
    );

    let successor = world.on_host("host-b");
    let runs = run_dirs(&successor);
    let taking = take_over_in_background_redoing(&successor, &project, "first");
    // The takeover has read the store and admitted the redo; the planner then removes it.
    until_claimed_on(&world, &project, "host-b");
    let task = world
        .store()
        .join("tasks")
        .join(crate::harness::project_id(name))
        .join("000-first.md");
    std::fs::remove_file(&task).expect("the planner removes first's task");
    holding.release();
    let answered = taking.wait_with_output().expect("the takeover answers");
    let said = String::from_utf8_lossy(&answered.stderr);
    assert_eq!(answered.status.code(), Some(2), "{said}");
    assert!(
        said.contains("--redo refused: node 'first'") && said.contains("is not a node of the plan"),
        "{said}"
    );
    world.until("the displaced run to settle", |world| {
        world.run_file(&displaced, "result.json").is_file()
    });
    assert_eq!(dispatched(&world, &displaced), vec!["hold".to_owned()]);
    assert_eq!(
        run_dirs(&successor),
        runs,
        "the refused takeover minted a run"
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

/// A node whose task changed is not redone onto a branch that is gone: a person discarded the
/// branch it settled on, so the launch leaves it settled, says why on stderr, and dispatches
/// nothing.
#[test]
fn a_changed_node_whose_branch_is_gone_stays_settled_and_says_why() {
    let name = "redo-gone";
    let (world, _repo) = world_of(name, &["solo"]);
    let project = author(&world, name, vec![kept("solo")]);
    launch(&world, name, &project, &[]).exited(0);
    let branch = stored_settlements(&world, &project)["solo"]["branch"]
        .as_str()
        .expect("a branch")
        .to_owned();
    // The person discards the work the node kept, everywhere this host holds it.
    let discarded = world
        .cmd_on(
            &crate::harness::onevcs_binary(),
            &["reclaim", &branch, "--repo", "service", "--discard"],
        )
        .output()
        .expect("onevcs runs");
    assert!(
        discarded.status.success(),
        "{}",
        String::from_utf8_lossy(&discarded.stderr)
    );

    revise(&world, &project, "solo", "Also cover the export.");
    let again = format!("{name}-2");
    launch(&world, &again, &project, &[])
        .exited(0)
        .err_has(&format!(
            "leaves node 'solo' settled rather than redoing it as its task changed since it \
             settled: the branch it settled on, {branch}, is gone"
        ));
    assert_eq!(dispatched(&world, &again), Vec::<String>::new());
    assert!(redone(&world, &again).is_empty());
    assert_eq!(
        settled(&world, &again, "solo")["continued"]["source"],
        "store"
    );
}

/// What a task's digest reads, end to end: a node a live edit added records no digest, which a
/// later launch reads as unchanged; a task nobody titled reads the same after the write-back
/// titles it by its id, so an unedited relaunch redoes nothing; and a dependency a person adds to
/// a task is a change to it, so the node is redone.
#[test]
fn a_digest_reads_a_tasks_edges_not_its_projected_title_and_a_live_added_node_records_none() {
    let name = "redo-edges";
    let world = World::new(name);
    let project = author(&world, name, vec![agent("p", &[]), agent("q", &[])]);
    // `p`'s dispatch is held, so the run is still going when a node is added to it live.
    let held = world.rendezvous("p");
    world
        .run_from(&world.project, &["start", &project, "--detach"])
        .exited(0);
    let holding = held.arrived();
    world
        .run_with_stdin(
            &["reply", name],
            &json!({"version": 2, "commands": [{"op": "add", "node": agent("z", &[])}]})
                .to_string(),
        )
        .exited(0);
    drop(holding);
    world.unscript("p.rendezvous");
    assert_eq!(world.until_continued(name), "complete");
    world.until("the run to release the project", |world| {
        stored_claim(world, &project) == Value::Null
    });
    let stored = stored_settlements(&world, &project);
    for id in ["p", "q"] {
        assert!(stored[id]["task_digest"].is_string(), "{id}: {stored:#?}");
    }
    assert!(stored["z"].get("task_digest").is_none(), "{stored:#?}");
    // Authored with no title, and titled by its id by the write-back.
    assert_eq!(task_of(&world, &project, "p")["item"]["title"], "p");

    let unedited = format!("{name}-2");
    launch(&world, &unedited, &project, &[]).exited(0);
    assert_eq!(dispatched(&world, &unedited), Vec::<String>::new());

    // A person makes `q` depend on `p`: the task is re-authored with the edge, keeping every
    // metadata key it held.
    let held = task_of(&world, &project, "q")["item"]["metadata"].clone();
    world.add_task(name, "001-q", &agent("q", &["p"]), &[("p", "000-p")]);
    update_task(
        &world,
        &project,
        "q",
        onetaskgraph_plugin_api::TaskUpdate {
            metadata_set: held
                .as_object()
                .expect("metadata")
                .iter()
                .map(|(key, value)| (metadata_key(key), value.clone()))
                .collect(),
            ..onetaskgraph_plugin_api::TaskUpdate::default()
        },
    );
    // The unedited run's close-out has released its claim on the project.
    world.until("the unedited run to release the project", |world| {
        stored_claim(world, &project) == Value::Null
    });
    let edged = format!("{name}-3");
    launch(&world, &edged, &project, &[])
        .exited(0)
        .err_has("redoes node 'q', as its task changed since it settled");
    assert_eq!(dispatched(&world, &edged), ["q"]);
}
