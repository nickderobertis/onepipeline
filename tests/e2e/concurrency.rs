//! Same-identity launch exclusion through the real `onevcs session holders` verb.

use std::process::{Command, Stdio};

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
    world.script("driver.wait", "hold");
    world.script("build.wait", "hold");

    let lifecycle = || {
        json!({
            "id": "build",
            "task": "## What\nBuild.\n\n## Why\nNeeded.\n\n## Acceptance criteria\n- Built.",
            "persona": "engineer",
            "repo": "service"
        })
    };
    let first_plan = world.plan("first", &plan_of("first", vec![lifecycle()]));
    world
        .run_on(
            command(
                &world,
                &["start", &first_plan.to_string_lossy(), "--detach"],
            ),
            "start first",
        )
        .exited(0);

    // Drive the first launch's real lifecycle while its dag-scope driver is
    // paused. This `round run` is the process that asks the linked `onevcs` to
    // open the session, and the held worker keeps both owner and session live.
    let mut first_owner = command(&world, &["round", "run", "first"]);
    let mut first_owner = first_owner
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the first run's owner starts");
    world.until(
        "the first launcher run to open its repository session",
        |world| {
            world.journal("first").iter().any(|event| {
                event["source"] == "vcs"
                    && event["kind"] == "session-opened"
                    && event["payload"]["token"].is_string()
            })
        },
    );
    let live_token = world
        .journal("first")
        .into_iter()
        .find(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .and_then(|event| event["payload"]["token"].as_str().map(str::to_string))
        .expect("the first launcher run recorded its session");
    let owner_pid = first_owner.id();

    let plan = world.plan("second", &plan_of("second", vec![lifecycle()]));

    let refused = world.run_on(
        command(&world, &["start", &plan.to_string_lossy(), "--detach"]),
        "start",
    );
    refused
        .exited(2)
        .err_has("concurrent project work refused")
        .err_has("github.com/owner/service")
        .err_has(&live_token)
        .err_has(&format!("owner_pid {owner_pid}"));

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
        .err_has(&live_token);
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
        .is_some_and(|runs| runs.iter().any(|run| run == &live_token)));

    // The holder becomes stale because its actual owner exits without closing
    // the session. Waiting for it makes the liveness transition a fact before
    // the next launcher asks `onevcs`, rather than a timing assumption.
    first_owner
        .kill()
        .expect("the first session owner is terminated");
    first_owner.wait().expect("the first session owner exits");
    world.release("build.go");

    let stale_plan = world.plan("third", &plan_of("third", vec![lifecycle()]));
    world
        .run_on(
            command(
                &world,
                &["start", &stale_plan.to_string_lossy(), "--detach"],
            ),
            "start stale",
        )
        .exited(0)
        .err_has("stale repository holder")
        .err_has(&live_token)
        .err_has("proceeding");
    world.release("driver.go");
}
