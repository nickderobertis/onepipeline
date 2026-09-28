//! A launched run claims its whole plan on the store before it starts any of it, releases what
//! it never started at closeout, and keeps each task's `delivers` intact on the board — so the
//! store moves the tickets those tasks deliver.
//!
//! Every journey here drives the compiled binary over the real `onetaskgraph`, with the plan's
//! own source and a second `local-md` source holding the tickets. Where a journey needs to hold
//! a store command still or to have one refuse, it goes through the store double, which
//! delegates every call it does not hold or refuse to that same real store.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled binary.
// Every plan and ticket below is read and written by the real `onetaskgraph` against real
// folders of Markdown; the store double in front of it only holds or refuses a named command
// and delegates every other call to that real store. `harness.rs` carries the full rationale.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::harness::{agent, double, plan_of, World, REFUSED, RENDEZVOUS_SECONDS_ENV};

/// The second source, holding the tickets a plan's tasks deliver.
const TICKETS: &str = "tickets";
/// The call the write-back changes an existing task with, which is where the scripted source
/// holds a projection — at its first update, once per attempt — or refuses one.
const UPDATE: &str = "update_task";

/// A world whose store configures a second `local-md` source of tickets beside the plan's.
fn a_world_with_tickets(name: &str) -> World {
    let world = World::new(name);
    let root = tickets_root(&world);
    world
        .with_env(
            &format!("ONETASKGRAPH_SOURCES__{}__PLUGIN", TICKETS.to_uppercase()),
            "local-md",
        )
        .with_env(
            &format!(
                "ONETASKGRAPH_SOURCES__{}__CONFIG__ROOT",
                TICKETS.to_uppercase()
            ),
            &root.to_string_lossy(),
        )
        // A held node outlasts everything a journey does around it, an adoption included.
        .with_env(RENDEZVOUS_SECONDS_ENV, "600")
}

/// The same world, its tickets source reached through the scripted source under the key
/// `tickets` — the same folder, through the real `local-md` plugin — so a journey can refuse
/// the store's write of one ticket.
fn a_world_with_scripted_tickets(name: &str) -> World {
    let world = World::new(name);
    let prefix = format!("ONETASKGRAPH_SOURCES__{}", TICKETS.to_uppercase());
    let root = tickets_root(&world).to_string_lossy().into_owned();
    let script = world.fakes.to_string_lossy().into_owned();
    world
        .with_env(&format!("{prefix}__PLUGIN"), "subprocess")
        .with_env(
            &format!("{prefix}__CONFIG__COMMAND"),
            &double("scripted-source").to_string_lossy(),
        )
        .with_env(&format!("{prefix}__CONFIG__SETTINGS__ROOT"), &root)
        .with_env(&format!("{prefix}__CONFIG__SETTINGS__SCRIPT"), &script)
        .with_env(&format!("{prefix}__CONFIG__SETTINGS__KEY"), TICKETS)
        .with_env(RENDEZVOUS_SECONDS_ENV, "600")
}

fn tickets_root(world: &World) -> PathBuf {
    world.root.join("ticket-store")
}

/// Author one ticket in the tickets source, at `status`, and answer its qualified id.
fn ticket(world: &World, name: &str, status: &str) -> String {
    let root = tickets_root(world);
    let project = root.join("projects").join("board.md");
    std::fs::create_dir_all(project.parent().expect("a directory")).expect("a projects directory");
    if !project.is_file() {
        std::fs::write(&project, "---\ntitle: \"Tickets\"\n---\n").expect("the ticket board");
    }
    let task = root.join("tasks").join("board").join(format!("{name}.md"));
    std::fs::create_dir_all(task.parent().expect("a directory")).expect("a tasks directory");
    std::fs::write(
        &task,
        format!("---\ntitle: \"Ticket {name}\"\nproject: board\nstatus: {status}\n---\n"),
    )
    .expect("the ticket is written");
    format!("{TICKETS}:board/{name}")
}

/// A plan node whose task delivers `tickets`.
fn delivering(mut node: Value, tickets: &[&str]) -> Value {
    node["delivers"] = json!(tickets);
    node
}

/// The ticket's status category, read through the real store as any other reader reads it.
fn ticket_reads(world: &World, id: &str) -> String {
    let ticket = crate::harness::global(id);
    let answer = world.store_call(|engine| async move { engine.task(&ticket).await });
    let answer =
        answer.unwrap_or_else(|error| panic!("the ticket {id} could not be read: {error}"));
    let task = answer
        .items
        .first()
        .unwrap_or_else(|| panic!("the store holds no ticket {id}: {:?}", answer.errors));
    serde_json::to_value(task.item.status.category)
        .expect("a category renders")
        .as_str()
        .expect("a category is a word")
        .to_owned()
}

