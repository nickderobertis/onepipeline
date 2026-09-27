//! C6b journeys, through the compiled binary over a real `local-md` store; see
//! `docs/contract-divergences.md` entry 91.

// llmlint: ignore-file[e2e_not_mocked] this suite's stand-ins are `harness.rs`'s, whose
// suppression states them and why; the store is the real `onetaskgraph` reading a folder of
// Markdown, and the refusals are the compiled binary's own.

use serde_json::{json, Value};

use crate::harness::{agent, human, plan_of, World, REFUSED};

/// The contract's own C6b block, which every refusal these journeys expect is
/// rendered from, so the shapes it states are the ones the binary prints.
fn contract_block() -> Value {
    let contract = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract.md"),
    )
    .expect("the contract ships");
    let block = contract
        .split("```json\n")
        .skip(1)
        .filter_map(|rest| rest.split("\n```").next())
        .find(|body| body.contains("\"criteria_rule\": {"))
        .expect("the contract carries the C6b block");
    serde_json::from_str::<Value>(block).expect("the C6b block is JSON")["criteria_rule"].clone()
}

fn refused(id: &str, rule: &str) -> String {
    contract_block()["refusal"]
        .as_str()
        .expect("the block states the refusal")
        .replace("<id>", id)
        .replace("<rule>", rule)
}

fn step_refused(id: &str, step: &str, rule: &str) -> String {
    contract_block()["step_refusal"]
        .as_str()
        .expect("the block states the step refusal")
        .replace("<id>", id)
        .replace("<step>", step)
        .replace("<rule>", rule)
}

/// `plan check`'s exit when the loader refused the plan.
const HAS_REFUSALS: i32 = 1;

/// A task breaking no rule, with further sections after its criteria.
const FOLLOWED: &str = "## What\nBuild it.\n\n## Acceptance criteria\n- It builds.\n\n\
                        ## Additional info\nRun the gate.";

/// A task whose criteria section ends the body.
const LAST: &str = "## What\nBuild it.\n\n## Acceptance criteria\n\n1. It builds.\n";

fn stating(id: &str, task: &str, deps: &[&str]) -> Value {
    json!({"id": id, "persona": "engineer", "task": task, "deps": deps})
}

/// A lifecycle node whose two steps are an agent step carrying `task` and a
/// human step whose prose states no criteria at all.
fn stepped(id: &str, task: &str, deps: &[&str]) -> Value {
    json!({
        "id": id, "repo": "service", "title": "feat: ship it", "deps": deps,
        "steps": [
            {"id": "implement", "persona": "engineer", "task": task},
            {"id": "approve", "kind": "human", "task": "Approve the branch.",
             "deps": ["implement"]}
        ]
    })
}

/// The three rules, each with a task breaking exactly it.
const BROKEN: [(&str, &str); 3] = [
    (
        "## What\nBuild it.\n\n## Why\nSo it is built.",
        "no criteria section",
    ),
    (
        "## Acceptance criteria\n- It builds.\n\n## Acceptance criteria\n- It ships.",
        "criteria section repeated",
    ),
    (
        "## What\nBuild it.\n\n## Acceptance criteria\nIt builds, in prose.\n-   \n\n\
         ## Additional info\n- a note, not a criterion",
        "no criteria listed",
    ),
];

