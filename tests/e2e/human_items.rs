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

/// Without a filter, a manager's `list` and `report` show what its own session
/// raised or its runs raised, and what from elsewhere holds its work — the host,
/// or a plan or run it launched — and count the rest on the line naming `--all`.
/// A process with no session sees the whole host and says so; an explicit filter
/// and `--json` keep reaching every session's items.
#[test]
fn each_manager_sees_its_own_items_and_what_holds_its_work() {
    let mine = World::new("human-item-reach");
    enabled(&mine);
    let theirs = mine.as_session("session-other-manager");
    let name = "reached";
    let project = mine.plan(
        name,
        &plan_of(name, vec![human("gate", &[]), agent("after", &["gate"])]),
    );
    // This manager's run waits on its person, so it and its plan stay live.
    mine.run(&["start", &project, "--attach"]).exited(0);
    assert_eq!(items(&mine, &[]).len(), 1);
    let me = mine.session.clone();
    let other = theirs.session.clone();
    let raised = [
        // This session's own, holding nothing.
        (
            "own",
            input(
                "own",
                "question",
                &me,
                None,
                None,
                json!({"scope": "nothing"}),
            ),
        ),
        // Raised by this session's run under another session's name.
        (
            "by-my-run",
            input(
                "by-my-run",
                "task",
                "session-sub-worker",
                Some(name),
                Some(&project),
                json!({"scope": "nothing"}),
            ),
        ),
        // The other manager's: its own, a hold on its own plan, and holds reaching
        // this session's work through the host, this plan and this run.
        (
            "their-own",
            input(
                "their-own",
                "task",
                &other,
                None,
                None,
                json!({"scope": "nothing"}),
            ),
        ),
        (
            "their-plan",
            input(
                "their-plan",
                "task",
                &other,
                None,
                Some("plans:elsewhere"),
                json!({"scope": "plan", "plan": "plans:elsewhere"}),
            ),
        ),
        (
            "their-host",
            input(
                "their-host",
                "task",
                &other,
                None,
                None,
                json!({"scope": "host"}),
            ),
        ),
        (
            "on-my-plan",
            input(
                "on-my-plan",
                "question",
                &other,
                None,
                None,
                json!({"scope": "plan", "plan": project}),
            ),
        ),
        (
            "on-my-run",
            input(
                "on-my-run",
                "task",
                &other,
                None,
                None,
                json!({"scope": "work", "run": name, "node": "after"}),
            ),
        ),
    ];
    for (file, item) in &raised {
        raise(&theirs, file, item);
    }
    let gate = "Do gate, which only a person can do.";
    let title = |dedup: &str| format!("- Decide {dedup} (hi-");
    let everything: Vec<String> = raised
        .iter()
        .map(|(dedup, _)| title(dedup))
        .chain([format!("- {gate} (hi-")])
        .collect();
    let shows = |text: &str, wanted: &[&str], left_out: &[&str]| {
        for dedup in wanted {
            let entry = if *dedup == "gate" {
                format!("- {gate} (hi-")
            } else {
                title(dedup)
            };
            assert!(text.contains(&entry), "{dedup} missing: {text}");
        }
        for dedup in left_out {
            assert!(!text.contains(&title(dedup)), "{dedup} shown: {text}");
        }
    };
    for verb in ["list", "report"] {
        let text = mine.run(&["human-items", verb]).exited(0).stdout.clone();
        shows(
            &text,
            &[
                "gate",
                "own",
                "by-my-run",
                "their-host",
                "on-my-plan",
                "on-my-run",
            ],
            &["their-own", "their-plan"],
        );
        assert!(
            text.contains("2 other open items on this host (--all lists every item)"),
            "{text}"
        );

        let text = theirs.run(&["human-items", verb]).exited(0).stdout.clone();
        shows(
            &text,
            &[
                "their-own",
                "their-plan",
                "their-host",
                "on-my-plan",
                "on-my-run",
            ],
            &["own", "by-my-run"],
        );
        assert!(!text.contains(gate), "{text}");
        assert!(
            text.contains("3 other open items on this host (--all lists every item)"),
            "{text}"
        );

        for world in [&mine, &theirs] {
            let text = world
                .run(&["human-items", verb, "--all"])
                .exited(0)
                .stdout
                .clone();
            for entry in &everything {
                assert!(text.contains(entry.as_str()), "--all lacks {entry}: {text}");
            }
            assert!(text.contains("Showing every item on this host."), "{text}");
        }

        // No session to resolve: the whole host, and the summary says so.
        let output = mine
            .cmd(&["human-items", verb])
            .env_remove("ONEPIPELINE_LAUNCHER_SESSION")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        for entry in &everything {
            assert!(
                text.contains(entry.as_str()),
                "unresolved lacks {entry}: {text}"
            );
        }
        assert!(text.contains("so this is showing the whole host"), "{text}");

        // An explicit filter, run as the other manager, reaches every session.
        let text = theirs
            .run(&["human-items", verb, "--kind", "question"])
            .exited(0)
            .stdout
            .clone();
        shows(
            &text,
            &["own", "on-my-plan"],
            &["their-own", "by-my-run", "their-host"],
        );
        assert!(!text.contains("--all"), "{text}");
        let text = theirs
            .run(&["human-items", verb, "--session", &me])
            .exited(0)
            .stdout
            .clone();
        shows(&text, &["gate", "own"], &["their-host", "by-my-run"]);
    }
    let report = mine
        .run(&["human-items", "report"])
        .exited(0)
        .stdout
        .clone();
    assert!(
        report.starts_with("Human items: 6 listed, 4 blocking\n"),
        "{report}"
    );

    // `--json` is never narrowed: every item on the host, in the shape it always had.
    let all = items(&theirs, &[]);
    assert_eq!(all.len(), everything.len());
    assert_eq!(all, items(&mine, &[]));
    assert_eq!(all, items(&mine, &["--all", "--full"]));
    for view in &all {
        let keys: Vec<&String> = view.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["item", "blocked"], "{view}");
        let fields: Vec<&String> = view["item"].as_object().unwrap().keys().collect();
        assert_eq!(fields.len(), 15, "{fields:?}");
    }

    // A run whose launch record cannot be read is nobody's, as the run views judge
    // it: what its run raised and what holds its plan or run fall out of this
    // session's view, while its own and the host's hold stay.
    std::fs::write(mine.runs.join(name).join("launch.json"), "{not json").unwrap();
    for verb in ["list", "report"] {
        let text = mine.run(&["human-items", verb]).stdout.clone();
        shows(
            &text,
            &["own", "their-host"],
            &["by-my-run", "on-my-plan", "on-my-run", "their-own"],
        );
        assert!(
            text.contains("other open items on this host (--all lists every item)"),
            "{text}"
        );
    }
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
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("unshare:"),
        "the namespace could not be set up (`just bootstrap` provisions it): {stderr}"
    );
    output.status.code().unwrap_or(-1)
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
         codex auth 401 Unauthorized: token expired\n\
         gemini server_overloaded 503 the provider is unavailable\n",
    );
    let hosts = host_items(&world);
    assert_eq!(hosts.len(), 1, "{hosts:?}");
    let host = &hosts[0]["item"];
    assert_eq!(host["priority"], "urgent");
    assert_eq!(host["owner"]["session"], "session-human-item-identities");
    let body = host["body"].as_str().unwrap();
    assert!(
        body.contains("claude-code:work [quota]")
            && body.contains("codex [auth]")
            && body.contains("gemini [server_overloaded]"),
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

    // The tracker returns: the next write projects what was owed, once, and binds it
    // — though the record that it did cannot be written, because the host refuses it
    // (a directory stands where the record goes). That is said, and costs a replay.
    std::fs::write(repo.join("onetaskgraph.yaml"), store(&github.board(1))).unwrap();
    let record = world.runs.join("human-items/projected.json");
    std::fs::create_dir(&record).unwrap();
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

    resumed.err_has("could not be recorded");
    // The host takes records again: the next write replays the projection, and the
    // store's intent answers the same task — no second one is created.
    std::fs::remove_dir(&record).unwrap();
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

    // An open task raised again with new content is updated in place locally and
    // keeps the one tracker task it was projected as: the store binds a projection
    // to the content it was first written with, so nothing second is filed.
    let mut renamed = input(
        "third",
        "task",
        "mgr",
        None,
        None,
        json!({"scope": "nothing"}),
    );
    renamed["title"] = json!("Decide third, renamed");
    renamed["priority"] = json!("urgent");
    let updated = raise_in_repo("renamed", &renamed);
    updated.exited(0);
    let updated: Value = serde_json::from_str(&updated.stdout).unwrap();
    assert_eq!(updated["item"]["item"]["title"], "Decide third, renamed");
    assert!(
        updated["item"]["item"]["tracker"]["url"].is_string(),
        "{updated}"
    );
    assert_eq!(
        projected(&github.state()).len(),
        2,
        "no second task for the update"
    );

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
        .find(|v| v["item"]["title"] == "Decide third, renamed")
        .unwrap()["item"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The projection record cannot be read now — a directory stands where it goes —
    // so the dismissal recovers the content the task was first projected with from
    // the journal's own tracker binding, not from the renamed item.
    std::fs::remove_file(&record).unwrap();
    std::fs::create_dir(&record).unwrap();
    in_repo(&[
        "human-items",
        "dismiss",
        &third_id,
        "--reason",
        "Handled in another plan",
    ])
    .exited(0)
    .err_has("could not be recorded");
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
        state["issues"][third["id"].as_str().unwrap()]["title"],
        "Decide third",
        "the task keeps the content it was first projected with"
    );
    // The record reads again: the next write replays what it owed, and the store
    // answers it with the same task.
    std::fs::remove_dir(&record).unwrap();
    raise_in_repo(
        "after",
        &input(
            "after",
            "question",
            "mgr",
            None,
            None,
            json!({"scope": "nothing"}),
        ),
    )
    .exited(0);
    let state = github.state();
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
    // Nothing of the store exists yet: the name is refused for itself, not because
    // a directory happened to be there.
    assert!(!world.runs.join("human-items").exists());
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

/// A launch whose configuration is malformed says so and raises nothing; the run
/// goes on as a run with human items off.
#[test]
fn a_launch_with_malformed_configuration_says_so_and_raises_nothing() {
    let world = World::new("human-item-malformed-launch");
    let dir = world.root.join("xdg/onepipeline");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.yaml"),
        "human_items:\n  enabled: true\n  task_source: ' '\n",
    )
    .unwrap();
    let name = "malformed";
    let project = world.plan(
        name,
        &plan_of(name, vec![human("gate", &[]), agent("b", &["gate"])]),
    );
    world
        .run(&["start", &project, "--attach"])
        .exited(0)
        .err_has("reads no human items, because its configuration is malformed");
    assert!(!world.runs.join("human-items/events.ndjson").exists());
}

