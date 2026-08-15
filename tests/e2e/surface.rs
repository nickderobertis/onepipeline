//! The argument surface: what a user may type, and what is rejected before
//! anything is attempted.
//!
//! A usage error costs nothing; a command line that parses and then does the
//! wrong thing costs a run of provider time. Everything here is checked at the
//! boundary.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes the two *siblings* at their
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The scenario this journey states is one a real sibling would need paid
// model turns to produce, and `dispatch.rs` is where the real `oneagentgraph` binary is
// driven instead. `harness.rs` carries the same suppression and the full rationale.

use crate::harness::{World, REFUSED, USAGE_ERROR};
use std::process::Command;

/// Every command the contract documents, with a minimal legal invocation.
const COMMANDS: &[(&str, &[&str])] = &[
    ("start", &["start", "plan.json"]),
    ("adopt", &["adopt", "run-1"]),
    ("channel", &["channel", "serve", "run-1"]),
    ("next", &["next", "run-1"]),
    ("reply", &["reply", "run-1"]),
    (
        "surface",
        &[
            "surface",
            "run-1",
            "--kind",
            "check-in",
            "--message",
            "all clear",
        ],
    ),
    ("attest", &["attest", "run-1", "approve"]),
    ("stop", &["stop", "run-1"]),
    ("runs", &["runs"]),
    ("status", &["status"]),
    ("host", &["host"]),
    ("monitor", &["monitor", "run-1"]),
    ("results", &["results", "run-1"]),
    ("goals", &["goals"]),
    ("transcript", &["transcript", "run-1"]),
    ("telemetry", &["telemetry"]),
];

fn onepipeline() -> Command {
    Command::new(crate::harness::binary())
}

#[test]
fn help_lists_every_documented_command() {
    let output = onepipeline().arg("--help").output().expect("it runs");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for (name, _) in COMMANDS {
        assert!(
            help.contains(name),
            "`--help` does not mention `{name}`:\n{help}"
        );
    }
}

#[test]
fn version_reports_the_crate_version() {
    let output = onepipeline().arg("--version").output().expect("it runs");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn every_command_the_contract_names_reaches_its_implementation() {
    // Pointed at an empty runs root, each verb that names a run refuses by
    // name. What matters is that none of them is a usage error and none of them
    // silently succeeds: the surface and the implementation agree.
    let world = World::new("surface-reach");
    for (name, args) in COMMANDS {
        let run = world.run(args);
        assert!(
            run.code == 0 || run.code == REFUSED,
            "`{name}` exited {}: {}{}",
            run.code,
            run.stdout,
            run.stderr
        );
    }
}

#[test]
fn a_verb_that_names_a_run_nobody_recorded_refuses_and_says_where_it_looked() {
    let world = World::new("surface-norun");
    for args in [
        vec!["next", "ghost"],
        vec!["monitor", "ghost"],
        vec!["results", "ghost"],
        vec!["transcript", "ghost"],
        vec!["status", "ghost"],
        vec!["stop", "ghost"],
        vec!["adopt", "ghost"],
        vec!["drive-run", "ghost"],
    ] {
        world
            .run(&args)
            .exited(REFUSED)
            .err_has("no such run 'ghost'")
            .err_has("runs");
    }
}

#[test]
fn every_optional_form_the_contract_names_reaches_the_binary() {
    let world = World::new("surface-optional");
    let plan = world.plan(
        "one",
        &crate::harness::plan_of("one", vec![crate::harness::agent("build", &[])]),
    );
    let plan = plan.to_string_lossy().into_owned();

    for args in [
        vec!["start", &plan, "--detach", "--dag-graph", "off"],
        vec!["start", &plan, "--detach", "--heartbeat-interval", "1800"],
    ] {
        world.run(&args).exited(0);
    }
    for args in [
        vec!["runs", "--mine"],
        vec!["telemetry", "--breakdown"],
        vec!["goals"],
        vec!["host"],
    ] {
        world.run(&args).exited(0);
    }
}

#[test]
fn a_command_outside_the_surface_is_a_usage_error() {
    let output = onepipeline()
        .args(["publish", "run-1"])
        .output()
        .expect("it runs");
    assert_eq!(output.status.code(), Some(USAGE_ERROR));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));
}

#[test]
fn a_missing_required_argument_is_a_usage_error() {
    for args in [
        vec!["attest", "run-1"],
        vec!["surface", "run-1", "--kind", "check-in"],
        vec!["channel"],
        // The interval is seconds, so a non-numeric value is rejected before
        // anything is launched rather than silently defaulted.
        vec!["start", "plan.json", "--heartbeat-interval", "half-hourly"],
    ] {
        let output = onepipeline().args(&args).output().expect("it runs");
        assert_eq!(
            output.status.code(),
            Some(USAGE_ERROR),
            "`{}` was not a usage error",
            args.join(" ")
        );
    }
}

#[test]
fn an_unknown_surface_kind_is_rejected_before_anything_is_attempted() {
    let output = onepipeline()
        .args(["surface", "run-1", "--kind", "digest", "--message", "hello"])
        .output()
        .expect("it runs");
    assert_eq!(output.status.code(), Some(USAGE_ERROR));
    assert!(String::from_utf8_lossy(&output.stderr).contains("check-in"));
}

#[test]
fn attach_and_detach_cannot_both_be_asked_for() {
    let output = onepipeline()
        .args(["start", "plan.json", "--attach", "--detach"])
        .output()
        .expect("it runs");
    assert_eq!(output.status.code(), Some(USAGE_ERROR));
}