#[test]
fn start_and_plan_check_refuse_each_rule_naming_the_node_and_create_no_run() {
    let world = World::new("c6b-start");
    for (index, (task, rule)) in BROKEN.iter().enumerate() {
        let name = format!("broken{index}");
        let project = world.plan(
            &name,
            &plan_of(
                &name,
                vec![human("approve", &[]), stating("build", task, &["approve"])],
            ),
        );
        world
            .run(&["start", &project, "--detach"])
            .exited(REFUSED)
            .err_has(&refused("build", rule));
        assert!(
            !world.runs.join(&name).exists(),
            "a refused launch minted the run '{name}'"
        );
        world
            .run(&["plan", "check", &project])
            .exited(HAS_REFUSALS)
            .out_has("build")
            .out_has(rule);
    }
    // An `expects_no_diff` node is an agent node, and is held to the rule too.
    let project = world.plan(
        "nodiff",
        &plan_of(
            "nodiff",
            vec![json!({"id": "wait", "expects_no_diff": true, "task": "## What\nWait."})],
        ),
    );
    world
        .run(&["start", &project, "--detach"])
        .exited(REFUSED)
        .err_has(&refused("wait", "no criteria section"));
}

#[test]
fn a_steps_task_is_refused_naming_the_step_and_a_human_step_is_exempt() {
    let world = World::new("c6b-steps");
    for (index, (task, rule)) in BROKEN.iter().enumerate() {
        let name = format!("steps{index}");
        let project = world.plan(&name, &plan_of(&name, vec![stepped("ship", task, &[])]));
        world
            .run(&["start", &project, "--detach"])
            .exited(REFUSED)
            .err_has(&step_refused("ship", "implement", rule));
        world
            .run(&["plan", "check", &project])
            .exited(HAS_REFUSALS)
            .out_has("step 'implement'")
            .out_has(rule);
    }
    // The human step's prose states no criteria, and the plan loads.
    let project = world.plan(
        "stepsok",
        &plan_of("stepsok", vec![stepped("ship", FOLLOWED, &[])]),
    );
    world.run(&["plan", "check", &project]).exited(0);
}

#[test]
fn a_section_followed_by_more_sections_or_ending_the_body_loads_and_a_human_node_is_exempt() {
    let world = World::new("c6b-loads");
    // `human` states no criteria section at all.
    let project = world.plan(
        "placed",
        &plan_of(
            "placed",
            vec![
                human("approve", &[]),
                stating("followed", FOLLOWED, &["approve"]),
                stating("last", LAST, &["approve"]),
            ],
        ),
    );
    world.run(&["plan", "check", &project]).exited(0);
    world.run(&["start", &project, "--attach"]).exited(0);
    // Loaded unchanged: the task the run was launched with is the task as written.
    let started = world.events_of("placed", "run-started");
    let tasks = &started[0]["payload"]["plan"]["tasks"];
    let task_of = |id: &str| {
        tasks
            .as_array()
            .expect("the plan's tasks")
            .iter()
            .find(|node| node["id"] == id)
            .and_then(|node| node["task"].as_str())
            .unwrap_or_else(|| panic!("no task for '{id}' in {tasks}"))
            .to_owned()
    };
    assert_eq!(task_of("followed"), FOLLOWED);
    assert_eq!(task_of("last").trim_end(), LAST.trim_end());
}

