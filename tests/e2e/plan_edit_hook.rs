//! The plan-edit hook, driven end to end through the real reply path.
//!
//! A launch names one command, and every node a reply's `add`, `retry`, `requeue`
//! (stating `task`, `steps` or `kind`) or `amend` introduces or changes is offered to it
//! with the task that node's next dispatch is given. What runs here is a **real** command —
//! a shell script this suite writes, which keeps every document it is handed and answers
//! with the exit status a journey arms — so what the hook saw is read off the process that
//! ran, and what a dispatch was given is read off the `--task` the agent-graph double was
//! really launched with.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The hook is the host's own command, and this suite supplies a real one.
// `harness.rs` carries the same suppression and the full rationale.

// llmlint: ignore-file[tests_mirror_real_usage] the one assertion about what a dispatch was
// handed reads the `--task` of the `oneagentgraph run` this crate really issued — the
// product boundary a dispatch is, as `amend.rs` says at length — because no view renders a
// dispatch's whole prose. Everything else is asserted through the CLI and the run's journal.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::harness::{
    agent, contract_block, human, plan_of, posix_script, shell_path, World, REFUSED,
};

/// What the contract's `generic_primitives` block states about one of its keys.
fn stated(key: &str) -> Value {
    contract_block("generic_primitives")[key].clone()
}

/// The keys of a JSON object, sorted.
fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .unwrap_or_else(|| panic!("{value} is not an object"))
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// The words the contract lists under `key`, sorted.
fn words(key: &str) -> Vec<String> {
    let mut words: Vec<String> = stated(key)
        .as_array()
        .unwrap_or_else(|| panic!("the contract states no list {key}"))
        .iter()
        .map(|word| word.as_str().expect("a word").to_owned())
        .collect();
    words.sort();
    words
}

/// The exit status the contract gives the hook's `verdict`.
fn exit(verdict: &str) -> String {
    stated("plan_edit_hook_exits")[verdict].to_string()
}

/// A ruling a manager issues, which binds the node's judge.
const RULING: &str = "Keep the public signature unchanged; callers outside this repository use it.";

/// The operational notes the node's own task states, which the composed task keeps.
const NOTES: &str = "## Additional info\nRun the narrow test module, never the whole suite.";

fn envelope(commands: Value) -> String {
    json!({"version": 2, "commands": commands}).to_string()
}

/// An agent node whose task carries operational notes below its criteria.
fn noted(id: &str, deps: &[&str]) -> Value {
    let mut node = agent(id, deps);
    node["task"] = json!(format!(
        "## What\nDo {id}.\n\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- {id} \
         is done.\n\n{NOTES}"
    ));
    node
}

/// The hook: it keeps each document under `hook.<n>.json`, writes its pid, sleeps where
/// `hook.sleep` says, and exits with `hook.exit`'s status (`0` where there is none) — or `1`,
/// saying `hook.refuse-<id>`'s words, for a node such a file names.
fn hook(world: &World) -> PathBuf {
    let base = shell_path(&world.root.join("hook"));
    posix_script(
        &world.root,
        "plan-edit-hook",
        &format!(
            "n=$(( $(cat '{base}.count' 2>/dev/null || echo 0) + 1 ))\n\
             echo $n > '{base}.count'\n\
             cat > '{base}.'$n'.json'\n\
             echo $$ > '{base}.pid'\n\
             echo \"$*\" > '{base}.args'\n\
             if [ -f '{base}.sleep' ]; then sleep \"$(cat '{base}.sleep')\"; fi\n\
             for refusal in '{base}'.refuse-*; do\n\
               [ -f \"$refusal\" ] || continue\n\
               id=${{refusal##*.refuse-}}\n\
               if grep -q \"\\\"node\\\":{{\\\"id\\\":\\\"$id\\\"\" '{base}.'$n'.json'; then cat \"$refusal\" >&2; exit 1; fi\n\
             done\n\
             if [ -f '{base}.say' ]; then cat '{base}.say' >&2; fi\n\
             exit $(cat '{base}.exit' 2>/dev/null || echo 0)"
        ),
    )
}