/// A driver that cannot read the item store keeps holding what it last read and
/// says so; once the store reads again, the release it was waiting for arrives.
#[cfg(unix)]
#[test]
fn a_driver_keeps_its_holds_while_the_item_store_is_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new("human-item-unreadable");
    enabled(&world);
    let name = "unreadable";
    let project = world.plan(name, &plan_of(name, vec![agent("a", &[])]));
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
    world.run(&["start", &project, "--detach"]).exited(0);
    stays_undispatched(&world, name, std::time::Duration::from_secs(2));
    let store = world.runs.join("human-items");
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o000)).unwrap();
    stays_undispatched(&world, name, std::time::Duration::from_secs(2));
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
    world.run(&["human-items", "complete", &host]).exited(0);
    world.until("the held node to run", |w| {
        settled(w, name, "a").as_deref() == Some("done")
    });
}

/// A driver whose very first read of the item store fails holds every step
/// rather than none, because a host hold it has never read may stand; once the
/// store reads, the hold it was waiting on is honored and its release arrives.
#[cfg(unix)]
#[test]
fn a_driver_that_has_never_read_the_item_store_holds_every_step() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new("human-item-unread-first");
    enabled(&world);
    let name = "unreadfirst";
    let project = world.plan(name, &plan_of(name, vec![agent("a", &[])]));
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
    let store = world.runs.join("human-items");
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o000)).unwrap();
    world.run(&["start", &project, "--detach"]).exited(0);
    stays_undispatched(&world, name, std::time::Duration::from_secs(3));
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
    stays_undispatched(&world, name, std::time::Duration::from_secs(1));
    world.run(&["human-items", "complete", &host]).exited(0);
    world.until("the held node to run", |w| {
        settled(w, name, "a").as_deref() == Some("done")
    });
}

