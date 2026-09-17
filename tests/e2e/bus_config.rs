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

use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{agent, double, ended, plan_of, World, REFUSED};

/// A configuration narrowing the monitor to `retry` alone, taking away the
/// `cancel` the profile grants it beside `retry`.
const WITHOUT_CANCEL: &str = "version: 1\n\
                              transport: {kind: local}\n\
                              profile: planner-channel\n\
                              authors:\n  monitor: {capabilities: [retry]}\n";

/// The part of an `onejudge` codec block the bus requires of every codec and
/// `channel serve` reads nothing of.
///
/// Since `onemessagebus` 0.6 a codec block describes a protocol the bus's own
/// `serve` runs — the frame field it `select`s on and the `frames` it binds —
/// and a block naming neither does not parse. `channel serve` is not that
/// loop: it reads the block's `asker_env`, `session_env` and
/// `reply_window_seconds` and answers every frame itself, so what these
/// journeys put here is the smallest thing the bus accepts, on a frame no
/// journey ever sends.
const CODEC_FRAMES: &str = "select: kind
    frames:
                                  never-sent: {schema: agent.planner-surface@1,                             bindings: [{do: answer, response: {}}]}
";

/// An `onejudge` codec block setting `keys`, over [`CODEC_FRAMES`].
fn onejudge_codec(keys: &str) -> String {
    format!("version: 1\ntransport: {{kind: local}}\ncodecs:\n  onejudge:\n    {keys}\n    {CODEC_FRAMES}")
}

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

fn channel_file(world: &World, run: &str, file: &str) -> String {
    std::fs::read_to_string(world.run_file(run, &format!("channel/{file}"))).unwrap_or_default()
}