/// Every document the hook has been handed, in order.
fn offers(world: &World) -> Vec<Value> {
    let count = std::fs::read_to_string(world.root.join("hook.count"))
        .map(|count| count.trim().parse::<usize>().expect("the hook counts"))
        .unwrap_or(0);
    (1..=count)
        .map(|n| {
            let path = world.root.join(format!("hook.{n}.json"));
            serde_json::from_slice(&std::fs::read(&path).expect("the hook kept its stdin"))
                .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()))
        })
        .collect()
}

/// Each offer as `(op, node id)`.
fn offered(world: &World) -> Vec<(String, String)> {
    offers(world)
        .iter()
        .map(|offer| {
            (
                offer["op"].as_str().expect("an op").to_owned(),
                offer["node"]["id"].as_str().expect("a node").to_owned(),
            )
        })
        .collect()
}

/// A launch config naming the hook at `timeout` seconds.
fn config(world: &World, timeout: u64) -> String {
    let path = world.root.join("launch.json");
    std::fs::write(
        &path,
        json!({
            "schema_version": 14,
            "plan_edit_hook": {
                "command": [hook(world).display().to_string(), "--bar", "strict"],
                "timeout_seconds": timeout,
            },
        })
        .to_string(),
    )
    .expect("the launch config is written");
    path.display().to_string()
}

/// Start a run whose `slow` node is held open beside nodes that wait on it.
fn held(world: &World, name: &str, nodes: Vec<Value>, extra: &[&str]) -> String {
    world.script("slow.wait", "hold");
    let mut all = vec![agent("slow", &[])];
    all.extend(nodes);
    let path = world.plan(name, &plan_of(name, all));
    let mut args = vec!["start", path.as_str(), "--detach"];
    args.extend(extra);
    world.run(&args).exited(0);
    world.until("the held node to be running", |world| {
        world
            .run(&["status", name])
            .stdout
            .contains("slow: running")
    });
    name.to_owned()
}

/// The `--task` each dispatch of `node` was launched with, in order.
fn tasks_dispatched(world: &World, node: &str) -> Vec<String> {
    let mine = format!("## What\nDo {node}.");
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .filter_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|arg| arg == "--task")?;
            args.get(at + 1)?.as_str().map(str::to_owned)
        })
        .filter(|task| task.starts_with(&mine))
        .collect()
}

fn committed(world: &World, run: &str) -> usize {
    world.events_of(run, "edit-committed").len()
}

fn rejected(world: &World, run: &str) -> Vec<Value> {
    world.events_of(run, "edit-rejected")
}

/// The journey the hook exists for: an `amend` is offered with the node's composed task —
/// the ruling under its criteria, its operational notes kept — and that body is, byte for
/// byte, the task the node's next dispatch is then given.
#[test]
fn an_amend_is_offered_with_the_task_the_nodes_next_dispatch_is_given() {
    let world = World::new("plan-edit-hook-amend");
    let launch = config(&world, 60);
    let run = held(
        &world,
        "hookamend",
        vec![noted("later", &["slow"])],
        &["--launch-config", &launch],
    );

    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "later", "text": RULING}])),
        )
        .exited(0)
        .out_has("\"applied\"");
    let offer = offers(&world);
    assert_eq!(offer.len(), 1, "{offer:?}");
    let offer = &offer[0];
    assert_eq!(
        std::fs::read_to_string(world.root.join("hook.args"))
            .expect("the hook kept its arguments")
            .trim(),
        "--bar strict",
        "the hook was not spawned with the arguments its launch config names"
    );
    let shape = stated("plan_edit_hook_document");
    assert_eq!(keys(offer), keys(&shape), "{offer}");
    assert_eq!(keys(&offer["node"]), keys(&shape["node"]), "{offer}");
    assert_eq!(keys(&offer["task"]), keys(&shape["task"]), "{offer}");
    assert_eq!(offer["schema_version"], shape["schema_version"]);
    assert_eq!(offer["run"], run);
    assert!(
        offer["plan"]
            .as_str()
            .expect("the plan")
            .ends_with(":hookamend-board"),
        "{offer}"
    );
    assert_eq!(offer["op"], "amend");
    assert_eq!(offer["node"]["id"], "later");
    assert_eq!(offer["node"]["kind"], "agent");
    assert_eq!(offer["node"]["persona"], "engineer");
    assert_eq!(offer["node"]["deps"], json!(["slow"]));
    let body = offer["task"]["body"].as_str().expect("a body").to_owned();
    let criteria = body.find("## Acceptance criteria").expect("its criteria");
    let ruling = body.find(RULING).expect("the ruling reached the hook");
    let notes = body
        .find(NOTES)
        .expect("its operational notes reached the hook");
    assert!(
        criteria < ruling && ruling < notes,
        "the ruling is not under the criteria and above the notes:\n{body}"
    );
    assert!(
        body.contains(onepipeline::plan::AMENDMENT_HEADING),
        "{body}"
    );

    world.release("slow.go");
    world.until("the amended node to be dispatched", |_| {
        !tasks_dispatched(&world, "later").is_empty()
    });
    assert_eq!(
        tasks_dispatched(&world, "later")[0],
        body,
        "the hook was shown a task other than the one the dispatch was given"
    );
}

