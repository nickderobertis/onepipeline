//! A node's guard, driven end to end against the compiled binary.
//!
//! A guard is a real program — a shell script this suite writes — that the engine runs
//! when its node first becomes actionable, and whose exit status decides whether the node
//! is needed at all. What the guard was handed is read off the files that program wrote,
//! and what the run did with its verdict off the run's own journal, its `results` and
//! `status` output and the doubles' own records: never off anything this crate wrote down
//! about which way it decided.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its subprocess
// boundary, and the code host the linked `onevcs` publishes a lifecycle node to (`gh`),
// and nothing inside the crate under test, which is driven as a real compiled binary. Each
// guard is a real program the binary spawns. `harness.rs` carries the same suppression and
// the full rationale.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{
    agent, human, lifecycle, plan_of, posix_script, repo_file, shell_path, World,
};

/// What the contract's `guards` block states about one of its keys.
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
        .find(|block| block.contains("\"guards\""))
        .expect("the contract carries the guards block");
    serde_json::from_str::<Value>(block).expect("a contract block is JSON")["guards"][key].clone()
}

/// One word the contract states under `key`, by its own name there.
fn word(key: &str, name: &str) -> String {
    stated(key)[name]
        .as_str()
        .unwrap_or_else(|| panic!("the contract states no {key}.{name}"))
        .to_owned()
}

/// The exit status the contract says answers `name`.
fn exit(name: &str) -> i64 {
    stated("exits")[name]
        .as_i64()
        .unwrap_or_else(|| panic!("the contract states no exit for {name}"))
}

/// Whether the contract lists `item` under `key`.
fn listed(key: &str, item: &str) -> bool {
    stated(key)
        .as_array()
        .unwrap_or_else(|| panic!("the contract states no list {key}"))
        .iter()
        .any(|listed| listed == item)
}

/// How the contract says `results` and `status` print a `not-needed` node, with `detail`
/// where it has one.
fn rendered(detail: Option<&str>) -> String {
    match detail {
        Some(detail) => stated("rendered")["with_detail"]
            .as_str()
            .expect("the contract states the rendering")
            .replace("<detail>", detail),
        None => stated("rendered")["without_detail"]
            .as_str()
            .expect("the contract states the rendering")
            .to_owned(),
    }
}

/// Where the contract says a guard's output is kept, for this run and node.
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

/// A script under the world's root, its body POSIX shell.
fn script(world: &World, name: &str, body: &str) -> PathBuf {
    posix_script(&world.root, name, body)
}

/// A file under the world's root, as a script body names it.
fn at(world: &World, name: &str) -> String {
    shell_path(&world.root.join(name))
}

/// `node`, guarded by `guard` with no arguments: a guard's argv is the program alone.
fn guarded(mut node: Value, guard: &Path) -> Value {
    node["guard"] = json!([guard.display().to_string()]);
    node
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

/// How many records of `kind` the run journalled for `node`.
fn count(world: &World, run: &str, kind: &str, node: &str) -> usize {
    world
        .events_of(run, kind)
        .iter()
        .filter(|event| event["labels"]["node"] == node)
        .count()
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

/// The node's line in `results`' human output.
fn results_line(world: &World, run: &str, node: &str) -> String {
    let read = world.run(&["results", run]);
    read.stdout
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("{node} ")))
        .unwrap_or_else(|| panic!("`results {run}` has no line for {node}:\n{}", read.stdout))
        .to_owned()
}

/// Human items enabled for this world, through its host-local configuration.
fn human_items_enabled(world: &World) {
    let dir = world.root.join("xdg/onepipeline");
    std::fs::create_dir_all(&dir).expect("a configuration directory");
    std::fs::write(dir.join("config.yaml"), "human_items:\n  enabled: true\n")
        .expect("the configuration is written");
}

/// The open human items this world's host holds.
fn open_items(world: &World) -> Vec<Value> {
    let listed = world.run(&["human-items", "list", "--json"]);
    listed.exited(0);
    serde_json::from_str::<Value>(&listed.stdout)
        .expect("the listing is JSON")
        .as_array()
        .expect("a list")
        .clone()
}

/// A guard that appends its node to `guards.ran` and answers `code`, saying `said` on
/// stderr where it says anything.
fn counting_guard(world: &World, name: &str, code: i64, said: &str) -> PathBuf {
    let ran = at(world, "guards.ran");
    let say = if said.is_empty() {
        String::new()
    } else {
        format!("echo '{said}' >&2\n")
    };
    script(
        world,
        name,
        &format!("echo \"$ONEPIPELINE_NODE_ID\" >> '{ran}'\n{say}exit {code}"),
    )
}

/// How many times a [`counting_guard`] ran for `node`.
fn guard_runs(world: &World, node: &str) -> usize {
    std::fs::read_to_string(world.root.join("guards.ran"))
        .map(|ran| ran.lines().filter(|line| *line == node).count())
        .unwrap_or(0)
}