fn from_the_monitor(commands: Value) -> String {
    json!({"version": 3, "author": "monitor", "commands": commands}).to_string()
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

/// A configuration may narrow an author's grants and never widen them.
#[test]
fn a_bus_config_widening_the_monitors_grants_is_refused_before_a_run_exists() {
    let world = World::new("bus-config-widened");
    refused_before_a_run_exists(
        &world,
        "version: 1\ntransport: {kind: local}\nauthors:\n  monitor: {capabilities: [retry, drop]}\n",
        &["authors.monitor.capabilities", "`drop`"],
    );
}

/// Every other part of a run's channel this crate decides for itself is refused
/// at the launch, naming the key and its value, before any run exists: a
/// transport option the local transport does not take, queues of the
/// configuration's own, a codec `channel serve` does not speak, and an
/// `onejudge` codec pointed at another queue or at a variable `channel serve`
/// does not read its node from. The `run_env` an earlier bus let a codec name
/// is no key of one since `onemessagebus` 0.6, so it is refused by the bus's
/// own reading — as an unknown field, naming the key — before this crate reads
/// the block at all.
#[test]
fn a_bus_config_moving_what_the_channel_decides_for_itself_is_refused_before_a_run_exists() {
    let world = World::new("bus-config-corners");
    for (body, named) in [
        (
            "version: 1\ntransport: {kind: local, poll: fast}\n".to_owned(),
            ["transport.poll", "`fast`"],
        ),
        (
            "version: 1\ntransport: {kind: local}\nqueues:\n  surfaces: {}\n".to_owned(),
            ["queues.surfaces", "leave `queues` out"],
        ),
        (
            format!(
                "version: 1\ntransport: {{kind: local}}\ncodecs:\n  another:\n    {CODEC_FRAMES}"
            ),
            ["codecs.another", "`onejudge`"],
        ),
        (
            onejudge_codec("queue: replies"),
            ["codecs.onejudge.queue", "`replies`"],
        ),
        (
            onejudge_codec("run_env: HOST_RUN"),
            ["codecs.onejudge", "unknown field `run_env`"],
        ),
        (
            onejudge_codec("about_env: HOST_ABOUT"),
            ["codecs.onejudge.about_env", "`HOST_ABOUT`"],
        ),
    ] {
        refused_before_a_run_exists(&world, &body, &named);
    }
}

/// A validator the configuration names on the surfaces queue judges each
/// question `channel serve` raises: one it refuses is refused in its words and
/// queues nothing, and one it passes reaches the planner.
#[test]
fn a_validator_on_the_surfaces_queue_judges_the_questions_channel_serve_raises() {
    let world = World::new("bus-config-surfaces");
    let validator = double("bus-validator").to_string_lossy().into_owned();
    let file = configuration(
        &world,
        "onemessagebus.yaml",
        &format!(
            "version: 1\ntransport: {{kind: local}}\nvalidators:\n  \
             - {{on: surfaces, kind: command, command: [{validator:?}]}}\n"
        ),
    );
    let path = plan(&world, "bussurfaces");
    launched(&world, &path, "bussurfaces", &["--bus-config", &file]);
    let served = || {
        let mut serving = world.cmd(&["channel", "serve", "bussurfaces"]);
        // Nobody answers a question that is raised, and the wait is not under test.
        serving.env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1");
        serving
    };

    let reason = "a question has to name the node it is about";
    world.script("bus-validator.refuse", reason);
    let refused = "refused before it is raised";
    world
        .run_with_stdin_on(
            served(),
            &format!(r#"{{"kind":"blocker","message":"{refused}"}}"#),
        )
        .exited(REFUSED)
        .err_has(reason);
    assert!(
        !channel_file(&world, "bussurfaces", "surfaces.jsonl").contains(refused),
        "a question the validator refused was queued"
    );
    let judged: Vec<Value> = std::fs::read_to_string(world.fakes.join("bus-validator.jsonl"))
        .expect("the validator was run")
        .lines()
        .map(|line| serde_json::from_str(line).expect("the validator records JSON"))
        .collect();
    assert!(
        judged
            .iter()
            .any(|record| record["queue"] == json!("surfaces")
                && record["message"]["message"] == json!(refused)),
        "the validator was not offered the question on the surfaces queue: {judged:?}"
    );

    std::fs::remove_file(world.fakes.join("bus-validator.refuse")).expect("the refusal is lifted");
    let passed = "passed and raised";
    world
        .run_with_stdin_on(
            served(),
            &format!(r#"{{"kind":"blocker","message":"{passed}"}}"#),
        )
        .exited(0)
        .out_has("\"answer\":\"timeout\"");
    assert!(
        channel_file(&world, "bussurfaces", "surfaces.jsonl").contains(passed),
        "a question the validator passed never reached the planner"
    );
    world.release("slow.go");
}

/// `channel serve` reads its asker and its session bound from the variables the
/// configuration's `codecs.onejudge` block names rather than its own: the
/// question is raised under the asker the named variable holds, and a bound the
/// named variable holds that cannot be read is refused naming that variable.
#[test]
fn channel_serve_reads_its_asker_and_bound_from_the_variables_the_codec_names() {
    let world = World::new("bus-config-codec-env");
    let file = configuration(
        &world,
        "onemessagebus.yaml",
        &onejudge_codec("asker_env: HOST_ASKER\n    session_env: HOST_SESSION"),
    );
    let path = plan(&world, "buscodecenv");
    launched(&world, &path, "buscodecenv", &["--bus-config", &file]);

    let question = "asked under the asker the codec names";
    let mut serving = world.cmd(&["channel", "serve", "buscodecenv"]);
    serving
        .env("HOST_ASKER", "host-named-asker")
        .env("ONEPIPELINE_CHANNEL_ASKER", "the-default-variables-asker")
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "1");
    world
        .run_with_stdin_on(
            serving,
            &format!(r#"{{"kind":"blocker","message":"{question}"}}"#),
        )
        .exited(0);
    let queued = channel_file(&world, "buscodecenv", "surfaces.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a surface record"))
        .find(|record| record["event"] == json!("queued") && record["message"] == json!(question))
        .expect("the question was queued");
    assert_eq!(queued["asker"], json!("host-named-asker"), "{queued}");

    let mut bounded = world.cmd(&["channel", "serve", "buscodecenv"]);
    bounded
        .env("HOST_SESSION", "0")
        .env("ONEPIPELINE_SERVE_SESSION_SECONDS", "30");
    world
        .run_with_stdin_on(bounded, "")
        .exited(REFUSED)
        .err_has("HOST_SESSION");
    world.release("slow.go");
}

/// An author the configuration narrowed is refused what it took away — naming
/// the author, the op and the configuration's reason, with nothing appended to
/// the channel — and keeps what it left, while the same plan launched without the
/// file takes the same envelope.
#[test]
fn an_author_a_bus_config_narrowed_is_refused_what_it_took_away_and_keeps_what_it_left() {
    let world = World::new("bus-config-narrowed");
    let file = configuration(&world, "onemessagebus.yaml", WITHOUT_CANCEL);
    let path = plan(&world, "busnarrowed");
    // Both runs launched from the one plan, and both held, before anything is
    // edited: a retry supersedes the held node, and what a superseded dispatch
    // releases as it ends is the hold every run of this world shares.
    launched(&world, &path, "busnarrowed", &["--bus-config", &file]);
    launched(&world, &path, "busnarrowed-2", &[]);

    let cancel = from_the_monitor(json!([
        {"op": "cancel", "id": "spare", "reason": "a monitor's park"}
    ]));
    world
        .run_with_stdin(&["reply", "busnarrowed"], &cancel)
        .exited(REFUSED)
        .err_has(
            "'cancel' is not an op the monitor may issue: the configuration does not grant it",
        );
    for file in ["replies.jsonl", "commands.jsonl"] {
        assert_eq!(
            channel_file(&world, "busnarrowed", file),
            "",
            "a refused envelope reached {file}"
        );
    }

    // The same plan, launched without the configuration, takes the same cancel.
    world
        .run_with_stdin(&["reply", "busnarrowed-2"], &cancel)
        .exited(0);

    // And what the configuration kept still reaches the reconciler.
    world
        .run_with_stdin(
            &["reply", "busnarrowed"],
            &from_the_monitor(json!([
                {"op": "retry", "id": "slow", "node": agent("slow-2", &[])}
            ])),
        )
        .exited(0);
    world.until("the monitor's retry to be committed", |world| {
        world
            .events_of("busnarrowed", "edit-committed")
            .iter()
            .any(|event| event["payload"]["author"] == json!("monitor"))
    });
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

/// The reply window the configuration's `codecs.onejudge` block sets is how long
/// `channel serve` waits, with no variable overriding it: a question nobody
/// answers is answered with the elapsed wait no earlier than the window and
/// within a bounded margin after it, and one answered inside the window is
/// answered with the ruling.
#[test]
// llmlint: ignore[expensive_tests_stay_behind_their_own_edge] the six seconds are the claim rather than a knob: what this proves is that `channel serve` waits no less than the window a configuration sets, which only a wait of that window can show, and the window is read by the crate's own launch and serve code, so any change under `src/` can cut it short. The one separately-edged project here, `onepipeline-note-journeys`, is edged on conversational cost and would put it where a change to `src/driver.rs` does not run it.
fn the_reply_window_a_bus_config_sets_is_how_long_channel_serve_waits() {
    const WINDOW: u64 = 6;
    let world = World::new("bus-config-window");
    let file = configuration(
        &world,
        "onemessagebus.yaml",
        &onejudge_codec(&format!("reply_window_seconds: {WINDOW}")),
    );
    let path = plan(&world, "buswindow");
    launched(&world, &path, "buswindow", &["--bus-config", &file]);

    let mut serving = world
        .cmd(&["channel", "serve", "buswindow"])
        .env_remove("ONEPIPELINE_REPLY_TIMEOUT_SECONDS")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the channel server starts");
    let mut stdin = serving.stdin.take().expect("stdin is piped");
    let mut lines = BufReader::new(serving.stdout.take().expect("stdout is piped")).lines();
    let mut read = move || -> Value {
        serde_json::from_str(
            &lines
                .next()
                .expect("the server wrote a line")
                .expect("the line reads"),
        )
        .expect("the line is JSON")
    };

    let asked = Instant::now();
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"nobody answers this one"}}"#
    )
    .expect("written");
    stdin.flush().expect("flushed");
    let told = read();
    let waited = asked.elapsed();
    assert_eq!(told["answer"], json!("timeout"), "{told}");
    assert!(
        waited >= Duration::from_secs(WINDOW),
        "the wait was cut short of the configured window: {waited:?}"
    );
    assert!(
        waited < Duration::from_secs(WINDOW + 10),
        "the wait ran on past the configured window: {waited:?}"
    );
    // Ruled on late, by name, so it is not the question a plain verdict binds to.
    let first = told["correlation"]
        .as_str()
        .expect("a correlation")
        .to_owned();
    world
        .run_with_stdin(
            &["reply", "buswindow", "--correlation", &first],
            r#"{"completion":false,"reason":"late, for the first"}"#,
        )
        .exited(0);

    let asked = Instant::now();
    writeln!(
        stdin,
        r#"{{"kind":"blocker","message":"this one is answered"}}"#
    )
    .expect("written");
    stdin.flush().expect("flushed");
    world.until("the second question to reach the planner", |world| {
        world.events_of("buswindow", "planner-surface-queued").len() >= 2
    });
    world
        .run_with_stdin(
            &["reply", "buswindow"],
            r#"{"completion":false,"reason":"in time"}"#,
        )
        .exited(0);
    let ruled = read();
    assert_eq!(ruled["reason"], json!("in time"), "{ruled}");
    assert!(
        asked.elapsed() < Duration::from_secs(WINDOW),
        "the ruling did not end the wait inside the window: {:?}",
        asked.elapsed()
    );

    drop(stdin);
    ended(serving);
    world.release("slow.go");
}
