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

/// A plan launched under `onepipeline flow run` whose command node runs `onepipeline start`
/// attached on a second project: the inner run's launch record names the outer run's flow
/// and the same launcher session, a `watch --flow` of that flow reports the inner run
/// joining, the inner run's own surfaces arrive on its own channel and none of them on the
/// outer run's, and the command node settles `done` when the inner run settles with every
/// node done — and `failed` in the case whose inner node fails.
///
/// Unix only, as every flow journey is: the flow's program is a shell script.
#[cfg(unix)]
#[test]
fn a_command_that_starts_a_run_in_a_flow_joins_it_and_settles_on_the_inner_runs_ending() {
    use onepipeline_testfakes::profile::process::Stdio;

    for fails in [false, true] {
        let tag = if fails { "fails" } else { "settles" };
        let world = World::new(&format!("command-step-flow-{tag}"))
            .with_env(crate::harness::STALL_AFTER_ENV, "2");
        let inner_name = format!("inner{tag}");
        let outer_name = format!("outer{tag}");
        let flow = format!("lifecycle-{tag}");
        world.script("first.wait", "hold");
        if fails {
            world.script("asker.fail", "");
        } else {
            world.script("asker.wait", "hold");
        }
        let inner = world.plan(
            &inner_name,
            &plan_of(&inner_name, vec![agent("asker", &[])]),
        );
        let launch_inner = json!({
            "id": "launch",
            "kind": "command",
            "command": [
                crate::harness::binary().display().to_string(),
                "start", inner, "--attach",
            ],
            "deps": ["first"],
        });
        let outer = world.plan(
            &outer_name,
            &plan_of(&outer_name, vec![agent("first", &[]), launch_inner]),
        );
        let program = world.root.join("program.sh");
        std::fs::write(
            &program,
            format!("\"$ONEPIPELINE_FAKE_CLI_BIN\" start '{outer}' --attach >/dev/null\n"),
        )
        .expect("the flow's program is written");
        let mut flowing = world
            .cmd(&[
                "flow",
                "run",
                "--name",
                &flow,
                "--",
                "sh",
                program.to_str().expect("a path"),
            ])
            .env_remove("ONEPIPELINE_RUN_ID")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("flow run starts");
        world.until("the outer run to dispatch its first node", |world| {
            dispatched(world, &outer_name, "first")
        });
        assert_eq!(
            world.run_json(&outer_name, "launch.json")["flow"],
            json!(flow),
            "the outer run did not join the flow it was launched in"
        );

        // Armed before the command runs, so what it reports is the inner run joining.
        let watch = world
            .cmd(&[
                "watch",
                "--flow",
                &flow,
                "--timeout",
                "600",
                "--until",
                "run-joined",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the watch starts");
        let leases = world.runs.join(".flows").join(&flow).join("watchers");
        let mine = format!("{}-", watch.id());
        world.until("the flow watch to record itself", |_| {
            std::fs::read_dir(&leases).is_ok_and(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with(&mine))
                })
            })
        });
        world.release("first.go");
        world.until("the inner run to be launched", |world| {
            world.run_file(&inner_name, "launch.json").is_file()
        });
        let inner_record = world.run_json(&inner_name, "launch.json");
        let outer_record = world.run_json(&outer_name, "launch.json");
        assert_eq!(inner_record["flow"], json!(flow), "{inner_record}");
        assert_eq!(
            inner_record["session"], outer_record["session"],
            "{inner_record}"
        );
        assert_eq!(
            inner_record["session"],
            json!(world.session),
            "{inner_record}"
        );
        let output = watch.wait_with_output().expect("the watch returns");
        let reported: Value = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|record| record["watch"] == "return")
            .expect("the watch printed its return record");
        assert_eq!(reported["condition"], "run-joined", "{reported}");
        assert_eq!(reported["run_id"], json!(inner_name), "{reported}");

        if !fails {
            // The inner run's own surface — its held worker reported quiet — is on its own
            // channel, and the outer run's command, which records nothing while it runs, is
            // never reported quiet on the outer one.
            world.until("the inner run to raise its own surface", |world| {
                world
                    .queued_surfaces(&inner_name)
                    .iter()
                    .any(|surface| surface.to_string().contains("quiet-worker"))
            });
            assert!(
                !world
                    .events_of(&outer_name, "quiet-worker")
                    .iter()
                    .any(|event| event["labels"]["node"] == "launch"),
                "the command node was reported quiet"
            );
            assert!(
                !world
                    .queued_surfaces(&outer_name)
                    .iter()
                    .any(|surface| surface.to_string().contains("asker")),
                "the inner run's surface reached the outer run's channel"
            );
            world.release("asker.go");
        }
        world.until("the command node to settle", |world| {
            settled_as(
                world,
                &outer_name,
                "launch",
                if fails { "failed" } else { "done" },
            )
        });
        assert!(settled_as(
            &world,
            &inner_name,
            "asker",
            if fails { "failed" } else { "done" }
        ));
        if fails {
            assert_eq!(
                result(&world, &outer_name, "launch")["outcome"],
                "command-failed"
            );
        }
        let _ = flowing.wait();
    }
}

