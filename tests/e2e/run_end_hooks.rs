//! The run-end hooks, driven end to end against the compiled binary.
//!
//! A launch names a success hook and a failure hook; the engine judges the run
//! when a driver lets go of it, or when `stop` tears it down, and fires at most
//! one hook, once. Every journey here names a **real** command — the fixture pair
//! `run_end_hook.sh` / `run_end_hook.bat` — which records its working directory,
//! its environment and its stdin, so what a hook was handed is read off the
//! process that ran rather than off anything this crate wrote down about it.
//!
//! The contract's own run-end hooks block is read rather than restated: the log
//! path, the environment a hook is given, the stdin document's shape and how many
//! lines of output `results` repeats all come out of `docs/contract.md`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary; the one lifecycle journey publishes through the linked `onevcs` against
// a real git origin. The hook is not a substitution either: it is the operator's own
// command, and this suite supplies a real one. `harness.rs` carries the same suppression
// and the full rationale.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{
    agent, human, lifecycle, plan_of, repo_file, Run, World, NOTHING_DRIVING, REFUSED, USAGE_ERROR,
};

/// Where the fixture records what it was handed. Read by the fixture itself.
const RECORD_ENV: &str = "ONEPIPELINE_E2E_HOOK_RECORD";

/// The contract's run-end hooks block.
fn contract_block() -> Value {
    let contract =
        std::fs::read_to_string(repo_file("docs/contract.md")).expect("the contract ships");
    let block = contract
        .split("```json")
        .skip(1)
        .filter_map(|rest| rest.split("```").next())
        .find(|block| block.contains("\"run_end_hooks\""))
        .expect("the contract carries the run-end hooks block");
    serde_json::from_str::<Value>(block).expect("the block is JSON")["run_end_hooks"].clone()
}

/// A world whose hook fixture records into the world's own scratch.
fn hooked_world(name: &str) -> World {
    let world = World::new(name);
    std::fs::create_dir_all(records(&world)).expect("a record directory");
    let record = records(&world).to_string_lossy().into_owned();
    world.with_env(RECORD_ENV, &record)
}

fn records(world: &World) -> PathBuf {
    world.root.join("hook-records")
}

/// The run-end hook fixture, placed in the world, as the command a launch names.
#[cfg(unix)]
fn hook(world: &World) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = world.root.join("run_end_hook.sh");
    std::fs::write(&path, include_str!("run_end_hook.sh")).expect("the hook is written");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("the hook is executable");
    path.to_string_lossy().into_owned()
}

/// The run-end hook fixture, placed in the world, as the command a launch names.
///
/// Written with CRLF, for the reason `harness::write_hook_script` gives: cmd seeks
/// a batch file by byte offset and assumes two bytes end a line.
#[cfg(windows)]
fn hook(world: &World) -> String {
    let path = world.root.join("run_end_hook.bat");
    std::fs::write(
        &path,
        include_str!("run_end_hook.bat").replace('\n', "\r\n"),
    )
    .expect("the hook is written");
    path.to_string_lossy().into_owned()
}

/// The hooks the fixture was run as for one run, in order.
fn invocations(world: &World, run: &str) -> Vec<String> {
    let path = records(world).join(run).join("invocations");
    // Absent is a hook never run, and nothing else is: this file is the only
    // witness to a firing, so an unreadable one is not "none".
    match std::fs::read_to_string(&path) {
        Ok(text) => text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("{} cannot be read: {error}", path.display()),
    }
}

/// What one invocation of the fixture was handed.
struct Handed {
    cwd: PathBuf,
    /// Each variable the fixture reports, `None` where it was unset.
    env: BTreeMap<String, Option<String>>,
    stdin: Value,
}