/// Exit 1 refuses the whole reply with the hook's words, exit 2 refuses it as not judged,
/// a reply changing two nodes whose second the hook refuses applies neither — and exit 0
/// applies it. Every refusal is journalled `edit-rejected`, and no node's task moves.
#[test]
fn a_refusal_or_no_verdict_refuses_the_whole_reply_and_changes_no_node() {
    let world = World::new("plan-edit-hook-refuses");
    let launch = config(&world, 60);
    let run = held(
        &world,
        "hookrefuses",
        vec![noted("later", &["slow"]), noted("other", &["slow"])],
        &["--launch-config", &launch],
    );
    let amend = |id: &str| json!({"op": "amend", "id": id, "text": RULING});
    let unchanged = |world: &World| {
        world.run(&["status", &run]).exited(0).out_lacks(RULING);
        assert_eq!(committed(world, &run), 0, "an edit was committed");
    };

    let said = "the amendment names a procedure rather than a property";
    std::fs::write(world.root.join("hook.say"), said).expect("the words are armed");
    std::fs::write(world.root.join("hook.exit"), exit("refuse")).expect("the refusal is armed");
    world
        .run_with_stdin(&["reply", &run], &envelope(json!([amend("later")])))
        .exited(REFUSED)
        .err_has(said)
        .err_has("refused node 'later'");
    let records = rejected(&world, &run);
    assert_eq!(records.len(), 1, "{records:?}");
    let reason = records[0]["payload"]["reason"].as_str().expect("a reason");
    assert!(
        reason.contains("plan-edit hook") && reason.contains("'later'"),
        "{reason}"
    );
    assert_eq!(records[0]["payload"]["command"]["op"], "amend");
    unchanged(&world);

    std::fs::write(world.root.join("hook.exit"), "2").expect("no verdict is armed");
    world
        .run_with_stdin(&["reply", &run], &envelope(json!([amend("later")])))
        .exited(REFUSED)
        .err_has("could not judge node 'later'")
        .err_has("exited 2");
    assert_eq!(rejected(&world, &run).len(), 2);
    unchanged(&world);

    // Two nodes, the first passed and the second refused: neither applies, and the journal
    // gains the one rejection.
    std::fs::remove_file(world.root.join("hook.say")).expect("the words are lifted");
    std::fs::write(world.root.join("hook.exit"), exit("pass")).expect("a pass is armed");
    std::fs::write(world.root.join("hook.refuse-other"), "other may not move").expect("armed");
    let before = offered(&world).len();
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([amend("later"), amend("other")])),
        )
        .exited(REFUSED)
        .err_has("other may not move")
        .err_has("refused node 'other'");
    assert_eq!(
        offered(&world)[before..],
        [
            ("amend".to_owned(), "later".to_owned()),
            ("amend".to_owned(), "other".to_owned())
        ]
    );
    assert_eq!(rejected(&world, &run).len(), 3);
    unchanged(&world);

    std::fs::remove_file(world.root.join("hook.refuse-other")).expect("the refusal is lifted");
    world
        .run_with_stdin(&["reply", &run], &envelope(json!([amend("later")])))
        .exited(0)
        .out_has("\"applied\"");
    world.run(&["status", &run]).exited(0).out_has(RULING);
    world.run(&["stop", &run]);
}