/// Whether the process whose pid `file` holds is still running. Unix reads its own process
/// table; elsewhere nothing here can ask, and the journey's other claims stand alone.
fn still_running(world: &World, file: &str) -> bool {
    let pid = std::fs::read_to_string(world.root.join(file))
        .expect("the guard wrote its pid")
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

/// A guard that writes its pid and its child's, and waits on that child until stopped.
fn holding_guard(world: &World, name: &str) -> PathBuf {
    let pid = at(world, &format!("{name}.pid"));
    let child = at(world, &format!("{name}.child"));
    script(
        world,
        name,
        &format!("echo $$ > '{pid}'\nsleep 120 &\necho $! > '{child}'\nwait"),
    )
}

/// The branches `repository` holds, its origin's and its checkout's.
fn branches(repository: &crate::harness::Repository) -> String {
    [&repository.origin, &repository.checkout]
        .iter()
        .map(|place| {
            let listed = std::process::Command::new("git")
                .arg("-C")
                .arg(place)
                .args(["branch", "--list", "--all"])
                .output()
                .expect("git lists branches");
            String::from_utf8_lossy(&listed.stdout).into_owned()
        })
        .collect()
}

/// Every refusal the loader makes of a guard, each naming the node and the field it is
/// about — through `plan check`, through a launch that refuses before any run exists, and
/// through a live edit, which refuses the reply before anything of it is applied.
#[test]
fn the_loader_refuses_every_invalid_guard_naming_the_node_and_the_field() {
    let world = World::new("guard-refused");
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "empty",
            json!({"id": "g", "kind": "command", "command": ["true"], "guard": []}),
            "guard",
        ),
        (
            "blank",
            {
                let mut node = agent("g", &[]);
                node["guard"] = json!(["  ", "--flag"]);
                node
            },
            "guard",
        ),
        (
            "onstep",
            json!({"id": "g", "repo": "service", "title": "feat: g", "steps": [
                {"id": "run", "persona": "engineer", "guard": ["true"],
                 "task": "## What\nrun\n\n## Acceptance criteria\n- It ran."}]}),
            "guard",
        ),
        (
            "nodiff",
            {
                let mut node = agent("g", &[]);
                node["guard"] = json!(["true"]);
                node["expects_no_diff"] = json!(true);
                node
            },
            "guard",
        ),
        (
            "zerotimeout",
            {
                let mut node = human("g", &[]);
                node["guard"] = json!(["true"]);
                node["guard_timeout_seconds"] = json!(0);
                node
            },
            "guard_timeout_seconds",
        ),
        (
            "negativetimeout",
            {
                let mut node = agent("g", &[]);
                node["guard"] = json!(["true"]);
                node["guard_timeout_seconds"] = json!(-5);
                node
            },
            "guard_timeout_seconds",
        ),
        (
            "fractionaltimeout",
            {
                let mut node = agent("g", &[]);
                node["guard"] = json!(["true"]);
                node["guard_timeout_seconds"] = json!(1.5);
                node
            },
            "guard_timeout_seconds",
        ),
        (
            "timeoutalone",
            {
                let mut node = agent("g", &[]);
                node["guard_timeout_seconds"] = json!(5);
                node
            },
            "guard_timeout_seconds",
        ),
        // The id names the guard's log, so it is one file name on every host.
        (
            "logname",
            {
                let mut node = agent("g", &[]);
                node["id"] = json!("g:elsewhere");
                node["guard"] = json!(["true"]);
                node
            },
            "id",
        ),
    ];
    for (name, node, field) in &cases {
        let project = world.plan(name, &plan_of(name, vec![node.clone()]));
        let id = node["id"].as_str().expect("an id");
        let named = |said: &str| said.contains(&format!("node '{id}'")) && said.contains(field);
        let checked = world.run(&["plan", "check", &project]);
        checked.exited(1);
        assert!(named(&checked.stdout), "{name}: {}", checked.stdout);
        let launched = world.run(&["start", &project, "--detach"]);
        launched.exited(2);
        assert!(named(&launched.stderr), "{name}: {}", launched.stderr);
        assert!(
            !world.runs.join(name).exists(),
            "{name}: a refused launch left a run behind"
        );
    }

    // A live edit: a run paused on a person, and a reply adding each invalid node.
    let project = world.plan("live", &plan_of("live", vec![human("approve", &[])]));
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the run to pause on the approval", |world| {
        settled_as(world, "live", "approve", "waiting")
    });
    for (name, node, field) in &cases {
        let mut node = node.clone();
        // A fresh id the run does not hold, refused for the same reason as at launch: the
        // id case keeps the separator its refusal is about.
        let id = if *field == "id" {
            format!("added:{name}")
        } else {
            format!("added-{name}")
        };
        node["id"] = json!(id);
        let replied = world.run_with_stdin(
            &["reply", "live"],
            &json!({"version": 2, "commands": [{"op": "add", "node": node}]}).to_string(),
        );
        replied.exited(2);
        assert!(
            replied.stderr.contains(&format!("node '{id}'")) && replied.stderr.contains(field),
            "{name}: {}",
            replied.stderr
        );
    }
    assert!(
        world.events_of("live", "edit-committed").is_empty(),
        "a refused edit was applied"
    );
    let added: Vec<Value> = world.journal("live");
    assert!(
        !added.iter().any(|event| event["labels"]["node"]
            .as_str()
            .is_some_and(|node| node.starts_with("added"))),
        "a refused node reached the run's journal"
    );
}