fn handed(world: &World, run: &str, nth: usize) -> Handed {
    let dir = records(world).join(run).join(nth.to_string());
    let read = |name: &str| {
        std::fs::read_to_string(dir.join(name)).unwrap_or_else(|error| {
            panic!("the hook recorded no {name} at {}: {error}", dir.display())
        })
    };
    let env = read("env")
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty())
        .map(|line| match line.split_once('=') {
            Some((key, value)) => (key.to_string(), Some(value.to_string())),
            None => (line.trim_end_matches(" unset").to_string(), None),
        })
        .collect();
    let stdin = read("stdin");
    Handed {
        cwd: PathBuf::from(read("cwd").trim()),
        env,
        stdin: serde_json::from_str(stdin.trim())
            .unwrap_or_else(|error| panic!("the hook was handed no JSON ({error}): {stdin}")),
    }
}

/// Whether two spellings name one directory or file.
fn same_place(one: &Path, other: &Path) -> bool {
    match (std::fs::canonicalize(one), std::fs::canonicalize(other)) {
        (Ok(one), Ok(other)) => one == other,
        _ => false,
    }
}

/// Start a run attached, from the world's project directory.
fn attached(world: &World, name: &str, nodes: Vec<Value>, extra: &[&str]) -> Run {
    let path = world.plan(name, &plan_of(name, nodes));
    let mut args = vec!["start", path.as_str(), "--attach"];
    args.extend_from_slice(extra);
    world.run_from(&world.project, &args)
}

fn hook_kinds(world: &World, run: &str) -> Vec<String> {
    world
        .kinds(run)
        .into_iter()
        .filter(|kind| kind.starts_with("run-hook-"))
        .collect()
}

/// Every node of a result that is not `done`, in the shape a failure reason
/// lists them — so a reason is held against what the run itself recorded.
fn not_done(result: &Value) -> Value {
    json!(result["nodes"]
        .as_array()
        .expect("the result lists nodes")
        .iter()
        .filter(|node| node["status"] != "done" && node["superseded_by"].is_null())
        .map(|node| json!({"id": node["id"], "status": node["status"], "outcome": node["outcome"]}))
        .collect::<Vec<_>>())
}

/// The lines `results` repeats of a hook's output.
fn output_lines(results: &str) -> Vec<String> {
    results
        .lines()
        .filter_map(|line| line.trim().strip_prefix("output: "))
        .map(str::to_string)
        .collect()
}

/// The last lines of a log, as many as the contract says `results` repeats.
fn tail_of(log: &str) -> Vec<String> {
    let lines: Vec<&str> = log.lines().collect();
    let shown = usize::try_from(
        contract_block()["results_output_lines"]
            .as_u64()
            .expect("the block says how many lines results repeats"),
    )
    .expect("a line count fits");
    lines[lines.len().saturating_sub(shown)..]
        .iter()
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect()
}

