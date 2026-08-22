//! Same-identity launch exclusion through the real `onevcs session holders` verb.
// llmlint: ignore-file[e2e_not_mocked] the layer under test is the compiled launcher
// and its released `onevcs` holders executable, both driven for real. Only the paid
// model turn behind `oneagentgraph` uses the repository's established subprocess seam;
// scripting it holds the first real owner process and real `onevcs` session open.

use std::process::Stdio;

use serde_json::json;

use crate::harness::{plan_of, World};

#[test]
fn live_holders_refuse_unless_acknowledged_and_stale_holders_do_not_refuse() {
    let world = World::new("concurrent");
    let _repository = world.repository("local-direct", &[]);
    world.script("build.wait", "hold");

    let lifecycle = || {
        json!({
            "id": "build",
            "task": "## What\nBuild.\n\n## Why\nNeeded.\n\n## Acceptance criteria\n- Built.",
            "persona": "engineer",
            "repo": "service",
            "title": "feat: build it"
        })
    };
    let first_plan = world.plan("first", &plan_of("first", vec![lifecycle()]));

    // Attached, so the process this test holds *is* the run's driver: the loop
    // runs in it, and it is what asks the linked `onevcs` to open the session.
    // The held worker keeps both owner and session live.
    let mut first_owner = world.cmd(&["start", &first_plan.to_string_lossy(), "--attach"]);
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
        world.cmd(&["start", &plan.to_string_lossy(), "--detach"]),
        "start",
    );
    refused
        .exited(2)
        .err_has("concurrent project work refused")
        .err_has("github.com/owner/service")
        .err_has(&live_token)
        .err_has(&format!("owner_pid {owner_pid}"));

    let acknowledged = world.run_on(
        world.cmd(&[
            "start",
            &plan.to_string_lossy(),
            "--detach",
            "--acknowledge-concurrent",
        ]),
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
    assert!(audit["payload"]["runs"]["holding_sessions"]
        .as_array()
        .is_some_and(|runs| runs.iter().any(|run| run == &live_token)));

    // The holder becomes stale because its actual owner exits without closing
    // the session. Waiting for it makes the liveness transition a fact before
    // the next launcher asks `onevcs`, rather than a timing assumption.
    first_owner
        .kill()
        .expect("the first session owner is terminated");
    first_owner.wait().expect("the first session owner exits");
    // And so does the acknowledged run's: it drove itself the moment it was
    // launched, so it opened a session of its own on the same identity, and a
    // launch that met *that* one would be refused by a live holder rather than
    // proceeding past a stale one — which is the claim below.
    world.run(&["stop", "second"]).exited(0);
    world.release("build.go");

    let stale_plan = world.plan("third", &plan_of("third", vec![lifecycle()]));
    world
        .run_on(
            world.cmd(&["start", &stale_plan.to_string_lossy(), "--detach"]),
            "start stale",
        )
        .exited(0)
        .err_has("stale repository holder")
        .err_has(&live_token)
        .err_has("proceeding");
}
