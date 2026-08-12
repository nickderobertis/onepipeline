//! Same-identity launch exclusion through the real `onevcs session holders` verb.

use std::process::Command;

use onevcs::SessionRequest;
use serde_json::json;

use crate::harness::{onevcs_binary, plan_of, World};

fn command(world: &World, args: &[&str]) -> Command {
    let onevcs = onevcs_binary();
    let path = std::env::join_paths(
        std::iter::once(
            onevcs
                .parent()
                .expect("onevcs has a directory")
                .to_path_buf(),
        )
        .chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .expect("a PATH");
    let mut command = world.cmd(args);
    command.env("PATH", path);
    command
}

#[test]
fn live_holders_refuse_unless_acknowledged_and_stale_holders_do_not_refuse() {
    let world = World::new("concurrent");
    let _repository = world.repository("local-direct", &["true"]);
    let plan = world.plan(
        "second",
        &plan_of(
            "second",
            vec![json!({
                "id": "build",
                "task": "## What\nBuild.\n\n## Why\nNeeded.\n\n## Acceptance criteria\n- Built.",
                "persona": "engineer",
                "repo": "service"
            })],
        ),
    );

    // Open through the owning library. The record names this still-live test
    // process, while the launcher under test reads it only through the CLI verb.
    std::env::set_var("ONEVCS_HOME", world.onevcs_home());
    std::env::set_var("GIT_CONFIG_GLOBAL", world.gitconfig());
    let providers = onevcs::Providers::real();
    let live = providers
        .vcs
        .open_session(SessionRequest {
            repo: "service".into(),
            branch: Some("held-live".into()),
            base: Some("main".into()),
            execution_checkout: Some("service".into()),
        })
        .expect("a live holder");

    let refused = world.run_on(
        command(&world, &["start", &plan.to_string_lossy(), "--detach"]),
        "start",
    );
    refused
        .exited(2)
        .err_has("concurrent project work refused")
        .err_has("github.com/owner/service")
        .err_has(&live.token.0)
        .err_has(&format!("owner_pid {}", std::process::id()));

    let acknowledged = world.run_on(
        command(
            &world,
            &[
                "start",
                &plan.to_string_lossy(),
                "--detach",
                "--acknowledge-concurrent",
            ],
        ),
        "start --acknowledge-concurrent",
    );
    acknowledged
        .exited(0)
        .err_has("proceeding alongside live run")
        .err_has(&live.token.0);
    let audit = world
        .journal("second")
        .into_iter()
        .find(|event| event["kind"] == "concurrent-acknowledged")
        .expect("the acknowledgement is audited");
    assert_eq!(
        audit["payload"]["shared_identities"],
        json!(["github.com/owner/service"])
    );
    assert!(audit["payload"]["runs"]
        .as_array()
        .is_some_and(|runs| runs.iter().any(|run| run == &live.token.0)));

    let stale_world = World::new("concurrent-stale");
    let _repository = stale_world.repository("local-direct", &["true"]);
    let stale = Command::new(onevcs_binary())
        .args([
            "session",
            "open",
            "service",
            "--branch",
            "held-stale",
            "--base",
            "main",
        ])
        .env("ONEVCS_HOME", stale_world.onevcs_home())
        .env("GIT_CONFIG_GLOBAL", stale_world.gitconfig())
        .output()
        .expect("onevcs opens a session");
    assert!(
        stale.status.success(),
        "{}",
        String::from_utf8_lossy(&stale.stderr)
    );
    let stale_session: serde_json::Value =
        serde_json::from_slice(&stale.stdout).expect("onevcs prints its session");
    let stale_token = stale_session["token"]
        .as_str()
        .expect("the session has a token")
        .to_string();

    let stale_plan = stale_world.plan(
        "third",
        &plan_of(
            "third",
            vec![json!({
                "id": "build",
                "task": "## What\nBuild.\n\n## Why\nNeeded.\n\n## Acceptance criteria\n- Built.",
                "persona": "engineer",
                "repo": "service"
            })],
        ),
    );
    stale_world
        .run_on(
            command(
                &stale_world,
                &["start", &stale_plan.to_string_lossy(), "--detach"],
            ),
            "start stale",
        )
        .exited(0)
        .err_has("stale repository holder")
        .err_has(&stale_token)
        .err_has("proceeding");
}