/// The journey the success hook exists for: every node settles done — under each
/// outcome that settles done — and the hook fires once, in the launch directory,
/// with the run named in its environment and the contract's document on its stdin.
#[test]
fn a_run_whose_nodes_all_settle_done_fires_the_success_hook_once_with_what_the_contract_states() {
    let world = hooked_world("hooks-success");
    let hook = hook(&world);
    world.repository("change-auto", &[]);
    world.script("gh.merged", "");
    world.script("drafted.work", "the worker wrote this\n");
    let mut drafted = lifecycle("drafted", &[]);
    drafted["draft"] = json!(true);
    let handoff = json!({
        "id": "handoff",
        "task": "## What\nRecord that nothing changes.",
        "expects_no_diff": true,
    });
    let run = "allgood";
    let started = attached(
        &world,
        run,
        vec![agent("build", &[]), handoff, drafted],
        &["--success-hook", &hook, "--failure-hook", &hook],
    );
    started.exited(0).out_has("\"settlement\":\"complete\"");

    let result = world.run_json(run, "result.json");
    let settled = |id: &str| {
        let node = result["nodes"]
            .as_array()
            .expect("the result lists nodes")
            .iter()
            .find(|node| node["id"] == id)
            .unwrap_or_else(|| panic!("{id} is missing from {result}"));
        (node["status"].clone(), node["outcome"].clone())
    };
    assert_eq!(settled("build").0, json!("done"));
    assert_eq!(settled("handoff"), (json!("done"), json!("no-changes")));
    assert_eq!(settled("drafted"), (json!("done"), json!("change-draft")));

    // Once, and only the success hook.
    assert_eq!(invocations(&world, run), ["success"], "{}", world.dump());
    let fired = world.events_of(run, "run-hook-fired");
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(
        fired[0]["payload"],
        json!({"hook": "success", "command": hook, "reason": null})
    );
    let block = contract_block();
    let log = world.runs.join(run).join(
        block["log"]
            .as_str()
            .expect("the block names the log")
            .replace("<hook>", "success"),
    );
    let finished = world.events_of(run, "run-hook-finished");
    assert_eq!(finished.len(), 1, "{finished:?}");
    assert_eq!(finished[0]["payload"]["hook"], "success");
    assert_eq!(finished[0]["payload"]["ending"], "succeeded");
    assert_eq!(finished[0]["payload"]["exit"], 0);
    let recorded_log = finished[0]["payload"]["log"].as_str().expect("a log path");
    assert!(
        same_place(Path::new(recorded_log), &log),
        "{recorded_log} is not {}",
        log.display()
    );

    // What it was handed, off the process that ran.
    let handed = handed(&world, run, 1);
    let launch = world.run_json(run, "launch.json");
    assert!(
        same_place(
            &handed.cwd,
            Path::new(launch["dir"].as_str().expect("a dir"))
        ) && same_place(&handed.cwd, &world.project),
        "the hook ran in {} rather than the launch directory",
        handed.cwd.display()
    );
    let environment: Vec<String> = serde_json::from_value(block["environment"].clone())
        .expect("the block names the environment");
    assert_eq!(
        environment,
        [
            "ONEPIPELINE_HOOK",
            "ONEPIPELINE_RUN_ID",
            "ONEPIPELINE_RUN_ROOT",
            "ONEPIPELINE_LAUNCHER",
            "ONEPIPELINE_LAUNCHER_SESSION"
        ],
        "the contract names an environment the fixture does not record"
    );
    assert_eq!(handed.env["hook"].as_deref(), Some("success"));
    assert_eq!(handed.env["run_id"].as_deref(), Some(run));
    let run_root = handed.env["run_root"].clone().expect("a run root");
    assert!(Path::new(&run_root).is_absolute(), "{run_root}");
    assert!(same_place(Path::new(&run_root), &world.runs.join(run)));
    assert_eq!(handed.env["launcher"], Some("e2e".to_string()));
    assert_eq!(
        handed.env["launcher"].as_deref(),
        launch["launcher"].as_str()
    );
    assert_eq!(handed.env["session"], Some(world.session.clone()));
    assert_eq!(handed.env["session"].as_deref(), launch["session"].as_str());
    assert_eq!(
        handed.stdin,
        json!({"version": 1, "hook": "success", "run_id": run, "run_root": run_root, "reason": null})
    );
    let keys = |document: &Value| -> Vec<String> {
        document
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect()
    };
    assert_eq!(keys(&handed.stdin), keys(&block["stdin"]["success"]));

    // Relayed on the attached driver's stderr, kept in the log, and rendered.
    started.err_has("said line 25").err_has("said on stderr");
    let kept = std::fs::read_to_string(&log).expect("the hook's log is kept");
    assert!(
        kept.contains("said line 1") && kept.contains("said on stderr"),
        "{kept}"
    );
    let results = world.run(&["results", run]);
    results.exited(0).out_has(&format!(
        "success hook fired — reason: none; ending: succeeded; exit: 0; log: {recorded_log}"
    ));
    assert_eq!(output_lines(&results.stdout), tail_of(&kept));
}