/// The journey a guard exists for: a guard exiting 3 settles an agent node, a lifecycle
/// node, a command node and a human node `done (not-needed)`, its stderr the detail, with no
/// dispatch, command, session, branch or human item for any of them — and the node behind
/// all four runs, the run settles `complete` and its success hook fires. A guard saying
/// nothing leaves a detail-less `done (not-needed)`.
#[test]
fn a_guard_exiting_3_settles_each_kind_not_needed_and_nothing_is_started_for_it() {
    let world = World::new("guard-not-needed");
    human_items_enabled(&world);
    let repository = world.repository("change-auto", &[]);
    let said = "nothing for this node to do";
    let not_needed = counting_guard(&world, "not-needed", exit("not_needed"), said);
    let silent = counting_guard(&world, "silent", exit("not_needed"), "");
    let chatty = script(
        &world,
        "chatty",
        &format!(
            "echo 'EARLY-MARKER' >&2\n\
             i=0; while [ $i -lt 100 ]; do echo 'padding padding padding padding padding' >&2; \
             i=$((i+1)); done\necho 'LATE-MARKER' >&2\necho '   ' >&2\nexit {}",
            exit("not_needed")
        ),
    );
    let ran = at(&world, "command.ran");
    let work = script(&world, "work", &format!("touch '{ran}'"));
    let fired = at(&world, "success.fired");
    let hook = script(&world, "success", &format!("touch '{fired}'"));
    let run = "notneeded";
    let guarded_kinds = ["direct", "shipped", "cmd", "approve"];
    let project = world.plan(
        run,
        &plan_of(
            run,
            vec![
                guarded(agent("direct", &[]), &not_needed),
                guarded(lifecycle("shipped", &[]), &not_needed),
                guarded(
                    json!({"id": "cmd", "kind": "command", "command": [work.display().to_string()]}),
                    &not_needed,
                ),
                guarded(human("approve", &[]), &not_needed),
                guarded(agent("quiet", &[]), &silent),
                guarded(agent("chatty", &[]), &chatty),
                agent(
                    "after",
                    &["direct", "shipped", "cmd", "approve", "quiet", "chatty"],
                ),
            ],
        ),
    );
    world
        .run(&[
            "start",
            &project,
            "--attach",
            "--success-hook",
            &hook.display().to_string(),
        ])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");

    let outcome = word("outcomes", "not_needed");
    for node in guarded_kinds {
        let settled = result(&world, run, node);
        assert_eq!(settled["status"], "done", "{node}: {settled}");
        assert_eq!(settled["outcome"], json!(outcome), "{node}: {settled}");
        assert_eq!(settled["detail"], json!(said), "{node}: {settled}");
        let line = results_line(&world, run, node);
        assert!(
            line.trim_end().ends_with(&rendered(Some(said))),
            "{node}: {line}"
        );
        let evaluated: Vec<Value> = world
            .events_of(run, "guard-evaluated")
            .into_iter()
            .filter(|event| event["labels"]["node"] == node)
            .collect();
        assert_eq!(evaluated.len(), 1, "{node}: {evaluated:?}");
        assert_eq!(evaluated[0]["payload"]["verdict"], "not-needed");
        assert!(listed("verdicts", "not-needed"));
        assert_eq!(evaluated[0]["payload"]["ending"], "exited");
        assert_eq!(evaluated[0]["payload"]["exit_code"], exit("not_needed"));
        assert!(
            position(&world, run, "guard-started", node)
                < position(&world, run, "guard-evaluated", node)
                && position(&world, run, "guard-evaluated", node)
                    < position(&world, run, "node-settled", node),
            "{node}: the verdict was not journalled between its start and the settlement"
        );
        assert_eq!(guard_runs(&world, node), 1, "{node}");
        // Nothing but the guard and its settlement was ever recorded for the node: no
        // dispatch, no command, no session, no waiting.
        let recorded: std::collections::BTreeSet<String> = world
            .journal(run)
            .iter()
            .filter(|event| event["labels"]["node"] == node)
            .map(|event| event["kind"].as_str().expect("a kind").to_owned())
            .collect();
        for kind in &recorded {
            assert!(
                [
                    "node-ready",
                    "guard-started",
                    "guard-evaluated",
                    "node-settled"
                ]
                .contains(&kind.as_str()),
                "{node} recorded {kind}: {recorded:?}"
            );
        }
    }
    assert!(!world.root.join("command.ran").exists(), "the command ran");
    assert!(
        !branches(&repository).contains("shipped"),
        "a branch was cut for the not-needed lifecycle node:\n{}",
        branches(&repository)
    );
    assert!(
        open_items(&world).is_empty(),
        "a human item was raised for a node its guard found not needed"
    );
    assert!(
        !world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "approve"
                && event["payload"]["status"] == "waiting"),
        "the human node was recorded waiting"
    );

    let status = world.run(&["status", run]);
    for node in guarded_kinds {
        status.out_has(&format!("  {node}: {}\n", rendered(Some(said))));
    }
    status.out_has(&format!("  quiet: {}\n", rendered(None)));
    let document: Value = serde_json::from_str(&world.run(&["results", run, "--json"]).stdout)
        .expect("results --json is JSON");
    assert_eq!(document["schema_version"], stated("results_schema_version"));
    let quiet = result(&world, run, "quiet");
    assert_eq!(quiet["outcome"], json!(outcome), "{quiet}");
    assert!(quiet.get("detail").is_none(), "{quiet}");
    assert!(
        results_line(&world, run, "quiet")
            .trim_end()
            .ends_with(&rendered(None)),
        "{}",
        results_line(&world, run, "quiet")
    );
    // A guard saying more than the detail keeps leaves only the end of it there, trimmed;
    // its log keeps the whole.
    let chatty = result(&world, run, "chatty");
    let detail = chatty["detail"].as_str().expect("a detail");
    assert!(detail.ends_with("LATE-MARKER"), "{detail}");
    assert!(!detail.contains("EARLY-MARKER"), "{detail}");
    assert!(
        detail.len() as u64 <= stated("detail_tail_bytes").as_u64().expect("a bound"),
        "{} bytes",
        detail.len()
    );
    let log = std::fs::read_to_string(log_of(&world, run, "chatty")).expect("its log");
    assert!(log.contains("EARLY-MARKER") && log.contains("LATE-MARKER"));

    assert!(dispatched(&world, run, "after"));
    assert_eq!(result(&world, run, "after")["status"], "done");
    assert_eq!(world.run_json(run, "result.json")["state"], "complete");
    assert!(
        world.root.join("success.fired").is_file(),
        "the success hook did not fire"
    );
    assert!(
        world
            .events_of(run, "run-hook-fired")
            .iter()
            .any(|event| event["payload"]["hook"] == "success"),
        "{}",
        world.dump()
    );
}