/// A `cancel` that reaches a running command stops its process tree and settles the node
/// without its command having finished.
#[test]
fn a_cancel_stops_a_running_command() {
    let world = World::new("command-step-cancel");
    let pid = at(&world, "held.pid");
    let held = script(&world, "held", &format!("echo $$ > '{pid}'\nsleep 120"));
    let project = world.plan(
        "commandcancel",
        &plan_of("commandcancel", vec![command("held", &[], &held)]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    let run = "commandcancel";
    world.until("the command to be running", |world| {
        world.root.join("held.pid").is_file()
    });
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "cancel", "id": "held"}]}).to_string(),
        )
        .exited(0);
    world.until("the command to end", |world| {
        !world.events_of(run, "command-exited").is_empty()
    });
    let exited = world.events_of(run, "command-exited");
    assert_eq!(exited[0]["payload"]["ending"], "cancelled", "{exited:?}");
    world.until("the node to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });
    assert!(
        settled_as(&world, run, "held", "cancelled"),
        "{:?}",
        world.events_of(run, "node-settled")
    );
    #[cfg(unix)]
    {
        let pid = std::fs::read_to_string(world.root.join("held.pid"))
            .expect("the command wrote its pid")
            .trim()
            .to_owned();
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("this host answers about a process it owns")
            .success();
        assert!(!alive, "the cancelled command {pid} is still running");
    }
    world.run(&["stop", run]);
}

/// Unchanged behaviour: a launch config at the previous schema version and a plan using
/// none of these fields launch, schedule and settle as before — agent, then the person,
/// then agent — and nothing of the new primitives is recorded.
#[test]
fn a_schema_13_launch_and_a_plan_naming_none_of_the_primitives_run_as_before() {
    let world = World::new("command-step-unchanged");
    let config = world.root.join("launch-13.json");
    std::fs::write(
        &config,
        json!({"schema_version": 13, "node_sets": ["members.worker.agent.model=node"]}).to_string(),
    )
    .expect("the launch config is written");
    let project = world.plan(
        "unchanged",
        &plan_of(
            "unchanged",
            vec![
                agent("build", &[]),
                human("approve", &["build"]),
                agent("ship", &["approve"]),
            ],
        ),
    );
    world
        .run(&[
            "start",
            &project,
            "--detach",
            "--launch-config",
            &config.display().to_string(),
        ])
        .exited(0);
    let run = "unchanged";
    world.until("the run to pause on the approval", |world| {
        settled_as(world, run, "approve", "waiting")
            && world.run(&["status", run]).stdout.contains("PAUSED")
    });
    assert!(settled_as(&world, run, "build", "done"));
    assert!(!dispatched(&world, run, "ship"));
    world.run(&["attest", run, "approve"]).exited(0);
    world.run(&["adopt", run]).exited(0);
    for node in ["build", "approve", "ship"] {
        assert_eq!(result(&world, run, node)["status"], "done", "{node}");
    }
    let kinds = world.kinds(run);
    assert!(
        !kinds.iter().any(|kind| kind.starts_with("command-")),
        "{kinds:?}"
    );
    assert!(world
        .run_json(run, "launch.json")
        .get("plan_edit_hook")
        .is_none());
}