/// A hook that exits non-zero, one that cannot start, and one that outlives its
/// timeout are each recorded under their own ending — and none of them moves
/// anything the run settled to: exit status, settlement line, `result.json` and
/// node statuses are a hookless run's.
#[test]
fn a_hook_that_fails_cannot_start_or_outlives_its_timeout_changes_nothing_the_run_settles_to() {
    let world = hooked_world("hooks-endings");
    let hook = hook(&world);
    world.script("build.fail", "1");
    let nodes = || vec![agent("build", &[]), agent("ship", &["build"])];

    let plain = attached(&world, "plain", nodes(), &[]);
    plain.exited(NOTHING_DRIVING);
    assert!(hook_kinds(&world, "plain").is_empty());
    let baseline = world.run_json("plain", "result.json");

    let missing = world
        .root
        .join("no-such-hook")
        .to_string_lossy()
        .into_owned();
    std::fs::write(records(&world).join("exits.exit"), "7").expect("the exit is scripted");
    std::fs::write(records(&world).join("outlives.hold"), "").expect("the hold is scripted");
    for (run, ending, command, timeout, exit) in [
        ("exits", "failed", hook.as_str(), None, Some(7)),
        (
            "unstartable",
            "could-not-start",
            missing.as_str(),
            None,
            None,
        ),
        ("outlives", "timed-out", hook.as_str(), Some("1"), None),
    ] {
        let mut extra = vec!["--success-hook", command, "--failure-hook", command];
        if let Some(seconds) = timeout {
            extra.extend(["--hook-timeout", seconds]);
        }
        let began = Instant::now();
        let started = attached(&world, run, nodes(), &extra);

        // Exactly what the run without hooks settled to.
        assert_eq!(started.code, plain.code, "{run}: {}", started.stderr);
        assert_eq!(started.json()["settlement"], plain.json()["settlement"]);
        let mut result = world.run_json(run, "result.json");
        result["run_id"] = baseline["run_id"].clone();
        assert_eq!(
            result, baseline,
            "{run} settled differently beside its hook"
        );

        // The failure hook, as `nodes`: the failed node and the one it skipped.
        let fired = world.events_of(run, "run-hook-fired");
        assert_eq!(fired.len(), 1, "{run}: {fired:?}");
        assert_eq!(fired[0]["payload"]["hook"], "failure");
        assert_eq!(fired[0]["payload"]["command"], command);
        assert_eq!(fired[0]["payload"]["reason"]["kind"], "nodes");
        assert_eq!(fired[0]["payload"]["reason"]["nodes"], not_done(&baseline));
        assert_eq!(
            fired[0]["payload"]["reason"]["nodes"]
                .as_array()
                .expect("nodes")
                .iter()
                .map(|node| (node["id"].clone(), node["status"].clone()))
                .collect::<Vec<_>>(),
            [
                (json!("build"), json!("failed")),
                (json!("ship"), json!("skipped"))
            ]
        );

        let finished = world.events_of(run, "run-hook-finished");
        assert_eq!(finished.len(), 1, "{run}: {finished:?}");
        assert_eq!(finished[0]["payload"]["ending"], ending, "{run}");
        assert_eq!(finished[0]["payload"]["exit"], json!(exit), "{run}");
        let log = finished[0]["payload"]["log"].as_str().expect("a log path");
        assert!(same_place(
            Path::new(log),
            &world.runs.join(run).join("hooks").join("failure.log")
        ));
        let kept = std::fs::read_to_string(log).expect("the hook's log is kept");
        if ending == "could-not-start" {
            assert!(kept.contains("could not be started"), "{kept}");
            assert!(invocations(&world, run).is_empty());
        } else {
            assert!(
                kept.contains("said line 25") && kept.contains("said on stderr"),
                "{run}: {kept}"
            );
            assert_eq!(invocations(&world, run), ["failure"]);
        }
        if ending == "timed-out" {
            // The fixture holds for five minutes; ending it is the timeout's doing.
            assert!(
                began.elapsed() < Duration::from_secs(120),
                "the hook was waited out rather than ended at its timeout"
            );
        }

        let results = world.run(&["results", run]);
        results.exited(0).out_has(&format!(
            "failure hook fired — reason: nodes; ending: {ending}; exit: {}; log: {log}",
            exit.map_or_else(|| "none".to_string(), |code| code.to_string())
        ));
        assert_eq!(output_lines(&results.stdout), tail_of(&kept), "{run}");
    }
}

