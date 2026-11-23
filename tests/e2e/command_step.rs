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

use crate::harness::{agent, human, plan_of, posix_script, repo_file, shell_path, World};

/// What the contract's `generic_primitives` block states about one of its keys.
///
/// Read rather than restated, so a journey proving what the contract promises fails the
/// moment the two disagree — the contract is the source, and the journey its drift gate.
fn stated(key: &str) -> Value {
    let contract =
        std::fs::read_to_string(repo_file("docs/contract.md")).expect("the contract ships");
    let block = contract
        .split("```json")
        .skip(1)
        .filter_map(|rest| rest.split("```").next())
        .find(|block| block.contains("\"generic_primitives\""))
        .expect("the contract carries the generic_primitives block");
    serde_json::from_str::<Value>(block).expect("a contract block is JSON")["generic_primitives"]
        [key]
        .clone()
}

/// One word the contract states under `key`, by its own name there.
fn word(key: &str, name: &str) -> String {
    stated(key)[name]
        .as_str()
        .unwrap_or_else(|| panic!("the contract states no {key}.{name}"))
        .to_owned()
}

/// The one word the contract states as `key`.
fn named(key: &str) -> String {
    stated(key)
        .as_str()
        .unwrap_or_else(|| panic!("the contract states no {key}"))
        .to_owned()
}

/// Whether the contract lists `item` under `key`.
fn listed(key: &str, item: &str) -> bool {
    stated(key)
        .as_array()
        .unwrap_or_else(|| panic!("the contract states no list {key}"))
        .iter()
        .any(|listed| listed == item)
}

