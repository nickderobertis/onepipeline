//! The run-end hooks, driven end to end against the compiled binary.
//!
//! A launch names a success hook and a failure hook; the engine judges the run
//! when a driver lets go of it, or when `stop` tears it down, and fires at most
//! one hook per ending the run reaches. Every journey here names a **real**
//! command — the fixture pair `run_end_hook.sh` / `run_end_hook.bat` — which
//! records its working directory, its environment and its stdin, so what a hook
//! was handed is read off the process that ran rather than off anything this
//! crate wrote down about it.
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

/// The hook timeout every launch here names, in seconds.
///
/// Named rather than left to the contract's `600`, because `.config/nextest.toml`
/// ends a test at 360 seconds: a hook stuck under the default is ended by the
/// runner, as a bare `TIMEOUT` naming only the test, before the engine reaches
/// its own `timed-out` ending — the record that says what was stuck. Two minutes
/// is well inside that bound and above any hook here that is meant to finish; the
/// one journey about the timeout itself names a shorter one, the one launch that
/// asks what the default *is* names none, and the product default is untouched.
const HOOK_TIMEOUT: &str = "120";

fn both_hooks_under_timeout(hook: &str) -> [&str; 6] {
    [
        "--success-hook",
        hook,
        "--failure-hook",
        hook,
        "--hook-timeout",
        HOOK_TIMEOUT,
    ]
}

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

/// The two halves of the fixture number an invocation the same way.
///
/// One contract in two languages, for the reason
/// `harness::both_hook_scripts_answer_the_same_verbs` gives: no platform runs
/// both, so a half that reaches its record directory by a route the other does
/// not is a journey that passes on one leg and hangs on the other.
// llmlint: ignore-block[tests_mirror_real_usage] a drift gate over the suite's own
// scaffolding rather than a journey, exactly as `harness.rs`'s two are: what it holds is
// that two files stay in step, no platform executes both, and reading them is the only way
// to compare them. The fixture itself is driven as the operator's real command by every
// journey below.
#[test]
fn both_run_end_hook_halves_number_an_invocation_the_same_way() {
    let ceiling = |script: &str, source: &str| -> String {
        source
            .lines()
            .map(str::trim)
            .filter_map(|line| line.split_once("ceiling=").map(|(_, rest)| rest))
            .map(|rest| {
                rest.chars()
                    .take_while(|character| character.is_ascii_digit())
                    .collect::<String>()
            })
            .find(|digits| !digits.is_empty())
            .unwrap_or_else(|| {
                panic!("{script} states no ceiling for how many firings one run records")
            })
    };
    assert_eq!(
        ceiling("run_end_hook.sh", include_str!("run_end_hook.sh")),
        ceiling("run_end_hook.bat", include_str!("run_end_hook.bat")),
        "the fixture's halves give up claiming a record directory at different ceilings"
    );
    assert_eq!(
        ceiling("run_end_hook.sh", include_str!("run_end_hook.sh")),
        "100",
        "the ceiling moved: it bounds how many firings one run records, and a journey \
         that fires more hooks than that is one this suite does not have"
    );
    // The claim itself, in each half's own words. Read rather than inferred from
    // behaviour, because no platform executes both — and what has to agree is that
    // neither half reaches `<n>` by a route its *first* firing skips, which is
    // exactly what left the Windows half unexercised until a run fired twice.
    for (script, source, claim) in [
        (
            "run_end_hook.sh",
            include_str!("run_end_hook.sh"),
            r#"until mkdir "$record/$run/$nth""#,
        ),
        (
            "run_end_hook.bat",
            include_str!("run_end_hook.bat"),
            r#"mkdir "%record%\%run%\!nth!" 2>nul || goto number"#,
        ),
    ] {
        assert!(
            source.contains(claim),
            "{script} no longer claims its record directory with `{claim}`, so the two \
             halves number an invocation differently now"
        );
    }
} // llmlint: ignore-end[tests_mirror_real_usage]

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
        // Only the view's own indent is stripped: what follows `output: ` is the
        // log's line as the hook wrote it, trailing whitespace included.
        .filter_map(|line| line.trim_start().strip_prefix("output: "))
        .map(str::to_string)
        .collect()
}

/// Whether `results` says `before`, then names the `op` edit — committed at a
/// time, which the line states — and then says `then`.
///
/// The time is matched by its shape rather than looked up: it is the one part of
/// the line a reader has no other command to learn, and the edit is told apart
/// from any other by its command and by what it retried.
fn names_edit(results: &str, before: &str, op: &str, then: &str) -> bool {
    names_committed_at(
        results,
        &format!("{before}the {op} edit committed at "),
        then,
    )
}

/// Whether `results` says `before`, then names an edit it could read nothing of
/// but when it was committed — which the line states — and then says `then`.
fn names_unreadable_edit(results: &str, before: &str, then: &str) -> bool {
    names_committed_at(
        results,
        &format!("{before}an edit committed at "),
        &format!(" whose record this build cannot read{then}"),
    )
}