/// The replies a host's edits take, with a hook and without one: `add`, `retry`, and a
/// `requeue` stating `task`, `steps` or `kind` are each offered once per node they
/// introduce or change; a `note`, and a `requeue` stating none of the three, are never
/// offered; and with no hook named every one of them applies as before.
#[test]
fn each_op_that_changes_what_a_node_is_judged_against_is_offered_once_and_no_other() {
    for hooked in [true, false] {
        let name = if hooked { "hookops" } else { "nohookops" };
        let world = World::new(&format!("plan-edit-hook-{name}"));
        world.script("broken.fail", "");
        let launch = config(&world, 60);
        let extra: Vec<&str> = if hooked {
            vec!["--launch-config", &launch]
        } else {
            Vec::new()
        };
        let run = held(&world, name, vec![agent("broken", &[])], &extra);
        world.until("the broken node to fail", |world| {
            world
                .run(&["results", &run])
                .stdout
                .lines()
                .any(|line| line.trim_start().starts_with("broken") && line.contains("failed"))
        });
        let reply = |commands: Value| {
            world
                .run_with_stdin(&["reply", &run], &envelope(commands))
                .exited(0);
        };
        let parked = |id: &str| {
            let mut node = agent(id, &["slow"]);
            node["parked"] = json!(true);
            node
        };
        reply(json!([{"op": "add", "node": agent("fresh", &["slow"])}]));
        reply(json!([{"op": "retry", "id": "broken", "node": agent("mended", &[])}]));
        reply(json!([
            {"op": "add", "node": parked("tasked")},
            {"op": "add", "node": {"id": "kinded", "kind": "human", "parked": true, "deps": ["slow"],
                                   "task": "## What\nDo kinded.\n\n## Acceptance criteria\n- kinded is done."}},
            {"op": "add", "node": parked("bare")},
        ]));
        reply(json!([{"op": "requeue", "id": "tasked", "amend": {
            "task": "## What\nDo tasked again.\n\n## Acceptance criteria\n- tasked is done."
        }}]));
        reply(
            json!([{"op": "requeue", "id": "kinded", "amend": {"kind": "agent", "persona": "engineer"}}]),
        );
        reply(json!([{"op": "requeue", "id": "bare", "amend": {"max_turns": 7}}]));
        // Two commands naming one node offer it once.
        reply(json!([
            {"op": "amend", "id": "fresh", "text": RULING},
            {"op": "amend", "id": "fresh", "text": RULING},
        ]));
        reply(json!([{"op": "note", "id": "fresh", "addressee": "worker",
                      "text": "the fixture is under tests/", "deliver": "next"}]));
        if hooked {
            assert_eq!(
                offered(&world),
                [
                    ("add", "fresh"),
                    ("retry", "mended"),
                    ("add", "tasked"),
                    ("add", "kinded"),
                    ("add", "bare"),
                    ("requeue", "tasked"),
                    ("requeue", "kinded"),
                    ("amend", "fresh"),
                ]
                .map(|(op, id)| (op.to_owned(), id.to_owned())),
            );
            let mut ops: Vec<String> = offered(&world).into_iter().map(|(op, _)| op).collect();
            ops.sort();
            ops.dedup();
            assert_eq!(ops, words("plan_edit_hook_ops"));
        } else {
            assert!(offered(&world).is_empty(), "a run naming no hook ran it");
        }
        assert!(
            rejected(&world, &run).is_empty(),
            "{:?}",
            rejected(&world, &run)
        );
        world.run(&["stop", &run]);
    }
}