/// While a driver is running, a resolution whose resolver could not release its
/// step — the run's command queue refused the resolver — is released by that same
/// driver once the resolver has had its chance.
#[cfg(unix)]
#[test]
fn a_running_driver_releases_a_resolution_its_resolver_could_not() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new("human-item-grace");
    enabled(&world);
    world.script("side.wait", "hold");
    let name = "grace";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                human("gate", &[]),
                agent("b", &["gate"]),
                agent("side", &[]),
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the gate's item and the held side", |w| {
        dispatched(w, name).contains(&"side".to_owned())
            && items(w, &[])
                .iter()
                .any(|v| v["item"]["dedup_key"] == format!("run:{name}#gate"))
    });
    let gate = id_of(&items(&world, &[])[0]);
    let channel = world.runs.join(name).join("channel");
    let mode = std::fs::metadata(&channel).unwrap().permissions().mode();
    std::fs::set_permissions(&channel, std::fs::Permissions::from_mode(0o500)).unwrap();
    let answered = world.run(&["human-items", "complete", &gate]);
    std::fs::set_permissions(&channel, std::fs::Permissions::from_mode(mode)).unwrap();
    answered.exited(0).err_has("was not released");
    world.until_within(
        std::time::Duration::from_secs(60),
        "the driver to release the step",
        |w| settled(w, name, "b").as_deref() == Some("done"),
    );
    world.release("side.go");
}

