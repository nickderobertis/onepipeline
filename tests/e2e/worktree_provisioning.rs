//! A session whose repository's `post-checkout` failed, read from what a worker
//! and a manager are shown.
//!
//! Every journey runs the real `onevcs` this binary links against a real origin
//! whose tracked hook git itself runs at session open; only the paid agent is
//! doubled, at the harness boundary.
//!
// llmlint: ignore-file[e2e_not_mocked] `onevcs` and git are real here and the hook is a tracked script they run; the agent double is the suite's standard paid-turn boundary, and what these journeys are about is the repository side and the prompt the worker is given.

use std::path::Path;

use serde_json::{json, Value};

use crate::harness::{agent, git, lifecycle, plan_of, Repository, World, SURFACE_WAITING};

const HEADING: &str = "## Worktree provisioning failed";
const KIND: &str = "provisioning-failed";

/// Commit `hook` as the repository's tracked `post-checkout` and point git at it.
fn tracked_post_checkout(world: &World, repo: &Repository, hook: &str) {
    let hooks = repo.checkout.join(".githooks");
    std::fs::create_dir_all(&hooks).expect("the tracked hooks directory");
    onepipeline_testfakes::executable(&hooks.join("post-checkout"), hook.to_owned());
    git(world, &repo.checkout, &["add", "-A"]);
    git(
        world,
        &repo.checkout,
        &["commit", "-m", "chore: track the provisioning hook"],
    );
    git(world, &repo.checkout, &["push", "origin", "main"]);
    git(
        world,
        &repo.checkout,
        &["config", "core.hooksPath", ".githooks"],
    );
}

/// Start `plan`, a lifecycle `service` beside an agent holding the run open.
fn started(world: &World, run: &str) {
    world.script("holder.wait", "hold");
    world.script("service.work", "the worker wrote this\n");
    let path = world.plan(
        run,
        &plan_of(run, vec![lifecycle("service", &[]), agent("holder", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
}

fn settled(world: &World, run: &str, node: &str) {
    world.until(&format!("{node} to settle"), |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == node)
    });
}

/// The whole task the first dispatch of `node` handed its worker, off the
/// double's own argv: a turn's journal echo is bounded, and truncation is under test.
fn first_turn(world: &World, node: &str) -> String {
    let mine = format!("## What\nShip {node}.");
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .find_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|arg| arg == "--task")?;
            let task = args.get(at + 1)?.as_str()?;
            task.starts_with(&mine).then(|| task.to_owned())
        })
        .unwrap_or_else(|| panic!("no dispatch of {node}: {:?}", world.invocations()))
}

fn results_json(world: &World, run: &str) -> Value {
    let results = world.run(&["results", run, "--json"]);
    results.exited(0);
    serde_json::from_str(&results.stdout).expect("results --json is JSON")
}

fn node_of<'a>(results: &'a Value, id: &str) -> &'a Value {
    results["nodes"]
        .as_array()
        .expect("results lists nodes")
        .iter()
        .find(|node| node["id"] == id)
        .unwrap_or_else(|| panic!("no result for {id}: {results}"))
}

/// Let the holder settle and the run end.
fn stop(world: &World, run: &str) {
    world.release("holder.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
}

/// Everything the journey's verdict half asserts of a dispatch told nothing.
fn nothing_was_reported(world: &World, run: &str) {
    settled(world, run, "service");
    assert!(
        !first_turn(world, "service").contains(HEADING),
        "a passing or absent hook composed a failure section"
    );
    let surfaces: Vec<Value> = world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == KIND)
        .collect();
    assert!(surfaces.is_empty(), "{surfaces:?}");
    assert!(
        node_of(&results_json(world, run), "service")
            .get("provisioning_failures")
            .is_none(),
        "results reported a failure nothing recorded"
    );
    let text = world.run(&["results", run]);
    text.exited(0);
    assert!(
        !text.stdout.contains("provisioning failed"),
        "{}",
        text.stdout
    );
    stop(world, run);
}