/// Whether some line of `results` says `before`, then a time, then `then`.
fn names_committed_at(results: &str, before: &str, then: &str) -> bool {
    results.lines().any(|line| {
        line.split_once(before)
            .and_then(|(_, rest)| rest.split_once(then))
            .is_some_and(|(at, _)| {
                at.len() == "2026-09-18T12:00:00.000Z".len()
                    && at.as_bytes()[10] == b'T'
                    && at.ends_with('Z')
            })
    })
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
        &both_hooks_under_timeout(&hook),
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
        ("exits", "failed", hook.as_str(), HOOK_TIMEOUT, Some(7)),
        (
            "unstartable",
            "could-not-start",
            missing.as_str(),
            HOOK_TIMEOUT,
            None,
        ),
        ("outlives", "timed-out", hook.as_str(), "1", None),
    ] {
        let extra = [
            "--success-hook",
            command,
            "--failure-hook",
            command,
            "--hook-timeout",
            timeout,
        ];
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
        &both_hooks_under_timeout(&hook),
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
/// that ends over it has ended unfinished — divergence entry 74's ruling.
#[test]
fn a_run_that_ends_over_a_node_its_upstream_never_released_fires_failure_as_unfinished() {
    let world = hooked_world("hooks-unreleased");
    let hook = hook(&world);
    let mut ship = agent("ship", &[]);
    ship["deps"] = json!(["run:neverran#build"]);
    let run = "unreleased";
    attached(&world, run, vec![ship], &both_hooks_under_timeout(&hook)).exited(NOTHING_DRIVING);

    assert_eq!(invocations(&world, run), ["failure"]);
    let reason = &world.events_of(run, "run-hook-fired")[0]["payload"]["reason"];
    assert_eq!(reason["kind"], "unfinished");
    assert_eq!(
        reason["nodes"],
        json!([{"id": "ship", "status": "blocked", "outcome": null}])
    );
}

/// A planner's `cancel` of a running node parks it, and the node behind it is held:
/// neither is failed or skipped, so a run that ends over them has ended unfinished,
/// and its reason lists both, in plan order.
///
/// The graph reads the park ahead of the `cancelled` the stopped dispatch settles,
/// so the node reads `parked` here; `hooks::tests` holds the rule over a
/// `cancelled` and a `pending` node, which no let-go reaches.
#[test]
fn a_cancel_that_stops_a_running_node_ends_the_run_unfinished_over_it_and_its_dependent() {
    let world = hooked_world("hooks-cancelled");
    let hook = hook(&world);
    world.script("slow.turn-open", "");
    world.script("slow.wait", "hold");
    world.script("slow.stops-when-interrupted", "");
    let run = "cancelled";
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("slow", &[]), agent("after", &["slow"])]),
    );
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
                "--hook-timeout",
                HOOK_TIMEOUT,
            ],
        )
        .exited(0);
    world.until("the held node's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
    });
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "cancel", "id": "slow"}]}).to_string(),
        )
        .exited(0);
    world.until("the run's hook to be recorded as ended", |world| {
        !world.events_of(run, "run-hook-finished").is_empty()
    });

    assert_eq!(invocations(&world, run), ["failure"]);
    let reason = &world.events_of(run, "run-hook-fired")[0]["payload"]["reason"];
    assert_eq!(reason["kind"], "unfinished");
    assert_eq!(
        reason["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .map(|node| (node["id"].clone(), node["status"].clone()))
            .collect::<Vec<_>>(),
        [
            (json!("slow"), json!("parked")),
            (json!("after"), json!("blocked"))
        ],
        "{reason}"
    );
    assert_eq!(
        reason["nodes"],
        not_done(&world.run_json(run, "result.json"))
    );
    assert_eq!(&handed(&world, run, 1).stdin["reason"], reason);
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
                "--hook-timeout",
                HOOK_TIMEOUT,
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
    let mut launch = world.cmd(&[
        "start",
        &path,
        "--attach",
        "--success-hook",
        &hook,
        "--hook-timeout",
        HOOK_TIMEOUT,
    ]);
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
        &["--success-hook", &hook, "--hook-timeout", HOOK_TIMEOUT],
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

/// The incident the epoch exists for: a run whose **failure** hook fired, whose
/// failed node was retried through an accepted edit, and which then completed,
/// fires its **success** hook — so a run recovered from a failure still runs the
/// automation only a completion launches.
#[test]
fn a_run_whose_failure_hook_fired_fires_success_once_a_retry_takes_it_on_to_complete() {
    let world = hooked_world("hooks-retry-success");
    let hook = hook(&world);
    world.script("build.fail", "1");
    let run = "recovered";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &both_hooks_under_timeout(&hook),
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);
    assert_eq!(
        world.events_of(run, "run-hook-fired")[0]["payload"]["reason"]["nodes"],
        json!([{"id": "build", "status": "failed", "outcome": "task-failed"}])
    );

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

    assert_eq!(
        invocations(&world, run),
        ["failure", "success"],
        "{}",
        world.dump()
    );
    let fired = world.events_of(run, "run-hook-fired");
    assert_eq!(fired.len(), 2, "{fired:?}");
    assert_eq!(
        fired[1]["payload"],
        json!({"hook": "success", "command": hook, "reason": null})
    );
    let handed = handed(&world, run, 2);
    assert_eq!(handed.env["hook"], Some("success".to_string()));
    assert_eq!(handed.stdin["hook"], "success");
    assert_eq!(handed.stdin["reason"], Value::Null);

    // Both firings are the run's record, and `results` reads them in order.
    let results = world.run(&["results", run]);
    results.exited(0).out_has("failure hook fired");
    assert!(
        results
            .stdout
            .contains("success hook fired — reason: none; ending: succeeded; exit: 0"),
        "{}",
        results.stdout
    );
}

