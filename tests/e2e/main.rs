//! End-to-end journeys against the compiled binary.
//!
//! Every test here spawns the real `onepipeline` executable as a subprocess and
//! asserts on its exit code, stdout, and stderr — the way a user reaches it.
//! Nothing is stubbed: at the interface-only stage the product *is* the argument
//! surface and the refusal, so that is what these drive.

use std::io::Write;

use assert_cmd::Command;
use predicates::prelude::*;

/// The exit code the interface-only build refuses with. `EX_SOFTWARE`, kept
/// clear of every code the contract spends (`0`, `1`, `2`, `3`).
const NOT_IMPLEMENTED: i32 = 70;

/// clap's exit code for a usage error — a command line the surface does not
/// accept, rejected before anything is attempted.
const USAGE_ERROR: i32 = 2;

/// The compiled binary under test, resolved by cargo rather than by PATH.
fn onepipeline() -> Command {
    Command::new(env!("CARGO_BIN_EXE_onepipeline"))
}

/// Every command the contract documents, with a minimal legal invocation.
const COMMANDS: &[(&str, &[&str])] = &[
    ("start", &["start", "plan.json"]),
    ("adopt", &["adopt", "run-1"]),
    ("round", &["round", "run", "run-1"]),
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
    ("telemetry", &["telemetry"]),
];

#[test]
fn help_lists_every_documented_command() {
    let assert = onepipeline().arg("--help").assert().success();
    let help = String::from_utf8(assert.get_output().stdout.clone()).expect("help is UTF-8");

    for (name, _) in COMMANDS {
        assert!(
            help.contains(name),
            "`--help` does not mention `{name}`:\n{help}"
        );
    }
}

#[test]
fn version_reports_the_crate_version() {
    onepipeline()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn every_command_parses_and_then_refuses_loudly() {
    for (name, args) in COMMANDS {
        let assert = onepipeline().args(*args).assert().code(NOT_IMPLEMENTED);
        let stderr =
            String::from_utf8(assert.get_output().stderr.clone()).expect("stderr is UTF-8");

        assert!(
            stderr.contains("NOT IMPLEMENTED"),
            "`{name}` did not refuse loudly:\n{stderr}"
        );
        assert!(
            stderr.contains("ACTION:"),
            "`{name}`'s refusal gives no suggested action:\n{stderr}"
        );
        assert!(
            assert.get_output().stdout.is_empty(),
            "`{name}` wrote to stdout, which a caller could read as output"
        );
    }
}

/// Every optional flag and positional the contract names, beyond the minimal
/// invocations in [`COMMANDS`]. Each is a form a user can type, so each is
/// driven through the binary rather than only through the parser.
const OPTIONAL_FORMS: &[(&str, &[&str])] = &[
    ("start --attach", &["start", "plan.json", "--attach"]),
    ("start --detach", &["start", "plan.json", "--detach"]),
    ("stop --force", &["stop", "run-1", "--force"]),
    ("runs --mine", &["runs", "--mine"]),
    ("status RUN", &["status", "run-1"]),
    ("goals RUN", &["goals", "run-1"]),
    ("telemetry RUN", &["telemetry", "run-1"]),
    (
        "telemetry RUN --breakdown",
        &["telemetry", "run-1", "--breakdown"],
    ),
];

#[test]
fn every_optional_form_the_contract_names_reaches_the_binary() {
    for (name, args) in OPTIONAL_FORMS {
        let assert = onepipeline().args(*args).assert().code(NOT_IMPLEMENTED);
        let stderr =
            String::from_utf8(assert.get_output().stderr.clone()).expect("stderr is UTF-8");
        assert!(
            stderr.contains("NOT IMPLEMENTED"),
            "`{name}` did not reach the refusal:\n{stderr}"
        );
    }
}

#[test]
fn the_refusal_names_the_subcommand_the_user_typed() {
    for (typed, args) in [
        ("round run", vec!["round", "run", "run-1"]),
        ("round next", vec!["round", "next", "run-1"]),
        ("channel serve", vec!["channel", "serve", "run-1"]),
        ("telemetry", vec!["telemetry", "--breakdown"]),
    ] {
        onepipeline()
            .args(&args)
            .assert()
            .code(NOT_IMPLEMENTED)
            .stderr(predicate::str::contains(format!("`{typed}`")));
    }
}

#[test]
fn the_refusal_never_uses_a_code_the_contract_has_spent() {
    // 0 applied, 1 accepted-not-yet-reconciled, 2 refused/malformed, 3 nothing
    // is driving the run. A caller wired in early must not read the
    // interface-only refusal as any of them.
    let assert = onepipeline().args(["next", "run-1"]).assert();
    let code = assert.get_output().status.code().expect("it exited");
    assert_eq!(code, NOT_IMPLEMENTED);
    for spent in [0, 1, 2, 3] {
        assert_ne!(code, spent, "exit {code} is already spent by the contract");
    }
}

#[test]
fn a_command_outside_the_surface_is_a_usage_error() {
    onepipeline()
        .args(["publish", "run-1"])
        .assert()
        .code(USAGE_ERROR)
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

#[test]
fn a_missing_required_argument_is_a_usage_error() {
    for args in [
        vec!["attest", "run-1"],
        vec!["surface", "run-1", "--kind", "check-in"],
        vec!["round"],
    ] {
        onepipeline().args(&args).assert().code(USAGE_ERROR);
    }
}

#[test]
fn an_unknown_surface_kind_is_rejected_before_anything_is_attempted() {
    onepipeline()
        .args(["surface", "run-1", "--kind", "digest", "--message", "hello"])
        .assert()
        .code(USAGE_ERROR)
        .stderr(predicate::str::contains("check-in"));
}

#[test]
fn attach_and_detach_cannot_both_be_asked_for() {
    onepipeline()
        .args(["start", "plan.json", "--attach", "--detach"])
        .assert()
        .code(USAGE_ERROR);
}

#[test]
fn a_reply_envelope_on_stdin_is_still_refused_rather_than_half_applied() {
    // The channel's own contract says a reply is applied or rejected with a
    // reason. A build that implements neither must not exit 0 on a well-formed
    // envelope, which is what a planner would read as "applied".
    let envelope = r#"{"version":1,"commands":[{"op":"attest","ref":"approve"}]}"#;
    onepipeline()
        .args(["reply", "run-1"])
        .write_stdin(envelope)
        .assert()
        .code(NOT_IMPLEMENTED)
        .stderr(predicate::str::contains("NOT IMPLEMENTED"));
}

#[test]
fn a_reply_file_is_accepted_as_a_positional() {
    let dir = std::env::temp_dir().join("onepipeline-e2e-reply");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let path = dir.join("edits.json");
    let mut file = std::fs::File::create(&path).expect("the reply file");
    file.write_all(br#"{"version":1,"commands":[]}"#)
        .expect("written");

    onepipeline()
        .args(["reply", "run-1"])
        .arg(&path)
        .assert()
        .code(NOT_IMPLEMENTED);

    std::fs::remove_dir_all(&dir).expect("cleaned up");
}

#[test]
fn the_shipped_plans_are_what_start_is_pointed_at() {
    // `start` takes the plan as a path, so the examples this repo ships are a
    // legal invocation rather than a separate format.
    for name in ["single-node.plan.json", "mixed-graph.plan.json"] {
        let plan = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/").to_string() + name;
        onepipeline()
            .args(["start", &plan])
            .assert()
            .code(NOT_IMPLEMENTED);
    }
}