/// The full report previews the first three steps an item holds in each plan
/// and counts the rest; a run removed from the runs root takes its steps out of
/// every summary and detail.
#[test]
fn the_report_previews_three_steps_and_a_removed_run_leaves_the_summary() {
    let world = World::new("human-item-report");
    enabled(&world);
    let name = "previewed";
    let mut nodes = vec![human("gate", &[])];
    for n in 1..=5 {
        let mut step = agent(&format!("step-{n}"), &["gate"]);
        step["title"] = json!(format!("Step {n}"));
        nodes.push(step);
    }
    let project = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &project, "--attach"]).exited(0);
    let report = world.run(&["human-items", "report", "--full"]);
    report.exited(0);
    let text = &report.stdout;
    assert!(
        text.contains(&format!("blocks 5 steps in {project}")),
        "{text}"
    );
    for n in 1..=3 {
        assert!(
            text.contains(&format!("  - {name} / step-{n}: Step {n}\n")),
            "{text}"
        );
    }
    assert!(!text.contains("step-4: Step 4"), "{text}");
    assert!(
        text.contains(&format!("  - and 2 more in {project}\n")),
        "{text}"
    );
    let gate = id_of(&items(&world, &[])[0]);
    let shown = world.run(&["human-items", "show", &gate, "--json"]);
    let shown: Value = serde_json::from_str(&shown.stdout).unwrap();
    assert_eq!(shown["nodes"].as_array().unwrap().len(), 5);

    // The run is removed from the runs root: nothing it held is reported any more.
    std::fs::remove_dir_all(world.runs.join(name)).unwrap();
    let after = items(&world, &[]);
    assert_eq!(after[0]["blocked"]["total"], 0, "{after:?}");
    let shown = world.run(&["human-items", "show", &gate, "--json"]);
    let shown: Value = serde_json::from_str(&shown.stdout).unwrap();
    assert!(shown["nodes"].as_array().unwrap().is_empty(), "{shown}");
}

