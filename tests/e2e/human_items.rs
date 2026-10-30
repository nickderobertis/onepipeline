//! Human items inside real runs: the compiled binary's reconcile loop over a real
//! run store and plan store, with the host's human-item journal beside it.
//!
//! Each journey launches a plan, acts on its items through `onepipeline
//! human-items` — the operations a person or a manager uses — and reads what the
//! scheduler did off the run's own journal: which nodes it dispatched, which it
//! held, and what became of the human step an item stood for.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the `oneagentgraph` sibling at its
// subprocess boundary and nothing inside the crate under test, which is driven as the real
// compiled binary over real run, plan and human-item stores; `harness.rs` carries the full
// rationale. The identity-recovery journey also stands a script in for the `claude` CLI that
// the real linked `oneharness` usage probe spawns — a third-party binary an offline check may
// not run — and the GitHub loopback stands in for the hosted tracker, as `private_github_home.rs`
// documents.

use crate::harness::{agent, human, plan_of, World};
use serde_json::{json, Value};

/// Human items enabled for this world, through its host-local configuration.
fn enabled(world: &World) {
    let dir = world.root.join("xdg/onepipeline");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.yaml"), "human_items:\n  enabled: true\n").unwrap();
}

fn items(world: &World, args: &[&str]) -> Vec<Value> {
    let mut all = vec!["human-items", "list", "--json"];
    all.extend_from_slice(args);
    let run = world.run(&all);
    run.exited(0);
    serde_json::from_str::<Value>(&run.stdout)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn dispatched(world: &World, run: &str) -> Vec<String> {
    world
        .events_of(run, "node-dispatched")
        .iter()
        .map(|e| e["labels"]["node"].as_str().unwrap().to_owned())
        .collect()
}

fn settled(world: &World, run: &str, node: &str) -> Option<String> {
    world
        .events_of(run, "node-settled")
        .iter()
        .rev()
        .find(|e| e["labels"]["node"] == node)
        .map(|e| {
            e["payload"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
}

fn id_of(view: &Value) -> String {
    view["item"]["id"].as_str().unwrap().to_owned()
}

#[test]
fn a_human_node_raises_one_item_and_completing_it_releases_only_its_branch() {
    let world = World::new("human-item-node");
    enabled(&world);
    let name = "gated";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                human("approve", &[]),
                agent("build", &["approve"]),
                agent("ship", &["build"]),
                agent("side", &[]),
            ],
        ),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    // The unrelated branch ran; the gated one waits on the person.
    assert_eq!(settled(&world, name, "side").as_deref(), Some("done"));
    assert!(!dispatched(&world, name).contains(&"build".to_owned()));
    let open = items(&world, &[]);
    assert_eq!(open.len(), 1, "{open:?}");
    let item = &open[0];
    assert_eq!(item["item"]["dedup_key"], format!("run:{name}#approve"));
    assert_eq!(item["item"]["kind"], "task");
    assert_eq!(
        item["item"]["priority"], "low",
        "a legacy node with no store priority is Low"
    );
    assert_eq!(
        item["item"]["title"],
        "Do approve, which only a person can do."
    );
    assert_eq!(
        item["item"]["block"],
        json!({"scope": "work", "run": name, "node": "approve"})
    );
    assert_eq!(item["item"]["owner"]["run"], name);
    assert_eq!(item["blocked"]["total"], 2);
    let plan = &item["blocked"]["plans"][0];
    assert_eq!(plan["count"], 2);
    let nodes: Vec<&Value> = plan["preview"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| &n["node"])
        .collect();
    assert_eq!(nodes, [&json!("build"), &json!("ship")]);
    let text = world.run(&["human-items", "list"]);
    text.exited(0).out_has("blocks 2 steps in");
    let detail = world.run(&["human-items", "show", &id_of(item), "--json"]);
    detail.exited(0);
    let detail: Value = serde_json::from_str(&detail.stdout).unwrap();
    assert_eq!(detail["nodes"].as_array().unwrap().len(), 2);

    // Adopting again reads the same eligible node: still one item.
    world.run(&["adopt", name]).exited(0);
    assert_eq!(items(&world, &[]).len(), 1);

    // Completing it attests the step once; the gated branch then runs.
    world
        .run(&["human-items", "complete", &id_of(item)])
        .exited(0);
    world.run(&["adopt", name]).exited(0);
    assert_eq!(settled(&world, name, "build").as_deref(), Some("done"));
    assert_eq!(settled(&world, name, "ship").as_deref(), Some("done"));
    assert_eq!(
        world
            .events_of(name, "edit-committed")
            .iter()
            .filter(|e| e["payload"]["command"]["op"] == "attest")
            .count(),
        1
    );
    assert!(items(&world, &[]).is_empty());
    let done = items(&world, &["--state", "completed"]);
    assert_eq!(done.len(), 1);
}

#[test]
fn dismissing_a_human_items_step_detaches_its_dependents_with_or_without_a_reason() {
    for reason in [Some("Superseded by the new rollout"), None] {
        let world = World::new("human-item-dismiss");
        enabled(&world);
        let name = "dismissed";
        let project = world.plan(
            name,
            &plan_of(
                name,
                vec![human("approve", &[]), agent("build", &["approve"])],
            ),
        );
        world.run(&["start", &project, "--attach"]).exited(0);
        let open = items(&world, &[]);
        let id = id_of(&open[0]);
        let mut dismiss = vec!["human-items", "dismiss", id.as_str()];
        if let Some(reason) = reason {
            dismiss.extend(["--reason", reason]);
        }
        world.run(&dismiss).exited(0);
        world.run(&["adopt", name]).exited(0);
        // The step was dropped, its dependent detached and run: the run did not fail.
        assert_eq!(settled(&world, name, "build").as_deref(), Some("done"));
        let drop = world
            .events_of(name, "edit-committed")
            .into_iter()
            .find(|e| e["payload"]["command"]["op"] == "drop")
            .expect("the step was dropped");
        assert_eq!(drop["payload"]["command"]["dependents"], "detach");
        assert_eq!(
            drop["payload"]["command"]["reason"],
            reason.map_or(Value::Null, |r| json!(r))
        );
        let result: Value = serde_json::from_str(
            &std::fs::read_to_string(world.run_file(name, "result.json")).unwrap(),
        )
        .unwrap();
        assert_ne!(result["state"], "failed", "{result}");
        let dismissed = items(&world, &["--state", "dismissed"]);
        assert_eq!(
            dismissed[0]["item"]["resolution"]["reason"],
            reason.map_or(Value::Null, |r| json!(r))
        );
    }
}

/// Raise one item through the CLI as `session`; answers its id.
fn raise(world: &World, file: &str, item: &Value) -> String {
    let path = world.root.join(format!("{file}.json"));
    std::fs::write(&path, item.to_string()).unwrap();
    let run = world.run(&["human-items", "raise", "--input", path.to_str().unwrap()]);
    run.exited(0);
    serde_json::from_str::<Value>(&run.stdout).unwrap()["item"]["item"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn input(
    dedup: &str,
    kind: &str,
    session: &str,
    run: Option<&str>,
    plan: Option<&str>,
    block: Value,
) -> Value {
    json!({
        "schema_version": 1,
        "dedup_key": dedup,
        "kind": kind,
        "title": format!("Decide {dedup}"),
        "body": format!("What {dedup} needs from a person."),
        "priority": "high",
        "owner": {"session": session, "run": run, "plan": plan},
        "block": block,
        "approval": null
    })
}

fn stays_undispatched(world: &World, run: &str, for_: std::time::Duration) {
    let until = std::time::Instant::now() + for_;
    while std::time::Instant::now() < until {
        assert!(
            dispatched(world, run).is_empty(),
            "{:?}",
            dispatched(world, run)
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn host_and_plan_holds_compose_and_each_releases_only_its_own_hold() {
    let world = World::new("human-item-holds");
    enabled(&world);
    let name = "held";
    let project = world.plan(
        name,
        &plan_of(name, vec![agent("a", &[]), agent("b", &["a"])]),
    );
    let host = raise(
        &world,
        "host",
        &input(
            "outage",
            "task",
            "ops",
            None,
            None,
            json!({"scope": "host"}),
        ),
    );
    let plan = raise(
        &world,
        "plan",
        &input(
            "freeze",
            "question",
            "ops",
            None,
            Some(&project),
            json!({"scope": "plan", "plan": project}),
        ),
    );
    let unrelated = raise(
        &world,
        "other",
        &input(
            "other-plan",
            "task",
            "ops",
            None,
            Some("plans:elsewhere"),
            json!({"scope": "plan", "plan": "plans:elsewhere"}),
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    stays_undispatched(&world, name, std::time::Duration::from_secs(3));
    // Both holds name every step of the plan, and blocking is derived per item.
    let blocking = items(&world, &["--blocking", "yes"]);
    assert_eq!(blocking.len(), 3);
    for view in &blocking {
        if view["item"]["id"] == unrelated.as_str() {
            assert_eq!(view["blocked"]["total"], 0);
        } else {
            assert_eq!(view["blocked"]["total"], 2, "{view}");
        }
    }
    world
        .run(&["human-items", "report"])
        .exited(0)
        .out_has("blocks all work on this host")
        .out_has(&format!("blocks all of {project}"));
    // Resolving the host hold leaves the plan hold standing.
    world.run(&["human-items", "complete", &host]).exited(0);
    stays_undispatched(&world, name, std::time::Duration::from_secs(2));
    // The plan hold's answer releases it; another plan's hold never held this one.
    world
        .run(&["human-items", "answer", &plan, "--text", "Unfreeze it."])
        .exited(0);
    world.until("the run to settle", |w| {
        w.run_file(name, "result.json").is_file()
            && settled(w, name, "b").as_deref() == Some("done")
    });
    assert_eq!(dispatched(&world, name), ["a", "b"]);
    assert_eq!(items(&world, &[])[0]["item"]["id"], unrelated.as_str());
}

#[test]
fn a_work_hold_keeps_its_branch_from_starting_and_lets_the_rest_run() {
    let world = World::new("human-item-work-hold");
    enabled(&world);
    let name = "branches";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                agent("seed", &[]),
                agent("left", &["seed"]),
                agent("right", &["seed"]),
                agent("tail", &["left"]),
            ],
        ),
    );
    let hold = raise(
        &world,
        "hold",
        &input(
            "left-hold",
            "task",
            "mgr",
            Some(name),
            Some(&project),
            json!({"scope": "work", "run": name, "node": "left"}),
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the unrelated branch to settle", |w| {
        settled(w, name, "right").as_deref() == Some("done")
    });
    std::thread::sleep(std::time::Duration::from_secs(1));
    let started = dispatched(&world, name);
    assert!(
        !started.contains(&"left".to_owned()) && !started.contains(&"tail".to_owned()),
        "{started:?}"
    );
    let view = &items(&world, &[])[0];
    assert_eq!(view["blocked"]["total"], 2);
    world.run(&["human-items", "complete", &hold]).exited(0);
    world.until("the held branch to run", |w| {
        settled(w, name, "tail").as_deref() == Some("done")
    });
}

/// What `session`'s watch has been delivered and not yet printed.
fn watch(world: &World, session: &str) -> Vec<Value> {
    let mut command = world.cmd(&["human-items", "watch", "--session", session]);
    let output = command.output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn state_of(world: &World, id: &str) -> Value {
    let run = world.run(&["human-items", "show", id, "--json"]);
    run.exited(0);
    serde_json::from_str::<Value>(&run.stdout).unwrap()["item"].clone()
}

#[test]
fn a_settled_run_dismisses_only_its_open_holds_and_tells_only_its_owner() {
    let world = World::new("human-item-settle");
    enabled(&world);
    let name = "settles";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                agent("side", &[]),
                agent("a", &[]),
                human("gate", &["a"]),
                agent("b", &["gate"]),
            ],
        ),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    let owner = "session-human-item-settle";
    let gate = id_of(&items(&world, &[])[0]);
    // An open hold this run owns, on a step of its own that already ran; a
    // question of the same manager's outside any run; another manager's item.
    let hold = raise(
        &world,
        "hold",
        &input(
            "side-hold",
            "task",
            owner,
            Some(name),
            Some(&project),
            json!({"scope": "work", "run": name, "node": "side"}),
        ),
    );
    let question = raise(
        &world,
        "question",
        &input(
            "outside",
            "question",
            owner,
            None,
            None,
            json!({"scope": "nothing"}),
        ),
    );
    let other = raise(
        &world,
        "other",
        &input(
            "theirs",
            "task",
            "someone-else",
            None,
            None,
            json!({"scope": "nothing"}),
        ),
    );
    watch(&world, owner);
    watch(&world, "someone-else");
    world.run(&["human-items", "complete", &gate]).exited(0);
    world.run(&["adopt", name]).exited(0);
    assert_eq!(settled(&world, name, "b").as_deref(), Some("done"));
    let retired = state_of(&world, &hold);
    assert_eq!(retired["state"], "dismissed");
    assert_eq!(retired["resolution"]["reason"], "the run ended");
    assert_eq!(
        state_of(&world, &gate)["state"],
        "completed",
        "a resolved item is left as it was"
    );
    assert_eq!(state_of(&world, &question)["state"], "open");
    assert_eq!(state_of(&world, &other)["state"], "open");
    let delivered: Vec<(String, String)> = watch(&world, owner)
        .iter()
        .map(|e| {
            (
                e["kind"].as_str().unwrap().to_owned(),
                e["payload"]["item_id"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        delivered,
        [
            ("human-item-completed".to_owned(), gate.clone()),
            ("human-item-auto-resolved".to_owned(), hold.clone())
        ]
    );
    assert!(watch(&world, "someone-else").is_empty());
}

#[test]
fn a_stopped_run_dismisses_its_work_and_plan_holds_and_host_holds_survive() {
    let world = World::new("human-item-stop");
    enabled(&world);
    let name = "stops";
    let project = world.plan(
        name,
        &plan_of(name, vec![human("gate", &[]), agent("b", &["gate"])]),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    let owner = "session-human-item-stop";
    let gate = id_of(&items(&world, &[])[0]);
    let plan = raise(
        &world,
        "plan",
        &input(
            "freeze",
            "task",
            owner,
            Some(name),
            Some(&project),
            json!({"scope": "plan", "plan": project}),
        ),
    );
    let host = raise(
        &world,
        "host",
        &input(
            "outage",
            "task",
            "ops",
            None,
            None,
            json!({"scope": "host"}),
        ),
    );
    world.run(&["stop", name]).exited(0);
    for id in [&gate, &plan] {
        let item = state_of(&world, id);
        assert_eq!(item["state"], "dismissed", "{item}");
        assert_eq!(item["resolution"]["reason"], "the run ended");
        assert_eq!(item["resolution"]["actor"], "engine");
    }
    assert_eq!(state_of(&world, &host)["state"], "open");
    let kinds: Vec<String> = watch(&world, owner)
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|k| *k == "human-item-auto-resolved")
            .count(),
        2,
        "{kinds:?}"
    );
    assert!(watch(&world, "ops")
        .iter()
        .all(|e| e["kind"] == "human-item-raised"));
}

#[test]
fn plan_store_launches_raise_one_item_per_human_node_from_its_priority_or_metadata() {
    let world = World::new("human-item-launch");
    enabled(&world);
    let name = "metadata";
    let mut urgent = human("urgent-step", &[]);
    urgent["priority"] = json!("urgent");
    urgent["title"] = json!("Rotate the leaked key");
    let mut unprioritised = human("plain-step", &[]);
    unprioritised["priority"] = json!("none");
    let mut question = human("ask-step", &[]);
    question["human_item"] = json!({
        "kind": "question",
        "priority": "medium",
        "title": "Which region hosts the pilot?",
        "body": "Pick one region; it decides the latency budget."
    });
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                urgent,
                unprioritised,
                question,
                agent("after", &["urgent-step", "plain-step", "ask-step"]),
            ],
        ),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    world.run(&["adopt", name]).exited(0);
    world.run(&["adopt", name]).exited(0);
    let open = items(&world, &[]);
    assert_eq!(
        open.len(),
        3,
        "repeated scheduler reads raise nothing twice: {open:?}"
    );
    let by_node = |node: &str| {
        open.iter()
            .find(|v| v["item"]["dedup_key"] == format!("run:{name}#{node}"))
            .unwrap_or_else(|| panic!("no item for {node}: {open:?}"))["item"]
            .clone()
    };
    let urgent = by_node("urgent-step");
    assert_eq!(urgent["priority"], "urgent");
    assert_eq!(urgent["kind"], "task");
    assert_eq!(urgent["title"], "Rotate the leaked key");
    assert_eq!(
        urgent["body"],
        "Do urgent-step, which only a person can do."
    );
    assert_eq!(by_node("plain-step")["priority"], "low");
    let asked = by_node("ask-step");
    assert_eq!(asked["kind"], "question");
    assert_eq!(asked["priority"], "medium");
    assert_eq!(asked["title"], "Which region hosts the pilot?");
    assert_eq!(
        asked["body"],
        "Pick one region; it decides the latency budget."
    );
    assert_eq!(asked["tracker"], Value::Null);
    world
        .run(&["human-items", "list"])
        .exited(0)
        .out_has("Critical")
        .out_has("Medium")
        .out_has("Low");
    // Answering the question attests its step exactly as completing a task does.
    world
        .run(&[
            "human-items",
            "answer",
            &id_of(&json!({"item": asked})),
            "--text",
            "eu-west",
        ])
        .exited(0);
    for node in ["urgent-step", "plain-step"] {
        world
            .run(&[
                "human-items",
                "complete",
                &id_of(&json!({"item": by_node(node)})),
            ])
            .exited(0);
    }
    world.run(&["adopt", name]).exited(0);
    assert_eq!(settled(&world, name, "after").as_deref(), Some("done"));
}

/// A checkout carrying the repository-tracked shared configuration.
fn checkout(world: &World, config: Option<&str>) -> std::path::PathBuf {
    let repo = world.root.join("shared-checkout");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join(".onepipeline")).unwrap();
    if let Some(config) = config {
        std::fs::write(repo.join(".onepipeline/config.yaml"), config).unwrap();
    }
    repo
}

#[test]
fn the_cli_the_embedded_sdk_and_a_launch_read_one_shared_configuration() {
    for shared in [true, false] {
        let world = World::new("human-item-shared-config");
        let repo = checkout(
            &world,
            shared.then_some("human_items:\n  enabled: true\n  task_source: plans\n"),
        );
        // The CLI, run in the checkout.
        let mut cli = world.cmd(&["human-items", "config", "--json"]);
        cli.current_dir(&repo);
        let run = world.run_on(cli, "human-items config --json");
        run.exited(0);
        let read: Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(read["human_items"]["enabled"], shared);
        assert_eq!(
            read["human_items"]["task_source"],
            if shared { json!("plans") } else { Value::Null }
        );

        // The SDK an embedding UI links, asked of the same checkout.
        let sdk = onepipeline::human_items::config::effective(&repo).unwrap();
        assert_eq!(sdk.human_items.enabled, shared);
        assert_eq!(
            sdk.human_items.task_source.as_deref(),
            shared.then_some("plans")
        );
        let opened = onepipeline::human_items::HumanItems::open(world.root.join("runs"), &repo);
        assert_eq!(opened.is_ok(), shared);

        // A launch from the checkout: the run acts on the same settings.
        let name = "shared";
        let project = world.plan(
            name,
            &plan_of(name, vec![human("gate", &[]), agent("b", &["gate"])]),
        );
        let mut start = world.cmd(&["start", &project, "--attach"]);
        start.current_dir(&repo);
        world.run_on(start, "start").exited(0);
        let mut list = world.cmd(&["human-items", "list", "--json"]);
        list.current_dir(&repo);
        let listed = world.run_on(list, "human-items list --json");
        if shared {
            listed.exited(0);
            let views: Value = serde_json::from_str(&listed.stdout).unwrap();
            assert_eq!(views.as_array().unwrap().len(), 1);
            assert_eq!(views[0]["item"]["dedup_key"], format!("run:{name}#gate"));
        } else {
            listed.err_has(r#"{"kind":"unavailable"}"#);
            assert!(!world.root.join("runs/human-items/events.ndjson").exists());
        }
    }
}

/// Run `steps`, a shell script of `onepipeline` invocations, inside an
/// unprivileged user and mount namespace whose `mount` is the script's first
/// line, with the environment this world gives the binary. Answers the script's
/// exit code.
#[cfg(target_os = "linux")]
fn in_namespace(world: &World, script: &str) -> i32 {
    let template = world.cmd(&[]);
    let mut command = std::process::Command::new("unshare");
    command.args(["-rm", "sh", "-c", script]);
    for (key, value) in template.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    command.env("ONEPIPELINE", template.get_program());
    command.status().unwrap().code().unwrap_or(-1)
}

/// A dispatch refused for want of space, for real: the run's scratch directory is
/// a tmpfs with no inode left, so the dispatch's own `mkdir` fails with `ENOSPC`
/// while every journal stays writable.
#[cfg(target_os = "linux")]
#[test]
fn a_dispatch_refused_for_space_raises_one_host_item_that_its_probe_resolves() {
    let world = World::new("human-item-dispatch-space");
    enabled(&world);
    let name = "spaceless";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                human("gate", &[]),
                agent("b", &["gate"]),
                agent("c", &["gate"]),
            ],
        ),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    let gate = id_of(&items(&world, &[])[0]);
    world.run(&["human-items", "complete", &gate]).exited(0);
    let scratch = world.runs.join(name).join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let out = world.root.join("namespace-out");
    std::fs::create_dir_all(&out).unwrap();
    let script = format!(
        r#"mount -t tmpfs -o size=64k,nr_inodes=1 tmpfs "{scratch}" || exit 90
ONEPIPELINE_BOUNDARY_ATTEMPTS=1 "$ONEPIPELINE" adopt {name} > "{out}/adopt.out" 2> "{out}/adopt.err"
"$ONEPIPELINE" human-items list --json --host > "{out}/full.json" 2> "{out}/full.err"
umount "{scratch}" || exit 91
"$ONEPIPELINE" human-items list --json --host --state dismissed > "{out}/after.json" 2> "{out}/after.err"
"#,
        scratch = scratch.display(),
        out = out.display(),
    );
    assert_eq!(in_namespace(&world, &script), 0);
    let read = |file: &str| std::fs::read_to_string(out.join(file)).unwrap();
    let full: Value = serde_json::from_str(&read("full.json"))
        .unwrap_or_else(|e| panic!("{e}: {}", read("full.err")));
    let hosts = full.as_array().unwrap();
    assert_eq!(
        hosts.len(),
        1,
        "two dispatches refused, one item for the condition: {full}\n{}",
        read("adopt.err")
    );
    let host = &hosts[0]["item"];
    assert_eq!(host["priority"], "urgent");
    assert_eq!(
        host["owner"]["session"],
        "session-human-item-dispatch-space"
    );
    assert!(host["body"].as_str().unwrap().contains("scratch"), "{host}");
    // The run's own end retires its holds, never a host incident.
    let after: Value = serde_json::from_str(&read("after.json"))
        .unwrap_or_else(|e| panic!("{e}: {}", read("after.err")));
    let resolved = &after[0]["item"];
    assert_eq!(resolved["id"], host["id"]);
    assert_eq!(resolved["resolution"]["actor"], "engine");
    assert!(
        resolved["resolution"]["reason"]
            .as_str()
            .unwrap()
            .contains("accepts writes again"),
        "{resolved}"
    );
    // The probe dispatched nothing: the refused nodes are where the refusal left them.
    assert!(world.events_of(name, "node-dispatched").len() <= 2);
    assert!(watch(&world, "session-human-item-dispatch-space")
        .iter()
        .any(|e| e["kind"] == "human-item-auto-resolved"));
}

fn host_items(world: &World) -> Vec<Value> {
    items(world, &["--host"])
}

/// Every identity a dispatched side could run as was refused for a reason only
/// a person can clear: one urgent host item, which then holds every dispatch on
/// the host. A refusal that is not definitive — one transient or unclassified
/// candidate among them, or a single classified death — raises nothing.
#[test]
fn only_every_identity_refused_definitively_raises_one_host_item() {
    let exhausted = |candidates: &str| {
        let world = World::new("human-item-identities");
        enabled(&world);
        world.script(
            "build.died-as",
            "provider-failure fallback_chain_exhausted every candidate in the chain was \
             attempted and none ran\n",
        );
        world.script("build.exhausted", candidates);
        let name = "refused";
        let project = world.plan(
            name,
            &plan_of(name, vec![agent("build", &[]), agent("later", &["build"])]),
        );
        world.run(&["start", &project, "--attach"]).settled();
        world
    };
    let world = exhausted(
        "claude-code:work quota out of extra usage until 5pm\n\
         codex auth 401 Unauthorized: token expired\n",
    );
    let hosts = host_items(&world);
    assert_eq!(hosts.len(), 1, "{hosts:?}");
    let host = &hosts[0]["item"];
    assert_eq!(host["priority"], "urgent");
    assert_eq!(host["owner"]["session"], "session-human-item-identities");
    let body = host["body"].as_str().unwrap();
    assert!(
        body.contains("claude-code:work [quota]") && body.contains("codex [auth]"),
        "{body}"
    );
    // The probe asks the harness's own zero-turn usage read of exactly those
    // identities; with neither able to report headroom the item stays open, and
    // nothing was dispatched to find that out.
    let dispatched_before = dispatched(&world, "refused").len();
    world.run(&["human-items", "list", "--host"]).exited(0);
    assert_eq!(host_items(&world).len(), 1);
    assert_eq!(dispatched(&world, "refused").len(), dispatched_before);

    for not_definitive in [
        // One candidate the harness could not classify.
        "claude-code:work quota out of extra usage\ngemini - \n",
        // A transient rate limit beside a quota refusal.
        "claude-code:work quota out of extra usage\ncodex rate_limit 429 slow down\n",
        // One identity alone.
        "claude-code:work quota out of extra usage\n",
    ] {
        let world = exhausted(not_definitive);
        assert!(
            host_items(&world).is_empty(),
            "{not_definitive:?} raised {:?}",
            host_items(&world)
        );
    }
    let world = World::new("human-item-classified");
    enabled(&world);
    world.script(
        "build.died-as",
        "provider-failure quota harness failed (quota)\n",
    );
    let project = world.plan("one", &plan_of("one", vec![agent("build", &[])]));
    world.run(&["start", &project, "--attach"]).settled();
    assert!(host_items(&world).is_empty());
}

/// Open tasks are projected into the configured `task_source` through the
/// linked store — the real `github-projects` plugin against the loopback GitHub
/// — once each, resumed after a projection that never happened, and never twice
/// however often it is replayed. Questions never reach the tracker.
#[test]
fn tasks_project_once_into_the_task_source_and_questions_never_do() {
    use crate::private_github_home::{described, GitHub};
    let onevcs = crate::harness::onevcs_binary()
        .to_string_lossy()
        .into_owned();
    let world = World::new("human-item-projection")
        .with_env("HOSTNAME", "projection-host")
        .with_env("ONEPIPELINE_LOOPBACK_BOARD_TOKEN", "loopback-token")
        .with_env(
            "ONETASKGRAPH_WRITE_POLICY__CHECK_COMMAND",
            &format!("{onevcs},boundary,check,--destination,public,--input,-"),
        )
        .with_env(
            "ONETASKGRAPH_WRITE_POLICY__VISIBILITY_COMMAND",
            &format!("{onevcs},boundary,inspect,--input,-,--json"),
        );
    let github = GitHub::serve(
        &world,
        &json!({
            "owner": "owner",
            "repositories": {"owner/service": "public"},
            "boards": {"1": {"public": true, "priority": true}},
            "issues": [{"id": "human", "repository": "owner/service", "title": "Human work", "board": 1,
                        "status": "Todo", "parent": null,
                        "body": described("## Plan\n- Work for people.", &json!({}))}],
        }),
    );
    let repo = checkout(
        &world,
        Some("human_items:\n  enabled: true\n  task_source: board\n"),
    );
    let intents = world.root.join("intents");
    std::fs::create_dir_all(&intents).unwrap();
    let store = |endpoint: &str| {
        format!(
            "sources:\n  board:\n    plugin: github-projects\n    visibility: public\n    \
             task_projection:\n      host: projection-host\n      directory: {}\n      project: human\n    \
             config:\n      owner: owner\n      project_number: 1\n      repository: owner/service\n      \
             token_env: ONEPIPELINE_LOOPBACK_BOARD_TOKEN\n      endpoint: {endpoint}\n      status_mapping:\n        \
             todo: Todo\n        queued: Queued\n        in-progress: In Progress\n        unknown: Needs Attention\n        \
             done: Done\n        cancelled: Canceled\n      priority_mapping:\n        urgent: Urgent\n        \
             high: High\n        medium: Medium\n        low: Low\n",
            intents.display()
        )
    };
    let in_repo = |args: &[&str]| {
        let mut command = world.cmd(args);
        command.current_dir(&repo);
        world.run_on(command, &args.join(" "))
    };
    let raise_in_repo = |file: &str, value: &Value| {
        let path = world.root.join(format!("{file}.json"));
        std::fs::write(&path, value.to_string()).unwrap();
        in_repo(&["human-items", "raise", "--input", path.to_str().unwrap()])
    };
    let projected = |state: &Value| -> Vec<Value> {
        state["issues"]
            .as_object()
            .unwrap()
            .values()
            .filter(|issue| issue["title"] != "Human work")
            .cloned()
            .collect()
    };

    // The tracker is unreachable: the task is recorded and holds as it should,
    // the refusal is said, and nothing reached any tracker.
    std::fs::write(
        repo.join("onetaskgraph.yaml"),
        store("http://127.0.0.1:9/graphql"),
    )
    .unwrap();
    let first = raise_in_repo(
        "first",
        &input("first", "task", "mgr", None, None, json!({"scope": "host"})),
    );
    first.exited(0).err_has("its projection into board failed");
    let id: String = serde_json::from_str::<Value>(&first.stdout).unwrap()["item"]["item"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(projected(&github.state()).is_empty());
    let held: Value =
        serde_json::from_str(&in_repo(&["human-items", "list", "--json", "--host"]).stdout)
            .unwrap();
    assert_eq!(
        held.as_array().unwrap().len(),
        1,
        "the hold stands while the projection is owed"
    );

    // The tracker returns: the next write projects what was owed, once, and binds it.
    std::fs::write(repo.join("onetaskgraph.yaml"), store(&github.board(1))).unwrap();
    let resumed = raise_in_repo(
        "question",
        &input(
            "asked",
            "question",
            "mgr",
            None,
            None,
            json!({"scope": "nothing"}),
        ),
    );
    resumed.exited(0);
    let state = github.state();
    let issues = projected(&state);
    assert_eq!(
        issues.len(),
        1,
        "{issues:#?}\nstderr: {}\nunsupported: {}",
        resumed.stderr,
        state["unsupported"]
    );
    assert_eq!(issues[0]["title"], "Decide first");
    let shown: Value =
        serde_json::from_str(&in_repo(&["human-items", "show", &id, "--json"]).stdout).unwrap();
    let tracker = &shown["item"]["tracker"];
    assert!(
        tracker["url"].as_str().unwrap().starts_with("http"),
        "{shown}"
    );

    // Replayed as though the record of it were lost: the store's intent answers
    // the same task, and no second one is created.
    std::fs::remove_file(world.runs.join("human-items/projected.json")).unwrap();
    raise_in_repo(
        "again",
        &input("first", "task", "mgr", None, None, json!({"scope": "host"})),
    )
    .exited(0);
    raise_in_repo(
        "third",
        &input(
            "third",
            "task",
            "mgr",
            None,
            None,
            json!({"scope": "nothing"}),
        ),
    )
    .exited(0);
    assert_eq!(projected(&github.state()).len(), 2);

    // Completing projects the terminal state onto the same task.
    in_repo(&["human-items", "complete", &id]).exited(0);
    let state = github.state();
    let done = projected(&state)
        .into_iter()
        .find(|i| i["title"] == "Decide first")
        .unwrap();
    let item = state["items"]
        .as_object()
        .unwrap()
        .values()
        .find(|it| it["content"] == done["id"])
        .unwrap();
    assert_eq!(item["values"]["STATUS_1"], "Done", "{item:#}");
    assert_eq!(item["values"]["PRIORITY_1"], "High");
    // The question was never written anywhere.
    assert!(projected(&state)
        .iter()
        .all(|i| i["title"] != "Decide asked"));

    // Dismissing projects the dismissal and its reason onto the same task.
    let third = projected(&state)
        .into_iter()
        .find(|i| i["title"] == "Decide third")
        .unwrap();
    let listed: Value =
        serde_json::from_str(&in_repo(&["human-items", "list", "--json"]).stdout).unwrap();
    let third_id = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["item"]["title"] == "Decide third")
        .unwrap()["item"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    in_repo(&[
        "human-items",
        "dismiss",
        &third_id,
        "--reason",
        "Handled in another plan",
    ])
    .exited(0);
    let state = github.state();
    let item = state["items"]
        .as_object()
        .unwrap()
        .values()
        .find(|it| it["content"] == third["id"])
        .unwrap();
    assert_eq!(item["values"]["STATUS_1"], "Canceled", "{item:#}");
    let body = state["issues"][third["id"].as_str().unwrap()]["body"]
        .as_str()
        .unwrap();
    assert!(body.contains("Handled in another plan"), "{body}");
    assert_eq!(
        projected(&state).len(),
        2,
        "no task was created for the dismissal"
    );
}

/// A planner ends a run by dropping its remaining work: applied with nothing
/// driving the run, that ending retires the run's own holds exactly as a
/// settlement does.
#[test]
fn a_run_its_planner_ends_retires_its_holds_with_the_run_ended() {
    let world = World::new("human-item-ended");
    enabled(&world);
    let name = "ended";
    let project = world.plan(
        name,
        &plan_of(name, vec![human("gate", &[]), agent("b", &["gate"])]),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    let owner = "session-human-item-ended";
    let gate = id_of(&items(&world, &[])[0]);
    let plan = raise(
        &world,
        "plan",
        &input(
            "freeze",
            "task",
            owner,
            Some(name),
            Some(&project),
            json!({"scope": "plan", "plan": project}),
        ),
    );
    let question = raise(
        &world,
        "question",
        &input(
            "unrelated",
            "question",
            owner,
            None,
            None,
            json!({"scope": "nothing"}),
        ),
    );
    world
        .run_with_stdin(
            &["reply", name],
            &json!({"version": 3, "commands": [{"op": "drop", "id": "gate", "dependents": "drop", "reason": "Superseded by another plan"}]}).to_string(),
        )
        .exited(0);
    for id in [&gate, &plan] {
        let item = state_of(&world, id);
        assert_eq!(item["state"], "dismissed", "{item}");
        assert_eq!(item["resolution"]["reason"], "the run ended");
    }
    assert_eq!(state_of(&world, &question)["state"], "open");
}

/// A plan whose name is the store's own directory launches under another run id,
/// so a run and the host's human-item store can never share a directory, and the
/// run's DAG is read under its own key.
#[test]
fn a_run_never_takes_the_human_item_stores_name() {
    let world = World::new("human-item-namespace");
    enabled(&world);
    raise(
        &world,
        "first",
        &input(
            "first",
            "question",
            "s",
            None,
            None,
            json!({"scope": "nothing"}),
        ),
    );
    let project = world.plan(
        "human-items",
        &plan_of(
            "human-items",
            vec![human("gate", &[]), agent("b", &["gate"])],
        ),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    assert!(!world.runs.join("human-items/launch.json").exists());
    let views = items(&world, &[]);
    let gate = views
        .iter()
        .find(|v| v["item"]["kind"] == "task")
        .expect("the gate's item");
    assert_eq!(gate["item"]["owner"]["run"], "human-items-2");
    assert_eq!(gate["blocked"]["total"], 1);
    world
        .run(&["runs", "--flat"])
        .exited(0)
        .out_has("human-items-2");
}

/// The harness's own zero-turn usage read is the identity outage's only probe:
/// the harness CLI here is a script answering `get_usage` the way `claude` does,
/// and once it reports headroom for both refused identities the item resolves
/// itself — with no dispatch made to find that out.
#[cfg(unix)]
#[test]
fn an_identity_outage_resolves_when_the_usage_probe_reports_headroom() {
    let world = World::new("human-item-identity-probe");
    enabled(&world);
    world.script(
        "build.died-as",
        "provider-failure fallback_chain_exhausted every candidate in the chain was attempted and none ran\n",
    );
    world.script(
        "build.exhausted",
        "claude-code:work quota out of extra usage\nclaude-code:home auth 401 Unauthorized\n",
    );
    let launch = checkout(&world, None);
    let headroom = world.root.join("headroom");
    let probes = world.root.join("probes.log");
    let usage = world.root.join("usage-claude.sh");
    std::fs::write(
        &usage,
        format!(
            r#"#!/bin/sh
echo "$@" >> "{probes}"
read line
id=$(printf '%s' "$line" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
if [ -f "{headroom}" ]; then
  limits='"rate_limits_available":true,"rate_limits":{{"five_hour":{{"utilization":10,"resets_at":"2026-07-29T18:30:00.000000+00:00"}}}}'
else
  limits='"rate_limits_available":false,"rate_limits":null'
fi
printf '{{"type":"control_response","response":{{"subtype":"success","request_id":"%s","response":{{"subscription_type":"max",%s}}}}}}\n' "$id" "$limits"
"#,
            probes = probes.display(),
            headroom = headroom.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&usage, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(
        launch.join("oneharness.toml"),
        format!(
            "[harness.claude-code]\nbin = \"{}\"\n[harness.claude-code.variant.work]\n[harness.claude-code.variant.home]\n",
            usage.display()
        ),
    )
    .unwrap();
    let name = "probed";
    let project = world.plan(name, &plan_of(name, vec![agent("build", &[])]));
    let mut start = world.cmd(&["start", &project, "--attach"]);
    start.current_dir(&launch);
    world.run_on(start, "start").settled();
    let host = host_items(&world);
    assert_eq!(host.len(), 1, "{host:?}");
    let id = id_of(&host[0]);
    let dispatched_before = dispatched(&world, name).len();
    // No headroom yet: probed, still open.
    world.until_within(
        std::time::Duration::from_secs(60),
        "the usage probe to run",
        |w| {
            w.run(&["human-items", "list", "--host"]).exited(0);
            std::fs::read_to_string(&probes)
                .is_ok_and(|log| log.contains("--input-format stream-json"))
        },
    );
    assert_eq!(state_of(&world, &id)["state"], "open");
    // Headroom returns: the next probe resolves it automatically.
    std::fs::write(&headroom, "").unwrap();
    world.until_within(
        std::time::Duration::from_secs(60),
        "the outage to resolve",
        |w| {
            w.run(&["human-items", "list", "--host"]).exited(0);
            state_of(w, &id)["state"] == "dismissed"
        },
    );
    let resolved = state_of(&world, &id);
    assert_eq!(resolved["resolution"]["actor"], "engine");
    assert!(
        resolved["resolution"]["reason"]
            .as_str()
            .unwrap()
            .contains("report headroom again"),
        "{resolved}"
    );
    assert_eq!(
        dispatched(&world, name).len(),
        dispatched_before,
        "a probe dispatches nothing"
    );
    let log = std::fs::read_to_string(&probes).unwrap();
    assert!(
        log.lines()
            .all(|l| l.contains("-p --input-format stream-json")),
        "{log}"
    );
}

/// A human step settled without its item — attested directly — leaves no item
/// open for it: the run's next driver resolves it as settled outside.
#[test]
fn a_step_settled_outside_its_item_resolves_the_item() {
    let world = World::new("human-item-outside");
    enabled(&world);
    let name = "outside";
    let project = world.plan(
        name,
        &plan_of(name, vec![human("gate", &[]), agent("b", &["gate"])]),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    let gate = id_of(&items(&world, &[])[0]);
    world.run(&["attest", name, "gate"]).exited(0);
    world.run(&["adopt", name]).exited(0);
    assert_eq!(settled(&world, name, "b").as_deref(), Some("done"));
    let item = state_of(&world, &gate);
    assert_eq!(item["state"], "dismissed", "{item}");
    assert!(
        item["resolution"]["reason"]
            .as_str()
            .unwrap()
            .contains("settled outside"),
        "{item}"
    );
}

/// A resolution recorded while its step could not be released — the run's store
/// refused the write — is released by the run's next driver once the resolver has
/// had its chance.
#[cfg(unix)]
#[test]
fn a_resolution_its_resolver_could_not_release_is_released_by_the_driver() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new("human-item-fallback");
    enabled(&world);
    let name = "fallback";
    let project = world.plan(
        name,
        &plan_of(name, vec![human("gate", &[]), agent("b", &["gate"])]),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    let gate = id_of(&items(&world, &[])[0]);
    let run = world.runs.join(name);
    let mode = std::fs::metadata(&run).unwrap().permissions().mode();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o500)).unwrap();
    let answered = world.run(&["human-items", "complete", &gate]);
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(mode)).unwrap();
    answered.exited(0).err_has("was not released");
    assert_eq!(state_of(&world, &gate)["state"], "completed");
    assert!(settled(&world, name, "b").is_none());
    world.run(&["adopt", name, "--detach"]).exited(0);
    world.until_within(
        std::time::Duration::from_secs(90),
        "the driver to release the step",
        |w| settled(w, name, "b").as_deref() == Some("done"),
    );
}
