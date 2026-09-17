//! A run's channel under the `onemessagebus` configuration its launch names.
//!
//! `start --bus-config` reads an `onemessagebus.yaml` at the launch — or a launch
//! config names it — refuses there, before any run exists, a configuration the
//! run's channel could not be kept under, and keeps the one it accepts in the
//! launch record. What these journeys hold is that the configuration a launch
//! accepts is the one the run enforces: the grants its `authors` block narrowed,
//! the validators its `validators` block names, and the reply window its
//! `codecs.onejudge` block sets — each against the same plan launched without the
//! file, so what is enforced is the configuration's doing and not the profile's.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The validator is not a substitution either: it is a real command a host
// names, and this suite supplies a real one. `harness.rs` carries the same suppression and
// the full rationale.

use std::io::Write;

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
    launched(
        &world,
        &path,
        "busrecorded-2",
        &["--launch-config", &launch_config],
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
    // `reply` to prove the driver's independent trust-boundary check; appending
    // is the real local bus transport boundary.
    // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] the
    // envelopes are spelled by hand because bypassing `reply` is the point, and
    // the gate on their spelling is the driver reading them back below: it
    // records each as rejected by *author*, which a record it could not read
    // would never reach.
    for payload in [
        json!({"id": 900, "author": "stranger", "commands": [
            {"op": "add", "node": agent("also-never", &[])}
        ]}),
        json!({"id": 901, "author": AUTHOR, "commands": [
            {"op": "complete", "reason": "looks done"}
        ]}),
    ] {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(world.run_file(RUN, "channel/commands.jsonl"))
            .expect("the host bus opens the command queue");
        writeln!(file, "{payload}").expect("the host bus appends the envelope");
    }
    // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
    // llmlint: ignore-end[tests_mirror_real_usage]
    world.until("the direct envelopes to be rejected", |world| {
        world.events_of(RUN, "edit-rejected").len() >= 2
    });
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
        .exited(0);
    assert!(
        !world.events_of(RUN, "decision-pending").is_empty(),
        "the run is not parked on its human node: {:?}",
        world.kinds(RUN)
    );

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

/// A validator the configuration names judges what is offered to the reply queue:
/// a reply it refuses is refused with its own words as the reason and queued
/// nowhere — a verdict and an edit envelope alike — while the same reply against
/// the same plan launched without the file is queued.
#[test]
fn a_validator_a_bus_config_names_refuses_a_reply_in_its_own_words_and_queues_it_nowhere() {
    let world = World::new("bus-config-validated");
    let validator = double("bus-validator").to_string_lossy().into_owned();
    let file = configuration(
        &world,
        "onemessagebus.yaml",
        &format!(
            "version: 1\ntransport: {{kind: local}}\nvalidators:\n  \
             - {{on: replies, kind: command, command: [{validator:?}]}}\n"
        ),
    );
    let path = plan(&world, "busvalidated");
    launched(&world, &path, "busvalidated", &["--bus-config", &file]);

    let reason = "this ruling names no evidence the run holds";
    world.script("bus-validator.refuse", reason);
    let verdict = r#"{"completion":false,"reason":"carry on"}"#;
    let edit = json!({"version": 3, "commands": [
        {"op": "finding", "message": "the fixture is slow"}
    ]})
    .to_string();
    for reply in [verdict, edit.as_str()] {
        world
            .run_with_stdin(&["reply", "busvalidated"], reply)
            .exited(REFUSED)
            .err_has(reason);
    }
    for file in ["replies.jsonl", "commands.jsonl"] {
        assert_eq!(
            channel_file(&world, "busvalidated", file),
            "",
            "a refused reply reached {file}"
        );
    }
    let judged: Vec<Value> = std::fs::read_to_string(world.fakes.join("bus-validator.jsonl"))
        .expect("the validator was run")
        .lines()
        .map(|line| serde_json::from_str(line).expect("the validator records JSON"))
        .collect();
    assert_eq!(judged.len(), 2, "{judged:?}");
    assert_eq!(judged[0]["queue"], json!("replies"), "{judged:?}");
    assert_eq!(
        judged[0]["message"],
        serde_json::from_str::<Value>(verdict).expect("the verdict is JSON"),
        "the validator was handed another document than the reply"
    );

    // A validator that gives no verdict at all refuses the reply as well, rather
    // than letting it through judged by nothing.
    std::fs::remove_file(world.fakes.join("bus-validator.refuse")).expect("the refusal is lifted");
    world.script("bus-validator.unjudged", "");
    world
        .run_with_stdin(&["reply", "busvalidated"], verdict)
        .exited(REFUSED);
    assert_eq!(
        channel_file(&world, "busvalidated", "replies.jsonl"),
        "",
        "a reply no validator gave a verdict on was queued"
    );
    std::fs::remove_file(world.fakes.join("bus-validator.unjudged"))
        .expect("the missing verdict is lifted");

    launched(&world, &path, "busvalidated-2", &[]);
    world
        .run_with_stdin(&["reply", "busvalidated-2"], verdict)
        .exited(0);
    assert!(
        channel_file(&world, "busvalidated-2", "replies.jsonl").contains("carry on"),
        "the reply was not queued where no validator judges it"
    );
    world.release("slow.go");
}

/// A validator the configuration names on the commands queue judges the edits a
/// reply carries: an envelope whose commands it refuses is refused in its words
/// with nothing queued, while a verdict carrying no commands is never offered to
/// it.
#[test]
fn a_validator_on_the_commands_queue_judges_the_edits_a_reply_carries() {
    let world = World::new("bus-config-commands");
    let validator = double("bus-validator").to_string_lossy().into_owned();
    let file = configuration(
        &world,
        "onemessagebus.yaml",
        &format!(
            "version: 1\ntransport: {{kind: local}}\nvalidators:\n  \
             - {{on: commands, kind: command, command: [{validator:?}]}}\n"
        ),
    );
    let path = plan(&world, "buscommands");
    launched(&world, &path, "buscommands", &["--bus-config", &file]);
    let judged = || -> Vec<Value> {
        std::fs::read_to_string(world.fakes.join("bus-validator.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("the validator records JSON"))
            .collect()
    };

    let reason = "an edit has to name the node it corrects";
    world.script("bus-validator.refuse", reason);
    let edit = json!({"version": 3, "commands": [
        {"op": "finding", "message": "the fixture is slow"}
    ]})
    .to_string();
    world
        .run_with_stdin(&["reply", "buscommands"], &edit)
        .exited(REFUSED)
        .err_has(reason);
    assert_eq!(
        channel_file(&world, "buscommands", "commands.jsonl"),
        "",
        "an edit the commands validator refused was queued"
    );
    let offered = judged();
    assert!(
        !offered.is_empty()
            && offered
                .iter()
                .all(|record| record["queue"] == json!("commands")),
        "the edit was not offered to the commands queue's validator: {offered:?}"
    );

    world
        .run_with_stdin(
            &["reply", "buscommands"],
            r#"{"completion":false,"reason":"carry on"}"#,
        )
        .exited(0);
    assert!(
        channel_file(&world, "buscommands", "replies.jsonl").contains("carry on"),
        "a verdict carrying no commands was not queued"
    );
    assert_eq!(
        judged().len(),
        offered.len(),
        "a verdict carrying no commands was offered to the commands queue's validator"
    );
    world.release("slow.go");
}
