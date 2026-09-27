//! C6b: a dispatched task states its acceptance criteria, or it is refused.
//!
//! Every journey drives the compiled binary against a real `local-md`
//! `onetaskgraph` store: `start`, `plan check` and `adopt` refuse a plan whose
//! agent node or agent step breaks the rule, naming the node, the step where
//! there is one, and the rule; and an `add`, `retry` or `requeue` stating such a
//! task is refused before anything is applied. A `kind: human` node or step is
//! exempt, and a node settled `done` does not hold an adoption back. See
//! `docs/contract-divergences.md` entry 91.

// llmlint: ignore-file[e2e_not_mocked] this suite's stand-ins are `harness.rs`'s, whose
// suppression states them and why; the store is the real `onetaskgraph` reading a folder of
// Markdown, and the refusals are the compiled binary's own.

use serde_json::{json, Value};

use crate::harness::{agent, human, plan_of, World, REFUSED};

/// `plan check`'s exit when the loader refused the plan.
const HAS_REFUSALS: i32 = 1;

/// A task breaking no rule, with further sections after its criteria.
const FOLLOWED: &str = "## What\nBuild it.\n\n## Acceptance criteria\n- It builds.\n\n\
                        ## Additional info\nRun the gate.";

/// A task whose criteria section ends the body.
const LAST: &str = "## What\nBuild it.\n\n## Acceptance criteria\n\n1. It builds.\n";

/// One direct agent node carrying `task`.
fn stating(id: &str, task: &str, deps: &[&str]) -> Value {
    json!({"id": id, "persona": "engineer", "task": task, "deps": deps})
}

/// A lifecycle node whose two steps are an agent step carrying `task` and a
/// human step whose prose states no criteria at all.
fn stepped(task: &str) -> Value {
    json!({
        "id": "ship", "repo": "service", "title": "feat: ship it",
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
            .err_has(&format!("node 'build': {rule}"));
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
        .err_has("node 'wait': no criteria section");
}

#[test]
fn a_steps_task_is_refused_naming_the_step_and_a_human_step_is_exempt() {
    let world = World::new("c6b-steps");
    for (index, (task, rule)) in BROKEN.iter().enumerate() {
        let name = format!("steps{index}");
        let project = world.plan(&name, &plan_of(&name, vec![stepped(task)]));
        world
            .run(&["start", &project, "--detach"])
            .exited(REFUSED)
            .err_has(&format!("node 'ship': step 'implement': {rule}"));
        world
            .run(&["plan", "check", &project])
            .exited(HAS_REFUSALS)
            .out_has("step 'implement'")
            .out_has(rule);
    }
    // The human step's prose states no criteria, and the plan loads.
    let project = world.plan("stepsok", &plan_of("stepsok", vec![stepped(FOLLOWED)]));
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
    // `flaky` fails, so a `retry` may supersede it; `build` waits on a person, so a
    // `cancel` parks it and a `requeue` may bring it back.
    world.script("flaky.fail", "1");
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                human("approve", &[]),
                agent("build", &["approve"]),
                agent("flaky", &[]),
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
    edit(json!({"op": "cancel", "id": "build"})).exited(0);
    let committed = world.events_of(name, "edit-committed").len();
    let journal = world.journal(name).len();

    let (no_section, repeated, none_listed) = (BROKEN[0].0, BROKEN[1].0, BROKEN[2].0);
    edit(json!({"op": "add", "node": stating("late", no_section, &[])}))
        .exited(REFUSED)
        .err_has("node 'late': no criteria section");
    edit(json!({"op": "add", "node": stepped(none_listed)}))
        .exited(REFUSED)
        .err_has("node 'ship': step 'implement': no criteria listed");
    edit(json!({"op": "retry", "id": "flaky", "node": stating("flaky-2", repeated, &[])}))
        .exited(REFUSED)
        .err_has("node 'flaky-2': criteria section repeated");
    edit(json!({"op": "requeue", "id": "build", "amend": {"task": none_listed}}))
        .exited(REFUSED)
        .err_has("node 'build': no criteria listed");

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
    for id in ["late", "ship", "flaky-2"] {
        assert!(
            !world
                .journal(name)
                .iter()
                .any(|event| event.to_string().contains(&format!("\"{id}\""))),
            "the refused node '{id}' reached the run's journal"
        );
    }

    // The same edits stating tasks that pass are applied.
    edit(json!({"op": "requeue", "id": "build", "amend": {"task": LAST}})).exited(0);
    edit(json!({"op": "add", "node": stating("late", FOLLOWED, &[])})).exited(0);
    edit(json!({"op": "retry", "id": "flaky", "node": stating("flaky-2", LAST, &[])})).exited(0);
    assert_eq!(world.events_of(name, "edit-committed").len(), committed + 3);
}

/// Rewrite the plan a run was launched with so node `id`'s task is `task` —
/// the record a build from before C6b left for a plan it loaded — and drop the
/// fold checkpoint, which the rewritten journal would no longer corroborate.
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

#[test]
fn adopt_refuses_a_run_whose_undone_node_breaks_c6b_before_writing_anything() {
    let world = World::new("c6b-adopt-refused");
    let name = "older";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![human("approve", &[]), agent("build", &["approve"])],
        ),
    );
    world.run(&["start", &project, "--attach"]).exited(0);
    launched_before_c6b(&world, name, "build", BROKEN[0].0);
    let launch = std::fs::read_to_string(world.run_file(name, "launch.json"))
        .expect("the launch record reads");

    world
        .run(&["adopt", name])
        .exited(REFUSED)
        .err_has("node 'build': no criteria section");
    world
        .run(&["adopt", name, "--detach"])
        .exited(REFUSED)
        .err_has("node 'build': no criteria section");
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