/// A guard exiting 0 leaves each kind to run exactly as an unguarded one would — the agent
/// node and the lifecycle node dispatched, the command run, the human node recorded
/// `waiting` with its item raised — each after its verdict and with its guard run once.
/// The guard ran from the launch directory, told the run's coordinates, held to the default
/// deadline, its output kept in the run's `guards/` log.
#[test]
fn a_guard_exiting_0_leaves_each_kind_to_run_and_runs_once_with_the_runs_coordinates() {
    let world = World::new("guard-needed");
    human_items_enabled(&world);
    world.repository("change-open", &[]);
    let launch_dir = world.root.join("launched-here");
    std::fs::create_dir_all(&launch_dir).expect("a launch directory");
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
    let ran = at(&world, "guards.ran");
    let told = at(&world, "told");
    let guard = script(
        &world,
        "needed",
        &format!(
            "echo \"$ONEPIPELINE_NODE_ID\" >> '{ran}'\n\
             printf '%s\\n' {printed} \"$(pwd)\" > '{told}.'\"$ONEPIPELINE_NODE_ID\"\n\
             printf '%s\\n' \"$@\" > '{told}.args.'\"$ONEPIPELINE_NODE_ID\"\n\
             echo 'the guard looked and found work'\nexit {}",
            exit("needed")
        ),
    );
    // Arguments a shell would split, expand and pipe, which the guard is handed as written.
    let arguments = ["two words", "$HOME; echo injected | cat *"];
    let mut argued = guarded(agent("direct", &[]), &guard);
    argued["guard"] = json!([guard.display().to_string(), arguments[0], arguments[1]]);
    let marker = at(&world, "command.ran");
    let work = script(&world, "work", &format!("touch '{marker}'"));
    let run = "needed";
    let project = world.plan(
        run,
        &plan_of(
            run,
            vec![
                argued,
                guarded(lifecycle("shipped", &[]), &guard),
                guarded(
                    json!({"id": "cmd", "kind": "command", "command": [work.display().to_string()]}),
                    &guard,
                ),
                guarded(human("approve", &[]), &guard),
            ],
        ),
    );
    world
        .run_from(&launch_dir, &["start", &project, "--detach"])
        .exited(0);
    world.until("every node to have gone past its guard", |world| {
        settled_as(world, run, "direct", "done")
            && settled_as(world, run, "cmd", "done")
            && settled_as(world, run, "approve", "waiting")
            && dispatched(world, run, "shipped")
    });

    for (node, started) in [
        ("direct", "node-dispatched"),
        ("shipped", "node-dispatched"),
        ("cmd", "command-started"),
        ("approve", "node-settled"),
    ] {
        let evaluated = world
            .events_of(run, "guard-evaluated")
            .into_iter()
            .find(|event| event["labels"]["node"] == node)
            .unwrap_or_else(|| panic!("{node} has no verdict: {}", world.dump()));
        assert_eq!(evaluated["payload"]["verdict"], "needed", "{node}");
        assert!(listed("verdicts", "needed"));
        assert_eq!(evaluated["payload"]["exit_code"], exit("needed"), "{node}");
        assert!(
            position(&world, run, "guard-evaluated", node) < position(&world, run, started, node),
            "{node}: {started} came before its verdict"
        );
        assert_eq!(count(&world, run, "guard-started", node), 1, "{node}");
        assert_eq!(guard_runs(&world, node), 1, "{node}: its guard ran again");
        let started = world
            .events_of(run, "guard-started")
            .into_iter()
            .find(|event| event["labels"]["node"] == node)
            .expect("it started");
        assert_eq!(
            started["payload"]["timeout_seconds"],
            stated("default_timeout_seconds"),
            "{node}: {started}"
        );
        let argv = if node == "direct" {
            json!([guard.display().to_string(), arguments[0], arguments[1]])
        } else {
            json!([guard.display().to_string()])
        };
        assert_eq!(started["payload"]["guard"], argv, "{node}");
    }
    let handed =
        std::fs::read_to_string(world.root.join("told.args.direct")).expect("it wrote its argv");
    assert_eq!(
        handed.lines().collect::<Vec<_>>(),
        arguments,
        "the guard was not handed its arguments as written, without a shell"
    );
    assert!(
        world.root.join("command.ran").is_file(),
        "the command never ran"
    );
    // Raised by the driver's own pass once the wait is recorded, so waited for on the
    // store's index rather than read once.
    let key = format!("run:{run}#approve");
    world.until("the human node's item to be raised", |world| {
        std::fs::read_to_string(world.runs.join("human-items/index.json"))
            .is_ok_and(|index| index.contains(&key))
    });
    let items = open_items(&world);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["item"]["dedup_key"], format!("run:{run}#approve"));

    let said = std::fs::read_to_string(world.root.join("told.direct")).expect("it wrote its env");
    let said: Vec<&str> = said.lines().collect();
    let told: std::collections::BTreeMap<&str, &str> = names
        .iter()
        .map(String::as_str)
        .zip(said.iter().copied())
        .collect();
    assert_eq!(told["ONEPIPELINE_RUN_ID"], run);
    assert_eq!(told["ONEPIPELINE_NODE_ID"], "direct");
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
    #[cfg(unix)]
    assert_eq!(
        std::fs::canonicalize(said[names.len()]).expect("a directory"),
        std::fs::canonicalize(&launch_dir).expect("a directory"),
        "the guard did not run from the launch directory"
    );
    let log = std::fs::read_to_string(log_of(&world, run, "direct")).expect("the guard's log");
    assert!(log.contains("the guard looked and found work"), "{log}");
    world.run(&["stop", run]);
}