#[test]
fn a_failed_hook_reaches_the_workers_first_turn_the_watch_and_the_results() {
    let world = World::new("provisioning-failed");
    let repo = world.repository("local-direct", &[]);
    tracked_post_checkout(
        &world,
        &repo,
        "#!/bin/sh\necho PROVISION-MARKER-LINE\nexit 3\n",
    );
    let run = "provisionfails";
    started(&world, run);

    let watched = world.run(&["watch", run, "--until", "surface", "--timeout", "120"]);
    watched.exited(SURFACE_WAITING);
    assert!(
        watched.stdout.contains(KIND) && watched.stdout.contains("service"),
        "the watch did not return on the surface: {}",
        watched.stdout
    );

    let surface = world.surfaced(run, KIND);
    assert_eq!(surface["labels"]["node"], "service");
    assert_eq!(surface["labels"]["significance"], "attention");
    assert_eq!(surface["payload"]["blocking"], false);
    let message = surface["payload"]["message"].as_str().expect("a message");
    for expected in [
        "node: service",
        "session: ",
        "worktree: ",
        "exited with status 3",
        "PROVISION-MARKER-LINE",
        "rerun: ",
    ] {
        assert!(message.contains(expected), "{expected}: {message}");
    }

    settled(&world, run, "service");
    let prompt = first_turn(&world, "service");
    let section = &prompt[prompt
        .find(HEADING)
        .unwrap_or_else(|| panic!("no failure section: {prompt}"))..];
    assert!(section.contains("PROVISION-MARKER-LINE"), "{section}");
    assert!(section.contains("exited with status 3"), "{section}");
    assert!(section.contains("post-checkout"), "{section}");
    assert!(
        section.contains("toolchain may be missing until that command succeeds"),
        "{section}"
    );

    let results = results_json(&world, run);
    assert_eq!(results["schema_version"], 3);
    let failures = node_of(&results, "service")["provisioning_failures"]
        .as_array()
        .expect("results names the failure under the node");
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0]["hook"], "post-checkout");
    assert_eq!(failures[0]["exit_status"], 3);
    let rerun = failures[0]["rerun"].as_str().expect("a rerun command");
    assert!(section.contains(rerun), "{rerun} not in {section}");
    assert!(message.contains(rerun), "{rerun} not in {message}");
    world
        .run(&["results", run])
        .exited(0)
        .out_has("provisioning failed in session");
    world
        .run(&["status", run])
        .exited(0)
        .out_has("service: provisioning failed in session");
    assert_eq!(
        world
            .events_of(run, "planner-surface-queued")
            .iter()
            .filter(|event| event["payload"]["kind"] == KIND)
            .count(),
        1,
        "one surface per failed session"
    );
    stop(&world, run);
}

#[test]
fn only_the_output_the_event_kept_reaches_the_section_and_the_surface() {
    let world = World::new("provisioning-truncated");
    let repo = world.repository("local-direct", &[]);
    tracked_post_checkout(
        &world,
        &repo,
        "#!/bin/sh\necho FIRST-LINE-OF-MANY\ni=0\nwhile [ \"$i\" -lt 400 ]; do\n  \
         echo filler-filler-filler-filler-filler-$i\n  i=$((i + 1))\ndone\n\
         echo LAST-LINE-OF-MANY\nexit 1\n",
    );
    let run = "provisiontruncated";
    started(&world, run);
    let surface = world.surfaced(run, KIND);
    let message = surface["payload"]["message"].as_str().expect("a message");
    assert!(message.contains("LAST-LINE-OF-MANY"), "{message}");
    assert!(!message.contains("FIRST-LINE-OF-MANY"), "{message}");
    assert!(message.contains("truncated"), "{message}");

    settled(&world, run, "service");
    let prompt = first_turn(&world, "service");
    let section = &prompt[prompt.find(HEADING).expect("a failure section")..];
    assert!(section.contains("LAST-LINE-OF-MANY"), "{section}");
    assert!(!section.contains("FIRST-LINE-OF-MANY"), "{section}");
    assert!(section.contains("It was truncated"), "{section}");
    stop(&world, run);
}

#[test]
fn a_hook_that_passes_is_reported_as_nothing() {
    let world = World::new("provisioning-passes");
    let repo = world.repository("local-direct", &[]);
    tracked_post_checkout(&world, &repo, "#!/bin/sh\necho provisioned\nexit 0\n");
    let run = "provisionpasses";
    started(&world, run);
    nothing_was_reported(&world, run);
}

#[test]
fn a_repository_with_no_hook_is_reported_as_nothing() {
    let world = World::new("provisioning-absent");
    let _repo = world.repository("local-direct", &[]);
    let run = "provisionabsent";
    started(&world, run);
    nothing_was_reported(&world, run);
}

/// The replacement opens its own session, so it reads its own session's events.
#[test]
fn a_retry_onto_a_session_whose_hook_passes_carries_no_failure_section() {
    let world = World::new("provisioning-retry");
    let repo = world.repository("local-direct", &[]);
    let provisioned: &Path = &world.root.join("provisioned");
    tracked_post_checkout(
        &world,
        &repo,
        &format!(
            "#!/bin/sh\n[ -f '{}' ] && exit 0\necho RETRY-MARKER\nexit 4\n",
            provisioned.display()
        ),
    );
    world.script("service.fail", "1");
    let run = "provisionretry";
    started(&world, run);
    settled(&world, run, "service");
    assert!(
        first_turn(&world, "service").contains("RETRY-MARKER"),
        "the failed dispatch was not told"
    );

    std::fs::write(provisioned, "").expect("the hook will now pass");
    world.script("service-2.work", "the retry wrote this\n");
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "retry", "id": "service", "node": {
                "id": "service-2",
                "repo": "service",
                "persona": "engineer",
                "title": "feat: ship service",
                "task": "## What\nShip service-2.\n\n## Why\nUsers need it.\n\n\
                         ## Acceptance criteria\n- service-2 is published.",
            }}]})
            .to_string(),
        )
        .exited(0);
    settled(&world, run, "service-2");
    assert!(
        !first_turn(&world, "service-2").contains(HEADING),
        "the retry read an earlier session's failure"
    );
    assert_eq!(
        world
            .events_of(run, "planner-surface-queued")
            .iter()
            .filter(|event| event["payload"]["kind"] == KIND)
            .count(),
        1,
        "the passing session raised a surface"
    );
    stop(&world, run);
}
