//! A run's channel under the `onemessagebus` configuration its launch names.
//!
//! `start --bus-config` reads an `onemessagebus.yaml` at the launch — or a launch
//! config names it — refuses there, before any run exists, a configuration the
//! run's channel could not be kept under, and keeps the one it accepts in the
//! launch record. What these journeys hold is that the configuration a launch
//! accepts is the one the run enforces: the grants its `authors` block narrowed,
//! the hooks its `hooks` block names, and the reply window its
//! `codecs.onejudge` block sets — each against the same plan launched without the
//! file, so what is enforced is the configuration's doing and not the profile's.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The hook is not a substitution either: it is a real command a host
// names, and this suite supplies a real one. `harness.rs` carries the same suppression and
// the full rationale.

use serde_json::{json, Value};

use crate::harness::{agent, double, human, plan_of, World, REFUSED};

/// A configuration narrowing the monitor to `retry` alone, taking away the
/// `cancel` the profile grants it beside `retry`.
const WITHOUT_CANCEL: &str = "version: 1\n\
                              transport: {kind: local}\n\
                              profile: planner-channel\n\
                              authors:\n  monitor: {capabilities: [retry]}\n";

fn configuration(world: &World, name: &str, body: &str) -> String {
    let path = world.root.join(name);
    std::fs::write(&path, body).expect("the configuration is written");
    path.to_string_lossy().into_owned()
}

/// The plan every run here is launched from: a node held open, so the graph is
/// live while replies arrive, and a node waiting on it that an edit can reach.
fn plan(world: &World, name: &str) -> String {
    world.plan(
        name,
        &plan_of(name, vec![agent("slow", &[]), agent("spare", &["slow"])]),
    )
}

/// Launch `path` detached under `extra`, and wait for its held node. `run` is the
/// id the launch mints: the plan's name, or that name numbered after a run that
/// already holds it.
fn launched(world: &World, path: &str, run: &str, extra: &[&str]) {
    // A fresh hold: a rendezvous an earlier run released is a file, and left in
    // place it would satisfy this run's hold the moment its dispatch reached it.
    let _ = std::fs::remove_file(world.fakes.join("slow.go"));
    world.script("slow.wait", "hold");
    let mut args = vec!["start", path];
    args.extend_from_slice(extra);
    args.push("--detach");
    world.run(&args).exited(0);
    world.until("the held node to be running", |world| {
        world.run(&["status", run]).stdout.contains("slow: running")
    });
}

/// Every surface waiting for the planner, read the way a planner reads them:
/// `next` until it answers with none. What is asserted about a surface is
/// asserted on what `next` handed over.
fn surfaces_read(world: &World, run: &str) -> Vec<Value> {
    let mut read = Vec::new();
    loop {
        let next = world.run(&["next", run]);
        next.exited(0);
        let answer = next.json();
        if answer["surface"].is_null() {
            return read;
        }
        read.push(answer["surface"].clone());
    }
}

/// The one `edit-applied` surface among `surfaces`, held to what the contract
/// says of it: non-blocking, sourced from `author`, and naming that author and
/// the `add` it reports.
fn the_edit_applied(surfaces: &[Value], author: &str) -> Value {
    let edits: Vec<&Value> = surfaces
        .iter()
        .filter(|surface| surface["kind"] == "edit-applied")
        .collect();
    let [edit] = &edits[..] else {
        panic!("one applied edit is one `edit-applied` surface, and a finding none: {surfaces:?}");
    };
    assert_eq!(edit["source"], author, "{edit}");
    assert_eq!(edit["blocking"], json!(false), "{edit}");
    assert!(
        edit["message"].as_str().is_some_and(|message| {
            message.starts_with(&format!("{author} applied an edit: "))
                && message.contains("\"add\"")
        }),
        "the surface does not name the author and the edit: {edit}"
    );
    (*edit).clone()
}

fn channel_file(world: &World, run: &str, file: &str) -> String {
    std::fs::read_to_string(world.run_file(run, &format!("channel/{file}"))).unwrap_or_default()
}

/// Launch a plan under a configuration its channel could not be kept under, and
/// hold that the launch is refused naming each of `named` with nothing written
/// into the runs root.
fn refused_before_a_run_exists(world: &World, body: &str, named: &[&str]) {
    let file = configuration(world, "refused.yaml", body);
    let path = plan(world, "refused");
    let refused = world.run(&["start", &path, "--bus-config", &file, "--detach"]);
    refused.exited(REFUSED);
    for fragment in named {
        refused.err_has(fragment);
    }
    let written: Vec<_> = std::fs::read_dir(&world.runs)
        .expect("the runs root")
        .map(|entry| entry.expect("an entry").file_name())
        .collect();
    assert!(
        written.is_empty(),
        "a refused launch wrote into the runs root: {written:?}"
    );
}

/// A configuration a launch names is read at the launch and kept whole in the
/// launch record, whether the flag names it or a launch config does at the
/// version that added the key — and a launch config below that version naming
/// it is refused by the key's own name.
#[test]
fn a_bus_config_is_read_at_the_launch_and_kept_in_the_launch_record() {
    let world = World::new("bus-config-recorded");
    let file = configuration(&world, "onemessagebus.yaml", WITHOUT_CANCEL);
    let path = plan(&world, "busrecorded");
    launched(&world, &path, "busrecorded", &["--bus-config", &file]);

    let launch_config = world.root.join("launch.yaml");
    std::fs::write(
        &launch_config,
        format!("schema_version: 7\nbus_config: {file:?}\n"),
    )
    .expect("the launch config is written");
    let launch_config = launch_config.to_string_lossy().into_owned();
    // The project's first run still claims it, so launching it again is a takeover — of a run
    // a shutdown ended, which this host's records prove nobody drives, so it waits on nothing.
    let ended = world.run(&["shutdown", "busrecorded", "--grace", "1"]);
    assert!([0, 2].contains(&ended.code), "{}", ended.stderr);
    launched(
        &world,
        &path,
        "busrecorded-2",
        &["--launch-config", &launch_config, "--take-over"],
    );

    for run in ["busrecorded", "busrecorded-2"] {
        let launch: Value = serde_json::from_str(
            &std::fs::read_to_string(world.run_file(run, "launch.json"))
                .expect("the launch record"),
        )
        .expect("the launch record is JSON");
        let kept = &launch["bus_config"];
        assert_eq!(
            kept["transport"],
            json!({"kind": "local"}),
            "{run}: {launch}"
        );
        assert_eq!(kept["profile"], json!("planner-channel"), "{run}: {launch}");
        assert_eq!(
            kept["authors"]["monitor"]["capabilities"],
            json!(["retry"]),
            "{run}: {launch}"
        );
    }

    std::fs::write(
        &launch_config,
        format!("schema_version: 6\nbus_config: {file:?}\n"),
    )
    .expect("the launch config is written");
    world
        .run(&[
            "start",
            &path,
            "--launch-config",
            &launch_config,
            "--detach",
        ])
        .exited(REFUSED)
        .err_has("`bus_config` is a schema 7 key");
    assert!(
        !world.runs.join("busrecorded-3").exists(),
        "a refused launch config minted a run"
    );
    world.release("slow.go");
}