/// Each plan task, by the node id it carries.
fn tasks(world: &World, project: &str) -> BTreeMap<String, Value> {
    world
        .store_tasks(project)
        .into_iter()
        .filter_map(|task| {
            Some((
                task["item"]["metadata"]["onepipeline.id"]
                    .as_str()?
                    .to_owned(),
                task,
            ))
        })
        .collect()
}

/// The status word each plan task reads under, by node id.
fn words(world: &World, project: &str) -> BTreeMap<String, String> {
    tasks(world, project)
        .into_iter()
        .filter_map(|(node, task)| {
            Some((node, task["item"]["status"]["name"].as_str()?.to_owned()))
        })
        .collect()
}

/// Every projection attempt the run recorded, in order.
fn records(world: &World, run: &str) -> Vec<Value> {
    let path = world.run_file(run, onepipeline::cli::WRITEBACK_PROJECTIONS_FILE);
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("a record line is JSON"))
        .collect()
}

/// Whether any attempt's `delivered` entries say the store wrote `ticket` to `category`.
fn delivered_to(world: &World, run: &str, ticket: &str, category: &str) -> bool {
    records(world, run).iter().any(|record| {
        record["delivered"].as_array().is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["ticket"] == ticket && entry["to"] == category)
        })
    })
}

fn dispatches(world: &World, run: &str) -> usize {
    world.events_of(run, "node-dispatched").len()
}

fn settled(world: &World, run: &str) -> bool {
    world.run_file(run, "result.json").is_file()
}

fn edit(world: &World, run: &str, command: Value) -> crate::harness::Run {
    world.run_with_stdin(
        &["reply", run],
        &json!({"version": 2, "commands": [command]}).to_string(),
    )
}

/// The run's planner surfaces saying its projection did not land.
fn unprojected_surfaces(world: &World, run: &str) -> Vec<String> {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter_map(|event| event["payload"]["message"].as_str().map(str::to_owned))
        .filter(|message| message.contains("did not take this run's projection"))
        .collect()
}

/// The claim reaches the store before the work it claims starts. The launch's first copy is
/// held at the store double, and while it is held no node is dispatched. Once it lands, the
/// build node's dispatch and the next copy are both held, so the board is frozen at the moment
/// of the first dispatch: every task reads `queued`, and so do the two tickets that read `todo`
/// before the launch.
#[test]
fn every_task_and_every_delivered_ticket_reads_queued_at_the_first_dispatch() {
    let world = a_world_with_tickets("delivers-first-dispatch");
    let first = ticket(&world, "first", "todo");
    let later = ticket(&world, "later", "todo");
    let name = "first-dispatch";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                delivering(agent("build", &[]), &[&first]),
                delivering(agent("ship", &["build"]), &[&later]),
                agent("announce", &["ship"]),
            ],
        ),
    );
    let world = world.through_scripted_source();
    let copies = world.store_holds(UPDATE);
    let build = world.rendezvous("build");
    world.run(&["start", &project, "--detach"]).exited(0);

    let claim = copies.arrived();
    // The copy that claims the plan is still held at the scripted source, so the board is as
    // authored.
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        dispatches(&world, name),
        0,
        "a node was dispatched while the projection claiming the plan had not landed"
    );
    assert_eq!(ticket_reads(&world, &later), "todo");
    claim.release();

    let dispatched = build.arrived();
    let next_copy = copies.arrived();
    // Frozen: the build worker is live and the projection of it running has not reached the
    // store. This is the board that worker reads.
    let board = words(&world, &project);
    for node in ["build", "ship", "announce"] {
        assert_eq!(
            board.get(node).map(String::as_str),
            Some("queued"),
            "{node} was not claimed at the first dispatch: {board:?}"
        );
    }
    for id in [&first, &later] {
        assert_eq!(
            ticket_reads(&world, id),
            "queued",
            "the ticket {id} was not claimed at the first dispatch; the projections recorded: \
             {:?}",
            records(&world, name)
        );
    }

    world.store_stops_holding(UPDATE);
    next_copy.release();
    dispatched.release();
    world.until("the run to settle", |world| settled(world, name));
}

