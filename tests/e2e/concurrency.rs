//! Same-identity launch exclusion through the real `onevcs session holders` verb.
// llmlint: ignore-file[e2e_not_mocked] the layer under test is the compiled launcher
// and its released `onevcs` holders executable, both driven for real. Only the paid
// model turn behind `oneagentgraph` uses the repository's established subprocess seam;
// scripting it holds the first real owner process and real `onevcs` session open.

use std::process::{Command, Stdio};

use serde_json::{json, Value};

use crate::harness::{plan_of, World};

#[test]
fn live_holders_refuse_unless_acknowledged_and_stale_ones_are_reported_or_left_out() {
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
    let mut first_owner = world.cmd(&["start", &first_plan, "--attach"]);
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
    let live_opening = world
        .journal("first")
        .into_iter()
        .find(|event| event["source"] == "vcs" && event["kind"] == "session-opened")
        .expect("the first launcher run recorded its session");
    let live_token = live_opening["payload"]["token"]
        .as_str()
        .map(str::to_string)
        .expect("the first launcher run named its session");
    // The run root that session was cut under, which is what decides whether
    // anybody is left to answer for its record once its owner has gone.
    #[cfg(unix)]
    let live_run_root = std::path::PathBuf::from(
        live_opening["payload"]["worktree"]
            .as_str()
            .expect("the sibling named the worktree it cut"),
    )
    .parent()
    .expect("a session worktree sits inside its run root")
    .to_path_buf();
    let owner_pid = first_owner.id();

    let plan = world.plan("second", &plan_of("second", vec![lifecycle()]));

    let refused = world.run_on(world.cmd(&["start", &plan, "--detach"]), "start");
    refused
        .exited(2)
        .err_has("concurrent project work refused")
        .err_has("github.com/owner/service")
        .err_has(&live_token)
        .err_has(&format!("owner_pid {owner_pid}"));

    let acknowledged = world.run_on(
        world.cmd(&["start", &plan, "--detach", "--acknowledge-concurrent"]),
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

    // A stale holder somebody is still working in: its launcher is gone, and a
    // process is working inside the run root the session was cut under. That is
    // the shape a dispatch whose launcher died is in, and the one `onevcs` keeps
    // answering for — so it reaches this launcher, which **reports it and
    // proceeds** rather than refusing.
    //
    // Unix-only, and the sibling says why: it decides who is working inside a run
    // root by reading a process's working directory, which Windows exposes no
    // supported way to ask. There the record is answered on its owner and its
    // branch alone and is left out instead, which is the case below.
    #[cfg(unix)]
    {
        let mut occupant = crate::harness::occupy(&world, &live_run_root);
        let held_plan = world.plan("third", &plan_of("third", vec![lifecycle()]));
        world
            .run_on(world.cmd(&["start", &held_plan, "--detach"]), "start stale")
            .exited(0)
            .err_has("stale repository holder")
            .err_has(&live_token)
            .err_has("proceeding");
        world.run(&["stop", "third"]).exited(0);
        occupant.kill().expect("the run root's occupant is ended");
        occupant.wait().expect("the run root's occupant exits");
    }

    // And a holder nobody is left to answer for at all: opened by a command that
    // has already exited, on a run root nothing is working inside, whose branch
    // carries nothing. The sibling leaves such a record out of the enumeration, so
    // it never reaches this launcher — and the launch says nothing about it rather
    // than printing a holder an operator cannot act on. Seven of those above a
    // launch is what made one real refusal read like seven ignorable ones.
    let abandoned = abandoned_session(&world, "feature/abandoned");
    let plan = world.plan("fourth", &plan_of("fourth", vec![lifecycle()]));
    let proceeded = world.run_on(
        world.cmd(&["start", &plan, "--detach"]),
        "start past a holder left out of the answer",
    );
    proceeded.exited(0);
    assert!(
        !proceeded.stderr.contains(&abandoned),
        "a holder nobody is left to answer for was reported above a launch: {}",
        proceeded.stderr
    );
    world.release("build.go");
}

/// A session opened by a command that has already exited, on a run root nothing
/// is working inside, whose branch carries nothing — which is what `onevcs` calls
/// **spent** and leaves out of the answer.
///
/// Left out rather than removed: `onevcs` 0.17.1 stopped a holders read deleting
/// the record, because one it deleted was a preserved branch's only route back.
/// Reaping one is `onevcs sweep`'s, which nothing here runs.
///
/// Its token, which is the whole of what the assertion above needs. Driven
/// through the sibling's own executable rather than fabricated, so what this
/// journey calls a holder is a record the sibling itself wrote.
fn abandoned_session(world: &World, branch: &str) -> String {
    let opened = Command::new(crate::harness::onevcs_binary())
        .args(["session", "open", "service", "--branch", branch])
        .env("ONEVCS_HOME", world.onevcs_home())
        .env("GIT_CONFIG_GLOBAL", world.gitconfig())
        .env("GIT_AUTHOR_NAME", crate::harness::GIT_WHO)
        .env("GIT_AUTHOR_EMAIL", crate::harness::GIT_EMAIL)
        .env("GIT_COMMITTER_NAME", crate::harness::GIT_WHO)
        .env("GIT_COMMITTER_EMAIL", crate::harness::GIT_EMAIL)
        .stdin(Stdio::null())
        .output()
        .expect("the onevcs binary runs");
    assert!(
        opened.status.success(),
        "`onevcs session open` did not open a session: {}{}",
        String::from_utf8_lossy(&opened.stdout),
        String::from_utf8_lossy(&opened.stderr)
    );
    let printed = String::from_utf8_lossy(&opened.stdout).into_owned();
    let session: Value =
        serde_json::from_str(printed.trim()).expect("`onevcs session open` prints a session");
    session["token"]
        .as_str()
        .expect("the opened session names its token")
        .to_owned()
}