/// A frontier idled by a park, with nothing to decide, has ended unfinished.
#[test]
fn a_parked_frontier_with_no_decision_outstanding_fires_failure_as_unfinished() {
    let world = hooked_world("hooks-parked");
    let hook = hook(&world);
    let mut later = agent("later", &["build"]);
    later["parked"] = json!(true);
    let run = "idled";
    attached(
        &world,
        run,
        vec![agent("build", &[]), later],
        &["--success-hook", &hook, "--failure-hook", &hook],
    )
    .exited(NOTHING_DRIVING)
    .out_has("\"settlement\":\"unattended\"");

    assert_eq!(invocations(&world, run), ["failure"]);
    let reason = &world.events_of(run, "run-hook-fired")[0]["payload"]["reason"];
    assert_eq!(reason["kind"], "unfinished");
    assert_eq!(
        reason["nodes"],
        json!([{"id": "later", "status": "parked", "outcome": null}])
    );
    assert_eq!(
        reason["nodes"],
        not_done(&world.run_json(run, "result.json"))
    );
    assert_eq!(&handed(&world, run, 1).stdin["reason"], reason);
}

/// A node its upstream never released is neither failed nor parked, and a run
/// that ends over it has ended unfinished — divergence entry 72's ruling.
#[test]
fn a_run_that_ends_over_a_node_its_upstream_never_released_fires_failure_as_unfinished() {
    let world = hooked_world("hooks-unreleased");
    let hook = hook(&world);
    let mut ship = agent("ship", &[]);
    ship["deps"] = json!(["run:neverran#build"]);
    let run = "unreleased";
    attached(
        &world,
        run,
        vec![ship],
        &["--success-hook", &hook, "--failure-hook", &hook],
    )
    .exited(NOTHING_DRIVING);

    assert_eq!(invocations(&world, run), ["failure"]);
    let reason = &world.events_of(run, "run-hook-fired")[0]["payload"]["reason"];
    assert_eq!(reason["kind"], "unfinished");
    assert_eq!(
        reason["nodes"],
        json!([{"id": "ship", "status": "blocked", "outcome": null}])
    );
}