/// A node's own task reads `in progress` while it runs and `done` once it is done, and the store
/// moves its ticket to `in-progress` and then `done`. The task's `delivers` reads back from the
/// board as the plan wrote it, on the store's own field and on no reserved key.
#[test]
fn a_running_then_done_node_moves_its_ticket_to_in_progress_then_done() {
    let world = a_world_with_tickets("delivers-running-done");
    let delivered = ticket(&world, "work", "todo");
    world.script("work.wait", "hold");
    let name = "running-done";
    let project = world.plan(
        name,
        &plan_of(name, vec![delivering(agent("work", &[]), &[&delivered])]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);

    world.until_store(
        "the running node and its ticket to reach the store",
        |world| {
            words(world, &project).get("work").map(String::as_str) == Some("in progress")
                && ticket_reads(world, &delivered) == "in-progress"
        },
    );
    let task = &tasks(&world, &project)["work"];
    assert_eq!(
        task["item"]["delivers"],
        json!([delivered]),
        "the projected task does not deliver what the plan says: {task}"
    );
    assert!(
        task["item"]["metadata"]
            .as_object()
            .expect("a projected task carries metadata")
            .keys()
            .all(|key| !key.contains("delivers")),
        "a reserved key carries the delivered tickets: {task}"
    );

    world.release("work.go");
    world.until("the run to settle", |world| settled(world, name));
    world.until_store(
        "the settled node and its ticket to reach the store",
        |world| {
            words(world, &project).get("work").map(String::as_str) == Some("done")
                && ticket_reads(world, &delivered) == "done"
        },
    );
}

/// A failed node keeps the settlement's own word, which the store reads as a release, so its
/// ticket returns to `todo` after the run had claimed it. The node that failure made unsafe never
/// started: it is written `skipped`, which the store reads as a release too, so the ticket it
/// delivers returns to `todo` from the claim the launch put on it.
#[test]
fn a_failed_node_reads_failed_and_its_ticket_returns_to_todo() {
    let world = a_world_with_tickets("delivers-failed");
    let delivered = ticket(&world, "fails", "todo");
    let behind = ticket(&world, "behind", "todo");
    world.script("fails.fail", "1");
    let name = "failed-node";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                delivering(agent("fails", &[]), &[&delivered]),
                delivering(agent("skipped", &["fails"]), &[&behind]),
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| settled(world, name));

    world.until_store(
        "the failure, the skip and both releases to reach the store",
        |world| {
            let board = words(world, &project);
            board.get("fails").map(String::as_str) == Some("failed")
                && board.get("skipped").map(String::as_str) == Some("skipped")
                && ticket_reads(world, &delivered) == "todo"
                && ticket_reads(world, &behind) == "todo"
        },
    );
    for claimed in [&delivered, &behind] {
        assert!(
            delivered_to(&world, name, claimed, "queued"),
            "the ticket {claimed} was never claimed, so reading `todo` proves no release: {:?}",
            records(&world, name)
        );
    }
}

/// A stop releases what the run claimed and never started: those tasks and their tickets read
/// `todo` as soon as the stop has answered. An adoption claims them again before its first
/// dispatch — its first copy is held, and nothing is dispatched while it is.
#[test]
fn a_stopped_run_releases_its_unstarted_tickets_and_an_adoption_claims_them_again() {
    let world = a_world_with_tickets("delivers-stopped");
    let delivered = ticket(&world, "later", "todo");
    world.script("first.wait", "hold");
    let name = "stopped-run";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                agent("first", &[]),
                delivering(agent("later", &["first"]), &[&delivered]),
            ],
        ),
    );
    let world = world.through_scripted_source();
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until_store("the run's claim to reach the store", |world| {
        let board = words(world, &project);
        board.get("first").map(String::as_str) == Some("in progress")
            && board.get("later").map(String::as_str) == Some("queued")
            && ticket_reads(world, &delivered) == "queued"
    });

    world.run(&["stop", name]).exited(0);
    assert_eq!(
        words(&world, &project).get("later").map(String::as_str),
        Some("todo"),
        "a node the stopped run never started is still claimed"
    );
    assert_eq!(
        ticket_reads(&world, &delivered),
        "todo",
        "the ticket of a node the stopped run never started is still claimed"
    );

    let dispatched_before = dispatches(&world, name);
    let copies = world.store_holds(UPDATE);
    world.run(&["adopt", name, "--detach"]).exited(0);
    let claim = copies.arrived();
    world.store_stops_holding(UPDATE);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        dispatches(&world, name),
        dispatched_before,
        "the adopted driver dispatched before its claim reached the store"
    );
    claim.release();
    world.until_store("the adopted driver's claim to reach the store", |world| {
        words(world, &project).get("later").map(String::as_str) == Some("queued")
            && ticket_reads(world, &delivered) == "queued"
    });
    world.run(&["stop", name]).exited(0);
}