/// A transport other than the run root's own is refused naming it — and the one
/// named is a transport the bus registers, so what is refused is a selection the
/// bus would otherwise honour rather than a word it does not know.
#[test]
fn a_bus_config_naming_another_transport_is_refused_before_a_run_exists() {
    let world = World::new("bus-config-memory");
    refused_before_a_run_exists(
        &world,
        "version: 1\ntransport: {kind: memory}\n",
        &["transport.kind", "`memory`"],
    );
}

/// Where a run's channel is kept is its run root's to decide.
#[test]
fn a_bus_config_naming_a_transport_directory_is_refused_before_a_run_exists() {
    let world = World::new("bus-config-dir");
    refused_before_a_run_exists(
        &world,
        "version: 1\ntransport: {kind: local, dir: elsewhere}\n",
        &["transport.dir", "`elsewhere`"],
    );
}

/// A run's channel is the `planner-channel` layout.
#[test]
fn a_bus_config_naming_another_profile_is_refused_before_a_run_exists() {
    let world = World::new("bus-config-profile");
    refused_before_a_run_exists(
        &world,
        "version: 1\ntransport: {kind: local}\nprofile: another-layout\n",
        &["profile", "`another-layout`"],
    );
}

/// A host may declare an author the built-in layout has never named.
#[test]
fn a_bus_config_declares_an_open_author() {
    let world = World::new("bus-config-author");
    let file = configuration(
        &world,
        "onemessagebus.yaml",
        "version: 1\ntransport: {kind: local}\nauthors:\n  sentinel: {capabilities: []}\n",
    );
    let path = plan(&world, "busauthor");
    launched(&world, &path, "busauthor", &["--bus-config", &file]);
    world.release("slow.go");
}

/// Every other part of a run's channel this crate decides for itself is refused
/// at the launch, naming the key and its value, before any run exists: a
/// transport option the local transport does not take and queues of the
/// configuration's own.
#[test]
fn a_bus_config_moving_what_the_channel_decides_for_itself_is_refused_before_a_run_exists() {
    let world = World::new("bus-config-corners");
    for (body, named) in [
        (
            "version: 1\ntransport: {kind: local, poll: fast}\n",
            ["transport.poll", "`fast`"],
        ),
        (
            "version: 1\ntransport: {kind: local}\nqueues:\n  surfaces: {}\n",
            ["queues.surfaces", "leave `queues` out"],
        ),
    ] {
        refused_before_a_run_exists(&world, body, &named);
    }
}

#[test]
fn codec_and_unreachable_schema_configuration_is_accepted_without_network() {
    let world = World::new("bus-config-host-owned");
    let file = configuration(
        &world,
        "onemessagebus.yaml",
        "version: 1\ntransport: {kind: local}\nschemas:\n  - https://unreachable.invalid/frames.json@1\ncodecs:\n  foreign:\n    queue: surfaces\n    select: kind\n    frames:\n      ping:\n        schema: agent.planner-surface@1\n        bindings:\n          - {do: answer, response: {ok: true}}\n",
    );
    let path = plan(&world, "bushostowned");
    launched(&world, &path, "bushostowned", &["--bus-config", &file]);
    world.release("slow.go");
}

/// A host-named author gets exactly its configured authority on both ingress
/// paths, and its identity survives edits, findings, surfaces, and parks.
#[test]
fn a_host_named_author_is_enforced_at_reply_and_at_driver_apply() {
    const RUN: &str = "bus-author-contract";
    const AUTHOR: &str = "sentinel";
    let world = World::new(RUN);
    let file = configuration(
        &world,
        "onemessagebus.yaml",
        "version: 1\ntransport: {kind: local}\nauthors:\n  sentinel:\n    capabilities: [add, cancel, requeue, finding]\n    refusals:\n      amend: only the planner may alter a task's acceptance bar\n",
    );
    let path = plan(&world, RUN);
    launched(&world, &path, RUN, &["--bus-config", &file]);
    let envelope =
        |commands: Value| json!({"version": 3, "author": AUTHOR, "commands": commands}).to_string();

    world
        .run_with_stdin(
            &["reply", RUN],
            &envelope(json!([
                {"op": "add", "node": agent("extra", &["slow"])},
                {"op": "finding", "id": "slow", "message": "the host noticed this"}
            ])),
        )
        .exited(0);
    world.until("the custom author's edit and finding to commit", |world| {
        world.events_of(RUN, "edit-committed").iter().any(|event| {
            event["payload"]["author"] == AUTHOR && event["payload"]["command"]["op"] == "add"
        }) && world
            .events_of(RUN, "planner-surface-queued")
            .iter()
            .any(|event| {
                event["payload"]["kind"] == "finding" && event["payload"]["source"] == AUTHOR
            })
    });
    // The applied edit is reported once, as what it is: a non-blocking
    // `edit-applied` from the author that applied it, naming that author — a
    // word nothing built in knows — and the edit. The finding beside it raised
    // no second surface: it has already said its piece. Read as the planner
    // reads them.
    let surfaces = surfaces_read(&world, RUN);
    the_edit_applied(&surfaces, AUTHOR);
    assert!(
        surfaces
            .iter()
            .any(|surface| surface["kind"] == "finding" && surface["source"] == AUTHOR),
        "the finding itself was not handed over under its author: {surfaces:?}"
    );

    for (command, refusal) in [
        (
            json!({"op": "amend", "id": "spare", "text": "a different bar"}),
            "only the planner may alter a task's acceptance bar",
        ),
        (
            json!({"op": "drop", "id": "spare", "dependents": "detach"}),
            "nothing grants it to this author",
        ),
    ] {
        world
            .run_with_stdin(&["reply", RUN], &envelope(json!([command])))
            .exited(REFUSED)
            .err_has(refusal);
    }

    let before = world.events_of(RUN, "edit-committed").len();
    world
        .run_with_stdin(
            &["reply", RUN],
            &json!({"version": 3, "author": "stranger", "commands": [
                {"op": "add", "node": agent("never", &[])}
            ]})
            .to_string(),
        )
        .exited(REFUSED)
        .err_has("author `stranger` is not declared")
        .err_has("planner, sentinel");
    assert_eq!(world.events_of(RUN, "edit-committed").len(), before);

    // llmlint: ignore-block[tests_mirror_real_usage] Deliberately bypasses
    // `reply` to prove the driver's independent trust-boundary check; the local
    // transport's own append is the real host-bus boundary. It takes the
    // queue's lock and lands each record in one write, which a bare file append
    // does not: that tears a record across writes, and a reconciler claiming
    // between two of them heals the fragment away as a dead writer's tail.
    // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] the
    // envelopes are spelled by hand because bypassing `reply` is the point, and
    // the gate on their spelling is the driver reading them back below: it
    // records each as rejected by *author*, which a record it could not read
    // would never reach.
    let bus = onemessagebus::LocalTransport::open(world.run_file(RUN, "channel"))
        .expect("the host bus opens the channel");
    let commands = onemessagebus::QueueName::try_from(onepipeline::channel::layout::COMMANDS)
        .expect("the command queue's name");
    for payload in [
        json!({"id": 900, "author": "stranger", "commands": [
            {"op": "add", "node": agent("also-never", &[])}
        ]}),
        json!({"id": 901, "author": AUTHOR, "commands": [
            {"op": "complete", "reason": "looks done"}
        ]}),
    ] {
        onemessagebus::Transport::append(&bus, &commands, payload.to_string().as_bytes())
            .expect("the host bus appends the envelope");
    }
    // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
    // llmlint: ignore-end[tests_mirror_real_usage]
    world.until("the direct envelopes to be rejected", |world| {
        world.events_of(RUN, "edit-rejected").len() >= 2
    });
    // Both landed whole: a reconciler that healed a fragment away would have
    // recorded it here, and the envelope it belonged to would never be judged.
    assert!(
        !world.run_file(RUN, "channel/commands.jsonl.torn").exists(),
        "the host bus's append was torn: {}",
        std::fs::read_to_string(world.run_file(RUN, "channel/commands.jsonl.torn"))
            .unwrap_or_default()
    );
    let rejected = world.events_of(RUN, "edit-rejected");
    assert!(rejected.iter().any(|event| {
        event["payload"]["author"] == "stranger"
            && event["payload"]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("not declared"))
    }));
    assert!(rejected.iter().any(|event| {
        event["payload"]["author"] == AUTHOR && event["payload"]["command"]["op"] == "complete"
    }));
    assert_eq!(world.events_of(RUN, "edit-committed").len(), before);
    assert!(world.events_of(RUN, "completion-requested").is_empty());

    world
        .run_with_stdin(
            &["reply", RUN],
            &json!({"author": AUTHOR, "completion": true, "reason": "looks done"}).to_string(),
        )
        .exited(REFUSED)
        .err_has("not something the sentinel may do");
    assert!(world.events_of(RUN, "completion-requested").is_empty());

    world
        .run_with_stdin(
            &["reply", RUN],
            &json!({"version": 3, "commands": [
                {"op": "cancel", "id": "spare", "reason": "planner hold"}
            ]})
            .to_string(),
        )
        .exited(0);
    world
        .run_with_stdin(
            &["reply", RUN],
            &envelope(json!([{"op": "requeue", "id": "spare"}])),
        )
        .exited(REFUSED)
        .err_has("parked by the planner");
    world
        .run_with_stdin(
            &["reply", RUN],
            &envelope(json!([
                {"op": "cancel", "id": "extra", "reason": "sentinel hold"},
                {"op": "requeue", "id": "extra"}
            ])),
        )
        .exited(0);
    world.release("slow.go");
}

