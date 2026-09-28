//! The deadlines the settlement write-back's writes run under, driven end to end
//! against the compiled binary, the real store it links, and a write the scripted
//! source holds: a targeted update of one item, allowed the floor and one item's
//! budget, and the copy that creates added items, allowed the floor and one
//! item's budget per item it creates.
//!
//! The write-back is what keeps the board in step with a run, and the backstop
//! that cancels an unreachable store's call is what stopped it: a fixed minute,
//! which a copy that writes one item per node outgrows. What runs here is the
//! real projection — every read and the copy itself reach the real `local-md`
//! store through `scripted-source` — with one variable an offline store cannot be
//! asked for: how long the copy takes. `crates/testfakes/src/bin/scripted-source.rs`
//! says how the hold is scripted, and why it is the only honest way to make a
//! store slow rather than wrong.
//!
//! The four journeys about the deadline wait past the sixty-second floor **by
//! construction**: a copy that outlasts a minute cannot be observed in less than
//! one. `tests/e2e/store.rs` is where the other minute-long write-back journeys
//! live, and these take their rendezvous settings from it.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The scripted source here serves every call out of the real `local-md`
// plugin and adds only a hold in front of one call, so what lands on the board is the real
// store's own. `harness.rs` carries the same suppression and the full rationale.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::harness::{
    agent, plan_of, repo_file, Rendezvous, World, REFUSED, RENDEZVOUS_SECONDS_ENV, SCRIPTED_KEY,
};

/// What entry 71 of the divergence record proposes, which is where the three
/// spellings of this launch-level setting, its default and its floor are written
/// down.
///
/// Read rather than restated: the contract is committed as approved and names
/// none of this, so that entry is the only source — and a journey that spelled
/// the flag itself would go on passing after the record and the code disagreed.
fn proposed() -> Value {
    let record = std::fs::read_to_string(repo_file("docs/contract-divergences.md"))
        .expect("the divergence record ships");
    let entry = record
        .split("\n## ")
        .find(|entry| entry.starts_with("71."))
        .expect("the record still carries entry 71");
    let block = entry
        .split("```json")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("entry 71 carries the json block these journeys drive");
    serde_json::from_str::<Value>(block).expect("entry 71's block is JSON")["budget"].clone()
}

/// One spelling out of that block, refused loudly when the entry stops naming
/// it: a journey that fell back to a literal would prove the literal.
fn spelling(named: &str) -> String {
    proposed()[named]
        .as_str()
        .unwrap_or_else(|| panic!("entry 71 no longer names the budget's {named}"))
        .to_string()
}

/// One number out of that block: the default, the floor, or the version the key
/// arrived at.
fn number(named: &str) -> u64 {
    proposed()[named]
        .as_u64()
        .unwrap_or_else(|| panic!("entry 71 no longer states the budget's {named}"))
}

/// The call the scripted source holds a creation copy at: its first task write, once per
/// attempt. No other write the write-back makes writes a whole task.
const COPY: &str = "write_task";

/// The call the scripted source holds a targeted update at: the first, once per attempt.
const UPDATE: &str = "update_task";

/// A detached run of `items` nodes projecting through the scripted source in front
/// of the real store, whose first targeted update meets this test.
///
/// One node is held open and any others depend on it, so the run is live for as
/// long as the journey needs and every snapshot it projects carries all `items`
/// nodes — which is the count the copy's deadline is computed from.
fn a_run_whose_copy_is_held(
    world: &str,
    run: &str,
    items: usize,
    extra: &[&str],
) -> (World, Rendezvous, String) {
    assert!(items >= 1, "a held node");
    let world = World::new(world);
    world.script("work.wait", "hold");
    let mut nodes = vec![agent("work", &[])];
    for behind in 1..items {
        nodes.push(agent(&format!("later{behind}"), &["work"]));
    }
    let project = world.plan(run, &plan_of(run, nodes));
    let meeting = world.store_holds(UPDATE);
    let world = world
        .through_scripted_source()
        // The held node has to outlast the write this journey is measuring, and what
        // it measures is a minute and more: the same setting `store.rs`'s schedule
        // journeys run under, for the same reason.
        .with_env(RENDEZVOUS_SECONDS_ENV, "600");
    let mut args = vec!["start", project.as_str()];
    args.extend_from_slice(extra);
    args.push("--detach");
    world.run(&args).exited(0);
    (world, meeting, project)
}

/// Whether the driver has reported any projection failing at all.
fn a_projection_failed(world: &World, run: &str) -> bool {
    std::fs::read_to_string(world.run_file(run, "driver.log"))
        .is_ok_and(|log| log.contains("write-back failed"))
}