/// A run adopted after its driver stopped still offers an `amend` to the hook its launch
/// named — by `--plan-edit-hook`, replayed off the launch record — and its refusal still
/// refuses the reply.
#[test]
fn an_adopted_run_still_offers_its_edits_to_the_hook_its_launch_named() {
    let world = World::new("plan-edit-hook-adopted");
    let path = world.plan(
        "hookadopted",
        &plan_of(
            "hookadopted",
            vec![human("approve", &[]), noted("later", &["approve"])],
        ),
    );
    let hook = hook(&world);
    world
        .run(&[
            "start",
            &path,
            "--detach",
            "--plan-edit-hook",
            &hook.display().to_string(),
        ])
        .exited(0);
    let run = "hookadopted";
    world.until("the driver to pause on the approval", |world| {
        world.run(&["status", run]).stdout.contains("PAUSED")
    });
    let recorded = &world.run_json(run, "launch.json")[stated("launch_record_key")
        .as_str()
        .expect("the record's key")];
    assert_eq!(recorded["command"], json!([hook.display().to_string()]));
    assert_eq!(
        recorded["timeout_seconds"],
        stated("plan_edit_hook_default_timeout_seconds")
    );
    world.run(&["adopt", run]).exited(0);
    world.until("the adopting driver to pause again", |world| {
        world.run(&["status", run]).stdout.contains("PAUSED")
    });

    std::fs::write(world.root.join("hook.exit"), exit("refuse")).expect("the refusal is armed");
    std::fs::write(world.root.join("hook.say"), "not on an adopted run").expect("armed");
    world
        .run_with_stdin(
            &["reply", run],
            &envelope(json!([{"op": "amend", "id": "later", "text": RULING}])),
        )
        .exited(REFUSED)
        .err_has("not on an adopted run");
    assert_eq!(offered(&world), [("amend".to_owned(), "later".to_owned())]);
    assert_eq!(rejected(&world, run).len(), 1);
    world.run(&["status", run]).exited(0).out_lacks(RULING);
}

/// A hook outliving its `timeout_seconds` refuses the whole reply as not judged, journalled,
/// with no node's task changed and the hook's process gone.
#[test]
fn a_hook_outliving_its_timeout_refuses_the_reply_as_not_judged() {
    let world = World::new("plan-edit-hook-timeout");
    let launch = config(&world, 2);
    let run = held(
        &world,
        "hooktimeout",
        vec![noted("later", &["slow"])],
        &["--launch-config", &launch],
    );
    std::fs::write(world.root.join("hook.sleep"), "60").expect("the hold is armed");
    let started = std::time::Instant::now();
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "amend", "id": "later", "text": RULING}])),
        )
        .exited(REFUSED)
        .err_has("could not judge node 'later'")
        .err_has("2-second timeout");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(50),
        "the reply waited out the hook rather than its timeout"
    );
    assert_eq!(rejected(&world, &run).len(), 1);
    assert_eq!(committed(&world, &run), 0);
    world.run(&["status", &run]).exited(0).out_lacks(RULING);
    #[cfg(unix)]
    {
        let pid = std::fs::read_to_string(world.root.join("hook.pid"))
            .expect("the hook wrote its pid")
            .trim()
            .to_owned();
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("this host answers about a process it owns")
            .success();
        assert!(!alive, "the hook {pid} outlived its timeout");
    }
    world.run(&["stop", &run]);
}

/// A run paused on a person, so a reply is applied by the replying process itself.
fn paused(world: &World, name: &str, extra: &[&str], from: &std::path::Path) -> String {
    let path = world.plan(
        name,
        &plan_of(
            name,
            vec![human("approve", &[]), noted("later", &["approve"])],
        ),
    );
    let mut args = vec!["start", path.as_str(), "--detach"];
    args.extend(extra);
    world.run_from(from, &args).exited(0);
    world.until("the run to pause on the approval", |world| {
        world.run(&["status", name]).stdout.contains("PAUSED")
    });
    name.to_owned()
}

fn amend_later(world: &World, run: &str) -> crate::harness::Run {
    world.run_with_stdin(
        &["reply", run],
        &envelope(json!([{"op": "amend", "id": "later", "text": RULING}])),
    )
}

