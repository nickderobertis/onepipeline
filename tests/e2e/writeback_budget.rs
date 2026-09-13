//! The deadline the settlement write-back's `project copy` runs under, driven end
//! to end against the real binary, the real store, and a copy the store double
//! holds.
//!
//! The write-back is what keeps the board in step with a run, and the backstop
//! that kills an unreachable store's command is what stopped it: a fixed minute,
//! which a copy that writes one item per node outgrows. What runs here is the
//! real projection — every read and the copy itself reach the real `onetaskgraph`
//! through the double — with one variable the real binary cannot be asked for:
//! how long the copy takes. `crates/testfakes/src/bin/fake-onetaskgraph.rs` says
//! how the hold is scripted, and why it is the only honest way to make a store
//! slow rather than wrong.
//!
//! Both journeys about the deadline wait past the sixty-second floor **by
//! construction**: a copy that outlasts a minute cannot be observed in less than
//! one. `tests/e2e/store.rs` is where the other minute-long write-back journeys
//! live, and these take their rendezvous settings from it.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The store double here delegates every answer to the real
// `onetaskgraph` and adds only a hold in front of one verb, so what lands on the board
// is the real store's own. `harness.rs` carries the same suppression and the full
// rationale.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::harness::{
    agent, double, onetaskgraph_binary, plan_of, repo_file, Rendezvous, World, REFUSED,
    RENDEZVOUS_SECONDS_ENV, STORE_BINARY_ENV,
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

/// The verb the double holds, spelled as the double names a verb: the command
/// line's words joined by `-`.
const COPY: &str = "onetaskgraph.project-copy";