/// A ticket the store cannot write is a projection that did not land: the planner hears of it,
/// naming the ticket and what the store said, and the run settles exactly as it would have.
#[test]
fn a_ticket_the_store_cannot_write_raises_the_planner_surface_and_settles_the_run_unchanged() {
    let world = a_world_with_scripted_tickets("delivers-refused-ticket");
    let delivered = ticket(&world, "work", "todo");
    // The tickets source cannot write this ticket's status, the way a hosted source that could
    // not be reached cannot: its own error, at the source's own boundary.
    copies_fail_tickets(&world, &[(&delivered, "unavailable")]);
    world.script("work.wait", "hold");
    let name = "refused-ticket";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                delivering(agent("work", &[]), &[&delivered]),
                agent("after", &["work"]),
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);

    world.until("the planner to hear the ticket was not moved", |world| {
        !unprojected_surfaces(world, name).is_empty()
    });
    let message = unprojected_surfaces(&world, name).remove(0);
    assert!(
        message.contains(&delivered) && message.contains("the store answered unavailable"),
        "the surface does not name the ticket and what the store said of it: {message}"
    );
    // Classed off the ticket's own failure in the copy report: a source the store could not
    // write is one a wait can change.
    assert!(
        message
            .lines()
            .any(|line| line == "class: transient, kind: unavailable"),
        "the surface does not carry the class and kind of the ticket's failure: {message}"
    );
    let partial = records(&world, name)
        .into_iter()
        .find(|record| record["outcome"] == "failed")
        .expect("the partial copy is recorded as an attempt that failed");
    assert_eq!(
        partial["delivered"][0]["ticket"],
        json!(delivered),
        "{partial}"
    );
    assert_eq!(partial["delivered"][0]["outcome"], "failed", "{partial}");

    world.release("work.go");
    world.until("the run to settle", |world| settled(world, name));
    let result = world.run_json(name, "result.json");
    assert_eq!(result["state"], "complete", "{result}");
    assert!(
        result["nodes"]
            .as_array()
            .expect("result nodes")
            .iter()
            .all(|node| node["status"] == "done"),
        "a ticket the store could not write changed a settlement: {result}"
    );
    assert_eq!(
        dispatches(&world, name),
        2,
        "the refusal changed scheduling"
    );
}

/// A launch whose first projection the store refuses still dispatches, and the planner hears
/// the projection did not land.
#[test]
fn a_failed_first_projection_does_not_hold_back_the_first_dispatch() {
    let world = a_world_with_tickets("delivers-first-refused");
    let delivered = ticket(&world, "work", "todo");
    let name = "first-refused";
    let project = world.plan(
        name,
        &plan_of(name, vec![delivering(agent("work", &[]), &[&delivered])]),
    );
    let world = world.through_scripted_source();
    refuse_every_update(&world);
    world.run(&["start", &project, "--detach"]).exited(0);

    world.until("the run to settle", |world| settled(world, name));
    assert_eq!(
        dispatches(&world, name),
        1,
        "the first dispatch did not happen"
    );
    assert_eq!(
        records(&world, name)[0]["outcome"],
        "failed",
        "the first projection was not the refused one"
    );
    assert!(
        !unprojected_surfaces(&world, name).is_empty(),
        "the planner did not hear the first projection failed"
    );
    assert_eq!(world.run_json(name, "result.json")["state"], "complete");
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] this journey waits past
// the sixty-second floor by construction — a claim held past its deadline cannot be observed
// in less than one — and the edge it needs is the crate under test: the compiled `onepipeline`
// binary against its own write-back worker, exactly as `writeback_budget.rs`'s minute-long
// journeys record.
/// A launch whose first projection the store never answers waits for it only as long as the
/// store call deadline allows, and then dispatches: nothing is dispatched while the held copy
/// is inside its deadline, the ready node is dispatched once it has passed, and the planner hears
/// the copy was cancelled. The deadline is the store's sixty-second floor plus one item's budget,
/// so this journey takes more than a minute by construction.
#[test]
fn a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch() {
    let world = a_world_with_tickets("delivers-first-held");
    let delivered = ticket(&world, "work", "todo");
    let name = "first-held";
    let project = world.plan(
        name,
        &plan_of(name, vec![delivering(agent("work", &[]), &[&delivered])]),
    );
    let world = world.through_scripted_source();
    let copies = world.store_holds(UPDATE);
    world.run(&["start", &project, "--detach"]).exited(0);

    let held = copies.arrived();
    let started = std::time::Instant::now();
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        dispatches(&world, name),
        0,
        "the driver dispatched while its claim was still inside the store's deadline"
    );
    world.until(
        "the first dispatch once the claim's deadline passed",
        |world| dispatches(world, name) == 1,
    );
    let floor = Duration::from_secs(onepipeline::cli::WRITEBACK_COMMAND_FLOOR_SECONDS);
    assert!(
        started.elapsed() + Duration::from_secs(5) >= floor,
        "the first dispatch came after {:?}, inside the {floor:?} the claim was allowed",
        started.elapsed()
    );

    // Later copies are not held: the one copy past its deadline is the evidence.
    world.store_stops_holding(UPDATE);
    world.until("the planner to hear the claim did not land", |world| {
        !unprojected_surfaces(world, name).is_empty()
    });
    let message = unprojected_surfaces(&world, name).remove(0);
    assert!(
        message.contains("task-update exceeded"),
        "the surface does not say the update outlasted its deadline: {message}"
    );
    drop(held);
    world.until("the run to settle", |world| settled(world, name));
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// One ticket, one node: a plan in which two nodes deliver the same ticket is refused where it
/// is read, naming both nodes and the ticket, and nothing is launched.
#[test]
fn two_nodes_delivering_one_ticket_refuse_the_plan_naming_both_and_the_ticket() {
    let world = a_world_with_tickets("delivers-duplicate");
    let shared = ticket(&world, "shared", "todo");
    let name = "duplicate-ticket";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                delivering(agent("one", &[]), &[&shared]),
                delivering(agent("two", &[]), &[&shared]),
            ],
        ),
    );
    world
        .run(&["start", &project])
        .exited(REFUSED)
        .err_has("'one'")
        .err_has("'two'")
        .err_has(&shared);
    assert!(
        !world.run_file(name, "launch.json").is_file(),
        "a plan refused for a duplicate ticket launched a run"
    );
    assert_eq!(ticket_reads(&world, &shared), "todo");
}

