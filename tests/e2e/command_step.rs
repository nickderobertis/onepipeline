//! `kind: command` nodes, driven end to end against the compiled binary.
//!
//! A command node runs a real program — a shell script this suite writes — from the launch
//! directory, and settles on how it ended. What the command was handed is read off the
//! files that program wrote, and what the run did with it off the run's own journal and
//! `results --json`: never off anything this crate wrote down about which command it picked.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. Each command is a real program the binary spawns. `harness.rs` carries
// the same suppression and the full rationale.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{agent, human, plan_of, posix_script, shell_path, World};

/// A command node running `script`.
fn command(id: &str, deps: &[&str], script: &Path) -> Value {
    json!({
        "id": id,
        "kind": "command",
        "command": [script.display().to_string()],
        "deps": deps,
    })
}

/// A script under the world's root, its body POSIX shell.
fn script(world: &World, name: &str, body: &str) -> PathBuf {
    posix_script(&world.root, name, body)
}

/// A file under the world's root, as a script body names it.
fn at(world: &World, name: &str) -> String {
    shell_path(&world.root.join(name))
}

/// The settlement `results --json` reports for `node`.
fn result(world: &World, run: &str, node: &str) -> Value {
    let read = world.run(&["results", run, "--json"]);
    let reading: Value = serde_json::from_str(&read.stdout)
        .unwrap_or_else(|error| panic!("`results --json` is not JSON ({error}):\n{}", read.stdout));
    reading["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|entry| entry["id"] == node).cloned())
        .unwrap_or_else(|| panic!("`results {run} --json` does not list {node}: {reading}"))
}

/// Where in the run's journal the first record of `kind` for `node` sits.
fn position(world: &World, run: &str, kind: &str, node: &str) -> Option<usize> {
    world
        .journal(run)
        .iter()
        .position(|event| event["kind"] == kind && event["labels"]["node"] == node)
}

fn settled_as(world: &World, run: &str, node: &str, status: &str) -> bool {
    world
        .events_of(run, "node-settled")
        .iter()
        .any(|event| event["labels"]["node"] == node && event["payload"]["status"] == status)
}

fn dispatched(world: &World, run: &str, node: &str) -> bool {
    position(world, run, "node-dispatched", node).is_some()
}

/// The journey the kind exists for: a command runs once the agent node before it has
/// settled, with the run's coordinates in its environment and its output in the run's
/// `commands/` log, and settles `done`; the node behind a human node waits for the
/// attestation.
#[test]
fn a_command_runs_after_its_dependency_with_the_runs_coordinates_and_a_human_still_gates() {
    let world = World::new("command-step-runs");
    let env = at(&world, "command.env");
    let recorder = script(
        &world,
        "record",
        &format!(
            "printf '%s\\n' \"$ONEPIPELINE_RUN_ID\" \"$ONEPIPELINE_NODE_ID\" \
             \"$ONEPIPELINE_RUN_ROOT\" \"$ONEPIPELINE_PLAN\" > '{env}'\n\
             echo 'hello from the command'\necho 'a warning on stderr' >&2"
        ),
    );
    let project = world.plan(
        "commandruns",
        &plan_of(
            "commandruns",
            vec![
                agent("build", &[]),
                command("record", &["build"], &recorder),
                human("approve", &["record"]),
                agent("ship", &["approve"]),
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    let run = "commandruns";
    // Paused on the person, with its driver gone: the state an attestation and an
    // adoption are made from.
    world.until("the run to pause on the approval", |world| {
        settled_as(world, run, "approve", "waiting")
            && world.run(&["status", run]).stdout.contains("PAUSED")
    });

    assert!(
        settled_as(&world, run, "record", "done"),
        "{}",
        world.dump()
    );
    let built = position(&world, run, "node-settled", "build").expect("build settled");
    let started = position(&world, run, "command-started", "record").expect("record started");
    assert!(
        built < started,
        "the command started before its dependency settled"
    );
    assert!(
        !dispatched(&world, run, "record"),
        "a command node was dispatched to an agent"
    );
    let exited = world.events_of(run, "command-exited");
    assert_eq!(exited.len(), 1, "{exited:?}");
    assert_eq!(exited[0]["payload"]["ending"], "exited");
    assert_eq!(exited[0]["payload"]["exit_code"], 0);

    let told = std::fs::read_to_string(world.root.join("command.env")).expect("it wrote its env");
    let told: Vec<&str> = told.lines().collect();
    assert_eq!(told[0], run);
    assert_eq!(told[1], "record");
    assert!(
        Path::new(told[2]).join("events.jsonl").is_file(),
        "ONEPIPELINE_RUN_ROOT is not the run's own directory: {}",
        told[2]
    );
    assert_eq!(told[3], project);
    let log = std::fs::read_to_string(world.run_file(run, "commands/record.log"))
        .expect("the command's output is kept under the run");
    assert!(log.contains("hello from the command"), "{log}");
    assert!(log.contains("a warning on stderr"), "{log}");
    assert_eq!(result(&world, run, "record")["status"], "done");

    // The node behind the human node waits for the attestation, and only for it.
    assert!(
        !dispatched(&world, run, "ship"),
        "ship ran before the approval"
    );
    world.run(&["attest", run, "approve"]).exited(0);
    // The driver let go once the run paused on the person; the adoption drives the rest.
    world.run(&["adopt", run]).exited(0);
    assert!(settled_as(&world, run, "ship", "done"), "{}", world.dump());
    assert!(
        position(&world, run, "human-attested", "approve")
            < position(&world, run, "node-dispatched", "ship"),
        "ship was dispatched before the approval was attested"
    );
}

/// A command exiting 3 settles `failed` with `command-failed`, its exit status and the tail
/// of its stderr in `results --json`, and nothing behind it is dispatched.
#[test]
fn a_command_exiting_non_zero_fails_with_its_status_and_stderr_and_holds_its_dependents() {
    let world = World::new("command-step-fails");
    let failing = script(
        &world,
        "fail",
        "echo 'checked 4 of 5 things' >&2\necho 'the fifth broke: no config' >&2\nexit 3",
    );
    let project = world.plan(
        "commandfails",
        &plan_of(
            "commandfails",
            vec![command("check", &[], &failing), agent("after", &["check"])],
        ),
    );
    world.run(&["start", &project, "--attach"]).settled();
    let run = "commandfails";
    let settled = result(&world, run, "check");
    assert_eq!(settled["status"], "failed", "{settled}");
    assert_eq!(settled["outcome"], "command-failed", "{settled}");
    let detail = settled["detail"].as_str().expect("a detail");
    assert!(detail.contains("status 3"), "{detail}");
    assert!(detail.contains("the fifth broke: no config"), "{detail}");
    assert!(
        !dispatched(&world, run, "after"),
        "a dependent of a failed command ran"
    );
    assert_ne!(result(&world, run, "after")["status"], "done");
}

/// A command outliving its `timeout_seconds` settles `command-timed-out` with its process
/// gone; one stating no timeout runs as long as it takes, past what a short one allows.
#[test]
fn a_command_past_its_timeout_is_stopped_and_one_with_none_runs_to_its_end() {
    let world = World::new("command-step-timeout");
    let pid = at(&world, "sleeper.pid");
    let sleeper = script(&world, "sleeper", &format!("echo $$ > '{pid}'\nsleep 60"));
    let patient = script(&world, "patient", "sleep 4\necho finished");
    let mut bounded = command("bounded", &[], &sleeper);
    bounded["timeout_seconds"] = json!(2);
    let project = world.plan(
        "commandtimeout",
        &plan_of(
            "commandtimeout",
            vec![bounded, command("unbounded", &[], &patient)],
        ),
    );
    let started = std::time::Instant::now();
    world.run(&["start", &project, "--attach"]).settled();
    let run = "commandtimeout";
    assert!(
        started.elapsed() < std::time::Duration::from_secs(50),
        "the run waited out the command rather than its timeout"
    );
    let timed_out = result(&world, run, "bounded");
    assert_eq!(timed_out["status"], "failed", "{timed_out}");
    assert_eq!(timed_out["outcome"], "command-timed-out", "{timed_out}");
    assert!(
        world
            .events_of(run, "command-exited")
            .iter()
            .any(|event| event["labels"]["node"] == "bounded"
                && event["payload"]["ending"] == "timed-out"),
        "{}",
        world.dump()
    );
    assert_eq!(result(&world, run, "unbounded")["status"], "done");
    #[cfg(unix)]
    {
        let pid = std::fs::read_to_string(world.root.join("sleeper.pid"))
            .expect("the command wrote its pid")
            .trim()
            .to_owned();
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("this host answers about a process it owns")
            .success();
        assert!(!alive, "the command {pid} outlived its timeout");
    }
}

/// The driver is stopped — its process group with it — while a command node runs; `adopt`
/// runs the command again from the start, its second exit settles the node, and the node
/// behind it is dispatched only after that settlement when it exits 0 and not at all when
/// it exits 3.
///
/// Unix only, for the reason the suite's other teardown journeys are: what stops the driver
/// with everything it started is a signal to its process group, which Windows has no
/// spelling of.
#[cfg(unix)]
#[test]
fn a_command_its_driver_died_in_is_run_again_by_adopt_and_settles_on_its_second_exit() {
    for exit in [0, 3] {
        let world = World::new(&format!("command-step-adopt-{exit}"));
        let count = at(&world, "count");
        let go = at(&world, "go");
        let repeated = script(
            &world,
            "repeated",
            &format!(
                "echo run >> '{count}'\nwhile [ ! -f '{go}' ]; do sleep 0.1; done\nexit {exit}"
            ),
        );
        let name = format!("commandadopt{exit}");
        let project = world.plan(
            &name,
            &plan_of(
                &name,
                vec![
                    command("repeat", &[], &repeated),
                    agent("after", &["repeat"]),
                ],
            ),
        );
        world.run(&["start", &project, "--detach"]).exited(0);
        let runs = |world: &World| {
            std::fs::read_to_string(world.root.join("count"))
                .map(|count| count.lines().count())
                .unwrap_or(0)
        };
        world.until("the command to start", |world| runs(world) == 1);

        // The driver and everything in its process group — the command among them.
        let driver = world.run_json(&name, "launch.json")["pid"]
            .as_u64()
            .expect("the launch record names its driver");
        std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{driver}")])
            .status()
            .expect("this host signals a process group it owns");
        world.until("the driver to be gone", |world| {
            world.run(&["status", &name]).stdout.contains("STOPPED")
                || !std::process::Command::new("kill")
                    .args(["-0", &driver.to_string()])
                    .stderr(std::process::Stdio::null())
                    .status()
                    .expect("this host answers about a process it owns")
                    .success()
        });
        assert!(!settled_as(&world, &name, "repeat", "done"));

        std::fs::write(world.root.join("go"), "").expect("the command is released");
        let adopted = world.run(&["adopt", &name]);
        if exit == 0 {
            adopted.exited(0);
        }
        assert_eq!(
            runs(&world),
            2,
            "the adopting driver did not run the command again"
        );
        let exits = world.events_of(&name, "command-exited");
        assert_eq!(exits.len(), 1, "{exits:?}");
        assert_eq!(exits[0]["payload"]["exit_code"], exit);
        if exit == 0 {
            assert!(settled_as(&world, &name, "repeat", "done"));
            assert!(
                position(&world, &name, "node-settled", "repeat")
                    < position(&world, &name, "node-dispatched", "after"),
                "after was dispatched before the command settled"
            );
        } else {
            assert_eq!(result(&world, &name, "repeat")["outcome"], "command-failed");
            assert!(
                !dispatched(&world, &name, "after"),
                "after ran past a failed command"
            );
        }
    }
}

/// A command node is a node like any other at the loader: its rules are refused by name,
/// and the agent's and the person's fields are no command's.
#[test]
fn the_loader_refuses_a_command_node_that_names_nothing_to_run_or_a_field_it_never_reads() {
    let world = World::new("command-step-refused");
    let cases = [
        (
            "nocommand",
            json!({"id": "c", "kind": "command"}),
            "needs a `command`",
        ),
        (
            "blankprogram",
            json!({"id": "c", "kind": "command", "command": [" "]}),
            "names no program",
        ),
        (
            "persona",
            json!({"id": "c", "kind": "command", "command": ["true"], "persona": "engineer"}),
            "`persona` would be read by nothing",
        ),
        (
            "onagent",
            {
                let mut node = agent("c", &[]);
                node["command"] = json!(["true"]);
                node
            },
            "`kind: command` node's",
        ),
    ];
    for (name, node, refusal) in cases {
        let project = world.plan(name, &plan_of(name, vec![node]));
        world
            .run(&["plan", "check", &project])
            .exited(1)
            .out_has(refusal);
    }
}