/// The other recovery a manager makes by hand: a run whose **failure** hook fired
/// and whose failed node was then **settled `done`** — the work had landed some
/// other way — fires its **success** hook for the complete ending that settle
/// produced, exactly once.
///
/// The defect this states (#396): the marker was retired only by an edit that
/// made the run live, and a settle that moves a failed node straight to `done`
/// never passes through a live graph, so the failure's marker went on suppressing
/// the success hook for an ending it never fired for. Driven beside the rule's
/// other half: a settle that changes *why* the run failed and not that it did —
/// the parked node settled `failed` — is not an epoch and fires nothing.
#[test]
fn a_run_whose_failure_hook_fired_fires_success_once_a_settle_takes_it_on_to_complete() {
    let world = hooked_world("hooks-settle-success");
    let hook = hook(&world);
    world.script("build.fail", "1");
    let mut later = agent("later", &[]);
    later["parked"] = json!(true);
    let run = "settled";
    attached(
        &world,
        run,
        vec![agent("build", &[]), later],
        &both_hooks_under_timeout(&hook),
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);
    assert_eq!(
        world.events_of(run, "run-hook-fired")[0]["payload"]["reason"]["nodes"],
        json!([
            {"id": "build", "status": "failed", "outcome": "task-failed"},
            {"id": "later", "status": "parked", "outcome": null}
        ])
    );

    // A settle that leaves the run failed has changed the failure's reason and
    // not the ending: the marker stands, and the same ending fires no second hook.
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "settle", "id": "later", "outcome": "failed",
                 "evidence": "nobody is going to pick this up"}
            ]})
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
    world.run(&["adopt", run]).exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"], "{}", world.dump());
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 1);

    // The settle that carries the run from a failed ending to a complete one is
    // the epoch, though the graph it leaves behind holds nothing to carry out.
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "settle", "id": "build", "outcome": "done",
                 "evidence": "the accepted branch was published by hand"},
                {"op": "settle", "id": "later", "outcome": "done",
                 "evidence": "and so was this one"}
            ]})
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
    // And `results` says so before any driver looks: the failure belongs to the
    // epoch the settle ended, and nothing has fired for the new one yet.
    let results = world.run(&["results", run]);
    results.exited(0);
    for (before, then) in [
        (
            "failure hook fired — superseded: ",
            " reopened the run after it — reason:",
        ),
        ("no run-end hook has fired since ", " reopened the run"),
    ] {
        assert!(
            names_edit(&results.stdout, before, "settle", then),
            "`results` does not say {before:?} the settle then {then:?}:\n{}",
            results.stdout
        );
    }
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");

    assert_eq!(
        invocations(&world, run),
        ["failure", "success"],
        "{}",
        world.dump()
    );
    let fired = world.events_of(run, "run-hook-fired");
    assert_eq!(fired.len(), 2, "{fired:?}");
    assert_eq!(
        fired[1]["payload"],
        json!({"hook": "success", "command": hook, "reason": null})
    );
    let handed = handed(&world, run, 2);
    assert_eq!(handed.env["hook"], Some("success".to_string()));
    assert_eq!(handed.stdin["hook"], "success");
    assert_eq!(handed.stdin["reason"], Value::Null);

    // Exactly once for that ending: a driver that looks again finds the marker.
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");
    assert_eq!(invocations(&world, run), ["failure", "success"]);
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 2);
    let results = world.run(&["results", run]);
    results.exited(0);
    assert!(
        results
            .stdout
            .contains("success hook fired — reason: none; ending: succeeded; exit: 0"),
        "{}",
        results.stdout
    );
}

/// `results` reads every hook record against the **epoch** it belongs to: a hook
/// from before the recovery edit that reopened the run says it is superseded and
/// names that edit, and what the run has fired since — or that it has fired
/// nothing yet — is what reads as the run now.
///
/// The defect this states: the records were listed in journal order with no epoch
/// at all, beneath a graph the `retry` had already replaced, so a recovered run
/// went on showing its first failure hook and that hook's instructions as if they
/// described it. Driven across two recoveries, so an epoch is told apart by the
/// edit that ended it rather than by being first, and so the same hook firing
/// twice — whose log the second firing rewrote — is covered too.
#[test]
fn results_labels_each_hook_from_an_earlier_epoch_as_superseded_by_the_edit_that_ended_it() {
    let world = hooked_world("hooks-results-epochs");
    let hook = hook(&world);
    world.script("build.fail", "1");
    world.script("build-2.fail", "1");
    let run = "reepoched";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &["--success-hook", &hook, "--failure-hook", &hook],
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);

    // One epoch, one record, and it is the run's present state.
    let results = world.run(&["results", run]);
    results
        .exited(0)
        .out_has("failure hook fired — reason: nodes")
        .out_lacks("superseded:")
        .out_lacks("no run-end hook has fired since");

    let retry = |from: &str, to: &str| {
        world
            .run_with_stdin(
                &["reply", run],
                &json!({"version": 2, "commands": [
                    {"op": "retry", "id": from, "node": agent(to, &[])}
                ]})
                .to_string(),
            )
            .exited(0);
        format!(": {from} retried as {to}")
    };
    let superseded = |results: &Run, record: &str, recovery: &str, then: &str| {
        assert!(
            names_edit(
                &results.stdout,
                &format!("{record} — superseded: "),
                "retry",
                &format!("{recovery} reopened the run after it{then}")
            ),
            "`results` does not say the {record} record was superseded by the retry that \
             said {recovery:?}:\n{}",
            results.stdout
        );
    };

    // The retry reopens the run and nothing has fired since: the first record is
    // superseded by the edit, and the run's present is that no hook has fired.
    let first_recovery = retry("build", "build-2");
    let results = world.run(&["results", run]);
    results.exited(0);
    superseded(
        &results,
        "failure hook fired",
        &first_recovery,
        " — reason: nodes",
    );
    assert!(
        names_edit(
            &results.stdout,
            "no run-end hook has fired since ",
            "retry",
            &format!("{first_recovery} reopened the run")
        ),
        "`results` does not say nothing has fired since the retry:\n{}",
        results.stdout
    );

    // The replacement fails in its turn: the new firing is the present one, and
    // the first — whose log this firing rewrote — keeps its label and is not
    // shown this one's output.
    world.run(&["adopt", run]).exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure", "failure"]);
    let results = world.run(&["results", run]);
    results
        .exited(0)
        .out_has("its output is not repeated: a later failure hook rewrote the log")
        .out_lacks("no run-end hook has fired since");
    superseded(&results, "failure hook fired", &first_recovery, "");
    let current: Vec<&str> = results
        .stdout
        .lines()
        .filter(|line| line.contains("hook fired") && !line.contains("superseded:"))
        .collect();
    assert_eq!(
        current.len(),
        1,
        "exactly one record reads as the run's present state:\n{}",
        results.stdout
    );
    assert!(
        current[0].contains("failure hook fired — reason: nodes"),
        "{}",
        results.stdout
    );

    // A second recovery, and a success: both failures belong to epochs it ended,
    // each named by its own edit, and the success is the run now.
    let second_recovery = retry("build-2", "build-3");
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");
    assert_eq!(invocations(&world, run), ["failure", "failure", "success"]);
    let results = world.run(&["results", run]);
    results
        .exited(0)
        .out_has("success hook fired — reason: none; ending: succeeded; exit: 0")
        .out_lacks("no run-end hook has fired since");
    superseded(&results, "failure hook fired", &first_recovery, "");
    superseded(&results, "failure hook fired", &second_recovery, "");
}