/// `add`, `retry` and `requeue` carry a node's `delivers` onto the board, and an edit
/// introducing a ticket another node delivers, or an entry that is not a qualified task id, is
/// refused naming what is wrong. A retry's replacement delivering its target's ticket is not a
/// second deliverer: the target leaves the graph, and the replacement's `delivers` reaches the
/// lineage's one item — the target's — which names the replacement under `onepipeline.node`.
#[test]
fn edits_carry_delivers_and_refuse_a_duplicate_or_unqualified_ticket() {
    let world = a_world_with_tickets("delivers-edits");
    let held = ticket(&world, "held", "todo");
    let added = ticket(&world, "added", "todo");
    let parked = ticket(&world, "parked", "todo");
    // Both the node and the replacement a retry gives it are held, so every node behind them
    // stays unstarted — and every ticket those nodes deliver stays claimed — for as long as
    // the journey reads the board.
    world.script("held.wait", "hold");
    world.script("held-again.wait", "hold");
    let name = "edited";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                delivering(agent("held", &[]), &[&held]),
                delivering(agent("waiting", &["held"]), &[&parked]),
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the held node to be dispatched", |world| {
        dispatches(world, name) == 1
    });

    edit(
        &world,
        name,
        json!({"op": "add", "node": delivering(agent("copycat", &["held"]), &[&held])}),
    )
    .exited(REFUSED)
    .err_has("'held'")
    .err_has("'copycat'")
    .err_has(&held);
    edit(
        &world,
        name,
        json!({"op": "add", "node": delivering(agent("sloppy", &["held"]), &["Tickets:added"])}),
    )
    .exited(REFUSED)
    .err_has("'sloppy'")
    .err_has("'Tickets:added'")
    .err_has("not a qualified task id");

    edit(
        &world,
        name,
        json!({"op": "add", "node": delivering(agent("extra", &["held"]), &[&added])}),
    )
    .exited(0);
    edit(&world, name, json!({"op": "cancel", "id": "waiting"})).exited(0);
    edit(&world, name, json!({"op": "requeue", "id": "waiting"})).exited(0);
    edit(
        &world,
        name,
        json!({"op": "retry", "id": "held", "node": delivering(json!({
            "id": "held-again", "persona": "engineer", "task": "## What\nAgain.\n\n## Acceptance criteria\n- It is done."
        }), &[&held])}),
    )
    .exited(0);

    world.until_store("every edit's delivers to reach the board", |world| {
        let board = tasks(world, &project);
        let delivers = |node: &str| board.get(node).map(|task| task["item"]["delivers"].clone());
        delivers("extra") == Some(json!([added]))
            && delivers("waiting") == Some(json!([parked]))
            && delivers("held") == Some(json!([held]))
            && board["held"]["item"]["metadata"]["onepipeline.node"] == "held-again"
            && ticket_reads(world, &added) == "queued"
    });
    assert!(
        !tasks(&world, &project).contains_key("held-again"),
        "the retry's replacement was given an item of its own"
    );
    world.run(&["stop", name]).exited(0);
}

