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

use crate::harness::{
    agent, double, onetaskgraph_binary, plan_of, World, REFUSED, RENDEZVOUS_SECONDS_ENV,
    STORE_BINARY_ENV,
};

/// The second source, holding the tickets a plan's tasks deliver.
const TICKETS: &str = "tickets";
/// The store double's rendezvous key for `project copy`.
const COPY: &str = "onetaskgraph.project-copy";

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

/// The same world with every store command going through the double, delegating to the real
/// store, so a journey can hold or refuse one named command.
fn through_the_double(world: World) -> World {
    world.script(
        "onetaskgraph.delegate",
        &onetaskgraph_binary().to_string_lossy(),
    );
    world.with_env(
        STORE_BINARY_ENV,
        &double("fake-onetaskgraph").to_string_lossy(),
    )
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
    let output = world
        .store_cmd(&["task", "show", id, "--json"])
        .output()
        .expect("the real onetaskgraph runs");
    assert!(
        output.status.success(),
        "the ticket {id} could not be read: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answer: Value = serde_json::from_slice(&output.stdout).expect("task show answers JSON");
    answer["items"][0]["item"]["status"]["category"]
        .as_str()
        .unwrap_or_else(|| panic!("the ticket {id} carries no status category: {answer}"))
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
    let world = through_the_double(world);
    let copies = world.rendezvous(COPY);
    let build = world.rendezvous("build");
    world.run(&["start", &project, "--detach"]).exited(0);

    let claim = copies.arrived();
    // The copy that claims the plan is still with the double, so the board is as authored.
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
            "the ticket {id} was not claimed at the first dispatch"
        );
    }

    std::fs::remove_file(world.fakes.join(format!("{COPY}.rendezvous")))
        .expect("the copy rendezvous is taken away");
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
/// ticket returns to `todo` after the run had claimed it.
#[test]
fn a_failed_node_reads_failed_and_its_ticket_returns_to_todo() {
    let world = a_world_with_tickets("delivers-failed");
    let delivered = ticket(&world, "fails", "todo");
    world.script("fails.fail", "1");
    let name = "failed-node";
    let project = world.plan(
        name,
        &plan_of(name, vec![delivering(agent("fails", &[]), &[&delivered])]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to settle", |world| settled(world, name));

    world.until_store("the failure and its release to reach the store", |world| {
        words(world, &project).get("fails").map(String::as_str) == Some("failed")
            && ticket_reads(world, &delivered) == "todo"
    });
    assert!(
        delivered_to(&world, name, &delivered, "queued"),
        "the ticket was never claimed, so reading `todo` proves no release: {:?}",
        records(&world, name)
    );
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
    let world = through_the_double(world);
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
    let copies = world.rendezvous(COPY);
    world.run(&["adopt", name, "--detach"]).exited(0);
    let claim = copies.arrived();
    std::fs::remove_file(world.fakes.join(format!("{COPY}.rendezvous")))
        .expect("the copy rendezvous is taken away");
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
fn a_refused_ticket_write_raises_the_planner_surface_and_settles_the_run_unchanged() {
    let world = a_world_with_tickets("delivers-refused-ticket");
    let delivered = ticket(&world, "work", "todo");
    // Read-only on every platform: the store replaces a ticket document in place, so a file it
    // may not write is a ticket it cannot move, with the directory around it left writable.
    let file = tickets_root(&world)
        .join("tasks")
        .join("board")
        .join("work.md");
    let writable = std::fs::metadata(&file)
        .expect("the ticket is on disk")
        .permissions();
    let mut read_only = writable.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&file, read_only).expect("the ticket is made unwritable");
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
        // The store's own words, which name the write it could not make on every platform;
        // what the operating system appends after them is worded differently on each.
        message.contains(&delivered) && message.contains("cannot write"),
        "the surface does not name the ticket and what the store said of it: {message}"
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

    std::fs::set_permissions(&file, writable).expect("the ticket is writable again");
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
    let world = through_the_double(world);
    refuse_every_copy(&world);
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
/// second deliverer: the target leaves the graph.
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
            "id": "held-again", "persona": "engineer", "task": "## What\nAgain."
        }), &[&held])}),
    )
    .exited(0);

    world.until_store("every edit's delivers to reach the board", |world| {
        let board = tasks(world, &project);
        let delivers = |node: &str| board.get(node).map(|task| task["item"]["delivers"].clone());
        delivers("extra") == Some(json!([added]))
            && delivers("waiting") == Some(json!([parked]))
            && delivers("held-again") == Some(json!([held]))
            && ticket_reads(world, &added) == "queued"
    });
    world.run(&["stop", name]).exited(0);
}

/// Have the store double refuse every `project copy` it is handed from now on, the way the store
/// refuses a source it cannot write, while every other command still reaches the real store.
fn refuse_every_copy(world: &World) {
    world.script(
        &format!("{COPY}.refuse.stdout"),
        r#"{"failure":{"class":"refused","kind":"refused","source":"plans","message":"source plans refused the request","retry_after_seconds":null}}"#,
    );
    world.script(&format!("{COPY}.refuse.exit"), "1");
    world.script(
        &format!("{COPY}.refuse"),
        "onetaskgraph: source plans refused the request",
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
    let world = through_the_double(world);
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until_store("the run's claim to reach the store", |world| {
        words(world, &project).get("first").map(String::as_str) == Some("in progress")
            && ticket_reads(world, &delivered) == "queued"
    });

    refuse_every_copy(&world);
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

/// Against a store older than the first release carrying `queued` and `delivers`, unstarted
/// nodes are written `todo`, no task carries `delivers`, no ticket moves, and the run says why
/// exactly once however many projections it makes.
#[test]
fn an_older_store_writes_todo_carries_no_delivers_and_says_why_once() {
    let world = a_world_with_tickets("delivers-older-store");
    let delivered = ticket(&world, "later", "todo");
    world.script("first.wait", "hold");
    let name = "older-store";
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
    let world = through_the_double(world);
    world.script("onetaskgraph.version", "onetaskgraph 0.2.31\n");
    world.run(&["start", &project, "--detach"]).exited(0);

    world.until_store("the running node to reach the older store", |world| {
        let board = words(world, &project);
        board.get("first").map(String::as_str) == Some("in progress")
            && board.get("later").map(String::as_str) == Some("todo")
    });
    // A second projection of the same run, so "once" is asked of more than one.
    let before = records(&world, name).len();
    edit(
        &world,
        name,
        json!({"op": "note", "id": "later", "addressee": "worker", "text": "a change",
               "deliver": "next"}),
    )
    .exited(0);
    world.until("the note to be projected", |world| {
        records(world, name).len() > before
    });

    let later = &tasks(&world, &project)["later"];
    assert!(
        later["item"]["delivers"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "a store without `delivers` was handed one: {later}"
    );
    assert_eq!(ticket_reads(&world, &delivered), "todo");
    let log = std::fs::read_to_string(world.run_file(name, "driver.log")).unwrap_or_default();
    assert_eq!(
        log.matches("will not move the tickets this plan's tasks deliver")
            .count(),
        1,
        "the run did not say exactly once why its tickets are not moved:\n{log}"
    );
    assert!(
        log.contains(onepipeline::cli::WRITEBACK_DELIVERS_FROM),
        "{log}"
    );
    world.run(&["stop", name]).exited(0);
}