/// The launch config's hook naming no program, a zero timeout, or the key below schema 14 is
/// refused by the key's name before any run exists; one naming no timeout takes the default.
#[test]
fn a_hook_naming_no_program_or_no_time_is_refused_at_launch_and_none_takes_the_default() {
    let world = World::new("plan-edit-hook-config");
    let hook = hook(&world).display().to_string();
    let path = world.plan(
        "hookconfig",
        &plan_of("hookconfig", vec![human("approve", &[])]),
    );
    let config = world.root.join("launch.json");
    for refused in [
        json!({"schema_version": 14, "plan_edit_hook": {"command": []}}),
        json!({"schema_version": 14, "plan_edit_hook": {"command": [" "]}}),
        json!({"schema_version": 14, "plan_edit_hook": {"command": [hook], "timeout_seconds": 0}}),
        json!({"schema_version": 13, "plan_edit_hook": {"command": [hook]}}),
        json!({"schema_version": 13, "plan_edit_hook": null}),
    ] {
        std::fs::write(&config, refused.to_string()).expect("written");
        world
            .run(&[
                "start",
                &path,
                "--detach",
                "--launch-config",
                &config.display().to_string(),
            ])
            .exited(REFUSED)
            .err_has("plan_edit_hook");
        assert!(!world.run_file("hookconfig", "launch.json").is_file());
    }
    std::fs::write(
        &config,
        json!({"schema_version": 14, "plan_edit_hook": {"command": [hook]}}).to_string(),
    )
    .expect("written");
    world
        .run(&[
            "start",
            &path,
            "--detach",
            "--launch-config",
            &config.display().to_string(),
        ])
        .exited(0);
    assert_eq!(
        world.run_json("hookconfig", "launch.json")["plan_edit_hook"]["timeout_seconds"],
        stated("plan_edit_hook_default_timeout_seconds")
    );
    world.run(&["stop", "hookconfig"]);
}

/// `--plan-edit-hook` replaces the launch config's hook entirely — its command and its
/// timeout — and the config's hook is never run.
#[test]
fn the_flag_replaces_the_configs_hook_and_its_timeout() {
    let world = World::new("plan-edit-hook-flag");
    let flagged = hook(&world);
    let ran = shell_path(&world.root.join("config-hook.ran"));
    let configured = posix_script(
        &world.root,
        "config-hook",
        &format!("touch '{ran}'\nexit 0"),
    );
    let config = world.root.join("launch.json");
    std::fs::write(
        &config,
        json!({"schema_version": 14, "plan_edit_hook": {
            "command": [configured.display().to_string()], "timeout_seconds": 1}})
        .to_string(),
    )
    .expect("written");
    let run = paused(
        &world,
        "hookflag",
        &[
            "--launch-config",
            &config.display().to_string(),
            "--plan-edit-hook",
            &flagged.display().to_string(),
        ],
        &world.root,
    );
    let recorded = &world.run_json(&run, "launch.json")["plan_edit_hook"];
    assert_eq!(recorded["command"], json!([flagged.display().to_string()]));
    assert_eq!(
        recorded["timeout_seconds"],
        stated("plan_edit_hook_default_timeout_seconds")
    );
    amend_later(&world, &run).exited(0);
    assert_eq!(offered(&world), [("amend".to_owned(), "later".to_owned())]);
    assert!(
        !world.root.join("config-hook.ran").exists(),
        "the config's hook ran"
    );
}

