//! The mechanically-checkable half of a node's review bar, read where the work
//! is.
//!
//! A criterion naming a literal value in a named file is checkable by reading
//! that file, and one shipped negated in the code it named passed a worker, a
//! judge, a monitor and a manager because nobody made the check. These journeys
//! drive the compiled binary over a real repository: the branch is git's, the
//! worktree is `onevcs`'s, and what the dispatch wrote there is what the
//! criterion is read against.

// llmlint: ignore-file[e2e_not_mocked] the crate under test is the compiled binary and the
// repository side is the real `onevcs` over real git. `oneagentgraph` is substituted at its
// subprocess boundary so the journey states what the worker wrote rather than paying for a
// model turn — and what it wrote is the whole subject here. `harness.rs` carries the same
// suppression and the full rationale.

use serde_json::{json, Value};

use crate::harness::{plan_of, World};

/// A lifecycle node whose bar names one file and one literal in it.
fn measured(id: &str, file: &str, literal: &str) -> Value {
    json!({
        "id": id,
        "repo": "service",
        "persona": "engineer",
        "title": format!("feat: ship {id}"),
        "task": format!(
            "## What\nShip {id}.\n\n## Why\nUsers need it.\n\n## Acceptance criteria\n\
             - the shared journey row for this source in `{file}` is `{literal}`.\n"
        ),
    })
}

/// Every finding one run recorded, as `(node, message)`.
fn findings(world: &World, run: &str) -> Vec<(String, String)> {
    world
        .events_of(run, "planner-surface-queued")
        .iter()
        .filter(|event| event["payload"]["kind"] == "finding")
        .map(|event| {
            (
                event["labels"]["node"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                event["payload"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
}

/// How each node of one run settled, as `(status, outcome)`.
fn settled(world: &World, run: &str, node: &str) -> (String, String) {
    let result = world.run_json(run, "result.json");
    let found = result["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|entry| entry["id"] == node)
        .unwrap_or_else(|| panic!("{node} is missing from {result}"))
        .clone();
    (
        found["status"].as_str().unwrap_or_default().to_owned(),
        found["outcome"].as_str().unwrap_or_default().to_owned(),
    )
}

/// A criterion the branch contradicts is a **finding**, and nothing else.
///
/// Three nodes in one run, so the claim is a comparison rather than a reading of
/// one node: `ships` writes the value its criterion forbids, `keeps` writes the
/// value its criterion names, and `prose` states a bar that names no file at
/// all. Only the first produces a finding, and it settles exactly as the second
/// does — the mechanical check is evidence for the planner and never a second
/// verdict over the dispatch.
#[test]
fn a_criterion_the_branch_contradicts_is_a_finding_that_does_not_change_the_settlement() {
    let world = World::new("criteria-literal");
    world.repository("local-direct", &[]);
    // What each worker leaves on its branch. `<node>.work` writes `<node>.md`
    // into the dispatch's workspace, which is the session's own worktree.
    world.script("ships.work", "complete_dataset: false\n");
    world.script("keeps.work", "complete_dataset: true\n");

    let mut prose = crate::harness::agent("prose", &[]);
    prose["task"] = json!(
        "## What\nWrite it down.\n\n## Why\nSomebody has to.\n\n## Acceptance criteria\n\
         - the dataset is complete, and a reviewer agrees.\n"
    );

    let name = "measured";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                measured("ships", "ships.md", "complete_dataset: true"),
                measured("keeps", "keeps.md", "complete_dataset: true"),
                prose,
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });

    let found = findings(&world, name);
    assert_eq!(
        found.len(),
        1,
        "exactly the contradicted criterion is a finding, and this run recorded {found:?}"
    );
    let (node, message) = &found[0];
    assert_eq!(node, "ships", "{found:?}");
    // The file, the literal, and the criterion in the author's own words.
    assert!(message.contains("ships.md"), "{message}");
    assert!(message.contains("complete_dataset: true"), "{message}");
    assert!(
        message.contains("the shared journey row for this source"),
        "{message}"
    );

    // And the node settled exactly as the one whose branch agrees with its
    // criterion did: the finding changed nothing about the outcome.
    assert_eq!(
        settled(&world, name, "ships"),
        settled(&world, name, "keeps"),
        "the finding changed how the node settled"
    );
    assert_eq!(settled(&world, name, "ships").0, "done");

    // The finding is non-blocking, so nothing waited on it.
    let blocking: Vec<Value> = world
        .events_of(name, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["blocking"] == json!(true))
        .collect();
    assert!(
        blocking.is_empty(),
        "a criterion finding blocked the run: {blocking:?}"
    );
}