/// A guard exiting 1, a guard naming a program that does not exist, a guard outliving a
/// `guard_timeout_seconds` of two seconds and — on Unix — a guard a signal ends each settle
/// their node `failed` with outcome
/// `guard-failed`, the detail naming the cause — never `not-needed` — and the node behind
/// each is not run. The timed-out guard's whole process tree is stopped.
#[test]
fn a_guard_that_fails_cannot_start_or_times_out_settles_guard_failed_and_holds_its_dependent() {
    let world = World::new("guard-failed");
    let broken = counting_guard(&world, "broken", 1, "the lookup could not read its store");
    let mut slow = guarded(agent("slow", &[]), &holding_guard(&world, "slow"));
    slow["guard_timeout_seconds"] = json!(2);
    let run = "guardfailed";
    let mut nodes = vec![
        guarded(agent("broken", &[]), &broken),
        agent("after-broken", &["broken"]),
        guarded(agent("missing", &[]), &world.root.join("no-such-guard")),
        agent("after-missing", &["missing"]),
        slow,
        agent("after-slow", &["slow"]),
    ];
    let mut cases = vec![
        (
            "broken",
            "exited",
            vec!["status 1", "the lookup could not read its store"],
        ),
        (
            "missing",
            "not-started",
            vec!["the guard could not be started"],
        ),
        ("slow", "timed-out", vec!["the guard timed out after 2 s"]),
    ];
    if cfg!(unix) {
        nodes.push(guarded(
            agent("signalled", &[]),
            &script(&world, "signalled", "kill -9 $$"),
        ));
        nodes.push(agent("after-signalled", &["signalled"]));
        cases.push((
            "signalled",
            "exited",
            vec!["the guard was ended by a signal"],
        ));
    }
    let project = world.plan(run, &plan_of(run, nodes));
    let started = std::time::Instant::now();
    world.run(&["start", &project, "--attach"]).settled();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(50),
        "the run waited out the guard rather than its deadline"
    );
    let failed = word("outcomes", "failed");
    for (node, ending, says) in cases {
        let settled = result(&world, run, node);
        assert_eq!(settled["status"], "failed", "{node}: {settled}");
        assert_eq!(settled["outcome"], json!(failed), "{node}: {settled}");
        let detail = settled["detail"].as_str().expect("a detail");
        for said in says {
            assert!(detail.contains(said), "{node}: {detail}");
        }
        let evaluated = world
            .events_of(run, "guard-evaluated")
            .into_iter()
            .find(|event| event["labels"]["node"] == node)
            .expect("a verdict");
        assert_eq!(evaluated["payload"]["verdict"], "failed", "{node}");
        assert!(listed("verdicts", "failed"));
        assert_eq!(evaluated["payload"]["ending"], ending, "{node}");
        assert!(stated("endings")
            .as_array()
            .expect("the endings")
            .contains(&json!(ending)));
        assert!(!dispatched(&world, run, node), "{node} was dispatched");
        let after = format!("after-{node}");
        assert!(!dispatched(&world, run, &after), "{after} ran");
        assert_ne!(result(&world, run, &after)["status"], "done", "{after}");
        assert!(
            results_line(&world, run, node).contains(&format!("failed ({failed})")),
            "{}",
            results_line(&world, run, node)
        );
    }
    for process in ["slow.pid", "slow.child"] {
        assert!(
            !still_running(&world, process),
            "the guard's {process} outlived its deadline"
        );
    }
}

/// A running guard holds one of the plan's `concurrency` slots while the driver goes on
/// reconciling the rest, and a guard waits for a slot as a dispatch does — a human node's
/// included, which takes no slot of its own once it waits. At two, a guard and an agent fill
/// the slots: neither the guarded human node's guard nor the last agent starts until the
/// agent settles, and then both run while the first guard is still running.
#[test]
fn a_running_guard_counts_against_concurrency_while_the_driver_reconciles_the_rest() {
    let world = World::new("guard-concurrency");
    let go = at(&world, "guard.go");
    let guard = script(
        &world,
        "waits",
        &format!("while [ ! -f '{go}' ]; do sleep 0.1; done\nexit 3"),
    );
    let quick = counting_guard(&world, "quick", exit("not_needed"), "");
    world.script("first.wait", "hold");
    let mut plan = plan_of(
        "guardslot",
        vec![
            guarded(agent("guarded", &[]), &guard),
            agent("first", &[]),
            guarded(human("approve", &[]), &quick),
            agent("second", &[]),
        ],
    );
    plan["concurrency"] = json!(2);
    let project = world.plan("guardslot", &plan);
    world.run(&["start", &project, "--detach"]).exited(0);
    let run = "guardslot";
    world.until("the guard and the first node to hold the slots", |world| {
        position(world, run, "guard-started", "guarded").is_some()
            && dispatched(world, run, "first")
    });
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(
        !dispatched(&world, run, "second"),
        "a node started while a guard and an agent held both slots"
    );
    assert!(
        position(&world, run, "guard-started", "approve").is_none(),
        "a human node's guard started while a guard and an agent held both slots"
    );
    world.release("first.go");
    world.until("the second node to be dispatched", |world| {
        dispatched(world, run, "second")
    });
    assert!(
        position(&world, run, "guard-evaluated", "guarded").is_none(),
        "the first guard ended before the rest started"
    );
    let freed = position(&world, run, "node-settled", "first");
    assert!(
        freed < position(&world, run, "guard-started", "approve")
            && freed < position(&world, run, "node-dispatched", "second"),
        "a node started before the first freed its slot"
    );
    std::fs::write(world.root.join("guard.go"), "").expect("the guard is released");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(world.run_json(run, "result.json")["state"], "complete");
    for node in ["guarded", "approve"] {
        assert_eq!(
            result(&world, run, node)["outcome"],
            json!(word("outcomes", "not_needed")),
            "{node}"
        );
    }
}