/// The status word the board holds for one node, as the real store answers it.
fn board_status(world: &World, project: &str, node: &str) -> Option<String> {
    world.store_tasks(project).iter().find_map(|task| {
        (task["item"]["metadata"]["onepipeline.id"] == node)
            .then(|| {
                task["item"]["status"]["category"]
                    .as_str()
                    .map(str::to_owned)
            })
            .flatten()
    })
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the four journeys
// below wait past the sixty-second floor by construction — a copy that outlasts a minute
// cannot be observed in less than one — and the edge they need is the crate under test:
// they drive the compiled `onepipeline` binary against its own write-back worker, exactly
// as the minute-long schedule journeys in `store.rs` do and for the reason given there.
/// A targeted update that outlasts the sixty-second floor still lands, because its deadline
/// is the floor plus one item's budget — and the driver whose claim it is dispatches nothing
/// meanwhile, because the wait before its first dispatch is bounded by the budget of every item
/// that claim carries, not by the floor.
///
/// Under the fixed minute this run's claim never reached the board. Under the shipped budget
/// the update is allowed the floor and 12 seconds besides, so a hold past the floor ends with the
/// real store holding what the run recorded, and the driver having reported nothing — which is
/// what an operator reads.
#[test]
fn a_targeted_update_held_past_the_floor_still_lands_inside_its_item_budget() {
    let floor = number("floor_seconds");
    let per_item = number("default_seconds");
    let held_past = 5;
    assert!(
        per_item > held_past + 2,
        "one item's budget of {per_item} seconds leaves no room past a {held_past} second hold"
    );
    // Enough items that the wait before the first dispatch outlasts the hold by far.
    let items = usize::try_from(2 * floor / per_item).expect("a count");
    let run = "budgetlifts";
    let (world, meeting, project) =
        a_run_whose_copy_is_held("writeback-budget-lifts", run, items, &[]);

    // The claim's first update is inside its hold: nothing has been reported, because the
    // update has not failed — it is still running.
    let held = meeting.arrived();
    let started = Instant::now();

    // Held past the floor. Slept rather than polled: there is nothing to observe until the
    // hold ends, and the whole point is that the update is still alive at the end of it.
    let past_the_floor = Duration::from_secs(floor + held_past);
    std::thread::sleep(past_the_floor.saturating_sub(started.elapsed()));
    assert!(
        !a_projection_failed(&world, run),
        "the update was cancelled inside {} seconds under a budget of {per_item}:\n{}",
        floor + held_past,
        std::fs::read_to_string(world.run_file(run, "driver.log")).unwrap_or_default()
    );
    assert_eq!(
        world.events_of(run, "node-dispatched").len(),
        0,
        "the driver dispatched while its claim of {items} items was still inside its deadline"
    );

    // Let the held update answer. Later ones are not held: the deadline is the subject, and one
    // write past the floor is the evidence.
    world.store_stops_holding(UPDATE);
    held.release();
    drop(meeting);
    world.until_store("the held claim to reach the board", |world| {
        board_status(world, &project, "work").is_some_and(|word| word == "in-progress")
    });
    assert!(
        started.elapsed() > Duration::from_secs(floor),
        "the update landed inside the floor, so this journey held nothing past it"
    );
    assert!(
        !a_projection_failed(&world, run),
        "an update that landed was reported as failed:\n{}",
        std::fs::read_to_string(world.run_file(run, "driver.log")).unwrap_or_default()
    );

    // And the run goes on to settle, with the board following it.
    world.release("work.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    world.until_store("the settlement to reach the board", |world| {
        board_status(world, &project, "work").is_some_and(|word| word == "done")
    });
    assert!(!a_projection_failed(&world, run));
}