/// A run whose journal or launch record cannot be read is said to be uncounted
/// on stderr, never answered from the counts an earlier read cached; its item
/// stays listed, and once the run reads again its blocked steps are counted.
#[test]
fn a_run_that_cannot_be_read_is_reported_uncounted_and_counted_once_it_reads() {
    let world = World::new("human-item-unreadable-run");
    enabled(&world);
    let name = "unread";
    let mut nodes = vec![human("gate", &[])];
    for n in 1..=2 {
        nodes.push(agent(&format!("step-{n}"), &["gate"]));
    }
    let project = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &project, "--attach"]).exited(0);
    let counted = |world: &World| {
        let listed = world.run(&["human-items", "list", "--json"]);
        listed.exited(0);
        let views: Value = serde_json::from_str(&listed.stdout).unwrap();
        assert_eq!(views.as_array().unwrap().len(), 1, "{views}");
        (
            views[0]["blocked"]["total"].as_u64().unwrap(),
            listed.stderr,
        )
    };
    // Read once, so the index caches the run's blocked steps.
    let (total, stderr) = counted(&world);
    assert_eq!(total, 2);
    assert!(!stderr.contains("are not counted"), "{stderr}");

    let run = world.runs.join(name);
    // The journal is what the cached counts are keyed to, so it is broken behind
    // a warm index. A launch record is written once and a count cached from it
    // stays true, so it is broken where the run must be derived afresh: with the
    // private index gone, as after a rebuild.
    for file in ["events.jsonl", "launch.json"] {
        let path = run.join(file);
        let kept = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        if file == "events.jsonl" {
            std::fs::create_dir(&path).unwrap();
        } else {
            std::fs::write(&path, "not a launch").unwrap();
            std::fs::remove_file(world.runs.join("human-items/index.json")).unwrap();
        }
        let (total, stderr) = counted(&world);
        assert_eq!(total, 0, "a stale count served for an unreadable {file}");
        assert!(
            stderr.contains(&format!("the steps run {name} blocks are not counted"))
                && stderr.contains(file),
            "{file}: {stderr}"
        );
        // Every read says so while it lasts, not only the one that dropped it.
        assert!(counted(&world).1.contains("are not counted"));

        if path.is_dir() {
            std::fs::remove_dir(&path).unwrap();
        }
        std::fs::write(&path, kept).unwrap();
        let (total, stderr) = counted(&world);
        assert_eq!(total, 2, "{file} restored: {stderr}");
        assert!(!stderr.contains("are not counted"), "{stderr}");
    }
}

/// A hold raised while work is running stops nothing already running: that work
/// settles, and only what would start after it is held.
#[test]
fn a_hold_raised_during_running_work_lets_it_finish_and_holds_what_follows() {
    let world = World::new("human-item-midrun");
    enabled(&world);
    world.script("slow.wait", "hold");
    let name = "midrun";
    let project = world.plan(
        name,
        &plan_of(name, vec![agent("slow", &[]), agent("after", &["slow"])]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the slow node to run", |w| {
        dispatched(w, name).contains(&"slow".to_owned())
    });
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
    world.release("slow.go");
    world.until("the running node to settle", |w| {
        settled(w, name, "slow").as_deref() == Some("done")
    });
    stays_undispatched_after(&world, name, "after", std::time::Duration::from_secs(2));
    world.run(&["human-items", "complete", &host]).exited(0);
    world.until("the held node to run", |w| {
        settled(w, name, "after").as_deref() == Some("done")
    });
}