/// A node delivering a ticket is retried with a replacement naming no `delivers`: the
/// replacement inherits them, the lineage's one item carries them, and the ticket reads claimed
/// — `queued` once the retry is projected, `in-progress` while the replacement runs, `done`
/// once it is done — and never `todo` across the retry. Both the node and its replacement are
/// held, so each word is read at rest.
#[test]
fn a_retried_deliverer_keeps_its_ticket_claimed_across_the_retry() {
    let world = a_world_with_tickets("delivers-retried");
    let delivered = ticket(&world, "work", "todo");
    world.script("build.wait", "hold");
    world.script("build-2.wait", "hold");
    let name = "retried-deliverer";
    let project = world.plan(
        name,
        &plan_of(name, vec![delivering(agent("build", &[]), &[&delivered])]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until_store("the running deliverer to claim its ticket", |world| {
        words(world, &project).get("build").map(String::as_str) == Some("in progress")
            && ticket_reads(world, &delivered) == "in-progress"
    });
    let item_before = tasks(&world, &project)["build"]["id"].clone();

    edit(
        &world,
        name,
        json!({"op": "retry", "id": "build", "node": {
            "id": "build-2", "persona": "engineer", "task": "## What\nAgain.\n\n## Acceptance criteria\n- It is done."
        }}),
    )
    .exited(0);
    // Every word the ticket reads from here on, so a moment at `todo` is caught rather than
    // overwritten by the next projection.
    let mut seen: Vec<String> = Vec::new();
    world.until_store(
        "the retry to claim the ticket for the replacement",
        |world| {
            let read = ticket_reads(world, &delivered);
            if seen.last() != Some(&read) {
                seen.push(read.clone());
            }
            let board = tasks(world, &project);
            board
                .get("build")
                .is_some_and(|task| task["item"]["metadata"]["onepipeline.node"] == "build-2")
                && matches!(read.as_str(), "queued" | "in-progress")
        },
    );
    let board = tasks(&world, &project);
    assert_eq!(board["build"]["id"], item_before, "{}", board["build"]);
    assert_eq!(
        board["build"]["item"]["delivers"],
        json!([delivered]),
        "the lineage's item lost the delivers the replacement inherited: {}",
        board["build"]
    );
    assert!(
        !board.contains_key("build-2"),
        "the replacement was given an item of its own: {board:?}"
    );

    world.release("build.go");
    world.release("build-2.go");
    world.until_store("the replacement to finish the ticket", |world| {
        let read = ticket_reads(world, &delivered);
        if seen.last() != Some(&read) {
            seen.push(read.clone());
        }
        read == "done"
    });
    world.until("the run to settle", |world| settled(world, name));
    assert!(
        !seen.iter().any(|word| word == "todo"),
        "the ticket returned to todo across the retry: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|word| word == "queued" || word == "in-progress"),
        "the ticket was never read claimed for the replacement: {seen:?}"
    );
}

/// Have the scripted source refuse every targeted update it is handed from now on, the way a
/// store refuses a source it cannot write, while every other call still reaches the real store.
fn refuse_every_update(world: &World) {
    world.store_refuses(
        UPDATE,
        &json!({"kind": "refused", "message": "source plans refused the request"}),
    );
}

/// A stop whose release the store refuses still stops the run and answers as a stop does, and
/// says on its own standard error that what the run claimed and never started was not released.
/// The ticket stays claimed, because nothing reached the store, and the refused attempt is on
/// the run's projection record.
#[test]
fn a_stop_whose_release_the_store_refuses_still_stops_and_says_so() {
    let world = a_world_with_tickets("delivers-stop-release-refused");
    let delivered = ticket(&world, "later", "todo");
    world.script("first.wait", "hold");
    let name = "stop-release-refused";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                agent("first", &[]),
                delivering(agent("later", &["first"]), &[&delivered]),
            ],
        ),
    );
    let world = world.through_scripted_source();
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until_store("the run's claim to reach the store", |world| {
        words(world, &project).get("first").map(String::as_str) == Some("in progress")
            && ticket_reads(world, &delivered) == "queued"
    });

    refuse_every_update(&world);
    let recorded_before = records(&world, name).len();
    world
        .run(&["stop", name])
        .exited(0)
        .out_has("\"stopped\":true")
        .err_has("could not release the nodes this stopped run never started")
        .err_has("source plans refused the request");
    assert_eq!(
        ticket_reads(&world, &delivered),
        "queued",
        "a release the store refused moved the ticket all the same"
    );
    let recorded = records(&world, name);
    assert!(
        recorded.len() > recorded_before
            && recorded
                .last()
                .is_some_and(|record| record["outcome"] == "failed"),
        "the refused release is not on the projection record: {recorded:?}"
    );
}

/// A stop run from a shell whose store cannot be reached — its source rooted at a folder that is
/// not there — still stops the run and answers as a stop does, and says on its own standard error
/// that what the run claimed and never started was not released. The ticket stays claimed,
/// because nothing reached the store.
#[test]
fn a_stop_that_cannot_reach_the_store_still_stops_and_says_what_it_did_not_release() {
    let world = a_world_with_tickets("delivers-stop-no-store");
    let delivered = ticket(&world, "later", "todo");
    world.script("first.wait", "hold");
    let name = "stop-no-store";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                agent("first", &[]),
                delivering(agent("later", &["first"]), &[&delivered]),
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until_store("the run's claim to reach the store", |world| {
        ticket_reads(world, &delivered) == "queued"
    });

    let mut stop = world.cmd(&["stop", name]);
    stop.env(
        crate::harness::store_root_env(),
        world.root.join("no-such-store"),
    );
    world
        .run_on(stop, "stop")
        .exited(0)
        .out_has("\"stopped\":true")
        .err_has("could not release the nodes this stopped run never started")
        .err_has("no-such-store");
    assert_eq!(
        ticket_reads(&world, &delivered),
        "queued",
        "a stop that never reached the store moved the ticket all the same"
    );
}