/// A clean stop fires the failure hook as `stopped` from the stop itself, with
/// the owner the launch record names — and a stop that was refused fires nothing.
#[test]
fn a_clean_stop_fires_failure_as_stopped_and_a_refused_stop_fires_nothing() {
    let world = hooked_world("hooks-stop");
    let hook = hook(&world);
    world.script("build.wait", "hold");
    let run = "stopped";
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world
        .run_from(
            &world.project,
            &[
                "start",
                &path,
                "--detach",
                "--success-hook",
                &hook,
                "--failure-hook",
                &hook,
            ],
        )
        .exited(0);
    world.until("a node to be in flight", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });

    let stranger = world.as_session("session-stranger");
    stranger
        .run(&["stop", run])
        .exited(REFUSED)
        .err_has("belongs to");
    assert!(hook_kinds(&world, run).is_empty());
    #[cfg(unix)]
    {
        // A teardown this host cannot establish is refused, and fires nothing.
        let mut blind = world.cmd(&["stop", run]);
        blind.env("PATH", world.path_whose_ps_fails());
        world
            .run_on(blind, "stop with a ps that fails")
            .exited(REFUSED)
            .err_has("was not stopped");
        assert!(hook_kinds(&world, run).is_empty());
    }
    assert!(invocations(&world, run).is_empty());

    stranger
        .run(&["stop", run, "--force"])
        .exited(0)
        .out_has("\"stopped\":true");
    // Fired by the stop before it returned, after the stop was journaled.
    assert_eq!(invocations(&world, run), ["failure"]);
    let kinds = world.kinds(run);
    let at = |kind: &str| kinds.iter().position(|seen| seen == kind);
    assert!(at("run-stopped") < at("run-hook-fired"), "{kinds:?}");
    assert_eq!(
        hook_kinds(&world, run),
        ["run-hook-fired", "run-hook-finished"]
    );
    let reason = &world.events_of(run, "run-hook-fired")[0]["payload"]["reason"];
    assert_eq!(reason["kind"], "stopped");
    assert_eq!(reason["nodes"][0]["id"], "build");
    assert_eq!(
        world.events_of(run, "run-hook-finished")[0]["payload"]["ending"],
        "succeeded"
    );

    // The run's owner, not the session that stopped it.
    let handed = handed(&world, run, 1);
    assert_eq!(handed.env["session"], Some(world.session.clone()));
    assert_eq!(handed.env["launcher"], Some("e2e".to_string()));
    assert_eq!(handed.stdin["reason"], *reason);
    world.release("build.go");
}

/// A launch whose record names no owner hands its hook no launcher variables
/// at all, rather than those of the process that fired it.
#[test]
fn a_run_whose_record_names_no_owner_hands_its_hook_no_launcher() {
    let world = hooked_world("hooks-unowned");
    let hook = hook(&world);
    let run = "unowned";
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    let mut launch = world.cmd(&["start", &path, "--attach", "--success-hook", &hook]);
    launch
        .current_dir(&world.project)
        .env_remove("ONEPIPELINE_LAUNCHER")
        .env_remove("ONEPIPELINE_LAUNCHER_SESSION");
    world
        .run_on(launch, "start with no launcher")
        .exited(0)
        .settled();

    assert_eq!(invocations(&world, run), ["success"]);
    let handed = handed(&world, run, 1);
    assert_eq!(handed.env["launcher"], None, "{:?}", handed.env);
    assert_eq!(handed.env["session"], None, "{:?}", handed.env);
}

/// A failed node a `retry` superseded has left the graph, so a run whose
/// replacement settles done has completed.
#[test]
fn a_failed_node_a_retry_supersedes_fires_success_once_its_replacement_settles_done() {
    let world = hooked_world("hooks-retry");
    let hook = hook(&world);
    world.script("build.fail", "1");
    let run = "retried";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &["--success-hook", &hook],
    )
    .exited(NOTHING_DRIVING);
    // The failure hook is not named, so that ending fires nothing and marks nothing.
    assert!(hook_kinds(&world, run).is_empty());

    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "retry", "id": "build", "node": agent("build-2", &[])}
            ]})
            .to_string(),
        )
        .exited(0);
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");

    assert_eq!(invocations(&world, run), ["success"]);
    assert_eq!(
        world.events_of(run, "run-hook-fired")[0]["payload"]["reason"],
        Value::Null
    );
    world
        .run(&["results", run])
        .exited(0)
        .out_has("superseded — retried as build-2");
}