/// Where the contract says a command node's output is kept, for this run and node.
fn log_of(world: &World, run: &str, node: &str) -> PathBuf {
    let template = stated("log");
    let relative = template
        .as_str()
        .expect("the contract states the log's place")
        .strip_prefix("<run>/")
        .expect("under the run")
        .replace("<node>", node);
    world.run_file(run, &relative)
}

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
    // Every variable the contract names, one line each, in its order; a run of no flow is
    // told none of the flow's.
    let names: Vec<String> = stated("environment")
        .as_array()
        .expect("the contract names the environment")
        .iter()
        .map(|name| name.as_str().expect("a name").to_owned())
        .collect();
    let printed = names
        .iter()
        .map(|name| format!("\"${{{name}:-unset}}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let recorder = script(
        &world,
        "record",
        &format!(
            "printf '%s\\n' {printed} > '{env}'\n\
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
    assert!(listed("endings", "exited"));
    assert_eq!(exited[0]["payload"]["exit_code"], 0);
    assert!(listed("journal_kinds", "command-exited"));

    let told = std::fs::read_to_string(world.root.join("command.env")).expect("it wrote its env");
    let told: std::collections::BTreeMap<&str, &str> =
        names.iter().map(String::as_str).zip(told.lines()).collect();
    assert_eq!(told["ONEPIPELINE_RUN_ID"], run);
    assert_eq!(told["ONEPIPELINE_NODE_ID"], "record");
    assert!(
        Path::new(told["ONEPIPELINE_RUN_ROOT"])
            .join("events.jsonl")
            .is_file(),
        "ONEPIPELINE_RUN_ROOT is not the run's own directory: {told:?}"
    );
    assert_eq!(told["ONEPIPELINE_PLAN"], project);
    assert_eq!(
        told["ONEPIPELINE_FLOW"], "unset",
        "a run of no flow named one"
    );
    assert_eq!(told.len(), 6, "{told:?}");
    let log = std::fs::read_to_string(log_of(&world, run, "record"))
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
    // The driver waiting on the person drives the rest, with nobody adopting the run.
    world.until_continued(run);
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
    let read = world.run(&["results", run, "--json"]);
    let document: Value = serde_json::from_str(&read.stdout).expect("results --json is JSON");
    assert_eq!(document["schema_version"], stated("results_schema_version"));
    let settled = result(&world, run, "check");
    assert_eq!(settled["status"], "failed", "{settled}");
    assert_eq!(
        settled["outcome"],
        json!(word("outcomes", "failed")),
        "{settled}"
    );
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
    let child = at(&world, "sleeper.child");
    // It waits on a child of its own, so what is stopped is a tree and not one process.
    let sleeper = script(
        &world,
        "sleeper",
        &format!("echo $$ > '{pid}'\nsleep 60 &\necho $! > '{child}'\nwait"),
    );
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
    assert_eq!(
        timed_out["outcome"],
        json!(word("outcomes", "timed_out")),
        "{timed_out}"
    );
    assert!(
        world
            .events_of(run, "command-exited")
            .iter()
            .any(|event| event["labels"]["node"] == "bounded"
                && event["payload"]["ending"] == "timed-out"
                && listed("endings", "timed-out")),
        "{}",
        world.dump()
    );
    assert_eq!(result(&world, run, "unbounded")["status"], "done");
    for process in ["sleeper.pid", "sleeper.child"] {
        assert!(
            !still_running(&world, process),
            "the command's {process} outlived its timeout"
        );
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
        // Each run notes the outputs file it was handed; the first — the one its driver
        // dies under — publishes to it, and the second publishes nothing.
        let variable = named("step_outputs_env");
        let repeated = script(
            &world,
            "repeated",
            &format!(
                "echo \"${variable}\" >> '{count}'\n\
                 [ -f '{go}' ] || printf '%s' '{{\"url\": \"first\"}}' > \"${variable}\"\n\
                 echo 'attempt began'\n\
                 while [ ! -f '{go}' ]; do sleep 0.1; done\nexit {exit}"
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
        let log = std::fs::read_to_string(log_of(&world, &name, "repeat")).expect("its log");
        assert_eq!(
            log.matches("attempt began").count(),
            2,
            "the second run's output did not join the first's: {log}"
        );
        let exits = world.events_of(&name, "command-exited");
        assert_eq!(exits.len(), 1, "{exits:?}");
        assert_eq!(exits[0]["payload"]["exit_code"], exit);
        // A run is handed a file no earlier run used, so what the run that died published
        // is never read as the second's.
        let handed = std::fs::read_to_string(world.root.join("count")).expect("the runs");
        let handed: Vec<&str> = handed.lines().collect();
        assert_ne!(handed[0], handed[1], "both runs were handed one file");
        assert!(
            exits[0]["payload"].get("outputs").is_none(),
            "the second run read the first's outputs: {exits:?}"
        );
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
        (
            "emptyonagent",
            {
                let mut node = agent("c", &[]);
                node["command"] = json!([]);
                node
            },
            "`kind: command` node's",
        ),
        (
            "timeoutonagent",
            {
                let mut node = agent("c", &[]);
                node["timeout_seconds"] = json!(5);
                node
            },
            "`kind: command` node's",
        ),
        (
            "zerotimeout",
            json!({"id": "c", "kind": "command", "command": ["true"], "timeout_seconds": 0}),
            "timeout_seconds",
        ),
        (
            "slashed",
            json!({"id": "a/b", "kind": "command", "command": ["true"]}),
            "cannot contain '/'",
        ),
        (
            "backslashed",
            json!({"id": "..\\outside", "kind": "command", "command": ["true"]}),
            "names its log file",
        ),
        (
            "driven",
            json!({"id": "c:outside", "kind": "command", "command": ["true"]}),
            "names its log file",
        ),
        (
            "parent",
            json!({"id": "..", "kind": "command", "command": ["true"]}),
            "names its log file",
        ),
        (
            "commandstep",
            json!({"id": "c", "repo": "service", "title": "feat: c", "steps": [
                {"id": "run", "kind": "command",
                 "task": "## What\nrun\n\n## Acceptance criteria\n- It ran."}]}),
            "never a step's",
        ),
    ];
    // Each field only a dispatch, a session or a person reads, refused by its own name.
    let unread = [
        ("max_turns", json!(3)),
        ("context", json!("a note")),
        ("executor", json!("local")),
        ("agent_graph", json!("./graph.yaml")),
        ("repo", json!("service")),
        ("resume", json!({"branch": "topic"})),
        ("expects_no_diff", json!(true)),
        ("draft", json!(true)),
        ("branch", json!("topic")),
        ("base_branch", json!("main")),
        ("sets", json!(["members.worker.agent.model=large"])),
        ("publish", json!("preserve")),
        (
            "steps",
            json!([{"id": "run", "persona": "engineer",
                    "task": "## What\nrun\n\n## Acceptance criteria\n- It ran."}]),
        ),
    ];
    for (field, value) in unread {
        let name = format!("unread{}", field.replace('_', ""));
        let mut node = json!({"id": "c", "kind": "command", "command": ["true"]});
        node[field] = value;
        let project = world.plan(&name, &plan_of(&name, vec![node]));
        world
            .run(&["plan", "check", &project])
            .exited(1)
            .out_has(&format!("`{field}"));
    }
    for (name, node, refusal) in cases {
        let project = world.plan(name, &plan_of(name, vec![node]));
        world
            .run(&["plan", "check", &project])
            .exited(1)
            .out_has(refusal);
    }

    // A legal one loads, and — having no judged task — is held to no rendering, as a human
    // node is not: the rendered-only check passes a plan of commands and people.
    let legal = world.plan(
        "legal",
        &plan_of(
            "legal",
            vec![
                json!({"id": "c", "kind": "command", "command": ["true"], "timeout_seconds": 5,
                       "task": "Publishes the notes; nobody is judged against this."}),
                human("approve", &["c"]),
            ],
        ),
    );
    world
        .run(&["plan", "check", &legal, "--require-rendered", "true"])
        .exited(0);
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
            .spawn_owned(
                world
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
                    .stderr(Stdio::null()),
            )
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

/// Whether the process whose pid `file` holds is still running. Unix reads its own process
/// table; elsewhere nothing here can ask, and the journey's other claims stand alone.
fn still_running(world: &World, file: &str) -> bool {
    let pid = std::fs::read_to_string(world.root.join(file))
        .expect("the command wrote its pid")
        .trim()
        .to_owned();
    cfg!(unix)
        && std::process::Command::new("kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("this host answers about a process it owns")
            .success()
}

/// A `cancel`, a `drop` or a `retry` that reaches a running command stops its process tree,
/// and its command ends `cancelled`; the cancelled node settles `cancelled`, and requeued it
/// runs again with its earlier settlement's detail no longer read as its own.
#[test]
fn a_cancel_drop_or_retry_stops_a_running_command() {
    for op in ["cancel", "drop", "retry"] {
        let world = World::new(&format!("command-step-{op}"));
        let pid = at(&world, "held.pid");
        let child = at(&world, "held.child");
        let held = script(
            &world,
            "held",
            &format!("echo $$ > '{pid}'\nsleep 120 &\necho $! > '{child}'\nwait"),
        );
        let name = format!("command{op}");
        let project = world.plan(&name, &plan_of(&name, vec![command("held", &[], &held)]));
        world.run(&["start", &project, "--detach"]).exited(0);
        world.until("the command to be running", |world| {
            world.root.join("held.child").is_file()
        });
        let stopping = match op {
            "retry" => json!({"op": "retry", "id": "held", "node": {
                "id": "held-2", "kind": "command", "command": ["true"]}}),
            "drop" => json!({"op": "drop", "id": "held", "dependents": "detach"}),
            _ => json!({"op": op, "id": "held"}),
        };
        world
            .run_with_stdin(
                &["reply", &name],
                &json!({"version": 2, "commands": [stopping]}).to_string(),
            )
            .exited(0);
        let held_exited = |world: &World| {
            world
                .events_of(&name, "command-exited")
                .into_iter()
                .find(|event| event["labels"]["node"] == "held")
        };
        world.until("the command to end", |world| held_exited(world).is_some());
        let exited = held_exited(&world).expect("it ended");
        assert_eq!(exited["payload"]["ending"], "cancelled", "{op}: {exited}");
        assert!(listed("endings", "cancelled"));
        for process in ["held.pid", "held.child"] {
            assert!(
                !still_running(&world, process),
                "{op}: the command's {process} is still running"
            );
        }
        if op == "cancel" {
            world.until("the node to settle", |world| {
                settled_as(world, &name, "held", "cancelled")
            });
            assert!(result(&world, &name, "held")["detail"].is_string());
            // Requeued, it runs again — and while it does, it has no settlement whose detail
            // could be read as its own.
            std::fs::remove_file(world.root.join("held.pid")).expect("the pid is cleared");
            // The run lets go once the node settled; an adoption drives the requeued node. Its
            // driver can still be exiting when the settlement reads, and a driver that claims
            // the requeue before it lets go drives it itself, so the requeue waits for it to
            // let go and reaches a run nothing drives.
            world.until("the driver to let go of the run", |world| {
                !world.run_file(&name, "owner.lock").exists()
            });
            world
                .run_with_stdin(
                    &["reply", &name],
                    &json!({"version": 2, "commands": [{"op": "requeue", "id": "held"}]})
                        .to_string(),
                )
                .exited(0);
            world.run(&["adopt", &name, "--detach"]).exited(0);
            world.until("the command to run again", |world| {
                world.root.join("held.pid").is_file()
            });
            let running = result(&world, &name, "held");
            assert_eq!(running["status"], "running", "{running}");
            assert!(running.get("detail").is_none(), "{running}");
        }
        if op == "retry" {
            world.until("the replacement to settle", |world| {
                settled_as(world, &name, "held-2", "done")
            });
        }
        world.run(&["stop", &name]);
    }
}

/// Command nodes are counted against the plan's `concurrency` beside agent nodes: at one, a
/// command waits for the agent dispatch ahead of it to settle before it starts.
#[test]
fn a_command_waits_for_a_free_concurrency_slot() {
    let world = World::new("command-step-concurrency");
    world.script("first.wait", "hold");
    let marker = at(&world, "second.ran");
    let second = script(&world, "second", &format!("touch '{marker}'"));
    let mut plan = plan_of(
        "commandslot",
        vec![agent("first", &[]), command("second", &[], &second)],
    );
    plan["concurrency"] = json!(1);
    let project = world.plan("commandslot", &plan);
    world.run(&["start", &project, "--detach"]).exited(0);
    let run = "commandslot";
    world.until("the agent node to be dispatched", |world| {
        dispatched(world, run, "first")
    });
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(
        position(&world, run, "command-started", "second").is_none(),
        "the command started while the one slot was taken"
    );
    world.release("first.go");
    world.until("the command to settle", |world| {
        settled_as(world, run, "second", "done")
    });
    assert!(
        position(&world, run, "node-settled", "first")
            < position(&world, run, "command-started", "second"),
        "the command started before the agent node freed its slot"
    );
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
    // The driver waiting on the person drives the rest, with nobody adopting the run.
    world.until_continued(run);
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

/// Every other way a command ends: one that cannot be started settles `command-failed`
/// `not-started`; one that says more on stderr than the detail keeps leaves only the end of
/// it there; one a signal ends settles `command-failed` saying so — and each runs from the
/// directory the run was launched from.
#[test]
fn a_command_that_cannot_start_says_too_much_or_is_signalled_settles_command_failed() {
    let world = World::new("command-step-endings");
    let launch_dir = world.root.join("launched-here");
    std::fs::create_dir_all(&launch_dir).expect("a launch directory");
    let cwd = at(&world, "chatty.cwd");
    let chatty = script(
        &world,
        "chatty",
        &format!(
            "pwd > '{cwd}'\necho 'EARLY-MARKER' >&2\n\
             i=0; while [ $i -lt 100 ]; do echo 'padding padding padding padding padding' >&2; \
             i=$((i+1)); done\necho 'LATE-MARKER' >&2\nexit 1"
        ),
    );
    // A program named relative to the launch directory is the file beside the launch.
    let beside = posix_script(&launch_dir, "beside", "echo 'ran beside the launch'");
    let mut nodes = vec![
        command("missing", &[], &world.root.join("no-such-program")),
        command("chatty", &[], &chatty),
        json!({"id": "relative", "kind": "command", "command": [format!(
            "./{}",
            beside.file_name().expect("a file").to_string_lossy()
        )]}),
    ];
    if cfg!(unix) {
        nodes.push(command(
            "signalled",
            &[],
            &script(&world, "signalled", "kill -9 $$"),
        ));
    }
    let project = world.plan("commandendings", &plan_of("commandendings", nodes));
    world
        .run_from(&launch_dir, &["start", &project, "--attach"])
        .settled();
    let run = "commandendings";
    let failed = word("outcomes", "failed");

    let missing = result(&world, run, "missing");
    assert_eq!(missing["outcome"], json!(failed), "{missing}");
    assert!(
        missing["detail"]
            .as_str()
            .expect("a detail")
            .contains("could not be started"),
        "{missing}"
    );
    assert!(listed("endings", "not-started"));
    assert!(world
        .events_of(run, "command-exited")
        .iter()
        .any(|event| event["labels"]["node"] == "missing"
            && event["payload"]["ending"] == "not-started"));

    assert_eq!(result(&world, run, "relative")["status"], "done");
    let said = result(&world, run, "chatty");
    let detail = said["detail"].as_str().expect("a detail");
    assert_eq!(said["outcome"], json!(failed), "{said}");
    assert!(detail.contains("LATE-MARKER"), "{detail}");
    assert!(
        !detail.contains("EARLY-MARKER"),
        "the detail kept more than the end of stderr: {detail}"
    );
    let log = std::fs::read_to_string(log_of(&world, run, "chatty")).expect("its log");
    assert!(
        log.contains("EARLY-MARKER") && log.contains("LATE-MARKER"),
        "{log}"
    );
    #[cfg(unix)]
    {
        let ran_in = std::fs::read_to_string(world.root.join("chatty.cwd")).expect("its cwd");
        assert_eq!(
            std::fs::canonicalize(ran_in.trim()).expect("a directory"),
            std::fs::canonicalize(&launch_dir).expect("a directory"),
            "the command did not run from the launch directory"
        );
        let signalled = result(&world, run, "signalled");
        assert_eq!(signalled["outcome"], json!(failed), "{signalled}");
        assert!(
            signalled["detail"]
                .as_str()
                .expect("a detail")
                .contains("ended by a signal"),
            "{signalled}"
        );
    }
}

/// A command that exits while a process it started still holds its stderr settles on its own
/// exit — promptly, not when what it left behind lets go — with its stderr tail as far as it
/// was read.
#[test]
fn a_command_whose_stderr_outlives_it_settles_on_its_own_exit() {
    let world = World::new("command-step-held-stderr");
    let leaver = script(
        &world,
        "leaver",
        "sleep 20 >&2 &\necho 'left a sleeper behind' >&2\nexit 3",
    );
    let project = world.plan(
        "commandheld",
        &plan_of("commandheld", vec![command("leaver", &[], &leaver)]),
    );
    let launched = std::time::Instant::now();
    world.run(&["start", &project, "--attach"]).settled();
    assert!(
        launched.elapsed() < std::time::Duration::from_secs(15),
        "the run waited on the process its command left behind"
    );
    let left = result(&world, "commandheld", "leaver");
    let detail = left["detail"].as_str().expect("a detail");
    assert!(detail.contains("status 3"), "{detail}");
    assert!(detail.contains("left a sleeper behind"), "{detail}");
}

/// Human items enabled for this world, through its host-local configuration.
fn items_enabled(world: &World) {
    let dir = world.root.join("xdg/onepipeline");
    std::fs::create_dir_all(&dir).expect("the configuration directory");
    std::fs::write(dir.join("config.yaml"), "human_items:\n  enabled: true\n")
        .expect("the configuration");
}

/// A command node whose script publishes `outputs`, written verbatim, to the file the
/// contract names the variable of.
fn publishing(world: &World, id: &str, outputs: &str) -> Value {
    writing(world, id, &format!("printf '%s' '{outputs}' > \"$OUT\""))
}

/// A command node running `body`, with `$OUT` the file the contract names the variable of.
fn writing(world: &World, id: &str, body: &str) -> Value {
    let variable = named("step_outputs_env");
    let script = script(world, id, &format!("OUT=\"${variable}\"\n{body}"));
    command(id, &[], &script)
}

/// A human node after `deps` whose item's title and body quote `quoting`.
fn quoting(id: &str, deps: &[&str], body: &str) -> Value {
    let mut node = human(id, deps);
    node["human_item"] = json!({
        "kind": "task",
        "priority": "high",
        "title": "Approve the design",
        "body": body,
    });
    node
}

/// The open items of this world, as `human-items list --json` reads them.
fn open_items(world: &World) -> Vec<Value> {
    let listed = world.run(&["human-items", "list", "--json"]);
    listed.exited(0);
    serde_json::from_str::<Value>(&listed.stdout)
        .expect("the list is JSON")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// A command step publishes a value through the file `ONEPIPELINE_STEP_OUTPUTS` names; its
/// `command-exited` records it as `outputs`, and the human node after it raises an item
/// whose body quotes it in place of `${steps.<node>.<output>}` — read back through
/// `human-items show`, as the person reading it does.
#[test]
fn a_steps_published_output_is_quoted_by_the_item_of_the_human_node_after_it() {
    let world = World::new("command-step-outputs");
    items_enabled(&world);
    let url = "https://example.invalid/doc";
    let placeholder = named("placeholder")
        .replace("<node>", "copy")
        .replace("<output>", "url");
    let run = "stepoutputs";
    let project = world.plan(
        run,
        &plan_of(
            run,
            vec![
                publishing(&world, "copy", &format!("{{\"url\": \"{url}\"}}")),
                {
                    let mut asked = quoting(
                        "approve",
                        &["copy"],
                        &format!("Read {placeholder} and approve it."),
                    );
                    asked["human_item"]["kind"] = json!("question");
                    asked["human_item"]["title"] = json!(format!("Approve {placeholder}?"));
                    asked["human_item"]["choices"] = json!([
                        {"id": "approve", "title": "Approve",
                         "description": format!("Ship {placeholder} as written.")},
                        {"id": "rework", "title": "Rework", "description": "Send it back."}
                    ]);
                    asked
                },
            ],
        ),
    );
    world.run(&["start", &project, "--attach"]).exited(0);

    let exited = world.events_of(run, "command-exited");
    assert_eq!(exited.len(), 1, "{exited:?}");
    assert_eq!(exited[0]["payload"]["outputs"], json!({"url": url}));
    let open = open_items(&world);
    assert_eq!(open.len(), 1, "{open:?}");
    let id = open[0]["item"]["id"].as_str().expect("an id");
    let shown = world.run(&["human-items", "show", id, "--json"]);
    shown.exited(0);
    let shown: Value = serde_json::from_str(&shown.stdout).expect("show prints JSON");
    assert_eq!(
        shown["item"]["body"],
        format!("Read {url} and approve it."),
        "{shown}"
    );
    assert_eq!(shown["item"]["title"], format!("Approve {url}?"), "{shown}");
    assert_eq!(
        shown["item"]["choices"][0]["description"],
        format!("Ship {url} as written."),
        "{shown}"
    );
    assert_eq!(shown["item"]["choices"][1]["description"], "Send it back.");
    // The file was the step's to write and the engine's to read once: none is left beside
    // the step's log.
    let commands = log_of(&world, run, "copy");
    let left: Vec<String> = std::fs::read_dir(commands.parent().expect("the commands directory"))
        .expect("the commands directory reads")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(left, ["copy.log"], "{left:?}");
    // A step that writes nothing publishes nothing, and its record says so by omission.
    let plain = world.plan(
        "silentstep",
        &plan_of(
            "silentstep",
            vec![command("quiet", &[], &script(&world, "quiet", "true"))],
        ),
    );
    world.run(&["start", &plain, "--attach"]).exited(0);
    let quiet = world.events_of("silentstep", "command-exited");
    assert!(quiet[0]["payload"].get("outputs").is_none(), "{quiet:?}");
}

/// A step's outputs file holding a list, or an object with a value that is not a string,
/// fails the step `command-failed` with a detail naming the file's shape, and its human
/// node is never reached; and a human node quoting a step that is not one of its deps is
/// refused at load, naming the node.
#[test]
fn outputs_that_are_not_an_object_of_strings_fail_and_a_quote_of_no_dependency_is_refused() {
    let world = World::new("command-step-outputs-refused");
    items_enabled(&world);
    let limit = stated("step_outputs_max_bytes")
        .as_u64()
        .expect("the contract states the bound");
    // `{"url":"` and `"}` are 10 bytes, so the file is `total` bytes long.
    let padded = |total: u64| {
        format!(
            "printf '{{\"url\":\"' > \"$OUT\"\n\
             head -c {} /dev/zero | tr '\\0' a >> \"$OUT\"\n\
             printf '\"}}' >> \"$OUT\"",
            total - 10
        )
    };
    for (run, body, said) in [
        (
            "listed",
            "printf '%s' '[\"https://example.invalid/doc\"]' > \"$OUT\"".to_owned(),
            "holds a JSON list".to_owned(),
        ),
        (
            "numbered",
            "printf '%s' '{\"url\": 7}' > \"$OUT\"".to_owned(),
            "value for \"url\" is a JSON number, not a string".to_owned(),
        ),
        (
            "garbled",
            "printf '%s' 'url=https://example.invalid/doc' > \"$OUT\"".to_owned(),
            "is not JSON".to_owned(),
        ),
        (
            "oversized",
            padded(limit + 1),
            format!("holds more than the {limit} bytes"),
        ),
        (
            "directory",
            "mkdir \"$OUT\"".to_owned(),
            "could not be read".to_owned(),
        ),
    ] {
        let project = world.plan(
            run,
            &plan_of(
                run,
                vec![
                    writing(&world, &format!("{run}-copy"), &body),
                    quoting(
                        "approve",
                        &[&format!("{run}-copy")],
                        &format!("Read ${{steps.{run}-copy.url}}."),
                    ),
                ],
            ),
        );
        world.run(&["start", &project, "--attach"]).settled();
        let settled = result(&world, run, &format!("{run}-copy"));
        assert_eq!(settled["status"], "failed", "{settled}");
        assert_eq!(settled["outcome"], word("outcomes", "failed"), "{settled}");
        let detail = settled["detail"].as_str().expect("a detail");
        assert!(detail.contains("ONEPIPELINE_STEP_OUTPUTS"), "{detail}");
        assert!(detail.contains(&said), "{said:?} is not in {detail}");
        let exited = world.events_of(run, "command-exited");
        assert!(exited[0]["payload"].get("outputs").is_none(), "{exited:?}");
    }
    assert!(open_items(&world).is_empty(), "an item was raised");

    // At the bound exactly, the file is read; and a command that publishes and then exits
    // non-zero has failed on its own terms, publishing nothing.
    let project = world.plan(
        "edges",
        &plan_of(
            "edges",
            vec![
                writing(&world, "full", &padded(limit)),
                writing(
                    &world,
                    "broke",
                    "printf '%s' '{\"url\": \"https://example.invalid/doc\"}' > \"$OUT\"\nexit 3",
                ),
            ],
        ),
    );
    world.run(&["start", &project, "--attach"]).settled();
    let exited = |node: &str| {
        world
            .events_of("edges", "command-exited")
            .into_iter()
            .find(|event| event["labels"]["node"] == node)
            .unwrap_or_else(|| panic!("{node} never exited"))["payload"]
            .clone()
    };
    assert_eq!(result(&world, "edges", "full")["status"], "done");
    assert_eq!(
        exited("full")["outputs"]["url"].as_str().map(str::len),
        usize::try_from(limit - 10).ok()
    );
    let broke = result(&world, "edges", "broke");
    assert_eq!(broke["status"], "failed", "{broke}");
    assert!(
        broke["detail"]
            .as_str()
            .is_some_and(|detail| detail.starts_with("the command exited with status 3")),
        "{broke}"
    );
    assert!(exited("broke").get("outputs").is_none());

    let stray = world.plan(
        "stray",
        &plan_of(
            "stray",
            vec![
                publishing(&world, "copy", "{}"),
                agent("other", &[]),
                quoting("approve", &["other"], "Read ${steps.copy.url}."),
            ],
        ),
    );
    world
        .run(&["plan", "check", &stray])
        .exited(1)
        .out_has("node 'approve'")
        .out_has("quotes step 'copy', which is not one of this node's deps");
    let unclosed = world.plan(
        "unclosed",
        &plan_of(
            "unclosed",
            vec![
                publishing(&world, "copy", "{}"),
                quoting("approve", &["copy"], "Read ${steps.copy.url and approve."),
            ],
        ),
    );
    world
        .run(&["plan", "check", &unclosed])
        .exited(1)
        .out_has("node 'approve'")
        .out_has("is never closed");
    for (name, quoted) in [
        ("nooutput", "${steps.copy}"),
        ("badoutput", "${steps.copy.the url}"),
    ] {
        let malformed = world.plan(
            name,
            &plan_of(
                name,
                vec![
                    publishing(&world, "copy", "{}"),
                    quoting("approve", &["copy"], &format!("Read {quoted}.")),
                ],
            ),
        );
        world
            .run(&["plan", "check", &malformed])
            .exited(1)
            .out_has("node 'approve'")
            .out_has("`human_item`")
            .out_has(&format!("`{quoted}` is not a placeholder"));
    }
}

/// A human node quoting an output its step did not publish raises no item: the step waits,
/// and the planner is told once — one non-blocking `human-item-unraised` surface about the
/// node naming the placeholder, which a second driver does not raise again — and attesting
/// the step releases what follows it.
#[test]
fn an_item_quoting_an_unpublished_output_is_not_raised_and_the_planner_is_told_once() {
    let world = World::new("command-step-outputs-unpublished");
    items_enabled(&world);
    let run = "unpublished";
    let project = world.plan(
        run,
        &plan_of(
            run,
            vec![
                publishing(&world, "copy", "{\"title\": \"the design\"}"),
                quoting("approve", &["copy"], "Read ${steps.copy.url}."),
                agent("ship", &["approve"]),
            ],
        ),
    );
    world.run(&["start", &project, "--attach"]).exited(0);

    assert!(open_items(&world).is_empty(), "an item was raised");
    let kind = named("unraised_surface_kind");
    let told: Vec<Value> = world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .filter(|event| event["payload"]["kind"] == kind.as_str())
        .collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["labels"]["node"], "approve");
    assert_eq!(told[0]["payload"]["blocking"], false);
    let message = told[0]["payload"]["message"].as_str().expect("a message");
    assert!(
        message.contains("`${steps.copy.url}` quotes output 'url' of step 'copy'"),
        "{message}"
    );
    assert!(settled_as(&world, run, "approve", "waiting"));
    assert!(!dispatched(&world, run, "ship"));

    world.run(&["attest", run, "approve"]).exited(0);
    // The decision continues the run: the driver waiting on it drives it on, with nobody
    // adopting it.
    world.until_continued(run);
    assert!(settled_as(&world, run, "ship", "done"), "{}", world.dump());
}