/// An edit applied by `reply` while nothing is driving the run is reported the
/// same way the loop reports one: one non-blocking `edit-applied` surface from
/// the author that applied it, and none for a `finding`.
///
/// Which side applies an edit is an accident of whether a driver happened to be
/// alive, and the planner reads the same report either way — through `next`,
/// which is where a planner away from a parked run meets it.
#[test]
fn an_edit_applied_with_nothing_driving_is_surfaced_as_edit_applied_by_its_author() {
    const RUN: &str = "bus-undriven-edit";
    const AUTHOR: &str = "sentinel";
    let world = World::new(RUN);
    let file = configuration(
        &world,
        "onemessagebus.yaml",
        "version: 1\ntransport: {kind: local}\nauthors:\n  sentinel:\n    capabilities: [add, finding]\n",
    );
    // A human node: the attached launch returns awaiting the planner, and
    // nothing is driving the run when the replies below arrive.
    let path = world.plan(RUN, &plan_of(RUN, vec![human("approve", &[])]));
    world
        .run(&["start", &path, "--attach", "--bus-config", &file])
        .exited(0)
        .out_has("\"settlement\":\"awaiting-planner\"");

    let applied = world.run_with_stdin(
        &["reply", RUN],
        &json!({"version": 3, "author": AUTHOR, "commands": [
            {"op": "finding", "id": "approve", "message": "the approval has waited a while"},
            {"op": "add", "node": agent("extra", &["approve"])}
        ]})
        .to_string(),
    );
    applied.exited(0);
    assert_eq!(applied.json()["commands"], "applied", "{}", applied.stdout);

    // The planner reads exactly two surfaces — the finding and the report of
    // the applied `add` — both under the author's own word.
    let surfaces = surfaces_read(&world, RUN);
    the_edit_applied(&surfaces, AUTHOR);
    let mut kinds: Vec<&str> = surfaces
        .iter()
        .map(|surface| surface["kind"].as_str().unwrap_or_default())
        .collect();
    kinds.sort_unstable();
    assert_eq!(kinds, ["edit-applied", "finding"], "{surfaces:?}");
    assert!(
        surfaces.iter().all(|surface| surface["source"] == AUTHOR),
        "{surfaces:?}"
    );
    // And the stream a supervisor watches shows the report handed over.
    world
        .run(&["monitor", RUN])
        .exited(0)
        .out_has("planner-surfaced kind=\"edit-applied\"");
}

/// Recorded identity is data, not fresh authority: removing the launch grant
/// after a `monitor` park was recorded does not make its journal or checkpoint
/// unreadable.
#[test]
fn a_legacy_monitor_author_replays_from_the_journal_and_checkpoint() {
    const RUN: &str = "bus-legacy-monitor";
    let world = World::new(RUN);
    let file = configuration(
        &world,
        "onemessagebus.yaml",
        "version: 1\ntransport: {kind: local}\nauthors:\n  monitor: {capabilities: [cancel]}\n",
    );
    let path = plan(&world, RUN);
    launched(&world, &path, RUN, &["--bus-config", &file]);
    world
        .run_with_stdin(
            &["reply", RUN],
            &json!({"version": 3, "author": "monitor", "commands": [
                {"op": "cancel", "id": "spare", "reason": "legacy watcher park"}
            ]})
            .to_string(),
        )
        .exited(0);
    world.run(&["status", RUN]).exited(0);
    let checkpoint = std::fs::read_to_string(world.run_file(RUN, "checkpoint.json"))
        .expect("the replay checkpoint exists");
    assert!(checkpoint.contains("monitor"), "{checkpoint}");

    // llmlint: ignore-block[tests_mirror_real_usage] A legacy launch cannot be
    // minted by today's CLI. Removing the newly optional key constructs exactly
    // the historical serialized record this compatibility journey must read.
    let launch_path = world.run_file(RUN, "launch.json");
    let mut launch: Value = serde_json::from_str(
        &std::fs::read_to_string(&launch_path).expect("the launch record reads"),
    )
    .expect("the launch record is JSON");
    launch
        .as_object_mut()
        .expect("the launch record is an object")
        .remove("bus_config");
    std::fs::write(
        &launch_path,
        serde_json::to_vec_pretty(&launch).expect("the legacy launch serializes"),
    )
    .expect("the launch now carries only the default planner author");

    // llmlint: ignore-end[tests_mirror_real_usage]
    world.run(&["status", RUN]).exited(0).out_has("0/2 done");
    world
        .run(&["monitor", RUN])
        .exited(0)
        .out_has("legacy watcher park");
    world.release("slow.go");
}

/// A configuration naming `hooks` as YAML lines over the real `bus-hook` command,
/// each item `(queue, argv, extra keys)`.
fn hooked(world: &World, hooks: &[(&str, &[&str], &str)]) -> String {
    let command = double("bus-hook").to_string_lossy().into_owned();
    let mut body = "version: 1\ntransport: {kind: local}\nhooks:\n".to_owned();
    for (queue, argv, extra) in hooks {
        let argv: Vec<String> = std::iter::once(command.clone())
            .chain(argv.iter().map(|arg| (*arg).to_owned()))
            .collect();
        body.push_str(&format!(
            "  - {{on: {queue}, command: {}{extra}}}\n",
            serde_json::to_string(&argv).expect("the argv serializes")
        ));
    }
    configuration(world, "onemessagebus.yaml", &body)
}