/// A `cancel`, a `drop` or a `retry` that reaches a node whose guard is running stops the
/// guard's whole process tree — its `guard-evaluated` ending `cancelled` — and nothing is
/// started for the node: the cancelled one settles `cancelled`, and the retry's replacement
/// runs in its place.
#[test]
fn a_cancel_drop_or_retry_stops_a_running_guard_and_its_process_tree() {
    for op in ["cancel", "drop", "retry"] {
        let world = World::new(&format!("guard-{op}"));
        let name = format!("guard{op}");
        let project = world.plan(
            &name,
            &plan_of(
                &name,
                vec![guarded(agent("held", &[]), &holding_guard(&world, "held"))],
            ),
        );
        world.run(&["start", &project, "--detach"]).exited(0);
        world.until("the guard to be running", |world| {
            world.root.join("held.child").is_file()
        });
        assert_eq!(result(&world, &name, "held")["status"], "running");
        let stopping = match op {
            "retry" => json!({"op": "retry", "id": "held", "node": agent("held-2", &[])}),
            "drop" => json!({"op": "drop", "id": "held", "dependents": "detach"}),
            _ => json!({"op": op, "id": "held"}),
        };
        world
            .run_with_stdin(
                &["reply", &name],
                &json!({"version": 2, "commands": [stopping]}).to_string(),
            )
            .exited(0);
        let ended = |world: &World| {
            world
                .events_of(&name, "guard-evaluated")
                .into_iter()
                .find(|event| event["labels"]["node"] == "held")
        };
        world.until("the guard to end", |world| ended(world).is_some());
        let ended = ended(&world).expect("it ended");
        assert_eq!(ended["payload"]["ending"], "cancelled", "{op}: {ended}");
        assert_ne!(ended["payload"]["verdict"], "not-needed", "{op}: {ended}");
        for process in ["held.pid", "held.child"] {
            assert!(
                !still_running(&world, process),
                "{op}: the guard's {process} is still running"
            );
        }
        assert!(
            !dispatched(&world, &name, "held"),
            "{op}: held was dispatched"
        );
        match op {
            "cancel" => world.until("the node to settle", |world| {
                settled_as(world, &name, "held", "cancelled")
            }),
            "retry" => world.until("the replacement to settle", |world| {
                settled_as(world, &name, "held-2", "done")
            }),
            _ => {}
        }
        world.run(&["stop", &name]);
    }
}

/// A `not-needed` node is `done` to every reader of a run's state: another run's
/// `run:<id>#<node>` reference to it is satisfied, and the store write-back projects its
/// settlement onto its task like any other.
#[test]
fn a_not_needed_node_satisfies_a_cross_run_reference_and_is_written_back() {
    let world = World::new("guard-crossrun");
    let not_needed = counting_guard(&world, "not-needed", exit("not_needed"), "no work today");
    let upstream = world.plan(
        "produced",
        &plan_of("produced", vec![guarded(agent("prep", &[]), &not_needed)]),
    );
    world.run(&["start", &upstream, "--attach"]).exited(0);
    assert_eq!(
        result(&world, "produced", "prep")["outcome"],
        json!(word("outcomes", "not_needed"))
    );
    world.until_store("the settlement to reach the board", |world| {
        world.store_tasks(&upstream).iter().any(|task| {
            task["item"]["metadata"]["onepipeline.id"] == "prep"
                && task["item"]["status"]["category"] == "done"
                && task["item"]["metadata"]["onepipeline.settlement"]["outcome"]
                    == json!(word("outcomes", "not_needed"))
        })
    });

    let mut consumer = agent("ship", &[]);
    consumer["deps"] = json!(["run:produced#prep"]);
    let downstream = world.plan("consumes", &plan_of("consumes", vec![consumer]));
    world.run(&["start", &downstream, "--attach"]).exited(0);
    assert!(dispatched(&world, "consumes", "ship"));
    assert_eq!(result(&world, "consumes", "ship")["status"], "done");
    assert!(
        world
            .events_of("consumes", "cross-dag-satisfied")
            .iter()
            .any(|event| event["payload"]["dependency"] == "run:produced#prep"),
        "{}",
        world.dump()
    );
}