/// A detached run of `items` nodes projecting through the store double in front
/// of the real store, whose `project copy` meets this test before it answers.
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
    world.script(
        "onetaskgraph.delegate",
        &onetaskgraph_binary().to_string_lossy(),
    );
    let meeting = world.rendezvous(COPY);
    let world = world
        .with_env(
            STORE_BINARY_ENV,
            &double("fake-onetaskgraph").to_string_lossy(),
        )
        // The held node has to outlast the copy this journey is measuring, and what
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

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the two journeys
// below wait past the sixty-second floor by construction — a copy that outlasts a minute
// cannot be observed in less than one — and the edge they need is the crate under test:
// they drive the compiled `onepipeline` binary against its own write-back worker, exactly
// as the minute-long schedule journeys in `store.rs` do and for the reason given there.
/// The journey the setting exists for: a copy that outlasts the sixty-second
/// floor still lands, because the plan carries enough items to lift its deadline
/// above the floor.
///
/// Under the fixed minute this run's settlement never reached the board. Under
/// the shipped budget the same copy is allowed `items × 10` seconds, so a hold
/// past the floor ends with the real store holding what the run recorded, and
/// the driver having reported nothing — which is what an operator reads.
#[test]
fn a_copy_held_past_the_floor_still_lands_when_the_item_count_lifts_its_deadline() {
    let floor = number("floor_seconds");
    let per_item = number("default_seconds");
    // Enough items that the shipped budget lifts the deadline well past the floor,
    // and past the hold below with room for the real copy to land after it.
    let items = usize::try_from(2 * floor / per_item).expect("a count");
    assert!(
        per_item * items as u64 >= floor + 30,
        "{items} items × {per_item} seconds does not lift the deadline past the hold"
    );
    let run = "budgetlifts";
    let (world, meeting, project) =
        a_run_whose_copy_is_held("writeback-budget-lifts", run, items, &[]);

    // The first copy is inside its hold: nothing has reached the board, and nothing
    // has been reported, because the copy has not failed — it is still running.
    let held = meeting.arrived();
    let started = Instant::now();
    assert_ne!(
        board_status(&world, &project, "work").as_deref(),
        Some("in-progress"),
        "the board moved before the held copy could have written it"
    );

    // Held past the floor. Slept rather than polled: there is nothing to observe
    // until the hold ends, and the whole point is that the copy is still alive at
    // the end of it. The five seconds past the floor are for a worker whose clock
    // started before this test's did.
    let past_the_floor = Duration::from_secs(floor + 5);
    std::thread::sleep(past_the_floor.saturating_sub(started.elapsed()));
    assert!(
        !a_projection_failed(&world, run),
        "the copy was killed inside {} seconds under a budget of {items} × {per_item}:\n{}",
        floor + 5,
        std::fs::read_to_string(world.run_file(run, "driver.log")).unwrap_or_default()
    );

    // Let the held copy answer. Later copies are not held: the deadline is the
    // subject, and one copy past it is the evidence.
    std::fs::remove_file(world.fakes.join(format!("{COPY}.rendezvous")))
        .expect("the hold is withdrawn for later copies");
    held.release();
    drop(meeting);
    world.until_store("the held copy to reach the board", |world| {
        board_status(world, &project, "work").is_some_and(|word| word == "in-progress")
    });
    assert!(
        started.elapsed() > Duration::from_secs(floor),
        "the copy landed inside the floor, so this journey held nothing past it"
    );
    assert!(
        !a_projection_failed(&world, run),
        "a copy that landed was reported as failed:\n{}",
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

/// A copy held past what a deliberately tiny budget allows is killed, and the
/// refusal names what the deadline was computed from — including that the
/// floor governed, since one item at one second is less than a minute — and the
/// driver that is killed by it is one an **adopt** started, under the budget
/// the launch chose rather than the one the adopting shell's environment names.
///
/// The line is read where it lands: on the driver's stderr, and on the planner
/// surface built from it, which are all anybody gets about a projection that
/// failed. Without the arithmetic an operator cannot tell a store that is down
/// from a plan that has outgrown its budget. The adoption is what makes the
/// retained budget more than a field: a fresh driver from a shell naming a
/// thousand seconds per item would, re-reading its environment, allow this copy
/// a thousand seconds and never kill it inside the wait below.
#[test]
fn a_copy_held_past_a_tiny_budget_is_killed_and_the_refusal_names_the_arithmetic() {
    let floor = number("floor_seconds");
    let items = 1;
    let run = "budgetfloor";
    let flag = spelling("flag");
    let (world, meeting, project) =
        a_run_whose_copy_is_held("writeback-budget-floor", run, items, &[flag.as_str(), "1"]);

    // The launching driver's first copy is let go at once, so it lands and the run
    // is quiet: nothing has failed yet, and the driver is one an adoption may end.
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

    // The adopted driver's copy is inside its hold, and this test never lets it go:
    // what ends it is the deadline.
    let _held = meeting.arrived();
    let expected = format!(
        "project-copy exceeded {floor} seconds (the {floor} second floor; {items} item × 1 \
         second per item is less)"
    );
    world.until_run_file_holds(run, "driver.log", &expected);
    let log = std::fs::read_to_string(world.run_file(run, "driver.log")).expect("the log");
    assert!(
        log.contains(&format!("write-back failed for '{project}': {expected}")),
        "the refusal is not the line an operator reads:\n{log}"
    );

    // The same sentence reaches the planner, on the surface that names the items
    // the copy was carrying — the list the deadline was multiplied by.
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
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

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

    // Each rung, from the bottom up: the config alone, then the variable over it,
    // then the flag over both. A fresh run each time, because the budget is
    // resolved once, at the launch.
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

    // Naming none, on no rung at all, records the shipped default rather than
    // nothing: the record says what the run runs under.
    let none = "budget-none";
    let path = world.plan(none, &plan_of(none, vec![agent("only", &[])]));
    let mut command = world.cmd(&["start", &path, "--attach"]);
    command.env_remove(spelling("environment"));
    world.run_on(command, "start").exited(0).settled();
    assert_eq!(
        recorded_budget(&world, none),
        Value::from(number("default_seconds"))
    );

    // A fresh driver takes up what its launch chose. Adopted from a shell whose
    // environment names another figure, which is exactly the drift this guards
    // against.
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

    // A record written before the field existed — by an older build — still
    // reads, and the adopted run carries no invented figure: the field reads as
    // `0`, which the worker resolves to the shipped default rather than to a
    // budget of zero.
    // llmlint: ignore-block[tests_mirror_real_usage] a launch record written by **another
    // build** is the input here, and there is no invocation a user can type that produces
    // one: this build writes the field on every record. What is written is the one file a
    // run *is* to an adoption — its launch record, as the build before this field left it
    // — and everything then asserted is the real compiled binary adopting it, which is
    // exactly how it meets a run root a preceding build wrote. `tests/e2e/compatibility.rs`
    // makes the same argument for a journal and a record an older build wrote.
    let launch = world.run_file(none, "launch.json");
    let mut older = world.run_json(none, "launch.json");
    older
        .as_object_mut()
        .expect("a launch record")
        .remove("writeback_item_budget")
        .expect("the record this build writes carries the field");
    std::fs::write(&launch, older.to_string()).expect("the older record is written");
    // llmlint: ignore-end[tests_mirror_real_usage]
    world.run(&["adopt", none]).exited(0);
    assert_eq!(recorded_budget(&world, none), Value::from(0));
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

    // The flag, with the config naming a usable figure beneath it.
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

    // The variable, with the same config beneath it — and a variable that holds
    // text rather than a number, which is not a launch naming none either.
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

    // The config key: zero, the key present and blank, and a value that is not a
    // whole number of seconds at all, each refused by the key's name where the
    // document is read.
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