/// A run launched under `hooks`, its reply as a planner writes it.
fn hooked_run(world: &World, run: &str, hooks: &[(&str, &[&str], &str)]) -> String {
    let file = hooked(world, hooks);
    let path = plan(world, run);
    launched(world, &path, run, &["--bus-config", &file]);
    file
}

fn add(id: &str) -> Value {
    json!({"op": "add", "node": agent(id, &["slow"])})
}

/// What a `commit` of the edit named `id` looks like on the journal.
fn committed(world: &World, run: &str, id: &str) -> bool {
    world
        .events_of(run, "edit-committed")
        .iter()
        .any(|event| event["payload"]["command"]["node"]["id"] == id)
}

fn queued_lines(world: &World, run: &str, file: &str) -> Vec<Value> {
    channel_file(world, run, file)
        .lines()
        .map(|line| serde_json::from_str(line).expect("the queue holds JSON"))
        .collect()
}

/// (a) A hook removing one of an envelope's two commands: the journal commits only
/// the survivor, the removed one appears nowhere, and the receipt says it changed.
#[test]
fn a_hook_that_removes_a_command_applies_only_the_rest_and_says_changed() {
    let world = World::new("hook-removes");
    hooked_run(
        &world,
        "hookremoves",
        &[("replies", &["keep-first-command"], "")],
    );
    let answered = world.run_with_stdin(
        &["reply", "hookremoves"],
        &json!({"version": 3, "commands": [add("kept"), add("removed")]}).to_string(),
    );
    answered.exited(0);
    let receipt = answered.json();
    assert_eq!(receipt["hooked"], "changed", "{receipt}");
    assert_eq!(receipt["commands"], "applied", "{receipt}");
    world.until("the surviving edit to commit", |world| {
        committed(world, "hookremoves", "kept")
    });
    assert!(!committed(&world, "hookremoves", "removed"));
    assert!(!channel_file(&world, "hookremoves", "commands.jsonl").contains("removed"));
    let invoked =
        std::fs::read_to_string(world.fakes.join("bus-hook.jsonl")).expect("the hook was run");
    assert_eq!(
        invoked.lines().count(),
        1,
        "a reply is offered to its hooks once: {invoked}"
    );
    world.release("slow.go");
}

/// (b) A hook rewriting a verdict's message: the asker reads the changed text and
/// not the original, the journal records it as changed, and so does the receipt.
#[test]
fn a_hook_that_rewrites_a_verdict_reaches_the_asker_and_the_journal() {
    let world = World::new("hook-verdict");
    hooked_run(
        &world,
        "hookverdict",
        &[("replies", &["set-message", "the changed ruling"], "")],
    );
    let mut asking = world.cmd(&["ask", "--timeout", "60", "what next?"]);
    asking.env("ONEPIPELINE_RUN_ID", "hookverdict");
    let asking = asking
        .stdin(onepipeline_testfakes::profile::process::Stdio::null())
        .stdout(onepipeline_testfakes::profile::process::Stdio::piped())
        .stderr(onepipeline_testfakes::profile::process::Stdio::piped())
        .spawn()
        .expect("the binary starts");
    world.until("the question to be queued", |world| {
        channel_file(world, "hookverdict", "surfaces.jsonl").contains("what next?")
    });
    let original = "the original ruling";
    let answered = world.run_with_stdin(
        &["reply", "hookverdict"],
        &json!({"completion": false, "message": original}).to_string(),
    );
    answered.exited(0);
    let receipt = answered.json();
    assert_eq!(receipt["hooked"], "changed", "{receipt}");
    assert_eq!(receipt["verdict"], "delivered", "{receipt}");
    let output = asking.wait_with_output().expect("ask returns");
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(said.contains("the changed ruling"), "{said}");
    assert!(!said.contains(original), "{said}");
    let replied = world.events_of("hookverdict", "planner-replied");
    assert!(
        replied.iter().any(|event| {
            event["payload"]["message"] == "the changed ruling"
                && event["payload"]["hooked"] == "changed"
        }),
        "{replied:?}"
    );
    world.release("slow.go");
}

/// (c) A hook rewriting a surface: the planner reads the changed text.
#[test]
fn a_hook_that_rewrites_a_surface_hands_the_planner_the_changed_text() {
    let world = World::new("hook-surface");
    hooked_run(
        &world,
        "hooksurface",
        &[("surfaces", &["set-message", "the changed report"], "")],
    );
    world
        .run(&[
            "surface",
            "hooksurface",
            "--kind",
            "finding",
            "--message",
            "the original report",
        ])
        .exited(0)
        .out_has("\"queued\"");
    let read = surfaces_read(&world, "hooksurface");
    assert!(
        read.iter()
            .any(|surface| surface["message"] == "the changed report"),
        "{read:?}"
    );
    assert!(
        read.iter()
            .all(|surface| surface["message"] != "the original report"),
        "{read:?}"
    );
    assert_eq!(
        world.events_of("hooksurface", "planner-surface-queued")[0]["payload"]["message"],
        "the changed report",
        "the journal records what was queued"
    );
    world.release("slow.go");
}

/// A hook may change a surface's `blocking` as it may its text, and the journal
/// records the surface as queued rather than as `surface` offered it.
#[test]
fn a_hook_that_marks_a_surface_blocking_is_journalled_blocking() {
    let world = World::new("hook-surface-blocking");
    hooked_run(
        &world,
        "hookblocking",
        &[("surfaces", &["set-blocking"], "")],
    );
    world
        .run(&[
            "surface",
            "hookblocking",
            "--kind",
            "finding",
            "--message",
            "the report",
        ])
        .exited(0);
    assert_eq!(
        surfaces_read(&world, "hookblocking")[0]["blocking"],
        true,
        "the queue holds what the hook left"
    );
    assert_eq!(
        world.events_of("hookblocking", "planner-surface-queued")[0]["payload"]["blocking"],
        true,
        "the journal records what was queued"
    );
    world.release("slow.go");
}

/// (d) A hook rerouting an envelope to `commands`: the reconciler applies it, the
/// reply queue holds nothing, and the receipt says changed. A route naming a queue
/// the layout does not declare is refused at the launch.
#[test]
fn a_hook_that_reroutes_an_envelope_to_commands_applies_it_and_delivers_no_verdict() {
    let world = World::new("hook-reroute");
    hooked_run(
        &world,
        "hookreroute",
        &[("replies", &["reroute-as-commands"], ", routes: [commands]")],
    );
    let answered = world.run_with_stdin(
        &["reply", "hookreroute"],
        &json!({"version": 3, "commands": [add("rerouted")]}).to_string(),
    );
    answered.exited(0);
    let receipt = answered.json();
    assert_eq!(receipt["hooked"], "changed", "{receipt}");
    world.until("the rerouted edit to commit", |world| {
        committed(world, "hookreroute", "rerouted")
    });
    assert_eq!(channel_file(&world, "hookreroute", "replies.jsonl"), "");
    assert!(receipt.get("verdict").is_none(), "{receipt}");
    world.release("slow.go");

    let unknown = World::new("hook-reroute-unknown");
    let file = hooked(
        &unknown,
        &[("replies", &["keep"], ", routes: [surfaces, nowhere]")],
    );
    let path = plan(&unknown, "hookunknown");
    unknown
        .run(&["start", &path, "--bus-config", &file, "--detach"])
        .exited(REFUSED)
        .err_has("hooks[0].routes[1]");
}