/// `requeue` and `retry` each start a new attempt, which evaluates the guard again: a
/// guarded command cancelled while it ran is requeued and its guard runs a second time
/// before its command does, and a guarded agent node that failed is retried with a guarded
/// replacement whose own guard runs before it is dispatched.
#[test]
fn a_requeue_or_a_retry_of_a_guarded_node_evaluates_its_guard_again() {
    let world = World::new("guard-new-attempt");
    let guard = counting_guard(&world, "needed", exit("needed"), "");
    let held = at(&world, "command.held");
    let go = at(&world, "command.go");
    let work = script(
        &world,
        "work",
        &format!("touch '{held}'\nwhile [ ! -f '{go}' ]; do sleep 0.1; done"),
    );
    world.script("flaky.fail", "1");
    let run = "attempts";
    let project = world.plan(
        run,
        &plan_of(
            run,
            vec![
                guarded(
                    json!({"id": "cmd", "kind": "command", "command": [work.display().to_string()]}),
                    &guard,
                ),
                guarded(agent("flaky", &[]), &guard),
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the command to run and the agent to fail", |world| {
        world.root.join("command.held").is_file() && settled_as(world, run, "flaky", "failed")
    });
    assert_eq!(guard_runs(&world, "cmd"), 1);
    assert_eq!(guard_runs(&world, "flaky"), 1);

    // The command is cancelled and requeued: its guard is asked again first.
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "cancel", "id": "cmd"}]}).to_string(),
        )
        .exited(0);
    world.until("the command to settle cancelled", |world| {
        settled_as(world, run, "cmd", "cancelled")
    });
    std::fs::remove_file(world.root.join("command.held")).expect("the marker is cleared");
    std::fs::write(world.root.join("command.go"), "").expect("the command is released");
    let mut replacement = guarded(agent("flaky-2", &[]), &guard);
    replacement["deps"] = json!([]);
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "requeue", "id": "cmd"},
                {"op": "retry", "id": "flaky", "node": replacement},
            ]})
            .to_string(),
        )
        .exited(0);
    // The driver that applied the reply drives both new attempts where it is still there;
    // where it had already let go, an adoption does.
    let adopted = world.run(&["adopt", run, "--detach"]);
    assert!(
        adopted.code == 0 || adopted.stderr.contains("still being driven"),
        "{}",
        adopted.stderr
    );
    world.until("both new attempts to settle", |world| {
        settled_as(world, run, "cmd", "done") && settled_as(world, run, "flaky-2", "done")
    });
    assert_eq!(
        guard_runs(&world, "cmd"),
        2,
        "the requeued node's guard was not asked again"
    );
    assert_eq!(count(&world, run, "guard-evaluated", "cmd"), 2);
    let evaluated = world
        .journal(run)
        .iter()
        .rposition(|event| event["kind"] == "guard-evaluated" && event["labels"]["node"] == "cmd");
    let started = world
        .journal(run)
        .iter()
        .rposition(|event| event["kind"] == "command-started" && event["labels"]["node"] == "cmd");
    assert!(
        evaluated < started,
        "the requeued command ran before its guard"
    );
    assert_eq!(
        guard_runs(&world, "flaky-2"),
        1,
        "the replacement's guard did not run"
    );
    assert!(
        position(&world, run, "guard-evaluated", "flaky-2")
            < position(&world, run, "node-dispatched", "flaky-2")
    );
    world.run(&["stop", run]);
}

/// Stop the driver of `run` and everything in its process group, and wait for it to be gone.
#[cfg(unix)]
fn kill_driver(world: &World, run: &str) {
    let driver = world.run_json(run, "launch.json")["pid"]
        .as_u64()
        .expect("the launch record names its driver");
    std::process::Command::new("kill")
        .args(["-KILL", "--", &format!("-{driver}")])
        .status()
        .expect("this host signals a process group it owns");
    world.until("the driver to be gone", |world| {
        world.run(&["status", run]).stdout.contains("STOPPED")
            || !std::process::Command::new("kill")
                .args(["-0", &driver.to_string()])
                .stderr(std::process::Stdio::null())
                .status()
                .expect("this host answers about a process it owns")
                .success()
    });
}