fn stays_undispatched_after(world: &World, run: &str, node: &str, for_: std::time::Duration) {
    let until = std::time::Instant::now() + for_;
    while std::time::Instant::now() < until {
        assert!(
            !dispatched(world, run).contains(&node.to_owned()),
            "{node} was dispatched under a hold"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// An outage whose probe could not be recorded — the host refuses the probe
/// record — is still raised and durable, says so, stays open until a person
/// dismisses it, and is probed again once it is raised again.
#[test]
fn an_outage_whose_probe_cannot_be_recorded_stays_open_until_raised_again() {
    let world = World::new("human-item-probe-record");
    enabled(&world);
    world.script(
        "build.died-as",
        "provider-failure fallback_chain_exhausted every candidate in the chain was attempted and none ran\n",
    );
    world.script(
        "build.exhausted",
        "claude-code:work quota out of extra usage\ncodex auth 401 Unauthorized\n",
    );
    let record = world.runs.join("human-items/probes.json");
    std::fs::create_dir_all(&record).unwrap();
    let project = world.plan("first", &plan_of("first", vec![agent("build", &[])]));
    world
        .run(&["start", &project, "--attach"])
        .settled()
        .err_has("probe record could not be written");
    let host = host_items(&world);
    assert_eq!(host.len(), 1, "{host:?}");
    let id = id_of(&host[0]);
    world
        .run(&["human-items", "list", "--host"])
        .exited(0)
        .err_has("cannot be read");
    assert_eq!(state_of(&world, &id)["state"], "open");
    world
        .run(&[
            "human-items",
            "dismiss",
            &id,
            "--reason",
            "Restored by hand",
        ])
        .exited(0);
    // The record can be written again; the next outage records its probe.
    std::fs::remove_dir(&record).unwrap();
    let project = world.plan("second", &plan_of("second", vec![agent("build", &[])]));
    world.run(&["start", &project, "--attach"]).settled();
    let again = host_items(&world);
    assert_eq!(again.len(), 1, "{again:?}");
    assert_ne!(id_of(&again[0]), id);
    let probes: Value = serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    assert_eq!(probes["identities-refused"]["probe"], "identities");
}

/// A design review whose document changed is projected as what it became in the
/// same command that refused it: the obsolete task cancelled on the tracker and
/// its replacement filed, then completed through the authority.
#[test]
fn a_stale_design_review_projects_its_dismissal_and_replacement_at_once() {
    use crate::private_github_home::{described, GitHub};
    let onevcs = crate::harness::onevcs_binary()
        .to_string_lossy()
        .into_owned();
    let world = World::new("human-item-design-tracker")
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
    let key = world.root.join("current-key");
    std::fs::write(&key, "v1").unwrap();
    let adapter = world.root.join("approve.sh");
    std::fs::write(
        &adapter,
        format!(
            "request=$(cat)\ncopy=$(printf '%s' \"$request\" | sed 's/.*\"board_copy\":\"\\([^\"]*\\)\".*/\\1/')\n\
             printf '{{\"board_copy\":\"%s\",\"key\":\"%s\"}}' \"$copy\" \"$(cat '{}')\"\n",
            // Single-quoted, so a Windows path's backslashes reach `cat` as written.
            key.display()
        ),
    )
    .unwrap();
    let repo = checkout(
        &world,
        Some(&format!(
            "human_items:\n  enabled: true\n  task_source: board\n  approval_adapters:\n    \
             approve-design:\n      command: [\"sh\", {}]\n  hooks:\n    - phase: design-review\n      \
             kind: task\n      title: Approve the design\n      body: Read the design and approve it.\n      \
             priority: high\n      block: {{scope: nothing}}\n      approval: {{adapter: approve-design}}\n",
            // A YAML double-quoted scalar, so a Windows path's backslashes are not escapes.
            serde_json::to_string(&adapter.to_string_lossy()).unwrap()
        )),
    );
    let intents = world.root.join("intents");
    std::fs::create_dir_all(&intents).unwrap();
    let store_at = |endpoint: &str| {
        std::fs::write(
            repo.join("onetaskgraph.yaml"),
            format!(
            "sources:\n  board:\n    plugin: github-projects\n    visibility: public\n    \
             task_projection:\n      host: projection-host\n      directory: {}\n      project: human\n    \
             config:\n      owner: owner\n      project_number: 1\n      repository: owner/service\n      \
             token_env: ONEPIPELINE_LOOPBACK_BOARD_TOKEN\n      endpoint: {}\n      status_mapping:\n        \
             todo: Todo\n        queued: Queued\n        in-progress: In Progress\n        unknown: Needs Attention\n        \
             done: Done\n        cancelled: Canceled\n      priority_mapping:\n        urgent: Urgent\n        \
             high: High\n        medium: Medium\n        low: Low\n",
            intents.display(),
            endpoint
        ),
        )
        .unwrap();
    };
    store_at(&github.board(1));
    let in_repo = |args: &[&str]| {
        let mut command = world.cmd(args);
        command.current_dir(&repo);
        world.run_on(command, &args.join(" "))
    };
    let context = world.root.join("context.json");
    std::fs::write(
        &context,
        json!({"owner": {"session": "mgr", "run": null, "plan": null}, "dedup_key": "doc",
               "approval": {"board_copy": "board:doc-1", "key": "v1"}})
        .to_string(),
    )
    .unwrap();
    let raised = in_repo(&[
        "human-items",
        "raise",
        "--phase",
        "design-review",
        "--context",
        context.to_str().unwrap(),
    ]);
    raised.exited(0);
    let obsolete: String = serde_json::from_str::<Value>(&raised.stdout).unwrap()["item"]["item"]
        ["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let status_of = |state: &Value, issue: &Value| {
        state["items"]
            .as_object()
            .unwrap()
            .values()
            .find(|it| it["content"] == issue["id"])
            .unwrap()["values"]["STATUS_1"]
            .clone()
    };
    let filed = |state: &Value| -> Vec<Value> {
        state["issues"]
            .as_object()
            .unwrap()
            .values()
            .filter(|i| i["title"] == "Approve the design")
            .cloned()
            .collect()
    };
    assert_eq!(filed(&github.state()).len(), 1);

    std::fs::write(&key, "v2").unwrap();
    in_repo(&["human-items", "complete", &obsolete]).err_has("Re-read the current document");
    let state = github.state();
    let issues = filed(&state);
    assert_eq!(
        issues.len(),
        2,
        "the replacement was filed in the same command: {issues:#?}"
    );
    let statuses: Vec<Value> = issues.iter().map(|i| status_of(&state, i)).collect();
    assert!(
        statuses.contains(&json!("Canceled")) && statuses.contains(&json!("Todo")),
        "{statuses:?}"
    );

    let listed: Value =
        serde_json::from_str(&in_repo(&["human-items", "list", "--json"]).stdout).unwrap();
    let replacement = listed[0]["item"]["id"].as_str().unwrap().to_owned();
    in_repo(&["human-items", "complete", &replacement]).exited(0);
    let state = github.state();
    let statuses: Vec<Value> = filed(&state).iter().map(|i| status_of(&state, i)).collect();
    assert!(
        statuses.contains(&json!("Canceled")) && statuses.contains(&json!("Done")),
        "{statuses:?}"
    );
    assert_eq!(filed(&state).len(), 2);

    // A task resolved while its tracker is unreachable is projected once the
    // tracker answers again — created and settled in that one projection — and a
    // replay of it creates nothing more.
    store_at("http://127.0.0.1:9/graphql");
    let plain = world.root.join("plain.json");
    std::fs::write(
        &plain,
        json!({"schema_version": 1, "dedup_key": "offline", "kind": "task",
               "title": "Rotate the offline key", "body": "Rotate it.", "priority": "low",
               "owner": {"session": "mgr", "run": null, "plan": null},
               "block": {"scope": "nothing"}, "approval": null})
        .to_string(),
    )
    .unwrap();
    let raised = in_repo(&["human-items", "raise", "--input", plain.to_str().unwrap()]);
    raised.exited(0).err_has("its projection into board failed");
    let offline: String = serde_json::from_str::<Value>(&raised.stdout).unwrap()["item"]["item"]
        ["id"]
        .as_str()
        .unwrap()
        .to_owned();
    in_repo(&["human-items", "complete", &offline])
        .exited(0)
        .err_has("its projection into board failed");
    store_at(&github.board(1));
    for _ in 0..2 {
        in_repo(&["human-items", "raise", "--input", plain.to_str().unwrap()]).exited(0);
    }
    let state = github.state();
    let resolved: Vec<Value> = state["issues"]
        .as_object()
        .unwrap()
        .values()
        .filter(|i| i["title"] == "Rotate the offline key")
        .cloned()
        .collect();
    assert_eq!(resolved.len(), 1, "{resolved:#?}");
    assert_eq!(status_of(&state, &resolved[0]), "Done");
}

/// A human step whose item could not be raised — the host refused the store —
/// still waits, says why, and gets exactly one item from the next driver once the
/// store takes writes again.
#[cfg(unix)]
#[test]
fn a_human_steps_item_the_store_refused_is_raised_once_by_the_next_driver() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new("human-item-raise-retry");
    enabled(&world);
    let store = world.runs.join("human-items");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o500)).unwrap();
    let name = "retried";
    let project = world.plan(
        name,
        &plan_of(name, vec![human("gate", &[]), agent("b", &["gate"])]),
    );
    let started = world.run(&["start", &project, "--attach"]);
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
    started
        .exited(0)
        .err_has("could not raise the human item for 'gate'");
    assert!(items(&world, &[]).is_empty());
    assert!(!dispatched(&world, name).contains(&"b".to_owned()));
    world.run(&["adopt", name]).exited(0);
    world.run(&["adopt", name]).exited(0);
    let open = items(&world, &[]);
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0]["item"]["dedup_key"], format!("run:{name}#gate"));
    assert!(!dispatched(&world, name).contains(&"b".to_owned()));
}