/// A `delivers` entry a task holds bare names the task's own source, which is the store's rule
/// for a bare id. The plan's task is authored delivering a ticket kept in the plan's own source,
/// named bare. The run claims that ticket, which it could only do had the bare entry been
/// qualified with the task's source before it reached the board, and the board task carries it
/// qualified.
#[test]
fn a_bare_delivers_entry_is_qualified_with_the_tasks_own_source() {
    let world = a_world_with_tickets("delivers-bare-entry");
    world.script("work.wait", "hold");
    let name = "bare-entry";
    let project = world.plan(name, &plan_of(name, vec![agent("work", &[])]));
    let source = project
        .split(':')
        .next()
        .expect("a qualified project")
        .to_owned();
    // A ticket kept in the plan's own source, in a project of its own.
    let store = world.store();
    std::fs::create_dir_all(store.join("tasks").join("local-tickets"))
        .expect("a tasks directory for the local tickets");
    std::fs::write(
        store.join("projects").join("local-tickets.md"),
        "---\ntitle: \"Local tickets\"\n---\n",
    )
    .expect("the local ticket board");
    std::fs::write(
        store.join("tasks").join("local-tickets").join("one.md"),
        "---\ntitle: \"Local ticket\"\nproject: local-tickets\nstatus: todo\n---\n",
    )
    .expect("the local ticket");
    let qualified = format!("{source}:local-tickets/one");
    assert!(
        tasks(&world, &project)["work"]["item"]["delivers"].is_null(),
        "the authored task already delivers something, so growing a bare entry proves nothing"
    );

    // The task as a person authors a bare entry in their own Markdown.
    let task = store
        .join("tasks")
        .join(crate::harness::project_id(name))
        .join("000-work.md");
    let authored = std::fs::read_to_string(&task).expect("the authored task");
    let bare = authored.replacen("---\n", "---\ndelivers:\n- local-tickets/one\n", 1);
    assert_ne!(
        bare, authored,
        "the task has no front matter to carry `delivers`"
    );
    std::fs::write(&task, bare).expect("the task delivers a bare entry");
    world.run(&["start", &project, "--detach"]).exited(0);

    world.until_store(
        "the run to claim the ticket its bare entry names",
        |world| ticket_reads(world, &qualified) == "in-progress",
    );
    assert_eq!(
        tasks(&world, &project)["work"]["item"]["delivers"],
        json!([qualified]),
        "the board task does not carry the bare entry qualified with its own source"
    );
    world.release("work.go");
    world.until("the run to settle", |world| settled(world, name));
}

/// Have the tickets source refuse the store's write of each `(ticket, kind)` given, with the
/// source error of that kind, while every other call — the tickets' reads, and every write of
/// the plan's own tasks — still reaches the real store. The copy lands, and what it could not
/// keep in step is those tickets.
fn copies_fail_tickets(world: &World, failures: &[(&str, &str)]) {
    for (ticket, kind) in failures {
        let native = ticket
            .split_once(':')
            .map(|(_, native)| native)
            .expect("a qualified ticket");
        world.script(
            &format!(
                "{TICKETS}.set_task_status@{}.refuse",
                onepipeline_testfakes::segment(native)
            ),
            &json!({"kind": kind, "message": format!("the store answered {kind} for this ticket")})
                .to_string(),
        );
    }
}

/// A partial copy is classed off its own `delivered` failures, by the rule a partial read's
/// `errors` are: where the store refused every ticket it failed, the planner hears the class
/// `refused` with every kind it named, and that the projection is not attempted again on a timer.
#[test]
fn a_partial_copy_whose_every_failed_ticket_was_refused_is_classed_refused() {
    let world = a_world_with_scripted_tickets("delivers-partial-refused");
    let one = ticket(&world, "one", "todo");
    let two = ticket(&world, "two", "todo");
    let name = "partial-refused";
    let project = world.plan(
        name,
        &plan_of(name, vec![delivering(agent("work", &[]), &[&one, &two])]),
    );
    copies_fail_tickets(&world, &[(&one, "refused"), (&two, "config")]);
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| settled(world, name));

    let message = unprojected_surfaces(&world, name)
        .into_iter()
        .next()
        .expect("the planner heard the partial copy");
    assert!(
        message
            .lines()
            .any(|line| line == "class: refused, kind: refused, config"),
        "the surface does not class a report whose every ticket was refused as refused: {message}"
    );
    assert!(
        message.contains("not attempted again on a timer"),
        "a refused partial copy was left on the retry timer: {message}"
    );
    for ticket in ["tickets:board/one", "tickets:board/two"] {
        assert!(
            message.contains(ticket),
            "the surface does not name {ticket}: {message}"
        );
    }
}