#[test]
fn an_add_retry_or_requeue_stating_a_task_that_breaks_c6b_is_refused_and_changes_nothing() {
    let world = World::new("c6b-edits");
    let name = "edited";
    // `flaky` fails, so a `retry` may supersede it; `build`, `ship` and `sign` wait on
    // a person, so a `cancel` parks each and a `requeue` may bring it back.
    world.script("flaky.fail", "1");
    world.repository("local-direct", &[]);
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                human("approve", &[]),
                agent("build", &["approve"]),
                agent("flaky", &[]),
                stepped("ship", FOLLOWED, &["approve"]),
                human("sign", &["approve"]),
            ],
        ),
    );
    world.run(&["start", &project, "--attach"]);
    assert!(
        world
            .events_of(name, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "flaky"),
        "the failing node never settled: {:?}",
        world.kinds(name)
    );
    let edit = |command: Value| {
        world.run_with_stdin(
            &["reply", name],
            &json!({"version": 2, "commands": [command]}).to_string(),
        )
    };
    for id in ["build", "ship", "sign"] {
        edit(json!({"op": "cancel", "id": id})).exited(0);
    }
    let committed = world.events_of(name, "edit-committed").len();
    let journal = world.journal(name).len();

    let (no_section, repeated, none_listed) = (BROKEN[0].0, BROKEN[1].0, BROKEN[2].0);
    edit(json!({"op": "add", "node": stating("late", no_section, &[])}))
        .exited(REFUSED)
        .err_has(&refused("late", "no criteria section"));
    edit(json!({"op": "add", "node": stepped("ship-late", none_listed, &[])}))
        .exited(REFUSED)
        .err_has(&step_refused(
            "ship-late",
            "implement",
            "no criteria listed",
        ));
    edit(json!({"op": "retry", "id": "flaky", "node": stating("flaky-2", repeated, &[])}))
        .exited(REFUSED)
        .err_has(&refused("flaky-2", "criteria section repeated"));
    edit(json!({"op": "requeue", "id": "build", "amend": {"task": none_listed}}))
        .exited(REFUSED)
        .err_has(&refused("build", "no criteria listed"));
    // A requeue restating the steps is held to C6b step by step, and one turning a
    // person's action into an agent's holds the task it already had to it.
    let restated = stepped("ship", repeated, &["approve"])["steps"].clone();
    edit(json!({"op": "requeue", "id": "ship", "amend": {"steps": restated}}))
        .exited(REFUSED)
        .err_has(&step_refused(
            "ship",
            "implement",
            "criteria section repeated",
        ));
    edit(json!({"op": "requeue", "id": "sign", "amend": {"kind": "agent", "persona": "engineer"}}))
        .exited(REFUSED)
        .err_has(&refused("sign", "no criteria section"));

    assert_eq!(
        world.events_of(name, "edit-committed").len(),
        committed,
        "a refused edit was committed"
    );
    assert_eq!(
        world.journal(name).len(),
        journal,
        "a refused edit wrote to the run's journal: {:?}",
        world.kinds(name)
    );
    for id in ["late", "ship-late", "flaky-2"] {
        assert!(
            !world
                .journal(name)
                .iter()
                .any(|event| event.to_string().contains(&format!("\"{id}\""))),
            "the refused node '{id}' reached the run's journal"
        );
    }

    edit(json!({"op": "requeue", "id": "build", "amend": {"task": LAST}})).exited(0);
    edit(json!({"op": "add", "node": stating("late", FOLLOWED, &[])})).exited(0);
    edit(json!({"op": "retry", "id": "flaky", "node": stating("flaky-2", LAST, &[])})).exited(0);
    assert_eq!(world.events_of(name, "edit-committed").len(), committed + 3);
}

/// Rewrite the plan a run was launched with so node `id`'s task is `task` —
/// the record a build from before C6b left for a plan it loaded — and drop the
/// fold checkpoint, which the rewritten journal would no longer corroborate.
// llmlint: ignore-block[tests_mirror_real_usage] the state `adopt` is refused over — a run whose
// graph holds an undone agent node with no criteria — is reachable through no interface of this
// build by design, because C6b refuses every route in; it exists only as the record a build from
// before C6b wrote. That record differs from the one this build writes for the same plan in
// exactly the task text, which is what this rewrites, and a run root with no checkpoint is the
// documented fallback every predecessor's run root already takes. `adopt` itself is the compiled
// binary's own.
fn launched_before_c6b(world: &World, run: &str, id: &str, task: &str) {
    let path = world.run_file(run, "events.jsonl");
    let rewritten: Vec<String> = std::fs::read_to_string(&path)
        .expect("the run's journal reads")
        .lines()
        .map(|line| {
            let mut event: Value = serde_json::from_str(line).expect("a journal line is JSON");
            if event["kind"] == "run-started" {
                for node in event["payload"]["plan"]["tasks"]
                    .as_array_mut()
                    .expect("the launch plan's tasks")
                {
                    if node["id"] == id {
                        node["task"] = json!(task);
                    }
                }
            }
            event.to_string()
        })
        .collect();
    std::fs::write(&path, rewritten.join("\n") + "\n").expect("the journal is rewritten");
    let _ = std::fs::remove_file(world.run_file(run, "checkpoint.json"));
}
// llmlint: ignore-end[tests_mirror_real_usage]