/// An epoch-ending edit whose record this build cannot read is named by the one
/// thing it can read of it — when it was committed — and by nothing that happened
/// to parse.
///
/// The record is one a **newer** build wrote. Its operations are the retry this
/// fold reads, which is what makes it the end of the failure's epoch; its command
/// is written in a vocabulary this build has no reader for. A reader that named it
/// by the parts that parsed would call it "the edit edit", or print a retry it
/// could not have attributed to any command, over a record it did not understand.
#[test]
fn an_epoch_ending_edit_whose_command_this_build_cannot_read_is_named_by_when_it_was_committed() {
    let world = hooked_world("hooks-unreadable-edit");
    let hook = hook(&world);
    world.script("build.fail", "1");
    let run = "unreadableedit";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &["--success-hook", &hook, "--failure-hook", &hook],
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);

    // A real retry, so the operations the record carries are exactly the ones a
    // reopening edit writes; then the command written over in the journal, in the
    // shape a newer build's vocabulary takes.
    // llmlint: ignore-block[tests_mirror_real_usage] no verb this build ships writes a
    // command it cannot read back — only a newer build does, and there is none to run
    // here. Rewriting the one field is the narrowest way to put that record in front of
    // the compiled binary, and it is the same fault injection this file's unfoldable-edit
    // journey makes for the same reason; the run, its journal, the retry that wrote the
    // record, the driver that adopts it and the hook fixture around it are all the real
    // ones, and every claim this journey makes is read back off the CLI.
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "retry", "id": "build", "node": agent("build-2", &[])}
            ]})
            .to_string(),
        )
        .exited(0);
    let journal = world.runs.join(run).join("events.jsonl");
    let rewritten: Vec<String> = std::fs::read_to_string(&journal)
        .expect("the journal is kept")
        .lines()
        .map(|line| {
            let mut record: Value = serde_json::from_str(line).expect("a record this run wrote");
            if record["kind"] == "edit-committed" {
                record["payload"]["command"] =
                    json!({"verb": "retry", "of": "build", "with": agent("build-2", &[])});
            }
            record.to_string()
        })
        .collect();
    std::fs::write(&journal, format!("{}\n", rewritten.join("\n")))
        .expect("the record is rewritten");
    // llmlint: ignore-end[tests_mirror_real_usage]

    // The record still ended the epoch — the fold read its operations — so the
    // failure is superseded, and the edit that did it is named by its time alone.
    let results = world.run(&["results", run]);
    results.exited(0);
    for (before, then) in [
        (
            "failure hook fired — superseded: ",
            " reopened the run after it — reason: nodes",
        ),
        ("no run-end hook has fired since ", " reopened the run"),
    ] {
        assert!(
            names_unreadable_edit(&results.stdout, before, then),
            "`results` does not say {before:?} an edit it cannot read then {then:?}:\n{}",
            results.stdout
        );
    }
    let hook_lines: Vec<&str> = results
        .stdout
        .lines()
        .filter(|line| line.contains("superseded: ") || line.contains("hook has fired since"))
        .collect();
    assert!(
        hook_lines
            .iter()
            .all(|line| !line.contains("the edit edit") && !line.contains("retried as")),
        "`results` named the unreadable record by the parts of it that parsed:\n{}",
        results.stdout
    );

    // And the epoch it opened is a real one: the replacement settles done under
    // it, the success hook fires, and the label on the superseded record stands.
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");
    assert_eq!(invocations(&world, run), ["failure", "success"]);
    let results = world.run(&["results", run]);
    results
        .exited(0)
        .out_has("success hook fired — reason: none; ending: succeeded; exit: 0")
        .out_lacks("no run-end hook has fired since");
    assert!(
        names_unreadable_edit(
            &results.stdout,
            "failure hook fired — superseded: ",
            " reopened the run after it"
        ),
        "the superseded label did not survive the success that followed it:\n{}",
        results.stdout
    );
}

/// An accepted edit that leaves the run **unable to advance** is not an epoch,
/// however much graph it moved: a node added behind the very failure that ended
/// the run is skipped the moment it joins, so the marker stands and the same
/// ending fires no second hook.
///
/// The case a classification by operation kind gets wrong — this `add` records the
/// same `node-added` a `retry`'s replacement does, and must not be read the same
/// way.
#[test]
fn an_added_node_the_failure_skips_reopens_nothing_and_leaves_the_marker_standing() {
    let world = hooked_world("hooks-added-blocked");
    let hook = hook(&world);
    world.script("build.fail", "1");
    let run = "stillblocked";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &both_hooks_under_timeout(&hook),
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);

    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "add", "node": agent("extra", &["build"])}
            ]})
            .to_string(),
        )
        .exited(0);
    world.run(&["adopt", run]).exited(NOTHING_DRIVING);

    // The node really did join the graph, and really cannot run.
    let result = world.run_json(run, "result.json");
    let status = |id: &str| {
        result["nodes"]
            .as_array()
            .expect("the result lists nodes")
            .iter()
            .find(|node| node["id"] == id)
            .unwrap_or_else(|| panic!("{id} is missing from {result}"))["status"]
            .clone()
    };
    assert_eq!(status("build"), json!("failed"));
    assert_eq!(
        status("extra"),
        json!("skipped"),
        "the added node can run, so this journey proves nothing"
    );
    assert_eq!(invocations(&world, run), ["failure"], "{}", world.dump());
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 1);
}