/// (e) A hook dropping the whole envelope commits nothing and appends nothing, the
/// bus's drop log holds it, and the receipt is a `dropped` receipt at exit 0.
#[test]
fn a_hook_that_drops_the_envelope_applies_nothing_and_says_dropped() {
    let world = World::new("hook-drop");
    hooked_run(&world, "hookdrop", &[("replies", &["drop"], "")]);
    let answered = world.run_with_stdin(
        &["reply", "hookdrop"],
        &json!({"version": 3, "completion": false, "reason": "ruled",
                "commands": [add("gone")]})
        .to_string(),
    );
    answered.exited(0);
    let receipt = answered.json();
    assert_eq!(receipt["state"], "dropped", "{receipt}");
    assert_eq!(receipt["reply"], 0, "{receipt}");
    assert_eq!(receipt["verdict"], "dropped", "{receipt}");
    assert_eq!(receipt["commands"], "dropped", "{receipt}");
    assert!(receipt.get("hooked").is_none(), "{receipt}");
    assert!(!committed(&world, "hookdrop", "gone"));
    assert_eq!(channel_file(&world, "hookdrop", "commands.jsonl"), "");
    assert_eq!(channel_file(&world, "hookdrop", "replies.jsonl"), "");
    let dropped = queued_lines(&world, "hookdrop", "replies-dropped.jsonl");
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert_eq!(dropped[0]["message"]["reason"], "ruled", "{dropped:?}");
    assert!(world.events_of("hookdrop", "planner-replied").is_empty());
    world.release("slow.go");
}

/// (f) A hook's refusal is refused in its own words, wrapped as a validator's are,
/// and a hook that cannot decide is refused too, with nothing queued either way.
#[test]
fn a_hook_that_refuses_or_cannot_decide_refuses_the_reply_in_its_own_words() {
    let world = World::new("hook-refuse");
    let reason = "this ruling names no evidence the run holds";
    hooked_run(
        &world,
        "hookrefuse",
        &[("replies", &["refuse", reason], "")],
    );
    let edit = json!({"version": 3, "commands": [add("never")]}).to_string();
    for reply in [r#"{"completion":false,"reason":"carry on"}"#, edit.as_str()] {
        world
            .run_with_stdin(&["reply", "hookrefuse"], reply)
            .exited(REFUSED)
            .err_has(reason);
    }
    for file in ["replies.jsonl", "commands.jsonl"] {
        assert_eq!(channel_file(&world, "hookrefuse", file), "", "{file}");
    }
    world.release("slow.go");

    let unsure = World::new("hook-unjudged");
    hooked_run(&unsure, "hookunsure", &[("replies", &["unjudged"], "")]);
    unsure
        .run_with_stdin(
            &["reply", "hookunsure"],
            r#"{"completion":false,"reason":"carry on"}"#,
        )
        .exited(REFUSED)
        .err_has("exited 3");
    assert_eq!(channel_file(&unsure, "hookunsure", "replies.jsonl"), "");
    unsure.release("slow.go");
}

/// (g) A hook on `commands` changing the edits a reply carries: the journal records
/// the changed edit and not the original.
#[test]
fn a_hook_on_commands_changes_the_edits_the_journal_records() {
    let world = World::new("hook-commands");
    hooked_run(
        &world,
        "hookcommands",
        &[("commands", &["rewrite-message", "the changed finding"], "")],
    );
    let answered = world.run_with_stdin(
        &["reply", "hookcommands"],
        &json!({"version": 3, "commands": [
            {"op": "finding", "message": "the original finding"}
        ]})
        .to_string(),
    );
    answered.exited(0);
    assert_eq!(answered.json()["hooked"], "changed", "{}", answered.stdout);
    world.until("the changed finding to be recorded", |world| {
        world
            .events_of("hookcommands", "edit-committed")
            .iter()
            .chain(world.events_of("hookcommands", "command-accepted").iter())
            .any(|event| event["payload"]["command"]["message"] == "the changed finding")
    });
    let journal =
        std::fs::read_to_string(world.run_file("hookcommands", "events.jsonl")).unwrap_or_default();
    assert!(
        !journal.contains("the original finding"),
        "the original edit reached the journal: {journal}"
    );
    world.release("slow.go");
}

/// A hook on `surfaces` dropping a question: `ask` returns at once on its own with
/// the bus's `dropped` answer naming the queue and the hook at exit 1, the question
/// is never queued, and the journal records the drop and the hook.
#[test]
fn a_question_a_hook_drops_answers_dropped_at_once_and_is_journalled() {
    let world = World::new("hook-ask-drop");
    hooked_run(
        &world,
        "hookaskdrop",
        &[(
            "surfaces",
            &["drop"],
            ", when: {field: kind, equals: planner-question}",
        )],
    );
    let asked = world
        .cmd(&["ask", "may I proceed?"])
        .env("ONEPIPELINE_RUN_ID", "hookaskdrop")
        .output()
        .expect("the binary starts");
    assert_eq!(asked.status.code(), Some(1), "{asked:?}");
    let answer: Value = serde_json::from_slice(&asked.stdout).expect("ask answers one JSON object");
    assert_eq!(answer["answer"], "dropped", "{answer}");
    assert_eq!(answer["queue"], "surfaces", "{answer}");
    assert_eq!(answer["hook"]["index"], 0, "{answer}");
    assert!(
        answer["hook"]["command"][0]
            .as_str()
            .is_some_and(|command| command.contains("bus-hook")),
        "{answer}"
    );
    assert!(!channel_file(&world, "hookaskdrop", "surfaces.jsonl").contains("may I proceed?"));
    assert!(
        channel_file(&world, "hookaskdrop", "surfaces-dropped.jsonl").contains("may I proceed?")
    );
    let recorded = world.events_of("hookaskdrop", "planner-surface-dropped");
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0]["payload"]["hook"]["index"], 0, "{recorded:?}");
    assert_eq!(
        recorded[0]["payload"]["message"], "may I proceed?",
        "{recorded:?}"
    );

    assert!(
        recorded[0]["payload"]["correlation"].is_string(),
        "the journal keeps the question's correlation: {recorded:?}"
    );

    // Other surfaces pass: the hook's `when` matched the question alone.
    world
        .run(&[
            "surface",
            "hookaskdrop",
            "--kind",
            "finding",
            "--message",
            "still reported",
        ])
        .exited(0)
        .out_has("\"queued\"");
    assert!(channel_file(&world, "hookaskdrop", "surfaces.jsonl").contains("still reported"));
    world.release("slow.go");
}

