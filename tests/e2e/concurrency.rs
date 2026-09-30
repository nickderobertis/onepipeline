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

/// The identity every `on_service` node's repository resolves to.
const IDENTITY: &str = "github.com/owner/service";

/// A lifecycle node on the one repository, waiting on `deps`.
fn on_service(id: &str, deps: &[&str]) -> Value {
    json!({
        "id": id,
        "task": "## What\nBuild.\n\n## Why\nNeeded.\n\n## Acceptance criteria\n- Built.",
        "persona": "engineer",
        "repo": "service",
        "title": "feat: build it",
        "deps": deps,
    })
}

/// The token of the session `run`'s node `node` opened, once it has.
fn opened_session(world: &World, run: &str, node: &str) -> String {
    let opened = |world: &World| {
        world.journal(run).into_iter().find_map(|event| {
            (event["source"] == "vcs"
                && event["kind"] == "session-opened"
                && event["labels"]["node"] == node)
                .then(|| event["payload"]["token"].as_str().map(str::to_string))
                .flatten()
        })
    };
    world.until(
        &format!("run '{run}' node '{node}' to open its session"),
        |world| opened(world).is_some(),
    );
    opened(world).expect("the session was opened")
}

/// A dependency on the holding node is an acknowledgement of that holder and of
/// no other: the launch proceeds past exactly the holders every one of its nodes
/// on the identity waits for, and refuses — naming each other holder, what would
/// acknowledge it, and when acknowledging is right — on anything less.
#[test]
fn a_dependency_on_the_holding_node_acknowledges_that_holder_and_no_other() {
    let world = World::new("deferred");
    let _repository = world.repository("local-direct", &[]);
    world.script("build.wait", "hold");
    world.script("ship.wait", "hold");

    // The first run holds one node live on the identity.
    let first = world.plan("first", &plan_of("first", vec![on_service("build", &[])]));
    let mut first_owner = world.cmd(&["start", &first, "--attach"]);
    let mut first_owner = first_owner
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the first run's owner starts");
    let build = opened_session(&world, "first", "build");
    let owner_pid = first_owner.id();
    let held = format!("identity '{IDENTITY}' held by session '{build}' (owner_pid {owner_pid})");

    // Every node on the identity reaches `run:first#build`: one directly, one
    // through an in-plan edge from a node on no identity at all.
    let covered = world.plan(
        "covered",
        &plan_of(
            "covered",
            vec![
                on_service("direct", &["run:first#build"]),
                json!({"id": "prep", "kind": "human", "task": "Prepare.", "deps": ["run:first#build"]}),
                on_service("through", &["prep"]),
            ],
        ),
    );
    let proceeded = world.run_on(world.cmd(&["start", &covered, "--detach"]), "start covered");
    proceeded
        .exited(0)
        .err_has(&format!(
            "identity '{IDENTITY}' run 'first' node 'build' via `run:first#build`"
        ))
        .err_lacks("concurrent project work refused")
        .err_lacks("proceeding alongside live run");
    let deferred = world.events_of("covered", "concurrent-deferred");
    assert_eq!(deferred.len(), 1, "{deferred:?}");
    assert_eq!(
        deferred[0]["payload"],
        json!({
            "launching": "covered",
            "holders": [{
                "identity": IDENTITY,
                "session": build,
                "owner_pid": owner_pid,
                "run": "first",
                "node": "build",
                "dependency": "run:first#build",
                "dependents": ["direct", "through"],
            }],
        })
    );
    assert!(world
        .events_of("covered", "concurrent-acknowledged")
        .is_empty());
    world.run(&["stop", "covered"]).exited(0);

    // Each way a plan can fall short of that is refused, naming the holder's run
    // and node, the nodes that do not reach it, and the remedy.
    for (name, nodes, undeclared) in [
        ("none", vec![on_service("alpha", &[])], "'alpha'"),
        (
            "partial",
            vec![
                on_service("alpha", &["run:first#build"]),
                on_service("beta", &[]),
            ],
            "'beta'",
        ),
        (
            "elsewhere",
            vec![on_service("alpha", &["run:first#later"])],
            "'alpha'",
        ),
    ] {
        let plan = world.plan(name, &plan_of(name, nodes));
        world
            .run_on(world.cmd(&["start", &plan, "--detach"]), name)
            .exited(2)
            .err_has(&format!(
                "concurrent project work refused for run '{name}':"
            ))
            .err_has(&format!("{held} for run 'first' node 'build'"))
            .err_has(&format!("plan node(s) {undeclared} do not depend on it"))
            .err_has("declare `run:first#build` under `onepipeline.deps`")
            .err_has("pass --acknowledge-concurrent")
            .err_has("acknowledge when this work outranks the concurrent run");
    }

    // A second live holder on the identity — another run's node — which the
    // plan does not depend on.
    let rival = world.plan("rival", &plan_of("rival", vec![on_service("ship", &[])]));
    world
        .run_on(
            world.cmd(&["start", &rival, "--detach", "--acknowledge-concurrent"]),
            "start rival",
        )
        .exited(0);
    let ship = opened_session(&world, "rival", "ship");

    let mixed_nodes = || vec![on_service("alpha", &["run:first#build"])];
    let mixed = world.plan("mixed", &plan_of("mixed", mixed_nodes()));
    let refused = world.run_on(world.cmd(&["start", &mixed, "--detach"]), "start mixed");
    refused
        .exited(2)
        .err_has(&format!("session '{ship}'"))
        .err_has("for run 'rival' node 'ship'; plan node(s) 'alpha' do not depend on it")
        .err_has("declare `run:rival#ship` under `onepipeline.deps`");
    assert!(
        !refused.stderr.contains(&build),
        "a holder the plan depends on was named as a conflict: {}",
        refused.stderr
    );

    // With the flag, every live holder is acknowledged — the covered one
    // carrying the dependency that also covers it — and the deferral is still
    // recorded.
    let acknowledged = world.run_on(
        world.cmd(&["start", &mixed, "--detach", "--acknowledge-concurrent"]),
        "start mixed --acknowledge-concurrent",
    );
    acknowledged
        .exited(0)
        .err_has("proceeding alongside live run")
        .err_has(&build)
        .err_has(&ship)
        .err_has("run 'first' node 'build' via `run:first#build`");
    let audit = world.events_of("mixed", "concurrent-acknowledged");
    assert_eq!(audit.len(), 1, "{audit:?}");
    let audit = &audit[0]["payload"];
    assert_eq!(audit["shared_identities"], json!([IDENTITY, IDENTITY]));
    assert_eq!(audit["runs"]["launching"], "mixed");
    let holding: Vec<&Value> = audit["runs"]["holding_sessions"]
        .as_array()
        .expect("the holding sessions")
        .iter()
        .collect();
    assert_eq!(holding.len(), 2);
    assert!(holding.contains(&&json!(build)) && holding.contains(&&json!(ship)));
    let holders = audit["holders"].as_array().expect("the holders");
    let entry = |token: &str| {
        holders
            .iter()
            .find(|holder| holder["session"] == token)
            .unwrap_or_else(|| panic!("{token} is not among {holders:?}"))
    };
    assert_eq!(entry(&build)["owner_pid"], json!(owner_pid));
    assert_eq!(entry(&build)["dependency"], "run:first#build");
    assert_eq!(entry(&build)["run"], "first");
    assert_eq!(entry(&build)["node"], "build");
    assert_eq!(entry(&ship)["run"], "rival");
    assert!(entry(&ship).get("dependency").is_none(), "{holders:?}");
    let deferred = world.events_of("mixed", "concurrent-deferred");
    assert_eq!(deferred.len(), 1, "{deferred:?}");
    assert_eq!(
        deferred[0]["payload"]["holders"][0]["session"],
        json!(build)
    );
    assert_eq!(
        deferred[0]["payload"]["holders"].as_array().map(Vec::len),
        Some(1)
    );
    world.run(&["stop", "mixed"]).exited(0);
    world.run(&["stop", "rival"]).exited(0);

    first_owner.kill().expect("the first owner is ended");
    first_owner.wait().expect("the first owner exits");
    world.release("build.go");
    world.release("ship.go");
}