/// An accepted edit that **frees work the graph already held** is an epoch even
/// though it adds no node at all: a `drop` that detaches a blocked node from the
/// parked node holding it makes that node ready, the run completes, and the
/// success hook fires for the ending it then reaches.
///
/// The other case a classification by operation kind gets wrong — nothing here
/// records a `node-added` or a `node-requeued`, and the run is live again.
#[test]
fn an_edit_that_frees_blocked_work_reopens_the_run_though_it_adds_no_node() {
    let world = hooked_world("hooks-freed");
    let hook = hook(&world);
    let mut gate = agent("gate", &[]);
    gate["parked"] = json!(true);
    let run = "freed";
    attached(
        &world,
        run,
        vec![agent("build", &[]), gate, agent("after", &["gate"])],
        &both_hooks_under_timeout(&hook),
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);
    assert_eq!(
        world.events_of(run, "run-hook-fired")[0]["payload"]["reason"]["kind"],
        "unfinished"
    );

    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "drop", "id": "gate", "dependents": "detach"}
            ]})
            .to_string(),
        )
        .exited(0);
    let kinds = |world: &World| {
        world
            .events_of(run, "edit-committed")
            .iter()
            .flat_map(|event| {
                event["payload"]["operation_kinds"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .filter_map(|kind| kind.as_str().map(str::to_string))
            .collect::<Vec<String>>()
    };
    world.until("the drop to be committed", |world| {
        kinds(world).iter().any(|kind| kind == "node-dropped")
    });
    let committed = kinds(&world);
    assert!(
        !committed
            .iter()
            .any(|kind| kind == "node-added" || kind == "node-requeued"),
        "this edit added or requeued a node, so it proves nothing about an edit that does \
         neither: {committed:?}"
    );

    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");

    assert_eq!(
        invocations(&world, run),
        ["failure", "success"],
        "{}",
        world.dump()
    );
    let fired = world.events_of(run, "run-hook-fired");
    assert_eq!(fired.len(), 2, "{fired:?}");
    assert_eq!(fired[1]["payload"]["reason"], Value::Null);
}

/// An edit that leaves a **ready human action** has made the run live again even
/// though nothing can dispatch: the run is paused on a decision rather than ended,
/// so the adopting driver withholds a hook it would otherwise have skipped
/// silently, and the ending reached once the action is attested fires.
///
/// The `waiting` arm of the liveness rule, which no other journey here reaches —
/// a run paused before it ever fired has no marker for an epoch to retire.
#[test]
fn an_edit_that_leaves_a_ready_human_action_reopens_the_run_and_the_next_ending_fires() {
    let world = hooked_world("hooks-human-epoch");
    let hook = hook(&world);
    world.script("build.fail", "1");
    let run = "gated";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &both_hooks_under_timeout(&hook),
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(
        hook_kinds(&world, run),
        ["run-hook-fired", "run-hook-finished"]
    );

    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "add", "node": human("approve", &[])}
            ]})
            .to_string(),
        )
        .exited(0);

    // The run is paused, not ended, so this driver withholds — which it reaches
    // only because the epoch retired the marker a withheld run is also past.
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"awaiting-planner\"")
        .err_has("paused on a decision");
    assert_eq!(
        hook_kinds(&world, run),
        ["run-hook-fired", "run-hook-finished", "run-hook-withheld"]
    );
    assert_eq!(invocations(&world, run), ["failure"]);

    world.run(&["attest", run, "approve"]).exited(0);
    world.run(&["adopt", run]).exited(NOTHING_DRIVING);
    assert_eq!(
        invocations(&world, run),
        ["failure", "failure"],
        "{}",
        world.dump()
    );
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 2);
}

/// A fold that has **lost a committed edit** recognises no epoch at all: the graph
/// beside an `edit-committed` this build cannot parse is not evidence of what that
/// edit did, so the marker stands and the requeue that would otherwise have
/// reopened the run fires nothing.
///
/// The conservative half of the rule, and the one a reader has to be able to see:
/// an operator whose run stops firing hooks after such a record is told by
/// `strict` that the graph they are looking at may be missing an edit.
#[test]
fn an_edit_this_build_cannot_fold_leaves_the_marker_standing_through_a_later_requeue() {
    let world = hooked_world("hooks-unfoldable");
    let hook = hook(&world);
    let mut later = agent("later", &["build"]);
    later["parked"] = json!(true);
    let run = "unfoldable";
    attached(
        &world,
        run,
        vec![agent("build", &[]), later],
        &both_hooks_under_timeout(&hook),
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);

    // An `edit-committed` carrying an operation this build has never read — the
    // shape a **newer** build's record takes. Copied from a record this run really
    // wrote so that everything but the payload is exactly what a reader meets.
    // llmlint: ignore-block[tests_mirror_real_usage] no verb this build ships writes an
    // operation it cannot parse — only a newer build does, and there is none to run here.
    // Appending one line is the narrowest way to put that record in front of the compiled
    // binary, and it is the same fault injection this file's unreadable-launch-record and
    // unopenable-log journeys make for the same reason; the run, its journal, the reply
    // that follows, the driver that adopts it and the hook fixture around it are all the
    // real ones, and every claim this journey makes is read back off them.
    let journal = world.runs.join(run).join("events.jsonl");
    let mut record = world.events_of(run, "run-hook-fired")[0].clone();
    record["kind"] = json!("edit-committed");
    record["payload"] = json!({
        "author": "planner",
        "command": {"op": "requeue", "id": "later"},
        "operations": [{"kind": "an-operation-from-a-later-build", "node": "later"}],
        "operation_kinds": ["an-operation-from-a-later-build"],
    });
    let mut lines = std::fs::read_to_string(&journal).expect("the journal is kept");
    lines.push_str(&format!("{record}\n"));
    std::fs::write(&journal, lines).expect("the record is appended");
    // llmlint: ignore-end[tests_mirror_real_usage]

    // A real requeue after it, which on a readable journal is an epoch.
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

    // The run really did reach a new ending — and still fires nothing, because the
    // fold that would have recognised the epoch is missing a record.
    assert_eq!(world.run_json(run, "result.json")["state"], "complete");
    assert_eq!(
        invocations(&world, run),
        ["failure"],
        "a run whose fold lost an edit fired again: {}",
        world.dump()
    );
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 1);
}

/// An edit that arrives on a run **already** carrying live work is not an epoch
/// either: what is asked is whether the edit *made* the run live, so an edit that
/// found it that way inherits nothing.
///
/// A `stop` is what reaches this state. It fires the failure hook the moment it
/// has torn the run down, while the node it signalled is still recorded `running`
/// — so the marker stands beside a graph the fold reads as live, and the next
/// accepted edit would otherwise retire a marker it had nothing to do with.
#[test]
fn an_edit_that_found_the_run_already_live_is_not_an_epoch() {
    let world = hooked_world("hooks-already-live");
    let hook = hook(&world);
    world.script("build.wait", "hold");
    let run = "torndown";
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
                "--hook-timeout",
                HOOK_TIMEOUT,
            ],
        )
        .exited(0);
    world.until("a node to be in flight", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });

    world.run(&["stop", run]).exited(0);
    world.until("the stop's hook to be recorded", |world| {
        !world.events_of(run, "run-hook-finished").is_empty()
    });
    assert_eq!(invocations(&world, run), ["failure"]);
    assert_eq!(
        world.events_of(run, "run-hook-fired")[0]["payload"]["reason"]["kind"],
        "stopped"
    );
    // The graph the fold reads is live: the node it signalled never settled.
    assert_eq!(
        world.events_of(run, "run-hook-fired")[0]["payload"]["reason"]["nodes"][0]["status"],
        "running",
        "the stopped node settled, so this journey reaches no already-live run"
    );

    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "amend", "id": "build", "text": "and say what the teardown left behind"}
            ]})
            .to_string(),
        )
        .exited(0);
    world.until("the amendment to be committed", |world| {
        world.events_of(run, "edit-committed").iter().any(|event| {
            event["payload"]["operation_kinds"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "task-amended"))
        })
    });

    world.run(&["stop", run, "--force"]).exited(0);
    assert_eq!(
        invocations(&world, run),
        ["failure"],
        "an edit that found the run already live fired a second hook: {}",
        world.dump()
    );
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 1);
    world.release("build.go");
}