/// A configuration declaring `hooks` is kept whole in the launch record, and
/// `status` and `adopt` read that record.
#[test]
fn a_bus_config_declaring_hooks_is_kept_and_read_by_status_and_adopt() {
    let world = World::new("hook-recorded");
    hooked_run(&world, "hookrecorded", &[("surfaces", &["keep"], "")]);
    let launch: Value = serde_json::from_str(
        &std::fs::read_to_string(world.run_file("hookrecorded", "launch.json"))
            .expect("the launch record"),
    )
    .expect("the launch record is JSON");
    let declared = &launch["bus_config"]["hooks"][0];
    assert_eq!(declared["on"], "surfaces", "{launch}");
    assert!(
        declared["command"][0]
            .as_str()
            .is_some_and(|command| command.contains("bus-hook")),
        "{launch}"
    );
    assert_eq!(declared["command"][1], "keep", "{launch}");
    world.run(&["status", "hookrecorded"]).exited(0);
    world.release("slow.go");
    world.until("the run to settle", |world| {
        world.run_file("hookrecorded", "result.json").is_file()
    });
    world.run(&["adopt", "hookrecorded"]).settled();
}

/// A surface a hook drops is not queued: the verb says so, the planner reads nothing
/// of it, and the journal records the drop and the hook, as a question's is.
#[test]
fn a_surface_a_hook_drops_is_not_queued_and_the_journal_records_the_hook() {
    let world = World::new("hook-surface-drop");
    hooked_run(&world, "hooksurfacedrop", &[("surfaces", &["drop"], "")]);
    let answered = world.run(&[
        "surface",
        "hooksurfacedrop",
        "--kind",
        "finding",
        "--message",
        "never read",
    ]);
    answered.exited(0).out_has("\"dropped\"");
    assert!(surfaces_read(&world, "hooksurfacedrop").is_empty());
    let recorded = world.events_of("hooksurfacedrop", "planner-surface-dropped");
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(
        recorded[0]["payload"]["message"], "never read",
        "{recorded:?}"
    );
    assert_eq!(recorded[0]["payload"]["hook"]["index"], 0, "{recorded:?}");
    assert!(world
        .events_of("hooksurfacedrop", "planner-surface-queued")
        .is_empty());
    world.release("slow.go");
}

/// A hook on `commands` dropping that half while one on `replies` changes the
/// verdict: the receipt names the verdict delivered and the commands dropped, and
/// the queued reply carries no copy of the dropped edits.
#[test]
fn a_dropped_half_beside_a_changed_half_says_changed_and_keeps_no_copy_of_it() {
    let world = World::new("hook-half-changed");
    hooked_run(
        &world,
        "hookhalf",
        &[
            ("replies", &["set-message", "the changed ruling"], ""),
            ("commands", &["drop"], ""),
        ],
    );
    let answered = world.run_with_stdin(
        &["reply", "hookhalf"],
        &json!({"version": 3, "completion": false, "message": "the original ruling",
                "commands": [{"op": "finding", "message": "dropped"}]})
        .to_string(),
    );
    answered.exited(0);
    let receipt = answered.json();
    assert_eq!(receipt["state"], "delivered", "{receipt}");
    assert_eq!(receipt["verdict"], "delivered", "{receipt}");
    assert_eq!(receipt["commands"], "dropped", "{receipt}");
    assert_eq!(receipt["hooked"], "changed", "{receipt}");
    let replies = channel_file(&world, "hookhalf", "replies.jsonl");
    assert!(replies.contains("the changed ruling"), "{replies}");
    assert!(!replies.contains("dropped"), "{replies}");
    assert_eq!(channel_file(&world, "hookhalf", "commands.jsonl"), "");
    world.release("slow.go");
}

/// A hook taking the verdict half off an envelope and keeping its commands: the edit
/// lands, the receipt says the verdict was dropped, and the asker hears no ruling.
#[test]
fn a_verdict_half_a_hook_drops_beside_applied_commands_reaches_no_asker() {
    let world = World::new("hook-verdict-half");
    hooked_run(
        &world,
        "hookverdicthalf",
        &[("replies", &["strip-verdict"], "")],
    );
    let mut asking = world.cmd(&["ask", "--timeout", "8", "what next?"]);
    asking.env("ONEPIPELINE_RUN_ID", "hookverdicthalf");
    let asking = asking
        .stdin(onepipeline_testfakes::profile::process::Stdio::null())
        .stdout(onepipeline_testfakes::profile::process::Stdio::piped())
        .stderr(onepipeline_testfakes::profile::process::Stdio::piped())
        .spawn()
        .expect("the binary starts");
    world.until("the question to be queued", |world| {
        channel_file(world, "hookverdicthalf", "surfaces.jsonl").contains("what next?")
    });
    let ruling = "the ruling a hook took off";
    let answered = world.run_with_stdin(
        &["reply", "hookverdicthalf"],
        &json!({"version": 3, "completion": false, "message": ruling,
                "commands": [add("kept")]})
        .to_string(),
    );
    answered.exited(0);
    let receipt = answered.json();
    assert_eq!(receipt["verdict"], "dropped", "{receipt}");
    assert_eq!(receipt["commands"], "applied", "{receipt}");
    world.until("the kept edit to commit", |world| {
        committed(world, "hookverdicthalf", "kept")
    });
    assert_eq!(channel_file(&world, "hookverdicthalf", "replies.jsonl"), "");
    let output = asking.wait_with_output().expect("ask returns");
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(!said.contains(ruling), "{said}");
    assert_eq!(
        serde_json::from_str::<Value>(said.trim()).expect("ask answers JSON")["answer"],
        "timeout",
        "{said}"
    );
    world.release("slow.go");
}

/// Mixed halves with a hook on `commands`: the verdict queued beside the edit carries
/// the changed edits, not the originals.
#[test]
fn a_queued_verdict_carries_the_edits_a_commands_hook_changed() {
    let world = World::new("hook-framed");
    hooked_run(
        &world,
        "hookframed",
        &[("commands", &["rewrite-message", "the changed finding"], "")],
    );
    world
        .run_with_stdin(
            &["reply", "hookframed"],
            &json!({"version": 3, "completion": false, "reason": "carry on",
                    "commands": [{"op": "finding", "message": "the original finding"}]})
            .to_string(),
        )
        .exited(0);
    let replies = channel_file(&world, "hookframed", "replies.jsonl");
    assert!(replies.contains("the changed finding"), "{replies}");
    assert!(!replies.contains("the original finding"), "{replies}");
    world.release("slow.go");
}

/// What a hook wrote that this build cannot apply is refused with nothing appended,
/// and a hook cannot hand an envelope to an author the configuration never declared.
#[test]
fn a_hook_leaving_an_unreadable_record_or_an_undeclared_author_is_refused() {
    let world = World::new("hook-unreadable");
    hooked_run(
        &world,
        "hookunreadable",
        &[("commands", &["unreadable-command"], "")],
    );
    let edit = json!({"version": 3, "commands": [add("kept")]}).to_string();
    world
        .run_with_stdin(&["reply", "hookunreadable"], &edit)
        .exited(REFUSED)
        .err_has("a hook left a record this build cannot read");
    assert_eq!(channel_file(&world, "hookunreadable", "commands.jsonl"), "");
    world.release("slow.go");

    let author = World::new("hook-author");
    hooked_run(
        &author,
        "hookauthor",
        &[("replies", &["set-author", "stranger"], "")],
    );
    author
        .run_with_stdin(
            &["reply", "hookauthor"],
            r#"{"completion":false,"reason":"carry on"}"#,
        )
        .exited(REFUSED)
        .err_has("stranger");
    assert_eq!(channel_file(&author, "hookauthor", "replies.jsonl"), "");
    author.release("slow.go");
}