/// Another node of the same run, live on the identity, is a holder of its own:
/// a dependency on one node of a run does not acknowledge its sibling, and one
/// on each does.
#[test]
fn a_dependency_on_one_node_of_a_run_does_not_acknowledge_another_of_its_nodes() {
    let world = World::new("sibling-holders");
    let _repository = world.repository("local-direct", &[]);
    world.script("build.wait", "hold");
    world.script("ship.wait", "hold");

    let first = world.plan(
        "first",
        &plan_of(
            "first",
            vec![on_service("build", &[]), on_service("ship", &[])],
        ),
    );
    let mut first_owner = world.cmd(&["start", &first, "--attach"]);
    let mut first_owner = first_owner
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the first run's owner starts");
    let build = opened_session(&world, "first", "build");
    let ship = opened_session(&world, "first", "ship");

    let one = world.plan(
        "one",
        &plan_of("one", vec![on_service("alpha", &["run:first#build"])]),
    );
    let refused = world.run_on(world.cmd(&["start", &one, "--detach"]), "start one");
    refused
        .exited(2)
        .err_has(&format!("session '{ship}'"))
        .err_has("for run 'first' node 'ship'; plan node(s) 'alpha' do not depend on it")
        .err_has("declare `run:first#ship` under `onepipeline.deps`");
    assert!(
        !refused.stderr.contains(&build),
        "a holder the plan depends on was named as a conflict: {}",
        refused.stderr
    );

    let both = world.plan(
        "both",
        &plan_of(
            "both",
            vec![on_service("alpha", &["run:first#build", "run:first#ship"])],
        ),
    );
    let proceeded = world.run_on(world.cmd(&["start", &both, "--detach"]), "start both");
    proceeded.exited(0);
    // Every dependency-covered holder is named on the one deferral line, each by
    // its identity, run, node and the dependency that defers to it.
    let lines: Vec<&str> = proceeded
        .stderr
        .lines()
        .filter(|line| line.contains("depends on live holder(s)"))
        .collect();
    assert_eq!(lines.len(), 1, "{}", proceeded.stderr);
    for holder in ["build", "ship"] {
        assert!(
            lines[0].contains(&format!(
                "identity '{IDENTITY}' run 'first' node '{holder}' via `run:first#{holder}`"
            )),
            "the deferral line does not name holder '{holder}': {}",
            lines[0]
        );
    }
    let deferred = world.events_of("both", "concurrent-deferred");
    let named: Vec<&Value> = deferred[0]["payload"]["holders"]
        .as_array()
        .expect("the deferred holders")
        .iter()
        .map(|holder| &holder["dependency"])
        .collect();
    assert_eq!(named.len(), 2, "{deferred:?}");
    assert!(
        named.contains(&&json!("run:first#build")) && named.contains(&&json!("run:first#ship"))
    );
    world.run(&["stop", "both"]).exited(0);

    // A live holder nothing attributes to a run's node: no dependency can name
    // it, so a plan depending on every attributed holder is still refused over
    // it, and only the flag passes it.
    let (mut stranger, unattributed) = unattributed_session(&world);
    let covered_nodes = || vec![on_service("alpha", &["run:first#build", "run:first#ship"])];
    let past = world.plan("past", &plan_of("past", covered_nodes()));
    let refused = world.run_on(world.cmd(&["start", &past, "--detach"]), "start past");
    refused
        .exited(2)
        .err_has("concurrent project work refused for run 'past':")
        .err_has(&format!(
            "held by session '{unattributed}' (owner_pid {}), which is not attributable to a run's node",
            stranger.id()
        ))
        .err_has("so only --acknowledge-concurrent passes it")
        .err_has("To launch, pass --acknowledge-concurrent to proceed deliberately.")
        .err_has("acknowledge when this work outranks the concurrent run")
        .err_lacks("declare each dependency");
    assert!(
        !refused.stderr.contains(&build) && !refused.stderr.contains(&ship),
        "a holder the plan depends on was named as a conflict: {}",
        refused.stderr
    );
    world
        .run_on(
            world.cmd(&["start", &past, "--detach", "--acknowledge-concurrent"]),
            "start past --acknowledge-concurrent",
        )
        .exited(0)
        .err_has("proceeding alongside live run")
        .err_has(&unattributed);
    let audit = world.events_of("past", "concurrent-acknowledged");
    let holders = audit[0]["payload"]["holders"]
        .as_array()
        .expect("the acknowledged holders");
    assert_eq!(holders.len(), 3, "{holders:?}");
    let stranger_entry = holders
        .iter()
        .find(|holder| holder["session"] == unattributed.as_str())
        .expect("the unattributed holder is acknowledged");
    for absent in ["run", "node", "dependency"] {
        assert!(stranger_entry.get(absent).is_none(), "{stranger_entry}");
    }
    assert!(holders
        .iter()
        .filter(|holder| holder["session"] != unattributed.as_str())
        .all(|holder| holder["dependency"].is_string()));
    assert_eq!(
        world.events_of("past", "concurrent-deferred")[0]["payload"]["holders"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    world.run(&["stop", "past"]).exited(0);
    drop(stranger.stdin.take());
    stranger.wait().expect("the unattributed holder exits");

    first_owner.kill().expect("the first owner is ended");
    first_owner.wait().expect("the first owner exits");
    world.release("build.go");
    world.release("ship.go");
}

/// A live session with no `run`/`node` labels, held open by the
/// `session-holder` program until its stdin is closed.
fn unattributed_session(world: &World) -> (std::process::Child, String) {
    let mut held = Command::new(crate::harness::double("session-holder"))
        .arg("service")
        .env("ONEVCS_HOME", world.onevcs_home())
        .env("GIT_CONFIG_GLOBAL", world.gitconfig())
        .env("GIT_AUTHOR_NAME", crate::harness::GIT_WHO)
        .env("GIT_AUTHOR_EMAIL", crate::harness::GIT_EMAIL)
        .env("GIT_COMMITTER_NAME", crate::harness::GIT_WHO)
        .env("GIT_COMMITTER_EMAIL", crate::harness::GIT_EMAIL)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("the session holder starts");
    let mut line = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(held.stdout.as_mut().expect("its stdout")),
        &mut line,
    )
    .expect("the session holder prints its token");
    assert!(
        line.trim().starts_with("s-"),
        "the session holder did not open its session: {line:?}"
    );
    (held, line.trim().to_owned())
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
