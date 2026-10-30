//! Human items inside real runs: the compiled binary's reconcile loop over a real
//! run store and plan store, with the host's human-item journal beside it.
//!
//! Each journey launches a plan, acts on its items through `onepipeline
//! human-items` — the operations a person or a manager uses — and reads what the
//! scheduler did off the run's own journal: which nodes it dispatched, which it
//! held, and what became of the human step an item stood for.

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