/// A partial copy mixing a refused ticket with one a wait could change is `transient`, exactly as
/// a partial read mixing the two is: the planner hears that class with both kinds, and the
/// projection stays on the retry schedule rather than waiting for the graph to change.
#[test]
fn a_partial_copy_mixing_refused_and_transient_tickets_is_classed_transient() {
    let world = a_world_with_scripted_tickets("delivers-partial-mixed");
    let one = ticket(&world, "one", "todo");
    let two = ticket(&world, "two", "todo");
    let name = "partial-mixed";
    let project = world.plan(
        name,
        &plan_of(name, vec![delivering(agent("work", &[]), &[&one, &two])]),
    );
    copies_fail_tickets(&world, &[(&one, "refused"), (&two, "unavailable")]);
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| settled(world, name));

    let message = unprojected_surfaces(&world, name)
        .into_iter()
        .next()
        .expect("the planner heard the partial copy");
    assert!(
        message
            .lines()
            .any(|line| line == "class: transient, kind: refused, unavailable"),
        "the surface does not class a mixed report as transient: {message}"
    );
    assert!(
        !message.contains("not attempted again on a timer"),
        "a mixed partial copy was taken off the retry timer: {message}"
    );
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] this journey waits out the
// four seconds the store's rate limit names by construction — that no call reaches the store
// inside a wait cannot be observed in less than the wait — and the edge it needs is the crate
// under test: the compiled `onepipeline` binary against its own write-back worker, as this
// module's minute-long first-projection journey records.
/// A copy whose deliverer's ticket the store refused for a rate limit naming a wait lands every
/// item but that deliverer's: the store is handed nothing, the tickets' source included, until
/// the wait has passed, and the attempt after it carries the deliverer again — though nothing
/// about it changed since, its ticket is behind — beside what did change, and none of what
/// already landed. The ticket then moves off `todo`.
#[test]
fn a_ticket_rate_limited_with_a_wait_holds_every_call_and_the_retry_carries_its_deliverer_alone() {
    let wait = Duration::from_secs(4);
    let world = a_world_with_scripted_tickets("delivers-ticket-rate-limited");
    world.script("gate.wait", "hold");
    let delivered = ticket(&world, "work", "todo");
    let name = "ticket-rate-limited";
    let project = world.plan(
        name,
        &plan_of(
            name,
            vec![
                agent("gate", &[]),
                // Both unstarted until `gate` settles, so the claim is all either has to say.
                delivering(agent("work", &["gate"]), &[&delivered]),
                agent("aside", &["gate"]),
            ],
        ),
    );
    let native = delivered
        .split_once(':')
        .map(|(_, native)| native)
        .expect("a qualified ticket");
    let refusal = format!(
        "{TICKETS}.set_task_status@{}.refuse",
        onepipeline_testfakes::segment(native)
    );
    world.script(
        &refusal,
        &json!({"kind": "rate-limited", "retry_after_seconds": wait.as_secs(),
                "message": "API rate limit exceeded"})
        .to_string(),
    );
    // The plans source is scripted too, so every call either source is handed is on record.
    let world = world.through_scripted_source();
    let store_calls = |world: &World| -> Vec<Value> {
        world
            .invocations()
            .into_iter()
            .filter(|call| call["tool"] == TICKETS || call["tool"] == crate::harness::SCRIPTED_KEY)
            .collect()
    };
    world.run(&["start", &project, "--detach"]).exited(0);

    // The claim: both items land, and the ticket it delivers is refused.
    let records = |world: &World| -> Vec<Value> {
        std::fs::read_to_string(world.run_file(name, "writeback-projections.jsonl"))
            .map(|text| {
                text.lines()
                    .map(|line| serde_json::from_str(line).expect("a record line"))
                    .collect()
            })
            .unwrap_or_default()
    };
    world.until("the claim's ticket to be refused", |world| {
        records(world)
            .first()
            .is_some_and(|record| record["outcome"] == "failed")
    });
    let failed_at = std::time::Instant::now();
    let asked = store_calls(&world).len();
    let claim = records(&world)[0].clone();
    assert_eq!(claim["items"], json!(["aside", "gate", "work"]), "{claim}");
    assert_eq!(claim["kind"], "rate-limited", "{claim}");
    world.unscript(&refusal);
    while failed_at.elapsed() + Duration::from_millis(500) < wait {
        assert_eq!(
            store_calls(&world).len(),
            asked,
            "the store was handed a call {:?} into the {wait:?} a ticket asked for: {:?}",
            failed_at.elapsed(),
            &store_calls(&world)[asked..]
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    world.until("the deliverer to be carried again", |world| {
        records(world).len() >= 2
    });
    let retried = records(&world)[1].clone();
    // `gate` was dispatched meanwhile; `work` changed nowhere, and is carried for its ticket;
    // `aside`, whose claim landed, is not.
    assert_eq!(
        retried["items"],
        json!(["gate", "work"]),
        "the retry did not carry the deliverer whose ticket fell behind, or carried what \
         landed: {retried}"
    );
    assert_eq!(retried["outcome"], "projected", "{retried}");
    assert!(
        failed_at.elapsed() >= wait - Duration::from_millis(500),
        "the retry came {:?} after a ticket asked for {wait:?}",
        failed_at.elapsed()
    );
    world.until("the ticket to move off `todo`", |world| {
        ticket_reads(world, &delivered) != "todo"
    });
    world.release("gate.go");
    world.until("the run to settle", |world| settled(world, name));
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