/// A hook rerouting a surface off the queue: the verb says rerouted, the planner reads
/// nothing, and the reply queue holds what the hook wrote.
#[test]
fn a_surface_a_hook_reroutes_is_reported_rerouted_and_lands_where_it_was_sent() {
    let world = World::new("hook-surface-reroute");
    hooked_run(
        &world,
        "hookroutesurface",
        &[("surfaces", &["reroute-as-verdict"], ", routes: [replies]")],
    );
    world
        .run(&[
            "surface",
            "hookroutesurface",
            "--kind",
            "finding",
            "--message",
            "sent elsewhere",
        ])
        .exited(0)
        .out_has("\"rerouted\"");
    assert!(surfaces_read(&world, "hookroutesurface").is_empty());
    assert!(channel_file(&world, "hookroutesurface", "replies.jsonl").contains("rerouted"));
    world.release("slow.go");
}

/// A surface the engine itself raises is decided too: a hook dropping it journals
/// the drop and the hook, and one refusing it journals why, and the run carries on
/// either way.
#[test]
fn an_engine_raised_surface_a_hook_drops_or_refuses_is_journalled_and_the_run_continues() {
    for (name, argv, cause) in [
        ("hook-engine-drop", vec!["drop"], "hook"),
        (
            "hook-engine-refuse",
            vec!["refuse", "not this one"],
            "refused",
        ),
    ] {
        let world = World::new(name);
        let run = name.replace('-', "");
        let argv = argv.clone();
        hooked_run(&world, &run, &[("surfaces", &argv, "")]);
        world
            .run_with_stdin(
                &["reply", &run],
                &json!({"version": 3, "commands": [
                    {"op": "finding", "message": "the fixture is slow"}
                ]})
                .to_string(),
            )
            .exited(0);
        world.until(
            "the engine's report to be journalled as kept off",
            |world| {
                world
                    .events_of(&run, "planner-surface-dropped")
                    .iter()
                    .any(|event| event["payload"]["kind"] == "finding")
            },
        );
        let recorded = world.events_of(&run, "planner-surface-dropped");
        assert!(
            recorded
                .iter()
                .any(|event| !event["payload"][cause].is_null()),
            "{recorded:?}"
        );
        world.run(&["status", &run]).exited(0);
        assert!(surfaces_read(&world, &run).is_empty());
        world.release("slow.go");
    }
}

/// A hook dropping the commands half of a mixed envelope leaves no copy of those
/// edits in the verdict it does queue.
#[test]
fn a_dropped_commands_half_leaves_no_copy_in_the_queued_verdict() {
    let world = World::new("hook-half-only");
    hooked_run(&world, "hookhalfonly", &[("commands", &["drop"], "")]);
    let answered = world.run_with_stdin(
        &["reply", "hookhalfonly"],
        &json!({"version": 3, "completion": false, "reason": "carry on",
                "commands": [{"op": "finding", "message": "never applied"}]})
        .to_string(),
    );
    answered.exited(0);
    let receipt = answered.json();
    assert_eq!(receipt["commands"], "dropped", "{receipt}");
    assert!(receipt.get("hooked").is_none(), "{receipt}");
    let replies = channel_file(&world, "hookhalfonly", "replies.jsonl");
    assert!(replies.contains("carry on"), "{replies}");
    assert!(!replies.contains("never applied"), "{replies}");
    world.release("slow.go");
}

/// A hook that adds a completion an author may not declare is refused, before the
/// reply is queued, as the same verdict sent directly is.
#[test]
fn a_hook_cannot_add_a_completion_its_author_may_not_declare() {
    let world = World::new("hook-completion");
    let command = double("bus-hook").to_string_lossy().into_owned();
    let file = configuration(
        &world,
        "onemessagebus.yaml",
        &format!(
            "version: 1\ntransport: {{kind: local}}\nauthors:\n  sentinel: {{capabilities: \
             [finding]}}\nhooks:\n  - {{on: replies, command: [{command:?}, declare-complete]}}\n"
        ),
    );
    let path = plan(&world, "hookcomplete");
    launched(&world, &path, "hookcomplete", &["--bus-config", &file]);
    world
        .run_with_stdin(
            &["reply", "hookcomplete"],
            r#"{"author":"sentinel","reason":"looks done"}"#,
        )
        .exited(REFUSED)
        .err_has("not something the sentinel may do");
    assert_eq!(channel_file(&world, "hookcomplete", "replies.jsonl"), "");
    world.release("slow.go");
}

/// A reply a hook reroutes to `surfaces`: the planner reads the surface, nothing was
/// delivered as a verdict, and the receipt says the envelope was changed.
#[test]
fn a_reply_a_hook_reroutes_to_surfaces_reaches_the_planner_and_delivers_no_verdict() {
    let world = World::new("hook-reply-surface");
    hooked_run(
        &world,
        "hookreplysurface",
        &[("replies", &["reroute-as-surface"], ", routes: [surfaces]")],
    );
    let answered = world.run_with_stdin(
        &["reply", "hookreplysurface"],
        r#"{"completion":false,"reason":"sent onward"}"#,
    );
    answered.exited(0);
    let receipt = answered.json();
    assert_eq!(receipt["hooked"], "changed", "{receipt}");
    assert!(receipt.get("verdict").is_none(), "{receipt}");
    assert_eq!(
        channel_file(&world, "hookreplysurface", "replies.jsonl"),
        ""
    );
    assert!(surfaces_read(&world, "hookreplysurface")
        .iter()
        .any(|surface| surface["message"] == "rerouted to the planner"));
    world.release("slow.go");
}

/// An engine-raised surface a hook reroutes lands where it was sent, is not counted
/// as a queued surface, and the run carries on.
#[test]
fn an_engine_raised_surface_a_hook_reroutes_lands_elsewhere_and_is_not_counted_queued() {
    let world = World::new("hook-engine-route");
    hooked_run(
        &world,
        "hookengineroute",
        &[("surfaces", &["reroute-as-verdict"], ", routes: [replies]")],
    );
    world
        .run_with_stdin(
            &["reply", "hookengineroute"],
            &json!({"version": 3, "commands": [
                {"op": "finding", "message": "the fixture is slow"}
            ]})
            .to_string(),
        )
        .exited(0);
    world.until("the engine's report to reach the reply queue", |world| {
        channel_file(world, "hookengineroute", "replies.jsonl").contains("rerouted")
    });
    assert!(world
        .events_of("hookengineroute", "planner-surface-queued")
        .iter()
        .all(|event| event["payload"]["kind"] != "finding"));
    world.run(&["status", "hookengineroute"]).exited(0);
    assert!(surfaces_read(&world, "hookengineroute").is_empty());
    world.release("slow.go");
}

/// A hook's refusal of a surface a verb raises reaches its caller in the hook's
/// words, with nothing queued.
#[test]
fn a_surface_a_hook_refuses_is_refused_in_its_words_at_the_verb() {
    let world = World::new("hook-surface-refuse");
    hooked_run(
        &world,
        "hooksurfacerefuse",
        &[("surfaces", &["refuse", "no reports today"], "")],
    );
    world
        .run(&[
            "surface",
            "hooksurfacerefuse",
            "--kind",
            "finding",
            "--message",
            "a report",
        ])
        .exited(REFUSED)
        .err_has("no reports today");
    assert!(channel_file(&world, "hooksurfacerefuse", "surfaces.jsonl").is_empty());
    world.release("slow.go");
}