/// A driver that lets go of a run paused on a decision fires nothing and says
/// so; the driver that adopts it after the decision fires the hook it reaches.
#[test]
fn a_run_paused_on_a_decision_withholds_its_hook_and_the_adopting_driver_fires_the_one_it_reaches()
{
    let world = hooked_world("hooks-paused");
    let hook = hook(&world);
    let run = "paused";
    attached(
        &world,
        run,
        vec![human("approve", &[]), agent("after", &["approve"])],
        &["--success-hook", &hook, "--failure-hook", &hook],
    )
    .exited(0)
    .out_has("\"settlement\":\"awaiting-planner\"")
    .err_has("paused on a decision");

    assert_eq!(hook_kinds(&world, run), ["run-hook-withheld"]);
    assert_eq!(
        world.events_of(run, "run-hook-withheld")[0]["payload"],
        json!({"settlement": "awaiting-planner"})
    );
    assert!(invocations(&world, run).is_empty());
    world.run(&["results", run]).exited(0).out_has(
        "run-end hook withheld — the run is paused on a decision (awaiting-planner), so no \
         hook fired",
    );

    world.run(&["attest", run, "approve"]).exited(0);
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");
    assert_eq!(invocations(&world, run), ["success"]);
    assert_eq!(
        hook_kinds(&world, run),
        ["run-hook-withheld", "run-hook-fired", "run-hook-finished"]
    );
}

/// Once a run carries a firing, nothing fires again: not a later adoption, and
/// not an adoption after a requeue that takes the run on to complete.
#[test]
fn once_a_hook_has_fired_no_later_adoption_fires_either_hook_again() {
    let world = hooked_world("hooks-once");
    let hook = hook(&world);
    let mut later = agent("later", &["build"]);
    later["parked"] = json!(true);
    let run = "once";
    attached(
        &world,
        run,
        vec![agent("build", &[]), later],
        &["--success-hook", &hook, "--failure-hook", &hook],
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);

    world.run(&["adopt", run]).exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);

    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "requeue", "id": "later"}]}).to_string(),
        )
        .exited(0);
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");

    assert_eq!(invocations(&world, run), ["failure"]);
    assert_eq!(
        hook_kinds(&world, run),
        ["run-hook-fired", "run-hook-finished"]
    );
}

/// A detached driver judges and fires exactly as an attached one does — and while
/// its hook runs, the run reads as nothing driving it, through the binary's own
/// views.
#[test]
fn a_detached_driver_fires_as_an_attached_one_does_and_its_run_reads_undriven_while_the_hook_runs()
{
    let world = hooked_world("hooks-detached");
    let hook = hook(&world);
    world.script("build.fail", "1");
    let nodes = || vec![agent("build", &[]), agent("ship", &["build"])];
    let flags = [
        "--success-hook",
        hook.as_str(),
        "--failure-hook",
        hook.as_str(),
    ];

    attached(&world, "attachedtwin", nodes(), &flags).exited(NOTHING_DRIVING);

    let run = "detachedtwin";
    std::fs::write(records(&world).join(format!("{run}.hold")), "").expect("the hold is scripted");
    let path = world.plan(run, &plan_of(run, nodes()));
    let mut args = vec!["start", path.as_str(), "--detach"];
    args.extend(flags);
    world.run_from(&world.project, &args).exited(0);
    let started = records(&world).join(run).join("1").join("started");
    world.until("the detached driver's hook to be running", |_| {
        started.is_file()
    });

    world
        .run(&["status", run])
        .exited(0)
        .out_has("DRIVER DEAD")
        .out_lacks("ACTIVE");
    world.run(&["runs"]).exited(0).out_lacks("ACTIVE");
    assert!(world.events_of(run, "run-hook-finished").is_empty());
    world
        .run(&["results", run])
        .exited(0)
        .out_has("failure hook fired — reason: nodes; still running");

    std::fs::write(records(&world).join(format!("{run}.go")), "go").expect("the hold is released");
    world.until("the detached driver's hook to finish", |world| {
        !world.events_of(run, "run-hook-finished").is_empty()
    });

    assert_eq!(
        invocations(&world, run),
        invocations(&world, "attachedtwin")
    );
    assert_eq!(hook_kinds(&world, run), hook_kinds(&world, "attachedtwin"));
    let payload = |run: &str, kind: &str| world.events_of(run, kind)[0]["payload"].clone();
    assert_eq!(
        payload(run, "run-hook-fired"),
        payload("attachedtwin", "run-hook-fired")
    );
    for field in ["hook", "ending", "exit"] {
        assert_eq!(
            payload(run, "run-hook-finished")[field],
            payload("attachedtwin", "run-hook-finished")[field]
        );
    }
    let (detached, attached) = (handed(&world, run, 1), handed(&world, "attachedtwin", 1));
    assert_eq!(detached.stdin["reason"], attached.stdin["reason"]);
    assert!(same_place(&detached.cwd, &attached.cwd));
    assert_eq!(detached.env["hook"], attached.env["hook"]);
    assert_eq!(detached.env["session"], attached.env["session"]);
}