/// A hook that cannot be started, or that a signal ends, gives no verdict: the reply is
/// refused as not judged and journalled. It runs from the directory the run was launched
/// from, whatever directory the reply is sent from.
#[test]
fn a_hook_that_cannot_start_or_is_signalled_is_not_judged_and_runs_from_the_launch_dir() {
    let world = World::new("plan-edit-hook-ends");
    let missing = world.root.join("no-such-hook");
    let run = paused(
        &world,
        "hookmissing",
        &["--plan-edit-hook", &missing.display().to_string()],
        &world.root,
    );
    amend_later(&world, &run)
        .exited(REFUSED)
        .err_has("could not judge node 'later'")
        .err_has("could not be started");
    assert_eq!(rejected(&world, &run).len(), 1);

    #[cfg(unix)]
    {
        let launch_dir = world.root.join("launched-here");
        std::fs::create_dir_all(&launch_dir).expect("a launch directory");
        let cwd = shell_path(&world.root.join("signalled.cwd"));
        let signalled = posix_script(
            &world.root,
            "signalled-hook",
            &format!("cat > /dev/null\npwd > '{cwd}'\nkill -9 $$"),
        );
        let run = paused(
            &world,
            "hooksignalled",
            &["--plan-edit-hook", &signalled.display().to_string()],
            &launch_dir,
        );
        amend_later(&world, &run)
            .exited(REFUSED)
            .err_has("could not judge node 'later'")
            .err_has("ended by a signal");
        assert_eq!(rejected(&world, &run).len(), 1);
        let ran_in = std::fs::read_to_string(world.root.join("signalled.cwd")).expect("its cwd");
        assert_eq!(
            std::fs::canonicalize(ran_in.trim()).expect("a directory"),
            std::fs::canonicalize(&launch_dir).expect("a directory"),
            "the hook did not run from the launch directory"
        );
    }
}

/// A `requeue` stating `steps` — and no `task` — is offered like one stating a task, its body
/// each agent step's task in order: with `task` and `kind`, which the journey above requeues
/// with, every key the contract names.
#[test]
fn a_requeue_stating_steps_offers_each_agent_steps_task() {
    let world = World::new("plan-edit-hook-steps");
    world.repository("local-direct", &[]);
    let launch = config(&world, 60);
    let run = held(
        &world,
        "hooksteps",
        Vec::new(),
        &["--launch-config", &launch],
    );
    let step = |id: &str| {
        json!({"id": id, "persona": "engineer",
               "task": format!("## What\nDo {id}.\n\n## Acceptance criteria\n- {id} is done.")})
    };
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "add", "node": {
                "id": "stepped", "repo": "service", "title": "feat: stepped", "parked": true,
                "deps": ["slow"], "steps": [step("build")]}}])),
        )
        .exited(0);
    world
        .run_with_stdin(
            &["reply", &run],
            &envelope(json!([{"op": "requeue", "id": "stepped", "amend": {
                "steps": [step("build"), step("check")]}}])),
        )
        .exited(0);
    assert_eq!(
        offered(&world),
        [("add", "stepped"), ("requeue", "stepped")].map(|(op, id)| (op.to_owned(), id.to_owned()))
    );
    let body = offers(&world)[1]["task"]["body"]
        .as_str()
        .expect("a body")
        .to_owned();
    let build = body.find("# Step build").expect("the first step");
    let check = body.find("# Step check").expect("the second step");
    assert!(build < check, "{body}");
    let mut exercised = vec!["kind".to_owned(), "steps".to_owned(), "task".to_owned()];
    exercised.sort();
    assert_eq!(words("requeue_keys"), exercised);
    world.run(&["stop", &run]);
}

/// A hook that exits while a process it started still holds its stderr is answered on its
/// own exit — its refusal carrying what it said — without waiting on what it left behind.
#[test]
fn a_hook_whose_stderr_outlives_it_is_answered_on_its_own_exit() {
    let world = World::new("plan-edit-hook-held-stderr");
    let leaver = posix_script(
        &world.root,
        "leaving-hook",
        "cat > /dev/null\nsleep 20 >&2 &\necho 'refused, and left a sleeper' >&2\nexit 1",
    );
    let run = paused(
        &world,
        "hookheld",
        &["--plan-edit-hook", &leaver.display().to_string()],
        &world.root,
    );
    let asked = std::time::Instant::now();
    amend_later(&world, &run)
        .exited(REFUSED)
        .err_has("refused, and left a sleeper");
    assert!(
        asked.elapsed() < std::time::Duration::from_secs(15),
        "the reply waited on the process the hook left behind"
    );
    assert_eq!(rejected(&world, &run).len(), 1);
}
