//! The debug build's one time scale, driven through the compiled binary: what it refuses,
//! and that a scale set, unset or `1` is the factor every wait it governs is applied at.
//!
//! `src/timescale.rs` says what the scale multiplies and why a value named through a wait's
//! own variable is applied as named. The families each have their journey where they are
//! already driven — the claim's lease in `continuation`, a source's first wait in `store`,
//! the write-back's arithmetic in `writeback_budget` — and these are the scale's own: its
//! refusal, the shipped value unset and at `1`, and the waits no other journey reaches
//! without waiting out the shipped default.
//!
//! What the release build does with the same variable is `scripts/smoke-published.sh`'s,
//! because a release build is what that script is run over, and this binary is the debug
//! one.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{agent, plan_of, World, REFUSED, STALL_AFTER_ENV, TIME_SCALE_ENV};

/// The write-back's per-item budget a launch that names none ships with, in seconds: the
/// crate's published default.
const SHIPPED_ITEM_BUDGET: u64 = onepipeline::cli::DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS.get();

/// How long a dispatch may record nothing before it is reported quiet, shipped, in seconds:
/// the suite's copy of `engine::DEFAULT_STALL_AFTER_SECONDS`, whose module is private, held
/// exact by `engine::tests::the_suites_copy_of_the_shipped_stall_threshold_is_this_one`.
const SHIPPED_STALL_AFTER: u64 = 2_400;

/// The per-item budget this world's binary takes where a launch names none, as it says when
/// it refuses a budget of zero: the cheapest place it states a wait it applies, because the
/// refusal comes before anything is read, written or waited on. Asked of an adoption of a run
/// directory that holds nothing, so no run is touched either.
fn applied_item_budget(world: &World, scale: Option<&str>) -> String {
    let empty = world.runs.join("nothing-recorded");
    std::fs::create_dir_all(&empty).expect("an empty run directory");
    let mut command = world.cmd(&["adopt", "nothing-recorded", "--writeback-item-budget", "0"]);
    match scale {
        Some(scale) => command.env(TIME_SCALE_ENV, scale),
        None => command.env_remove(TIME_SCALE_ENV),
    };
    let refused = world.run_on(command, "adopt nothing-recorded --writeback-item-budget 0");
    refused.exited(REFUSED);
    let said = refused
        .stderr
        .split("leave it out to take ")
        .nth(1)
        .and_then(|rest| rest.split(" seconds per item").next())
        .unwrap_or_else(|| panic!("the refusal names no default budget: {}", refused.stderr));
    said.to_owned()
}

/// A scale that is not a positive, finite number is refused before the verb runs, by the
/// variable's name and the value it carried — never quietly read as no scale, which would
/// leave a journey that meant to shorten its waits sitting out the shipped ones.
#[test]
fn an_invalid_scale_is_refused_by_the_variables_name() {
    let world = World::new("time-scale-invalid");
    let empty = world.runs.join("nothing-recorded");
    std::fs::create_dir_all(&empty).expect("an empty run directory");
    for invalid in ["soon", "0", "-0.25", "NaN", "inf", "1e400", ""] {
        let mut command = world.cmd(&["adopt", "nothing-recorded", "--writeback-item-budget", "0"]);
        command.env(TIME_SCALE_ENV, invalid);
        let refused = world.run_on(command, &format!("adopt with {TIME_SCALE_ENV}={invalid:?}"));
        refused
            .exited(REFUSED)
            .err_has(TIME_SCALE_ENV)
            .err_has(&format!("'{invalid}'"));
        assert!(
            !refused.stderr.contains("write-back budget"),
            "{invalid:?} reached the verb it should have been refused before: {}",
            refused.stderr
        );
    }
    // And a value that is not text at all, which is a value and not an unset variable.
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut command = world.cmd(&["adopt", "nothing-recorded", "--writeback-item-budget", "0"]);
        command.env(TIME_SCALE_ENV, std::ffi::OsStr::from_bytes(b"0.\xff"));
        world
            .run_on(command, "adopt with a scale that is not UTF-8")
            .exited(REFUSED)
            .err_has(TIME_SCALE_ENV)
            .err_has("must be a positive, finite number");
    }
}

/// Unset, and at `1`, the binary applies the shipped wait; at another scale, the shipped
/// wait times that scale — and the world's own scale is the one [`World::scaled_secs`] says.
#[test]
fn unset_or_one_applies_the_shipped_wait_and_a_scale_applies_it_scaled() {
    let world = World::new("time-scale-applied");
    let shipped = SHIPPED_ITEM_BUDGET.to_string();
    assert_eq!(applied_item_budget(&world, None), shipped, "unset");
    assert_eq!(applied_item_budget(&world, Some("1")), shipped, "at 1");
    assert_eq!(
        applied_item_budget(&world, Some("0.5")),
        (SHIPPED_ITEM_BUDGET / 2).to_string(),
        "at a half"
    );
    // The world's own, which every other journey runs at.
    world
        .run(&["adopt", "nothing-recorded", "--writeback-item-budget", "0"])
        .exited(REFUSED)
        .err_has(&format!(
            "leave it out to take {} seconds per item",
            world.scaled_secs(SHIPPED_ITEM_BUDGET)
        ));
    assert_ne!(
        world.scaled_secs(SHIPPED_ITEM_BUDGET),
        SHIPPED_ITEM_BUDGET,
        "the world's scale shortens nothing, so this journey proves nothing about it"
    );
}

/// A dispatch that only heartbeats, on a host naming no threshold of its own, is reported
/// quiet at the shipped forty minutes times the scale — here a thousandth, so two seconds:
/// the threshold the journal's `quiet-worker` record states is the one the binary applied.
/// The shipped threshold itself is the one no journey can wait out; the record's figure is
/// what shows the scale reached it, and an unscaled build would leave this journey waiting
/// forty minutes for a report that `World::until` gives up on.
#[test]
fn a_quiet_worker_is_reported_at_the_shipped_threshold_times_the_scale() {
    let world = World::new("time-scale-stall").with_env(TIME_SCALE_ENV, "0.001");
    world.script("stuck.turn-open", "");
    world.script("stuck.wait", "hold");
    world.script("stuck.heartbeat", "50");
    let path = world.plan("quietly", &plan_of("quietly", vec![agent("stuck", &[])]));
    let mut command = world.cmd(&["start", &path, "--detach"]);
    command.env_remove(STALL_AFTER_ENV);
    world.run_on(command, "start --detach").exited(0);
    world.until("the worker to be reported quiet", |world| {
        !world.events_of("quietly", "quiet-worker").is_empty()
    });
    let reported = world
        .events_of("quietly", "quiet-worker")
        .into_iter()
        .find(|event| event["labels"]["node"] == "stuck")
        .expect("the worker was reported quiet");
    assert_eq!(
        reported["payload"]["threshold_seconds"],
        world.scaled_secs(SHIPPED_STALL_AFTER),
        "{reported}"
    );
    world.release("stuck.go");
    world.until("the run to settle", |world| {
        world.run_file("quietly", "result.json").is_file()
    });
}