/// The flag beats the launch config — a blank one naming none — a timeout of zero
/// is refused before a run is minted, all three are retained, a launch naming
/// none writes the record it always wrote, and `adopt` takes none of them.
#[test]
fn a_launch_resolves_its_hooks_flag_over_config_refuses_a_zero_timeout_and_adopt_takes_none() {
    let world = hooked_world("hooks-launch");
    let hook = hook(&world);
    let config = world.root.join("launch.yaml");
    std::fs::write(
        &config,
        format!(
            "schema_version: 6\nsuccess_hook: {hook:?}\nfailure_hook: {hook:?}\nhook_timeout: 5\n"
        ),
    )
    .expect("the config is written");
    let config = config.to_string_lossy().into_owned();

    // A blank flag names none over the config's hook, and a flag's timeout beats
    // the config's.
    let run = "blanked";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &[
            "--launch-config",
            &config,
            "--success-hook",
            "",
            "--hook-timeout",
            "9",
        ],
    )
    .exited(0)
    .settled();
    let launch = world.run_json(run, "launch.json");
    assert!(launch.get("success_hook").is_none(), "{launch}");
    assert_eq!(launch["failure_hook"], hook);
    assert_eq!(launch["hook_timeout"], 9);
    assert!(hook_kinds(&world, run).is_empty());
    assert!(invocations(&world, run).is_empty());

    // The config alone names both, and its timeout.
    let run = "configured";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &["--launch-config", &config],
    )
    .exited(0)
    .settled();
    let launch = world.run_json(run, "launch.json");
    assert_eq!(launch["success_hook"], hook);
    assert_eq!(launch["hook_timeout"], 5);
    assert_eq!(invocations(&world, run), ["success"]);

    // Naming none writes none of the three and journals nothing about hooks.
    let run = "unhooked";
    attached(&world, run, vec![agent("build", &[])], &[])
        .exited(0)
        .settled();
    let launch = world.run_json(run, "launch.json");
    for key in ["success_hook", "failure_hook", "hook_timeout"] {
        assert!(
            launch.get(key).is_none(),
            "{key} reached the record: {launch}"
        );
    }
    assert!(hook_kinds(&world, run).is_empty());

    // Zero is refused by the spelling that carried it, before a run exists.
    let path = world.plan("zeroed", &plan_of("zeroed", vec![agent("build", &[])]));
    world
        .run_from(
            &world.project,
            &[
                "start",
                &path,
                "--failure-hook",
                &hook,
                "--hook-timeout",
                "0",
            ],
        )
        .exited(REFUSED)
        .err_has("--hook-timeout")
        .err_has("zero");
    let zero = world.root.join("zero.yaml");
    std::fs::write(&zero, "schema_version: 6\nhook_timeout: 0\n").expect("the config is written");
    world
        .run_from(
            &world.project,
            &["start", &path, "--launch-config", &zero.to_string_lossy()],
        )
        .exited(REFUSED)
        .err_has("`hook_timeout`")
        .err_has("zero");
    assert!(!world.run_file("zeroed", "launch.json").is_file());

    // `adopt` takes none of them: what it fires is what the record retained.
    world
        .run(&["adopt", "configured", "--success-hook", &hook])
        .exited(USAGE_ERROR)
        .err_has("--success-hook");
}