/// The copy #521 measured: seven items onto the `plans` board at about 10.2 seconds apiece,
/// 71 to 72 seconds in all, against the 70 the larger of the floor and ten seconds per item
/// allowed it. A copy now only creates, so here the same seven items are `add`ed in one reply
/// and the copy creating them is held for eleven seconds per item — slower than the board
/// measured — and lands under the shipped budget, because the deadline is the floor **plus**
/// the budget per item it creates: the driver reports nothing, and the board holds all seven.
#[test]
fn a_seven_item_copy_held_at_eleven_seconds_per_item_lands_inside_its_deadline() {
    let floor = number("floor_seconds");
    let per_item = number("default_seconds");
    let items: u64 = 7;
    let held_for = Duration::from_secs(11 * items);
    assert!(
        floor + per_item * items > held_for.as_secs() + 30,
        "the shipped deadline leaves no room for the real copy after an eleven-second-per-item \
         hold"
    );
    let world = World::new("writeback-budget-seven");
    world.script("work.wait", "hold");
    let run = "budgetseven";
    let project = world.plan(run, &plan_of(run, vec![agent("work", &[])]));
    let world = world
        .through_scripted_source()
        .with_env(RENDEZVOUS_SECONDS_ENV, "600");
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until_store("the running node to reach the board", |world| {
        board_status(world, &project, "work").is_some_and(|word| word == "in-progress")
    });

    // Seven nodes added in one reply, behind the held one: one copy creates all seven, held at
    // its first write for as long as seven eleven-second items take, and then let go.
    let meeting = world.store_holds(COPY);
    let added: Vec<Value> = (1..=items)
        .map(|nth| serde_json::json!({"op": "add", "node": agent(&format!("added{nth}"), &["work"])}))
        .collect();
    world
        .run_with_stdin(
            &["reply", run],
            &serde_json::json!({"version": 2, "commands": added}).to_string(),
        )
        .exited(0);
    let held = meeting.arrived();
    let started = Instant::now();
    std::thread::sleep(held_for);
    assert!(
        !a_projection_failed(&world, run),
        "the seven-item copy was cancelled inside {held_for:?}:\n{}",
        std::fs::read_to_string(world.run_file(run, "driver.log")).unwrap_or_default()
    );
    world.store_stops_holding(COPY);
    held.release();
    drop(meeting);
    world.until_store("the seven created items to reach the board", |world| {
        (1..=items).all(|nth| {
            board_status(world, &project, &format!("added{nth}")).as_deref() == Some("queued")
        })
    });
    assert!(
        started.elapsed() >= held_for,
        "the copy landed before the hold ended, so this journey held nothing"
    );
    assert!(
        !a_projection_failed(&world, run),
        "a seven-item copy held at eleven seconds per item was reported as failed:\n{}",
        std::fs::read_to_string(world.run_file(run, "driver.log")).unwrap_or_default()
    );
    world.release("work.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
}

/// A targeted update held past what a deliberately tiny budget allows is cancelled, and the
/// refusal names what the deadline was computed from — the floor, and the one
/// second one item at one second adds to it — and the
/// driver whose update it cancels is one an **adopt** started, under the budget the
/// launch chose rather than the one the adopting shell's environment names.
///
/// The line is read where it lands: on the driver's stderr, and on the planner
/// surface built from it, which are all anybody gets about a projection that
/// failed. Without the arithmetic an operator cannot tell a store that is down
/// from a plan that has outgrown its budget. The adoption is what makes the
/// retained budget more than a field: a fresh driver from a shell naming a
/// thousand seconds per item would, re-reading its environment, allow this copy
/// a thousand seconds and never cancel it inside the wait below.
#[test]
fn a_copy_held_past_a_tiny_budget_is_cancelled_and_the_refusal_names_the_arithmetic() {
    let floor = number("floor_seconds");
    let items = 1;
    let run = "budgetfloor";
    let flag = spelling("flag");
    let (world, meeting, project) =
        a_run_whose_copy_is_held("writeback-budget-floor", run, items, &[flag.as_str(), "1"]);

    // The launching driver's updates are let go at once, so they land and the run is
    // quiet: nothing has failed yet, and the driver is one an adoption may end. There
    // are two of them — the claim a driver projects before its first dispatch, which
    // writes the held node `queued`, and the projection of that node running.
    meeting.arrived().release();
    meeting.arrived().release();
    world.until_store("the launching driver's copy to reach the board", |world| {
        board_status(world, &project, "work").is_some_and(|word| word == "in-progress")
    });
    assert!(!a_projection_failed(&world, run));

    // Adopted from a shell whose environment names a far larger budget, with the
    // quiet driver ended for it — the same taking-over `driver.rs` drives, once
    // the view an operator reads calls the run parked.
    world.until("the quiet driver to be reported parked", |world| {
        let mut status = world.cmd(&["status", run]);
        status.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
        let out = status.output().expect("the binary runs");
        String::from_utf8_lossy(&out.stdout).contains("PARKED")
    });
    let mut adopt = world.cmd(&["adopt", run, "--detach"]);
    adopt
        .env(spelling("environment"), "1000")
        .env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    world
        .run_on(adopt, "adopt --detach")
        .exited(0)
        .err_has("ending it to adopt the run");
    assert_eq!(
        world.run_json(run, "launch.json")["writeback_item_budget"],
        Value::from(1),
        "the adoption re-resolved the budget"
    );

    // Something for the adopted driver to write: its update is held, and this test does not let
    // it go until the deadline has ended it.
    noted(&world, run, "work", "held past the deadline");
    let held = meeting.arrived();
    let expected = format!(
        "task-update exceeded {} seconds (the {floor} second floor + {items} item × 1 \
         second per item)",
        floor + 1
    );
    world.until_run_file_holds(run, "driver.log", &expected);
    let log = std::fs::read_to_string(world.run_file(run, "driver.log")).expect("the log");
    assert!(
        log.contains(&format!("write-back failed for '{project}': {expected}")),
        "the refusal is not the line an operator reads:\n{log}"
    );

    // Nothing the cancelled update was holding lands after the record says it was refused: the
    // write it was held at is let go, and the board goes on holding what it held — without the
    // change that write carried. An update the deadline had not really ended would write it now.
    let board = world.store_tasks(&project);
    held.release();
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(
        world.store_tasks(&project),
        board,
        "a write from the cancelled update landed after its refusal was recorded"
    );
    assert!(
        board.iter().all(|task| {
            task["item"]["metadata"]["onepipeline.context"] != "held past the deadline"
        }),
        "the cancelled update's write reached the board"
    );

    // The same sentence reaches the planner, on the surface that names the items
    // the attempt was carrying.
    world.until("the failed projection to reach the planner", |world| {
        !world.events_of(run, "planner-surface-queued").is_empty()
    });
    let raised = world.events_of(run, "planner-surface-queued");
    let message = raised
        .iter()
        .find_map(|event| {
            event["payload"]["message"]
                .as_str()
                .filter(|said| said.contains("did not take this run's projection"))
        })
        .unwrap_or_else(|| panic!("no projection surface was raised: {raised:?}"));
    let reason = message
        .lines()
        .find_map(|line| line.strip_prefix("reason: "))
        .unwrap_or_else(|| panic!("the surface names no reason: {message}"));
    assert_eq!(reason, expected, "{message}");
    let named = message
        .lines()
        .find_map(|line| line.strip_prefix("items: "))
        .unwrap_or_else(|| panic!("the surface names no items: {message}"));
    assert_eq!(
        named.split(", ").count(),
        items,
        "the surface names a different number of items than the deadline multiplied by: \
         {message}"
    );
}

/// A run whose launch record an older build wrote — no budget field on it — is
/// adopted, and the driver the adoption starts bounds its update by the shipped
/// default: the refusal its update is cancelled with names that figure, not one the
/// adopting shell's environment offered and not a zero read off the record.
///
/// The record is the one file a run *is* to an adoption, and a build before this
/// field left it without one. Reading the missing field as no budget at all would
/// cancel every copy; reading it as the environment's would let the shell that
/// adopted decide what the launch had. What the worker does with it is only
/// observable on a copy it actually bounds, so this holds one past the floor.
#[test]
fn a_record_an_older_build_wrote_is_adopted_and_its_copy_runs_under_the_shipped_default() {
    let floor = number("floor_seconds");
    let per_item = number("default_seconds");
    let items = 1;
    let run = "budgetolder";
    let (world, meeting, project) =
        a_run_whose_copy_is_held("writeback-budget-older-record", run, items, &[]);

    // The launching driver's updates are let go at once — its claim before the first
    // dispatch, and the projection of the held node running — so the run is quiet and
    // an adoption may end its driver.
    meeting.arrived().release();
    meeting.arrived().release();
    world.until_store("the launching driver's copy to reach the board", |world| {
        board_status(world, &project, "work").is_some_and(|word| word == "in-progress")
    });
    assert!(!a_projection_failed(&world, run));

    // llmlint: ignore-block[tests_mirror_real_usage] a launch record written by **another
    // build** is the input here, and there is no invocation a user can type that produces
    // one: this build writes the field on every record. What is written is the one file a
    // run *is* to an adoption — its launch record, as the build before this field left it
    // — and everything then asserted is the real compiled binary adopting it.
    let launch = world.run_file(run, "launch.json");
    let mut older = world.run_json(run, "launch.json");
    older
        .as_object_mut()
        .expect("a launch record")
        .remove("writeback_item_budget")
        .expect("the record this build writes carries the field");
    std::fs::write(&launch, older.to_string()).expect("the older record is written");
    // llmlint: ignore-end[tests_mirror_real_usage]

    world.until("the quiet driver to be reported parked", |world| {
        let mut status = world.cmd(&["status", run]);
        status.env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
        let out = status.output().expect("the binary runs");
        String::from_utf8_lossy(&out.stdout).contains("PARKED")
    });
    let mut adopt = world.cmd(&["adopt", run, "--detach"]);
    adopt
        .env(spelling("environment"), "1000")
        .env("ONEPIPELINE_PARKED_AFTER_SECONDS", "1");
    world
        .run_on(adopt, "adopt --detach")
        .exited(0)
        .err_has("ending it to adopt the run");
    assert_eq!(
        recorded_budget(&world, run),
        Value::from(0),
        "the adoption invented a budget the launch never recorded"
    );

    // Something for the adopted driver to write; its update is held and never let go: the
    // deadline ends it, and the refusal says which budget that deadline was computed from.
    noted(&world, run, "work", "held past the shipped default");
    let _held = meeting.arrived();
    let expected = format!(
        "task-update exceeded {} seconds (the {floor} second floor + {items} item × \
         {per_item} seconds per item)",
        floor + per_item
    );
    world.until_run_file_holds(run, "driver.log", &expected);
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// A hold the scripted source cannot read is a failure by the script's name, not a
/// store that answered at once.
///
/// The rendezvous file is the journey's own input to the source, and the two
/// journeys above time a store that has not answered yet against it. A source that
/// read an unreadable address as "no hold" would answer immediately and hand those
/// journeys a copy that landed well inside any deadline — proving the deadline
/// nothing. So the source stops, the store reports that as the copy's own failure on
/// the driver's log, and that is what this asserts: the write-back names the script,
/// and the board is not written as though the store had answered.
#[test]
fn a_hold_the_scripted_source_cannot_read_fails_the_copy_by_the_scripts_name() {
    let world = World::new("writeback-budget-unreadable-hold");
    let run = "budgetunreadable";
    let project = world.plan(run, &plan_of(run, vec![agent("work", &[])]));
    // A directory where the address should be: present, and not a file this source
    // can read.
    let hold = format!("{SCRIPTED_KEY}.{UPDATE}.first.rendezvous");
    std::fs::create_dir(world.fakes.join(&hold)).expect("the unreadable hold is in place");
    let world = world.through_scripted_source();
    // What the real store answers for the authored board, read before the run
    // exists. Held to *that* rather than to a status word: the fixture writes no
    // status, and the category a store reads off a task without one is the store
    // release's own — `backlog` at one, `todo` at another — while a copy that
    // landed rewrites the whole item.
    let authored = world.store_tasks(&project);
    world
        .run(&["start", project.as_str(), "--detach"])
        .exited(0);

    world.until_run_file_holds(
        run,
        "driver.log",
        &format!("write-back failed for '{project}'"),
    );
    world.until_run_file_holds(run, "driver.log", &format!("`{hold}` could not be read"));
    // The board still holds what the fixture authored, not what the copy carried.
    assert_eq!(
        world.store_tasks(&project),
        authored,
        "the board moved as though the store had answered"
    );
}

/// Give a run something new to project: a note for one node, which the next copy writes onto
/// that node's item.
fn noted(world: &World, run: &str, node: &str, text: &str) {
    world
        .run_with_stdin(
            &["reply", run],
            &serde_json::json!({
                "version": 2,
                "commands": [{"op": "note", "id": node, "addressee": "worker",
                              "text": text, "deliver": "next"}]
            })
            .to_string(),
        )
        .exited(0);
}

/// The budget the record carries for one run, as the launch record names it.
fn recorded_budget(world: &World, run: &str) -> Value {
    world.run_json(run, "launch.json")["writeback_item_budget"].clone()
}

/// The flag beats the variable, which beats the config key, and naming none
/// takes the shipped default — read off the launch record, which is what an
/// `adopt` replays rather than re-reading an environment that has since moved.
///
/// Resolved **once**, before the run exists: a fresh driver started from another
/// shell — with another `ONEPIPELINE_WRITEBACK_ITEM_BUDGET`, or none — would
/// otherwise bound the run's copies by a figure its launch never chose.
#[test]
fn the_flag_beats_the_variable_which_beats_the_config_and_an_adopt_replays_the_launch() {
    let precedence: Vec<String> = serde_json::from_value(proposed()["precedence"].clone())
        .expect("entry 71 states the precedence it proposes");
    assert_eq!(
        precedence,
        vec!["flag", "environment", "config_key"],
        "entry 71 proposes a different order than this journey drives"
    );
    let world = World::new("writeback-budget-precedence");
    let config = world.root.join("launch.yaml");
    std::fs::write(
        &config,
        format!(
            "schema_version: {}\n{}: 30\n",
            number("config_schema_version"),
            spelling("config_key"),
        ),
    )
    .expect("the launch config is written");

    // A fresh run per rung, because the budget is resolved once, at the launch.
    for (which, expected, extra, environment) in [
        ("by-config", 30, vec![], None),
        ("by-environment", 20, vec![], Some("20")),
        (
            "by-flag",
            40,
            vec![spelling("flag"), "40".to_string()],
            Some("20"),
        ),
    ] {
        let name = format!("budget-{which}");
        let path = world.plan(&name, &plan_of(&name, vec![agent("only", &[])]));
        let mut args = vec![
            "start".to_string(),
            path.clone(),
            "--launch-config".to_string(),
            config.to_string_lossy().into_owned(),
        ];
        args.extend(extra);
        args.push("--attach".to_string());
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut command = world.cmd(&borrowed);
        match environment {
            Some(value) => command.env(spelling("environment"), value),
            None => command.env_remove(spelling("environment")),
        };
        world.run_on(command, "start").exited(0).settled();
        assert_eq!(
            recorded_budget(&world, &name),
            Value::from(expected),
            "a launch {which} recorded another budget"
        );
    }

    let none = "budget-none";
    let path = world.plan(none, &plan_of(none, vec![agent("only", &[])]));
    let mut command = world.cmd(&["start", &path, "--attach"]);
    command.env_remove(spelling("environment"));
    world.run_on(command, "start").exited(0).settled();
    assert_eq!(
        recorded_budget(&world, none),
        Value::from(number("default_seconds"))
    );

    let mut adopt = world.cmd(&["adopt", "budget-by-flag"]);
    adopt.env(spelling("environment"), "99");
    world.run_on(adopt, "adopt").exited(0);
    let record = world.run_json("budget-by-flag", "launch.json");
    assert_eq!(record["adoptions"], Value::from(1), "{record}");
    assert_eq!(
        record["writeback_item_budget"],
        Value::from(40),
        "the adopted run runs under a budget its launch never chose: {record}"
    );
}

/// A budget of zero is refused at whichever spelling carried it, by that
/// spelling's name, and never falls through to the rung below; a value that is
/// not a whole number of seconds is refused the same way.
///
/// Zero is no budget at all — the floor would be the whole deadline for every
/// plan, which is the outgrown minute the setting exists to end — so a launch
/// that named it was not asking for the config's figure or the shipped default
/// underneath, and a refusal that did not say which spelling carried the zero
/// would send an operator to the wrong file.
#[test]
fn a_budget_of_zero_is_refused_by_the_spelling_that_carried_it() {
    let world = World::new("writeback-budget-zero");
    let path = world.plan("budgetzero", &plan_of("budgetzero", vec![agent("a", &[])]));
    let flag = spelling("flag");
    let variable = spelling("environment");
    let key = spelling("config_key");
    let version = number("config_schema_version");

    let config = world.root.join("launch.yaml");
    std::fs::write(&config, format!("schema_version: {version}\n{key}: 30\n"))
        .expect("the config is written");
    world
        .run(&[
            "start",
            &path,
            "--launch-config",
            &config.to_string_lossy(),
            &flag,
            "0",
            "--detach",
        ])
        .exited(REFUSED)
        .err_has(&flag)
        .err_has("zero")
        .err_lacks(&variable);

    for (held, said) in [("0", "zero"), ("ten", "not a whole number")] {
        let mut command = world.cmd(&[
            "start",
            &path,
            "--launch-config",
            &config.to_string_lossy(),
            "--detach",
        ]);
        command.env(&variable, held);
        world
            .run_on(command, "start")
            .exited(REFUSED)
            .err_has(&variable)
            .err_has(said)
            .err_lacks(&flag);
    }

    for (spelled, written, said) in [
        ("zero", format!("{key}: 0"), "zero"),
        ("bare", format!("{key}:"), "names nothing"),
        ("empty", format!("{key}: \"\""), "names nothing"),
        (
            "negative",
            format!("{key}: -5"),
            "not a positive whole number",
        ),
        (
            "fractional",
            format!("{key}: 2.5"),
            "not a positive whole number",
        ),
        ("text", format!("{key}: ten"), "not a positive whole number"),
    ] {
        let refused = world.root.join(format!("{spelled}.yaml"));
        std::fs::write(&refused, format!("schema_version: {version}\n{written}\n"))
            .expect("the config is written");
        let mut command = world.cmd(&[
            "start",
            &path,
            "--launch-config",
            &refused.to_string_lossy(),
            "--detach",
        ]);
        command.env_remove(&variable);
        world
            .run_on(command, "start")
            .exited(REFUSED)
            .err_has(&format!("`{key}`"))
            .err_has(said);
    }
    assert!(
        !world.run_file("budgetzero", "launch.json").is_file(),
        "a refused launch minted a run"
    );
}

/// A variable this build cannot read as text is a rung that is *there* and names
/// something unusable, and the launch is refused by that variable's name.
///
/// Discarded instead, it would read as an unset rung and hand the run whichever
/// budget the config file names — a launch bounded by a figure its operator did
/// not choose, with nothing said about why.
///
/// Unix-only for the provocation, not for the rule: an environment value that is
/// not text is bytes, and only this platform lets a caller hand one over.
#[test]
#[cfg(unix)]
fn a_budget_variable_this_build_cannot_read_refuses_the_launch_by_its_name() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let world = World::new("writeback-budget-not-text");
    let name = "budgetnottext";
    let path = world.plan(name, &plan_of(name, vec![agent("only", &[])]));
    let variable = spelling("environment");
    let not_text = OsString::from_vec(vec![0x31, 0xff, 0x30]);

    let mut refused = world.cmd(&["start", &path, "--detach"]);
    refused.env(&variable, &not_text);
    world
        .run_on(refused, "start")
        .exited(REFUSED)
        .err_has(&variable)
        .err_has("cannot read as text");

    // And a launch whose flag names one never consults the variable at all: it
    // was not going to use it, so an unreadable one is not its problem.
    let mut named = world.cmd(&["start", &path, &spelling("flag"), "12", "--attach"]);
    named.env(&variable, &not_text);
    world.run_on(named, "start").exited(0).settled();
    assert_eq!(recorded_budget(&world, name), Value::from(12));
}

/// A config naming the key at a version that never had it is refused by that
/// key's name, and a config of any earlier version this build reads that omits
/// the key still launches.
#[test]
fn a_config_naming_the_key_at_a_version_that_never_had_it_is_refused_by_that_name() {
    let world = World::new("writeback-budget-config-version");
    let key = spelling("config_key");
    let arrived = number("config_schema_version");
    let path = world.plan(
        "budgetearly",
        &plan_of("budgetearly", vec![agent("a", &[])]),
    );

    let early = world.root.join("early.yaml");
    std::fs::write(
        &early,
        format!("schema_version: {}\n{key}: 30\n", arrived - 1),
    )
    .expect("the config is written");
    world
        .run(&[
            "start",
            &path,
            "--launch-config",
            &early.to_string_lossy(),
            "--detach",
        ])
        .exited(REFUSED)
        .err_has(&format!("`{key}`"))
        .err_has(&format!("schema {arrived} key"));

    // The version before this one is still a whole document: it says nothing
    // about the budget, which is what a launch naming none means, and it launches
    // a run under the shipped default.
    let earlier = world.root.join("earlier.yaml");
    std::fs::write(
        &earlier,
        format!(
            "schema_version: {}\nenvelope_reviewer: ./review\n",
            arrived - 1
        ),
    )
    .expect("the config is written");
    world
        .run(&[
            "start",
            &path,
            "--launch-config",
            &earlier.to_string_lossy(),
            "--attach",
        ])
        .exited(0)
        .settled();
    assert_eq!(
        recorded_budget(&world, "budgetearly"),
        Value::from(number("default_seconds"))
    );
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] this journey waits past
// the sixty-second floor by construction — a store that has not opened within a minute cannot
// be observed in less than one — and the edge it needs is the crate under test: it drives the
// compiled `onepipeline` binary against its own write-back worker, as the four journeys above
// do and for the reason they record.
/// A store slow to start — a source whose handshake has not been answered — is held to the
/// floor every read is held to: an attempt whose store has not opened inside it is refused by
/// the name the floor gives the opening, retried on the schedule, and lands once the store
/// answers. The launch's own plan read is let through; every attempt's handshake after it is
/// held until the journey lets the store answer.
#[test]
fn a_store_that_does_not_open_within_the_floor_is_refused_retried_and_recovers() {
    let floor = number("floor_seconds");
    let world = World::new("writeback-budget-slow-open");
    world.script("work.wait", "hold");
    let run = "slowopen";
    let project = world.plan(run, &plan_of(run, vec![agent("work", &[])]));
    let opening = world.rendezvous(&format!("{SCRIPTED_KEY}.initialize"));
    let world = world
        .through_scripted_source()
        .with_env(RENDEZVOUS_SECONDS_ENV, "600");

    let mut start = world
        .cmd(&["start", &project, "--detach"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the launch starts");
    opening.arrived().release();
    let launched = start.wait().expect("the launch ends");
    assert!(launched.success(), "the launch exited {launched}");

    // The first attempt's store is held, and never let go: the floor is what ends the wait.
    let held = opening.arrived();
    let expected = format!("store-open exceeded {floor} seconds");
    world.until_run_file_holds(run, "driver.log", &expected);
    let log = std::fs::read_to_string(world.run_file(run, "driver.log")).expect("the log");
    assert!(
        log.contains(&format!("write-back failed for '{project}': {expected}"))
            && log.contains("retrying"),
        "a store that did not open was not reported as a failure to retry:\n{log}"
    );
    // Its record names what the attempt set out to carry, though the store never opened.
    let recorded = std::fs::read_to_string(world.run_file(run, "writeback-projections.jsonl"))
        .expect("the failed attempt was recorded");
    let first: Value =
        serde_json::from_str(recorded.lines().next().expect("a line")).expect("a record line");
    assert_eq!(first["outcome"], "failed", "{first}");
    assert_eq!(first["items"], serde_json::json!(["work"]), "{first}");
    assert_eq!(first["calls"], serde_json::json!({}), "{first}");

    // The store answers: nothing is held from here, and the retry lands.
    world.unscript(&format!("{SCRIPTED_KEY}.initialize.rendezvous"));
    held.release();
    drop(opening);
    world.until("the projection to recover", |world| {
        std::fs::read_to_string(world.run_file(run, "driver.log"))
            .is_ok_and(|log| log.contains("onetaskgraph write-back recovered"))
    });
    world.until_store("the running node to reach the board", |world| {
        board_status(world, &project, "work").is_some_and(|word| word == "in-progress")
    });
    world.release("work.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] this journey waits past
// the sixty-second floor twice by construction — a read held past a minute cannot be observed
// in less than one — and the edge it needs is the crate under test: it drives the compiled
// `onepipeline` binary against its own write-back worker, as the journeys above do and for the
// reason they record.
/// The project read an attempt creating an item makes before its copy is held to the floor: a
/// read held past it is cancelled and reported by the name and the seconds of the deadline it
/// outlasted, retried on the schedule, and the projection recovers — the item created — once the
/// store answers.
#[test]
fn a_destination_read_held_past_the_floor_is_cancelled_retried_and_recovers() {
    let floor = number("floor_seconds");
    let world = World::new("writeback-budget-held-read")
        .through_scripted_source()
        .with_env(RENDEZVOUS_SECONDS_ENV, "600");
    world.script("work.wait", "hold");
    let run = "heldread";
    let project = world.plan(
        run,
        &plan_of(run, vec![agent("work", &[]), agent("later", &["work"])]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until_store("the running node to reach the board", |world| {
        board_status(world, &project, "work").is_some_and(|word| word == "in-progress")
    });
    let log = |world: &World| {
        std::fs::read_to_string(world.run_file(run, "driver.log")).unwrap_or_default()
    };

    for (method, read) in [("get_project", "project-show")] {
        // Held from the next attempt on: its first call of the read, which is the read itself —
        // made by the attempt that creates the added node, and by no other.
        let holding = world.store_holds(method);
        let recovered = log(&world).matches("write-back recovered").count();
        world
            .run_with_stdin(
                &["reply", run],
                &serde_json::json!({"version": 2, "commands": [
                    {"op": "add", "node": agent("added", &["work"])}
                ]})
                .to_string(),
            )
            .exited(0);
        let held = holding.arrived();
        let expected = format!("{read} exceeded {floor} seconds");
        world.until_run_file_holds(run, "driver.log", &expected);
        assert!(
            log(&world).lines().any(|line| {
                line.contains(&format!("write-back failed for '{project}': {expected}"))
                    && line.contains("retrying")
            }),
            "a read held past its deadline was not reported as a failure to retry:\n{}",
            log(&world)
        );

        // The store answers again, and the retry lands.
        world.store_stops_holding(method);
        held.release();
        drop(holding);
        world.until(
            &format!("the projection to recover after {read}"),
            |world| log(world).matches("write-back recovered").count() > recovered,
        );
        world.until_store("the added node's item to be created", |world| {
            board_status(world, &project, "added").as_deref() == Some("queued")
        });
    }
    world.release("work.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