/// Liveness that **nobody edited** is not an epoch. A consumer blocked on a
/// cross-DAG upstream that does not exist yet ends the run unfinished and fires
/// the failure hook; the upstream then arriving makes that consumer runnable and
/// takes the run to `complete` — and no hook fires for it, because no accepted
/// edit made the run live again.
///
/// The half of the rule the liveness test alone would get wrong, and the only
/// path in this suite that revives a run without a command.
#[test]
fn a_run_made_live_by_something_nobody_edited_leaves_its_marker_standing() {
    let world = hooked_world("hooks-unedited");
    let hook = hook(&world);
    let run = "patient";
    let mut ship = agent("ship", &[]);
    ship["deps"] = json!(["run:arrives#build"]);
    attached(&world, run, vec![ship], &both_hooks_under_timeout(&hook)).exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);
    assert_eq!(
        world.events_of(run, "run-hook-fired")[0]["payload"]["reason"]["nodes"],
        json!([{"id": "ship", "status": "blocked", "outcome": null}])
    );

    // The upstream arrives. Nothing is replied to this run.
    let upstream = world.plan("arrives", &plan_of("arrives", vec![agent("build", &[])]));
    world
        .run_from(&world.project, &["start", &upstream, "--attach"])
        .exited(0)
        .settled();

    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");
    assert!(
        world.events_of(run, "edit-committed").is_empty(),
        "an edit reached this run, so it proves nothing about liveness without one"
    );
    assert_eq!(world.run_json(run, "result.json")["state"], "complete");
    assert_eq!(
        invocations(&world, run),
        ["failure"],
        "a run nobody edited fired a second hook: {}",
        world.dump()
    );
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 1);
}

/// The other half of the epoch: a retry that fails in its turn is a **new**
/// attempt, so the failure hook fires a second time — naming the replacement, and
/// not the superseded node, which has left the graph.
#[test]
fn a_retry_that_fails_in_its_turn_fires_a_second_failure_hook_for_the_new_attempt() {
    let world = hooked_world("hooks-retry-failure");
    let hook = hook(&world);
    world.script("build.fail", "1");
    world.script("build-2.fail", "1");
    let run = "refailed";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &["--failure-hook", &hook, "--hook-timeout", HOOK_TIMEOUT],
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);

    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "retry", "id": "build", "node": agent("build-2", &[])}
            ]})
            .to_string(),
        )
        .exited(0);
    world.run(&["adopt", run]).exited(NOTHING_DRIVING);

    assert_eq!(
        invocations(&world, run),
        ["failure", "failure"],
        "{}",
        world.dump()
    );
    let fired = world.events_of(run, "run-hook-fired");
    assert_eq!(fired.len(), 2, "{fired:?}");
    let reasons: Vec<&Value> = fired
        .iter()
        .map(|event| &event["payload"]["reason"])
        .collect();
    assert_eq!(
        *reasons[0],
        json!({"kind": "nodes",
               "nodes": [{"id": "build", "status": "failed", "outcome": "task-failed"}]})
    );
    assert_eq!(
        *reasons[1],
        json!({"kind": "nodes",
               "nodes": [{"id": "build-2", "status": "failed", "outcome": "task-failed"}]}),
        "the second firing is the new attempt's, not the superseded node's"
    );
    let handed = handed(&world, run, 2);
    assert_eq!(handed.stdin["reason"], *reasons[1]);
}

/// The property the gate around the marker has always carried, now carried across
/// an epoch: two drivers that judge one reopened run fire **exactly one** hook
/// between them.
///
/// The second driver arrives while the first one's hook is still running — which
/// is the window where the run reads as undriven and a second judgement is
/// genuinely possible — and finds the marker the first appended under this epoch.
#[test]
fn two_drivers_judging_one_reopened_run_fire_exactly_one_hook_between_them() {
    let world = hooked_world("hooks-epoch-race");
    let hook = hook(&world);
    world.script("build.fail", "1");
    let run = "raced";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &both_hooks_under_timeout(&hook),
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);

    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "retry", "id": "build", "node": agent("build-2", &[])}
            ]})
            .to_string(),
        )
        .exited(0);

    // The next hook holds until this journey releases it, so the two drivers
    // really do overlap rather than following one another.
    std::fs::write(records(&world).join(format!("{run}.hold")), "").expect("the hold is scripted");
    let mut first = world
        .cmd(&["adopt", run])
        .spawn()
        .expect("the first driver starts");
    let started = records(&world).join(run).join("2").join("started");
    world.until("the reopened run's hook to be running", |_| {
        started.is_file()
    });

    // The first driver has let go of the ownership lock to run its hook, so this
    // one takes the run over, judges the same epoch, and finds the marker.
    world.run(&["adopt", run]).exited(0);
    assert_eq!(
        invocations(&world, run),
        ["failure", "success"],
        "{}",
        world.dump()
    );

    std::fs::write(records(&world).join(format!("{run}.go")), "go").expect("the hold is released");
    assert!(
        first.wait().expect("the first driver ends").success(),
        "the first driver did not end cleanly"
    );
    world.until("the reopened run's hook to finish", |world| {
        world.events_of(run, "run-hook-finished").len() == 2
    });

    assert_eq!(invocations(&world, run), ["failure", "success"]);
    let fired = world.events_of(run, "run-hook-fired");
    assert_eq!(fired.len(), 2, "two drivers fired twice: {fired:?}");
    assert_eq!(fired[1]["payload"]["hook"], "success");
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
        &both_hooks_under_timeout(&hook),
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