#[test]
fn adopt_refuses_a_run_whose_undone_node_breaks_c6b_before_writing_anything() {
    let world = World::new("c6b-adopt-refused");
    let name = "older";
    // `build` is pending behind a person, `flaky` failed, and `held` is parked: each
    // could still be dispatched, and each holds the adoption back on its own.
    world.script("flaky.fail", "1");
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                human("approve", &[]),
                agent("build", &["approve"]),
                agent("flaky", &[]),
                agent("held", &["approve"]),
            ],
        ),
    );
    world.run(&["start", &project, "--attach"]);
    world
        .run_with_stdin(
            &["reply", name],
            &json!({"version": 2, "commands": [{"op": "cancel", "id": "held"}]}).to_string(),
        )
        .exited(0);
    let launch = std::fs::read_to_string(world.run_file(name, "launch.json"))
        .expect("the launch record reads");

    for broken in ["build", "flaky", "held"] {
        for id in ["build", "flaky", "held"] {
            let task = if id == broken { BROKEN[0].0 } else { FOLLOWED };
            launched_before_c6b(&world, name, id, task);
        }
        world
            .run(&["adopt", name])
            .exited(REFUSED)
            .err_has(&refused(broken, "no criteria section"));
    }
    world
        .run(&["adopt", name, "--detach"])
        .exited(REFUSED)
        .err_has(&refused("held", "no criteria section"));
    assert_eq!(
        std::fs::read_to_string(world.run_file(name, "launch.json"))
            .expect("the launch record reads"),
        launch,
        "a refused adoption rewrote the launch record"
    );
    assert!(!world.run_file(name, "launch.pre-adopt-1.json").exists());
    assert!(
        world.events_of(name, "driver-adopted").is_empty(),
        "{:?}",
        world.kinds(name)
    );
}

#[test]
fn adopt_goes_ahead_over_a_node_settled_done_that_breaks_c6b() {
    let world = World::new("c6b-adopt-done");
    let name = "finished";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![agent("early", &[]), human("approve", &["early"])],
        ),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    assert!(
        world
            .events_of(name, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "early"),
        "the agent node never settled: {:?}",
        world.kinds(name)
    );
    launched_before_c6b(&world, name, "early", BROKEN[0].0);
    world.run(&["attest", name, "approve"]).exited(0);
    world.run(&["adopt", name]).exited(0);
}

#[test]
fn a_run_launched_before_c6b_still_takes_edits_that_state_no_task_text() {
    let world = World::new("c6b-legacy-edits");
    let name = "legacy";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                human("approve", &[]),
                agent("build", &["approve"]),
                agent("spare", &["approve"]),
            ],
        ),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    let edit = |command: Value| {
        world.run_with_stdin(
            &["reply", name],
            &json!({"version": 2, "commands": [command]}).to_string(),
        )
    };
    edit(json!({"op": "cancel", "id": "spare"})).exited(0);
    for id in ["build", "spare"] {
        launched_before_c6b(&world, name, id, BROKEN[0].0);
    }
    let committed = world.events_of(name, "edit-committed").len();

    // Neither edit states task text, so neither is held to C6b over the tasks the
    // run was launched with.
    edit(json!({"op": "add", "node": stating("late", FOLLOWED, &["build"])}))
        .exited(0)
        .out_has("\"applied\"");
    edit(json!({"op": "requeue", "id": "spare", "amend": {"max_turns": 9}}))
        .exited(0)
        .out_has("\"applied\"");
    assert_eq!(world.events_of(name, "edit-committed").len(), committed + 2);
}