/// With nothing driving the run the reply applies itself, and still applies what
/// the hooks left of the envelope rather than what was sent.
#[test]
fn an_undriven_run_applies_what_the_hooks_left_of_the_envelope() {
    const RUN: &str = "hook-undriven";
    let world = World::new(RUN);
    let file = hooked(&world, &[("replies", &["keep-first-command"], "")]);
    let path = world.plan(RUN, &plan_of(RUN, vec![human("approve", &[])]));
    world
        .run(&["start", &path, "--attach", "--bus-config", &file])
        .exited(0)
        .out_has("\"settlement\":\"awaiting-planner\"");
    let applied = world.run_with_stdin(
        &["reply", RUN],
        &json!({"version": 3, "commands": [
            {"op": "add", "node": agent("kept", &["approve"])},
            {"op": "add", "node": agent("removed", &["approve"])}
        ]})
        .to_string(),
    );
    applied.exited(0);
    let receipt = applied.json();
    assert_eq!(receipt["commands"], "applied", "{receipt}");
    assert_eq!(receipt["hooked"], "changed", "{receipt}");
    assert!(committed(&world, RUN, "kept"));
    assert!(!committed(&world, RUN, "removed"));
}

/// A reply naming a question a hook reroutes away from the reply queue: the question
/// stands unanswered, the journal records no verdict, and the receipt says changed.
#[test]
fn a_correlated_reply_a_hook_reroutes_journals_no_verdict() {
    let world = World::new("hook-correlated");
    hooked_run(
        &world,
        "hookcorrelated",
        &[("replies", &["reroute-as-surface"], ", routes: [surfaces]")],
    );
    let asked = world
        .cmd(&["ask", "--timeout", "1", "may I proceed?"])
        .env("ONEPIPELINE_RUN_ID", "hookcorrelated")
        .output()
        .expect("the binary starts");
    assert_eq!(asked.status.code(), Some(1), "{asked:?}");
    let correlation: Value = serde_json::from_slice(&asked.stdout).expect("one JSON answer");
    let correlation = correlation["correlation"]
        .as_str()
        .expect("the question was raised")
        .to_owned();
    let answered = world.run_with_stdin(
        &["reply", "hookcorrelated", "--correlation", &correlation],
        r#"{"completion":false,"reason":"sent onward"}"#,
    );
    answered.exited(0);
    let receipt = answered.json();
    assert_eq!(receipt["hooked"], "changed", "{receipt}");
    assert!(receipt.get("verdict").is_none(), "{receipt}");
    assert!(world
        .events_of("hookcorrelated", "planner-replied")
        .is_empty());
    assert_eq!(channel_file(&world, "hookcorrelated", "replies.jsonl"), "");
    world.release("slow.go");
}

/// A surface the engine raises that a hook rewrites reaches the planner as rewritten,
/// and the journal's record of it says what was queued, not what was offered.
#[test]
fn an_engine_raised_surface_a_hook_rewrites_is_journalled_as_rewritten() {
    let world = World::new("hook-engine-rewrite");
    hooked_run(
        &world,
        "hookenginerewrite",
        &[("surfaces", &["set-message", "the rewritten report"], "")],
    );
    world
        .run_with_stdin(
            &["reply", "hookenginerewrite"],
            &json!({"version": 3, "commands": [
                {"op": "finding", "message": "the original report"}
            ]})
            .to_string(),
        )
        .exited(0);
    world.until("the rewritten report to be journalled", |world| {
        world
            .events_of("hookenginerewrite", "planner-surface-queued")
            .iter()
            .any(|event| event["payload"]["message"] == "the rewritten report")
    });
    assert!(surfaces_read(&world, "hookenginerewrite")
        .iter()
        .any(|surface| surface["message"] == "the rewritten report"));
    world.release("slow.go");
}

/// Amend `spare` on `run`, so the run holds a manager's ruling to decline, and
/// answer the reference it is declined by.
fn a_ruling_to_decline(world: &World, run: &str) -> String {
    let replied = world.run_with_stdin(
        &["reply", run],
        &json!({"version": 4, "commands": [{"op": "amend", "id": "spare",
            "text": "Keep the diff under fifty lines."}]})
        .to_string(),
    );
    replied.exited(0);
    let id = replied.json()["reply"].as_u64().expect("the envelope's id");
    format!("c{id}.0")
}

/// A declined ruling meets the channel's hooks as any surface `surface` raises
/// does: a hook rewriting its text queues the rewritten text with the typed
/// record intact, a hook dropping it queues nothing and journals the drop, and a
/// hook refusing it refuses the decline in its words, with nothing appended.
#[test]
fn a_declined_ruling_meets_the_channels_hooks_as_a_surface_does() {
    let world = World::new("hook-decline-rewrite");
    hooked_run(
        &world,
        "hookdeclinerewrite",
        &[("surfaces", &["set-message", "the changed decline"], "")],
    );
    let reference = a_ruling_to_decline(&world, "hookdeclinerewrite");
    world
        .run_with_stdin(
            &[
                "decline-ruling",
                "hookdeclinerewrite",
                "--ruling",
                &reference,
            ],
            "The user said otherwise.",
        )
        .exited(0)
        .out_has("\"queued\"");
    let read = surfaces_read(&world, "hookdeclinerewrite");
    let declined = read
        .iter()
        .find(|surface| surface["kind"] == "declined-ruling")
        .unwrap_or_else(|| panic!("no decline was queued: {read:?}"));
    assert_eq!(declined["message"], "the changed decline", "{declined}");
    assert_eq!(declined["declined_ruling"]["ruling"], json!(reference));
    assert_eq!(
        declined["declined_ruling"]["reason"],
        "The user said otherwise."
    );
    assert_eq!(
        world
            .events_of("hookdeclinerewrite", "planner-surface-queued")
            .last()
            .expect("the decline is journalled")["payload"]["message"],
        "the changed decline"
    );
    world.release("slow.go");

    let world = World::new("hook-decline-drop");
    hooked_run(&world, "hookdeclinedrop", &[("surfaces", &["drop"], "")]);
    let reference = a_ruling_to_decline(&world, "hookdeclinedrop");
    world
        .run_with_stdin(
            &["decline-ruling", "hookdeclinedrop", "--ruling", &reference],
            "The user said otherwise.",
        )
        .exited(0)
        .out_has("\"dropped\"");
    assert!(surfaces_read(&world, "hookdeclinedrop").is_empty());
    let dropped = world.events_of("hookdeclinedrop", "planner-surface-dropped");
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert_eq!(
        dropped[0]["payload"]["kind"], "declined-ruling",
        "{dropped:?}"
    );
    assert_eq!(dropped[0]["payload"]["hook"]["index"], 0, "{dropped:?}");
    world.release("slow.go");

    let world = World::new("hook-decline-refuse");
    hooked_run(
        &world,
        "hookdeclinerefuse",
        &[("surfaces", &["refuse", "no declines today"], "")],
    );
    let reference = a_ruling_to_decline(&world, "hookdeclinerefuse");
    world
        .run_with_stdin(
            &[
                "decline-ruling",
                "hookdeclinerefuse",
                "--ruling",
                &reference,
            ],
            "The user said otherwise.",
        )
        .exited(REFUSED)
        .err_has("no declines today");
    assert!(channel_file(&world, "hookdeclinerefuse", "surfaces.jsonl").is_empty());
    world.release("slow.go");
}