/// A `run-hook-withheld` belongs to an epoch as a firing does: once a recovery
/// edit reopens the run, the let-go that paused it on a decision describes an
/// ending the run has left, and `results` says it is superseded by that edit.
#[test]
fn a_withheld_hook_from_an_earlier_epoch_reads_as_superseded_by_the_edit_that_ended_it() {
    let world = hooked_world("hooks-withheld-epoch");
    let hook = hook(&world);
    world.script("build.fail", "1");
    let run = "rewithheld";
    attached(
        &world,
        run,
        vec![agent("build", &[]), human("approve", &[])],
        &["--success-hook", &hook, "--failure-hook", &hook],
    )
    .exited(0)
    .out_has("\"settlement\":\"awaiting-planner\"");
    world.run(&["attest", run, "approve"]).exited(0);
    world.run(&["adopt", run]).exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"], "{}", world.dump());
    world
        .run(&["results", run])
        .exited(0)
        .out_has("run-end hook withheld — the run is paused on a decision")
        .out_lacks("superseded:");

    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "retry", "id": "build", "node": agent("build-2", &[])}
            ]})
            .to_string(),
        )
        .exited(0);
    let results = world.run(&["results", run]);
    results.exited(0);
    for (before, then) in [
        (
            "run-end hook withheld — superseded: ",
            ": build retried as build-2 reopened the run after it — the run is paused on a \
             decision (awaiting-planner)",
        ),
        (
            "failure hook fired — superseded: ",
            ": build retried as build-2 reopened the run after it",
        ),
        (
            "no run-end hook has fired since ",
            ": build retried as build-2 reopened the run",
        ),
    ] {
        assert!(
            names_edit(&results.stdout, before, "retry", then),
            "`results` does not say {before:?} the retry then {then:?}:\n{}",
            results.stdout
        );
    }
}

/// Once a run carries a firing, only an accepted edit that makes the run **live
/// again** lets another fire: a later adoption fires nothing, an accepted note and
/// an accepted finding fire nothing, and the requeue that puts work back into the
/// graph opens the epoch the completed run's success hook fires under.
///
/// The note and the finding are the two shapes an inert command takes in the
/// record — one `edit-committed`, one `command-accepted` — so between them they
/// cover both of the ways a command that reopened nothing can reach this.
#[test]
fn once_a_hook_has_fired_only_an_edit_that_reopens_the_run_lets_another_fire() {
    let world = hooked_world("hooks-once");
    let hook = hook(&world);
    let mut later = agent("later", &["build"]);
    later["parked"] = json!(true);
    let run = "once";
    attached(
        &world,
        run,
        vec![agent("build", &[]), later],
        &both_hooks_under_timeout(&hook),
    )
    .exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);

    world.run(&["adopt", run]).exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);

    // An accepted command that reopens nothing is not an epoch. The note is
    // committed — the record says so — and the marker still stands, so the same
    // ending fires no second hook.
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "note", "id": "later", "addressee": "worker",
                 "text": "read the failure before you pick this up"}
            ]})
            .to_string(),
        )
        .exited(0);
    world.until("the note to be committed", |world| {
        world.events_of(run, "edit-committed").iter().any(|event| {
            event["payload"]["operation_kinds"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "note-delivered"))
        })
    });
    world.run(&["adopt", run]).exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"], "{}", world.dump());
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 1);

    // A finding commits nothing a reader folds, so it is recorded as
    // `command-accepted` rather than `edit-committed` and cannot be an epoch either.
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [
                {"op": "finding", "message": "the parked node is the whole of what is left"}
            ]})
            .to_string(),
        )
        .exited(0);
    world.until("the finding to be accepted", |world| {
        !world.events_of(run, "command-accepted").is_empty()
    });
    world.run(&["adopt", run]).exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"], "{}", world.dump());
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 1);

    // The requeue puts a node back on the desired frontier, which is the epoch:
    // the ending the adoption then reaches is a new one, and it fires.
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "requeue", "id": "later"}]}).to_string(),
        )
        .exited(0);
    // And `results` says so, naming an edit that retried nothing by its command
    // alone: the failure belongs to the epoch the requeue ended.
    let results = world.run(&["results", run]);
    results.exited(0);
    for (before, then) in [
        (
            "failure hook fired — superseded: ",
            " reopened the run after it — reason:",
        ),
        ("no run-end hook has fired since ", " reopened the run"),
    ] {
        assert!(
            names_edit(&results.stdout, before, "requeue", then),
            "`results` does not say {before:?} the requeue then {then:?}:\n{}",
            results.stdout
        );
    }
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");

    assert_eq!(invocations(&world, run), ["failure", "success"]);
    assert_eq!(
        hook_kinds(&world, run),
        [
            "run-hook-fired",
            "run-hook-finished",
            "run-hook-fired",
            "run-hook-finished"
        ]
    );
    let fired = world.events_of(run, "run-hook-fired");
    assert_eq!(fired[0]["payload"]["hook"], "failure");
    assert_eq!(fired[1]["payload"]["hook"], "success");
    assert_eq!(fired[1]["payload"]["reason"], Value::Null);
}

/// A detached driver judges and fires exactly as an attached one does — and while
/// its hook runs, the run reads as nothing driving it, through the binary's own
/// views, and an `adopt` takes the run over without ending the process awaiting
/// that hook or firing a second one.
#[test]
fn a_detached_driver_fires_as_an_attached_one_does_and_its_run_is_undriven_while_the_hook_runs() {
    let world = hooked_world("hooks-detached");
    let hook = hook(&world);
    world.script("build.fail", "1");
    let nodes = || vec![agent("build", &[]), agent("ship", &["build"])];
    let flags = both_hooks_under_timeout(&hook);

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

    // The verb that refuses a driven run takes this one over: the lock is free, and
    // the live process awaiting the hook is not a parked driver to end. The run has
    // already fired, so the adopting driver fires nothing of its own.
    world
        .run(&["adopt", run])
        .exited(NOTHING_DRIVING)
        .err_lacks("ending it to adopt the run");
    assert_eq!(invocations(&world, run), ["failure"]);
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 1);
    assert!(
        world.events_of(run, "run-hook-finished").is_empty(),
        "the adoption ended the hook it should have left running"
    );

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

    // A blank key names none, as a blank flag does, and a launch naming a hook
    // and no timeout retains the contract's default.
    let blank = world.root.join("blank.yaml");
    std::fs::write(
        &blank,
        format!("schema_version: 6\nsuccess_hook: \"  \"\nfailure_hook: {hook:?}\n"),
    )
    .expect("the config is written");
    let run = "blankkey";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &["--launch-config", &blank.to_string_lossy()],
    )
    .exited(0)
    .settled();
    let launch = world.run_json(run, "launch.json");
    assert!(launch.get("success_hook").is_none(), "{launch}");
    assert_eq!(launch["failure_hook"], hook);
    assert_eq!(
        launch["hook_timeout"],
        contract_block()["default_timeout_seconds"]
    );
    assert!(hook_kinds(&world, run).is_empty());
    assert!(invocations(&world, run).is_empty());

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