/// `adopt` reuses a verdict its run journalled: a node whose guard said *needed* and whose
/// dispatch died with its driver is dispatched again without its guard running again. A
/// guard whose driver died before its verdict was journalled is run again from the start,
/// and its second ending is the one the node settles on.
///
/// Unix only, for the reason the suite's other teardown journeys are: what stops the driver
/// with everything it started is a signal to its process group.
#[cfg(unix)]
#[test]
fn adopt_reuses_a_journalled_verdict_and_runs_again_a_guard_its_driver_died_in() {
    let world = World::new("guard-adopt");
    let needed = counting_guard(&world, "needed", exit("needed"), "");
    world.script("worked.wait", "hold");
    let run = "guardadopt";
    let project = world.plan(
        run,
        &plan_of(run, vec![guarded(agent("worked", &[]), &needed)]),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the node to be dispatched past its guard", |world| {
        dispatched(world, run, "worked")
    });
    kill_driver(&world, run);
    world.release("worked.go");
    world.run(&["adopt", run]).exited(0);
    assert!(
        settled_as(&world, run, "worked", "done"),
        "{}",
        world.dump()
    );
    assert_eq!(count(&world, run, "node-dispatched", "worked"), 2);
    assert_eq!(
        guard_runs(&world, "worked"),
        1,
        "adopt ran a guard whose verdict it had"
    );
    assert_eq!(count(&world, run, "guard-started", "worked"), 1);

    let go = at(&world, "interrupted.go");
    let ran = at(&world, "guards.ran");
    let interrupted = script(
        &world,
        "interrupted",
        &format!(
            "echo \"$ONEPIPELINE_NODE_ID\" >> '{ran}'\n\
             while [ ! -f '{go}' ]; do sleep 0.1; done\necho 'still nothing to do' >&2\nexit 3"
        ),
    );
    let run = "guarddied";
    let project = world.plan(
        run,
        &plan_of(
            run,
            vec![
                guarded(agent("asked", &[]), &interrupted),
                agent("after", &["asked"]),
            ],
        ),
    );
    world.run(&["start", &project, "--detach"]).exited(0);
    world.until("the guard to start", |world| {
        guard_runs(world, "asked") == 1
    });
    kill_driver(&world, run);
    assert!(position(&world, run, "guard-evaluated", "asked").is_none());
    std::fs::write(world.root.join("interrupted.go"), "").expect("the guard is released");
    world.run(&["adopt", run]).exited(0);
    assert_eq!(
        guard_runs(&world, "asked"),
        2,
        "the adopting driver did not run the guard again"
    );
    assert_eq!(count(&world, run, "guard-evaluated", "asked"), 1);
    let settled = result(&world, run, "asked");
    assert_eq!(settled["outcome"], json!(word("outcomes", "not_needed")));
    assert_eq!(settled["detail"], "still nothing to do");
    assert!(dispatched(&world, run, "after"));
    let log = std::fs::read_to_string(log_of(&world, run, "asked")).expect("its log");
    assert_eq!(log.matches("still nothing to do").count(), 1, "{log}");
}

/// A driver stopped after journalling a guard's verdict and before the settlement that
/// verdict leads to: the adopting driver settles the node from the journalled verdict —
/// `not-needed`, or `guard-failed` saying the exit status, signal, deadline or failure to
/// start the record names — without running the guard again, its detail naming the log that
/// kept what the guard said.
///
/// Unix only, as the other teardown journeys here are. The driver is held at that moment by
/// the engine's own debug failpoint, `after-guard-evaluated`, so the stop lands there every
/// time rather than by luck. A guard that ran again would journal a second `guard-started`.
#[cfg(unix)]
#[test]
fn adopt_settles_a_node_from_a_verdict_its_driver_journalled_and_never_settled() {
    let world = World::new("guard-answered");
    let seam = world.root.join("failpoints");
    std::fs::create_dir_all(&seam).expect("the failpoint directory");
    let world = world.with_env("ONEPIPELINE_REVIEW_FAILPOINT", &seam.to_string_lossy());
    let failed = word("outcomes", "failed");
    let cases = [
        (
            "notneeded",
            counting_guard(&world, "answers", exit("not_needed"), "said once"),
            None,
            "done",
            word("outcomes", "not_needed"),
            "the guard found the node not needed",
        ),
        (
            "exited",
            counting_guard(&world, "fails", 1, "said once"),
            None,
            "failed",
            failed.clone(),
            "the guard exited with status 1",
        ),
        (
            "signalled",
            script(&world, "signalled", "kill -9 $$"),
            None,
            "failed",
            failed.clone(),
            "the guard was ended by a signal",
        ),
        (
            "timedout",
            holding_guard(&world, "outlives"),
            Some(1),
            "failed",
            failed.clone(),
            "the guard timed out after 1 s",
        ),
        (
            "missing",
            world.root.join("no-such-guard"),
            None,
            "failed",
            failed,
            "the guard could not be started",
        ),
    ];
    for (case, guard, deadline, status, outcome, says) in cases {
        let run = format!("answered{case}");
        let asked = format!("asked-{case}");
        let mut node = guarded(agent(&asked, &[]), &guard);
        if let Some(deadline) = deadline {
            node["guard_timeout_seconds"] = json!(deadline);
        }
        let project = world.plan(
            &run,
            &plan_of(&run, vec![node, agent("after", &[asked.as_str()])]),
        );
        let stage = seam.join("after-guard-evaluated");
        let reached = seam.join("after-guard-evaluated.reached");
        let _ = std::fs::remove_file(&reached);
        std::fs::write(&stage, "").expect("the failpoint is armed");
        world.run(&["start", &project, "--detach"]).exited(0);
        world.until("the driver to stop after the verdict", |_| {
            reached.is_file()
        });
        kill_driver(&world, &run);
        std::fs::remove_file(&stage).expect("the failpoint is disarmed");
        assert_eq!(count(&world, &run, "guard-evaluated", &asked), 1);
        assert!(
            !settled_as(&world, &run, &asked, status),
            "{run}: settled before the driver stopped"
        );

        let adopted = world.run(&["adopt", &run]);
        assert!(adopted.code == 0 || status != "done", "{}", adopted.stderr);
        assert_eq!(
            count(&world, &run, "guard-started", &asked),
            1,
            "{run}: the guard was run again"
        );
        assert_eq!(count(&world, &run, "guard-evaluated", &asked), 1);
        let settled = result(&world, &run, &asked);
        assert_eq!(settled["status"], status, "{run}: {settled}");
        assert_eq!(settled["outcome"], json!(outcome), "{run}: {settled}");
        let detail = settled["detail"].as_str().expect("a detail");
        assert!(detail.contains(says), "{run}: {detail}");
        assert!(
            detail.contains(&log_of(&world, &run, &asked).display().to_string()),
            "{run}: {detail}"
        );
        assert_eq!(
            dispatched(&world, &run, "after"),
            status == "done",
            "{run}: the dependent's dispatch does not follow the verdict"
        );
    }
}