/// A hook that swaps its own log for a link to another file shows no reader of the
/// run what that file holds: not on the attached driver's stderr while it runs, and
/// not in `results` afterwards, which says the log was not read. The run settles
/// exactly as it would have.
///
/// Unix alone, for the reason `run_end_hook.bat` gives.
#[cfg(unix)]
#[test]
fn a_hook_that_swaps_its_log_for_a_link_shows_no_reader_the_file_it_names() {
    let world = hooked_world("hooks-relinked");
    let hook = hook(&world);
    let elsewhere = world.root.join("not-the-log.txt");
    let only_there = "a line only the linked file holds";
    std::fs::write(&elsewhere, format!("{only_there}\n")).expect("the linked file is written");
    let run = "relinked";
    std::fs::write(
        records(&world).join(format!("{run}.relink")),
        elsewhere.to_string_lossy().as_bytes(),
    )
    .expect("the relink is scripted");

    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &["--success-hook", &hook, "--hook-timeout", HOOK_TIMEOUT],
    )
    .exited(0)
    .out_has("\"settlement\":\"complete\"")
    .err_lacks(only_there);

    assert_eq!(invocations(&world, run), ["success"]);
    let log = world.runs.join(run).join("hooks").join("success.log");
    assert!(
        std::fs::symlink_metadata(&log)
            .expect("the log path exists")
            .file_type()
            .is_symlink(),
        "the fixture did not swap its log, so this journey proves nothing"
    );
    let finished = &world.events_of(run, "run-hook-finished")[0]["payload"];
    assert_eq!(finished["ending"], "succeeded");
    assert_eq!(world.run_json(run, "result.json")["state"], "complete");

    let results = world.run(&["results", run]);
    results
        .exited(0)
        .out_has("success hook fired — reason: none; ending: succeeded; exit: 0")
        .out_has("log not read")
        .out_lacks(only_there);
    assert!(
        output_lines(&results.stdout).is_empty(),
        "{}",
        results.stdout
    );
}

/// A launch record that can no longer be read when a driver lets go of its run is
/// not a record naming no hooks: the driver says, on the log a detached driver
/// keeps, that whether the run fires a hook could not be judged, and fires nothing
/// it could not read the command of.
#[test]
fn a_launch_record_unreadable_at_let_go_is_reported_rather_than_read_as_naming_no_hooks() {
    let world = hooked_world("hooks-unreadable-record");
    let hook = hook(&world);
    world.script("build.wait", "hold");
    let run = "unreadable";
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
                "--hook-timeout",
                HOOK_TIMEOUT,
            ],
        )
        .exited(0);
    world.until("a node to be in flight", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    // llmlint: ignore[tests_mirror_real_usage] a launch record that stops parsing under a
    // live driver is a storage fault, and no verb writes one; overwriting it is the narrowest
    // fault that reaches the let-go judgement through the compiled binary, and the detached
    // driver, its log, the journal and the hook fixture around it are all the real ones.
    std::fs::write(world.run_file(run, "launch.json"), "not json")
        .expect("the launch record is overwritten");
    world.release("build.go");
    let log = world.run_file(run, "driver.log");
    world.until("the driver to say the run could not be judged", |_| {
        std::fs::read_to_string(&log)
            .is_ok_and(|said| said.contains("fires a run-end hook could not be judged"))
    });

    let said = std::fs::read_to_string(&log).expect("the driver log is kept");
    assert!(said.contains("launch.json"), "{said}");
    assert!(hook_kinds(&world, run).is_empty(), "{:?}", world.kinds(run));
    assert!(invocations(&world, run).is_empty());
}

/// A hook whose log cannot be opened could not be started, and is recorded as
/// that — without its command ever running, and without the run settling any
/// differently.
#[test]
fn a_hook_whose_log_cannot_be_opened_is_recorded_as_one_that_could_not_start() {
    let world = hooked_world("hooks-no-log");
    let hook = hook(&world);
    world.script("build.wait", "hold");
    let run = "nolog";
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
                "--hook-timeout",
                HOOK_TIMEOUT,
            ],
        )
        .exited(0);
    world.until("a node to be in flight", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    // A file where the hook logs' directory has to be, which no host will create a
    // directory under.
    // llmlint: ignore[tests_mirror_real_usage] a log directory the host cannot create is a
    // filesystem fault, and no verb produces one; a file in its place is the narrowest fault
    // that reaches that branch through the compiled binary, and everything around it — the
    // detached driver, the journal, `result.json` and `results` — is the real one.
    std::fs::write(world.runs.join(run).join("hooks"), "not a directory")
        .expect("something in the way");
    world.release("build.go");
    world.until("the run's hook to be recorded as ended", |world| {
        !world.events_of(run, "run-hook-finished").is_empty()
    });

    assert_eq!(world.run_json(run, "result.json")["state"], "complete");
    assert_eq!(world.events_of(run, "run-hook-fired").len(), 1);
    let finished = &world.events_of(run, "run-hook-finished")[0]["payload"];
    assert_eq!(finished["hook"], "success");
    assert_eq!(finished["ending"], "could-not-start");
    assert_eq!(finished["exit"], Value::Null);
    assert!(
        invocations(&world, run).is_empty(),
        "a hook with nowhere to keep its output was run anyway"
    );
    world
        .run(&["results", run])
        .exited(0)
        .out_has("success hook fired — reason: none; ending: could-not-start; exit: none");
}
