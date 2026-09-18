//! The dispatch-env hook, driven end to end against the compiled binary.
//!
//! A launch names a command; the engine runs it immediately before every
//! node-scope dispatch, overlays the `env` of the one document it prints onto
//! that child launch alone, and checks every `env_from` source the launch's
//! oneharness configs name against the refreshed environment. Every journey here
//! names a **real** command — the fixture pair `dispatch_env_hook.sh` /
//! `dispatch_env_hook.bat` — which records its working directory, its environment
//! and what it was handed, so what the engine gave a hook is read off the process
//! that ran rather than off anything this crate wrote down. What the *child* was
//! handed is read off the child: the `oneagentgraph` double reports its own
//! environment where a journey asks, and the real sibling's harness double
//! reports the turn's.
//!
//! The contract's own dispatch-env hook block is read rather than restated: the
//! hook's name, the environment it is given, the log, the endings and the outcome
//! a refusal settles under all come out of `docs/contract.md`.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary; one journey drives the real sibling with only the paid model turn
// standing in, and the lifecycle journey publishes through the linked `onevcs` against a
// real git origin. The hook is not a substitution either: it is the operator's own
// command, and this suite supplies a real one. `harness.rs` carries the same suppression
// and the full rationale.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::harness::{
    agent, double, human, lifecycle, plan_of, repo_file, Run, World, REFUSED, USAGE_ERROR,
};

/// Where the fixture records what it was handed. Read by the fixture itself.
const RECORD_ENV: &str = "ONEPIPELINE_E2E_HOOK_RECORD";

/// The variable the hook adds in these journeys, and the source the
/// `env_from` configs below name.
const SUPPLIED: &str = "ONEPIPELINE_E2E_HOOK_SUPPLIED";

/// The variable a variant's `env_from` hands the harness, sourced from
/// [`SUPPLIED`].
const HANDED: &str = "ONEPIPELINE_E2E_HANDED";

/// The worker's oneharness config, as a routing change would leave it: a
/// variant that sources a variable the driver was never given.
///
/// The binary is named in the file, and has to be: oneharness reads the
/// `ONEHARNESS_BIN_<ID>` override by the **composed** selection, so a variant
/// selection is not covered by the `ONEHARNESS_BIN_CLAUDE_CODE` every launch
/// here carries, and a config selecting `claude-code:hooked` without a `bin`
/// would run the real `claude` for a real paid turn.
fn hooked_worker_config() -> String {
    format!(
        "run_mode = \"fallback\"\nharnesses = [\"claude-code:hooked\"]\n\n[env]\n\
         ONEPIPELINE_FAKE_MEMBER = \"worker\"\n\n[harness.claude-code]\nbin = {:?}\n\n\
         [harness.claude-code.variant.hooked.env_from]\n{HANDED} = \"{SUPPLIED}\"\n",
        double("fake-claude").to_string_lossy()
    )
}

/// The contract's dispatch-env hook block.
fn contract_block() -> Value {
    let contract =
        std::fs::read_to_string(repo_file("docs/contract.md")).expect("the contract ships");
    let block = contract
        .split("```json")
        .skip(1)
        .filter_map(|rest| rest.split("```").next())
        .find(|block| block.contains("\"dispatch_env_hook\""))
        .expect("the contract carries the dispatch-env hook block");
    serde_json::from_str::<Value>(block).expect("the block is JSON")["dispatch_env_hook"].clone()
}

/// A world whose hook fixture records into the world's own scratch, and whose
/// node-scope graph names oneharness configs that exist.
///
/// The shipped node-scope graph names configs it does not ship, and a launch
/// naming a hook reads every config its graph names — so these journeys launch
/// the world's own graph, written with a real worker config beside it.
fn hooked_world(name: &str) -> World {
    let world = World::new(name);
    std::fs::create_dir_all(records(&world)).expect("a record directory");
    let record = records(&world).to_string_lossy().into_owned();
    world.write_graphs();
    let graph = world.graphs().join("node-scope.yaml");
    world
        .with_env(RECORD_ENV, &record)
        .with_env("ONEPIPELINE_NODE_GRAPH", &graph.to_string_lossy())
}

fn records(world: &World) -> PathBuf {
    world.root.join("hook-records")
}

/// The dispatch-env hook fixture, placed in the world, as the command a launch
/// names.
#[cfg(unix)]
fn hook(world: &World) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = world.root.join("dispatch_env_hook.sh");
    std::fs::write(&path, include_str!("dispatch_env_hook.sh")).expect("the hook is written");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("the hook is executable");
    path.to_string_lossy().into_owned()
}

/// The dispatch-env hook fixture, placed in the world, as the command a launch
/// names — with CRLF, for the reason `harness::write_hook_script` gives.
#[cfg(windows)]
fn hook(world: &World) -> String {
    let path = world.root.join("dispatch_env_hook.bat");
    std::fs::write(
        &path,
        include_str!("dispatch_env_hook.bat").replace('\n', "\r\n"),
    )
    .expect("the hook is written");
    path.to_string_lossy().into_owned()
}

/// Script what the hook prints for one run.
fn prints(world: &World, run: &str, document: &Value) {
    std::fs::write(
        records(world).join(format!("{run}.stdout")),
        format!("{document}\n"),
    )
    .expect("the hook's document is scripted");
}

/// A document adding one variable.
fn adding(name: &str, value: &str) -> Value {
    json!({"version": 1, "env": {name: value}})
}

/// The nodes the fixture was run for, for one run, in order.
fn invocations(world: &World, run: &str) -> Vec<String> {
    let path = records(world).join(run).join("invocations");
    // Absent is a hook never run, and nothing else is: this file is the only
    // witness to a run, so an unreadable one is not "none".
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
    /// The four variables the fixture reports.
    env: BTreeMap<String, String>,
    /// Every variable of the environment it was started with, by name.
    environment: BTreeMap<String, String>,
    stdin: String,
}

fn handed(world: &World, run: &str, nth: usize) -> Handed {
    let dir = records(world).join(run).join(nth.to_string());
    let read = |name: &str| {
        std::fs::read_to_string(dir.join(name)).unwrap_or_else(|error| {
            panic!("the hook recorded no {name} at {}: {error}", dir.display())
        })
    };
    let pairs = |text: String| -> BTreeMap<String, String> {
        text.lines()
            .map(|line| line.trim_end_matches('\r'))
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    };
    Handed {
        cwd: PathBuf::from(read("cwd").trim()),
        env: pairs(read("env")),
        environment: pairs(read("environment")),
        stdin: read("stdin"),
    }
}

/// Whether two spellings name one directory or file.
fn same_place(one: &Path, other: &Path) -> bool {
    match (std::fs::canonicalize(one), std::fs::canonicalize(other)) {
        (Ok(one), Ok(other)) => one == other,
        _ => false,
    }
}

/// Start a run attached, from the world's project directory, naming the hook.
fn attached(world: &World, name: &str, nodes: Vec<Value>, extra: &[&str]) -> Run {
    let path = world.plan(name, &plan_of(name, nodes));
    let mut args = vec!["start", path.as_str(), "--attach"];
    args.extend_from_slice(extra);
    world.run_from(&world.project, &args)
}

/// What the `oneagentgraph` double reported of its own environment: each row is
/// `(who, name, state, value)`.
fn dispatch_env(world: &World) -> Vec<(String, String, String, String)> {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "dispatch-env")
        .map(|call| {
            let arg = |at: usize| {
                call["args"][at]
                    .as_str()
                    .unwrap_or_else(|| panic!("a dispatch-env row lacks argument {at}: {call}"))
                    .to_string()
            };
            (arg(0), arg(1), arg(2), arg(3))
        })
        .collect()
}

/// The `node-settled` of one node.
fn settlement(world: &World, run: &str, node: &str) -> Value {
    world
        .events_of(run, "node-settled")
        .into_iter()
        .find(|event| event["labels"]["node"] == node)
        .unwrap_or_else(|| panic!("{node} never settled in {run}:\n{}", world.dump()))
}

/// Every file under a run's own directory whose bytes hold `needle`.
fn files_holding(root: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if std::fs::read(&path)
                .map(|bytes| {
                    bytes
                        .windows(needle.len())
                        .any(|window| window == needle.as_bytes())
                })
                .unwrap_or(false)
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Nothing the run keeps, and no view of it, carries the value.
fn nothing_kept_holds(world: &World, run: &str, sentinel: &str) {
    let leaked = files_holding(&world.runs.join(run), sentinel);
    assert!(
        leaked.is_empty(),
        "a value the hook printed reached the run's own storage: {leaked:?}"
    );
    for view in [
        vec!["results", run],
        vec!["status", run],
        vec!["runs"],
        vec!["monitor", run, "--all"],
    ] {
        let shown = world.run(&view);
        assert!(
            !shown.stdout.contains(sentinel) && !shown.stderr.contains(sentinel),
            "`onepipeline {}` showed a value the hook printed:\n{}\n{}",
            view.join(" "),
            shown.stdout,
            shown.stderr
        );
    }
}

/// The two halves of the fixture number an invocation the same way.
///
/// One contract in two languages, for the reason
/// `run_end_hooks::both_run_end_hook_halves_number_an_invocation_the_same_way`
/// gives: no platform runs both, so a half that reaches its record directory by a
/// route the other does not is a journey that passes on one leg and hangs on the
/// other.
// llmlint: ignore-block[tests_mirror_real_usage] a drift gate over the suite's own
// scaffolding rather than a journey, exactly as `run_end_hooks.rs`'s is: what it holds is
// that two files stay in step, no platform executes both, and reading them is the only way
// to compare them. The fixture itself is driven as the operator's real command by every
// journey below.
#[test]
fn both_dispatch_env_hook_halves_number_an_invocation_the_same_way() {
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
                panic!("{script} states no ceiling for how many invocations one run records")
            })
    };
    assert_eq!(
        ceiling("dispatch_env_hook.sh", include_str!("dispatch_env_hook.sh")),
        ceiling(
            "dispatch_env_hook.bat",
            include_str!("dispatch_env_hook.bat")
        ),
        "the fixture's halves give up claiming a record directory at different ceilings"
    );
    for (script, source, claim) in [
        (
            "dispatch_env_hook.sh",
            include_str!("dispatch_env_hook.sh"),
            r#"until mkdir "$record/$run/$nth""#,
        ),
        (
            "dispatch_env_hook.bat",
            include_str!("dispatch_env_hook.bat"),
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

/// The journey the hook exists for: it runs before each node's dispatch — under a
/// detached driver too — in the launch directory, with the contract's
/// environment and nothing on its stdin; what it prints reaches that one child
/// and nothing else, a later dispatch is handed what the hook prints *then*, the
/// dag-scope observer is never asked and never given it, and no value it prints
/// reaches anything the run keeps or shows.
#[test]
fn a_named_hook_runs_before_every_node_dispatch_and_its_env_reaches_that_child_alone() {
    let world = hooked_world("dispatch-env-runs");
    let hook = hook(&world);
    let block = contract_block();
    let run = "refreshed";
    let variable = "ONEPIPELINE_E2E_REFRESHED";
    let first = "first-sentinel-3f9c1b";
    let second = "second-sentinel-8a2d7e";
    prints(&world, run, &adding(variable, first));
    world.script("dispatch.report-env", &format!("{variable}\n"));
    // The first node is held open, so the document can change between the two
    // dispatches and the second is handed what the hook prints then.
    world.script("build.wait", "");
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), agent("ship", &["build"])]),
    );
    let dag = world.dag_graph();
    world
        .run_from(
            &world.project,
            &[
                "start",
                &path,
                "--detach",
                "--dag-graph",
                &dag,
                "--dispatch-env-hook",
                &hook,
            ],
        )
        .exited(0);
    world.until("the first dispatch to be inside its hold", |world| {
        dispatch_env(world)
            .iter()
            .any(|(who, _, _, _)| who == "build")
    });
    prints(&world, run, &adding(variable, second));
    world.release("build.go");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(
        world.run_json(run, "result.json")["state"],
        "complete",
        "{}",
        world.dump()
    );

    // Once per node dispatch, in dispatch order, and never for the observer —
    // which did launch.
    assert_eq!(
        invocations(&world, run),
        ["build", "ship"],
        "{}",
        world.dump()
    );
    assert!(
        world.was_invoked("oneagentgraph", &["run", &dag]),
        "the observer never launched: {:?}",
        world.invocations()
    );

    // What each was handed, off the process that ran.
    let launch = world.run_json(run, "launch.json");
    let environment: Vec<String> = serde_json::from_value(block["environment"].clone())
        .expect("the block names the environment");
    assert_eq!(
        environment,
        [
            "ONEPIPELINE_HOOK",
            "ONEPIPELINE_RUN_ID",
            "ONEPIPELINE_RUN_ROOT",
            "ONEPIPELINE_NODE_ID"
        ],
        "the contract names an environment the fixture does not record"
    );
    for (nth, node) in [(1, "build"), (2, "ship")] {
        let handed = handed(&world, run, nth);
        assert!(
            same_place(
                &handed.cwd,
                Path::new(launch["dir"].as_str().expect("a dir"))
            ) && same_place(&handed.cwd, &world.project),
            "the hook for {node} ran in {} rather than the launch directory",
            handed.cwd.display()
        );
        assert_eq!(
            handed.env["hook"],
            block["hook"].as_str().expect("the block names the hook")
        );
        assert_eq!(handed.env["run_id"], run);
        let run_root = &handed.env["run_root"];
        assert!(Path::new(run_root).is_absolute(), "{run_root}");
        assert!(same_place(Path::new(run_root), &world.runs.join(run)));
        assert_eq!(handed.env["node_id"], node);
        assert!(
            handed.stdin.trim().is_empty(),
            "the hook was handed something on stdin: {}",
            handed.stdin
        );
        // The driver's own environment is unchanged: the second hook, run from
        // that same driver after the first dispatch was given the variable, does
        // not see it.
        assert!(
            !handed.environment.contains_key(variable),
            "the hook for {node} was started with {variable} in its environment, so an \
             earlier dispatch's additions leaked into the driver"
        );
    }

    // The child was handed it — each dispatch the value the hook printed for
    // it — and the observer was not.
    let reported = dispatch_env(&world);
    let of = |who: &str| -> (String, String) {
        reported
            .iter()
            .find(|(reported, name, _, _)| reported == who && name == variable)
            .map(|(_, _, state, value)| (state.clone(), value.clone()))
            .unwrap_or_else(|| panic!("{who} reported nothing about {variable}: {reported:?}"))
    };
    assert_eq!(of("build"), ("set".to_string(), first.to_string()));
    assert_eq!(of("ship"), ("set".to_string(), second.to_string()));
    assert_eq!(of("observer"), ("unset".to_string(), String::new()));

    // The launch record names the hook and the shipped timeout, and `adopt`
    // would replay them.
    assert_eq!(launch["dispatch_env_hook"], hook);
    assert_eq!(
        launch["dispatch_env_hook_timeout"],
        block["default_timeout_seconds"]
    );

    // Its stderr was kept — one entry per dispatch, named — and nothing of its
    // stdout was: no value it printed is anywhere the run keeps or shows.
    let log = world.runs.join(run).join(
        block["log"]
            .as_str()
            .expect("the block names the log")
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    );
    let kept = std::fs::read_to_string(&log).expect("the hook's log is kept");
    for node in ["build", "ship"] {
        assert!(
            kept.contains(&format!("dispatch-env hook ran for {node}")),
            "{kept}"
        );
    }
    for sentinel in [first, second] {
        assert!(!kept.contains(sentinel), "{kept}");
        nothing_kept_holds(&world, run, sentinel);
    }
}

/// A hook that exits non-zero, cannot start, outlives its timeout, or prints a
/// malformed document — another top-level member, a non-string value — refuses
/// the launch: nothing is dispatched, the node settles `infrastructure-failure`
/// with a detail naming that ending, and the hook is asked again on each retry
/// that outcome already has. A dispatch the hook admitted that then produces
/// nothing is re-asked with the hook run again before it.
#[test]
fn a_hook_that_fails_times_out_cannot_start_or_prints_a_malformed_document_refuses_the_launch() {
    let world = hooked_world("dispatch-env-endings");
    let hook = hook(&world);
    let block = contract_block();
    let endings: Vec<String> =
        serde_json::from_value(block["endings"].clone()).expect("the block names the endings");
    assert_eq!(endings, ["exit", "timeout", "could-not-start", "malformed"]);
    let outcome = block["refusal_outcome"]
        .as_str()
        .expect("the block names the outcome");
    let missing = world
        .root
        .join("no-such-hook")
        .to_string_lossy()
        .into_owned();
    let record = records(&world);
    std::fs::write(record.join("exits.exit"), "7").expect("the exit is scripted");
    std::fs::write(record.join("outlives.hold"), "").expect("the hold is scripted");
    prints(
        &world,
        "extra",
        &json!({"version": 1, "env": {"A": "x"}, "extra": true}),
    );
    prints(
        &world,
        "nonstring",
        &json!({"version": 1, "env": {"COUNT": 5, "OK": "yes"}}),
    );
    std::fs::write(
        record.join("twice.stdout"),
        "{\"version\":1,\"env\":{}}\n{\"version\":1,\"env\":{}}\n",
    )
    .expect("two documents are scripted");
    for (run, command, timeout, names) in [
        ("exits", hook.as_str(), None, vec!["exit 7"]),
        (
            "unstartable",
            missing.as_str(),
            None,
            vec!["could-not-start"],
        ),
        ("outlives", hook.as_str(), Some("1"), vec!["timeout"]),
        ("extra", hook.as_str(), None, vec!["malformed", "`extra`"]),
        (
            "nonstring",
            hook.as_str(),
            None,
            vec!["malformed", "`env.COUNT` is not a string"],
        ),
        (
            "twice",
            hook.as_str(),
            None,
            vec!["malformed", "not one JSON document"],
        ),
    ] {
        let mut extra = vec!["--dispatch-env-hook", command];
        if let Some(seconds) = timeout {
            extra.extend(["--dispatch-env-hook-timeout", seconds]);
        }
        attached(&world, run, vec![agent("build", &[])], &extra).settled();
        let settled = settlement(&world, run, "build");
        assert_eq!(settled["payload"]["status"], "failed", "{settled}");
        assert_eq!(settled["payload"]["outcome"], outcome, "{settled}");
        let detail = settled["payload"]["detail"].as_str().unwrap_or_default();
        for named in &names {
            assert!(
                detail.contains(named),
                "the {run} refusal does not name {named:?}: {detail}"
            );
        }
        assert!(
            detail.contains("build") && detail.contains("dispatch-env hook"),
            "the refusal does not say which node's launch which hook refused: {detail}"
        );
        // Retried as the outcome always is, the hook asked again each time — and
        // nothing dispatched on any attempt.
        let attempts = world.events_of(run, "node-dispatched").len();
        assert!(attempts > 1, "{run} was not retried: {}", world.dump());
        if command == hook.as_str() {
            assert_eq!(invocations(&world, run).len(), attempts, "{}", world.dump());
        }
        assert!(
            !world.was_invoked(
                "oneagentgraph",
                &["--label", &format!("onepipeline.run_id={run}")]
            ) && !world.was_invoked("oneagentgraph", &["--label", "onepipeline.node=build"]),
            "a launch the hook refused was dispatched anyway: {:?}",
            world.invocations()
        );
        world
            .run(&["results", run])
            .exited(0)
            .out_has("build")
            .out_has(outcome);
    }
    // A hook that printed a value alongside its non-string one leaks it nowhere.
    nothing_kept_holds(&world, "nonstring", "yes");

    // A dispatch the hook admitted and that then produced nothing is re-asked,
    // and each re-ask runs the hook again before it.
    world.script("build.silent", "");
    attached(
        &world,
        "reasked",
        vec![agent("build", &[])],
        &["--dispatch-env-hook", &hook],
    )
    .settled();
    let settled = settlement(&world, "reasked", "build");
    assert_eq!(
        settled["payload"]["outcome"], "no-agent-progress",
        "{settled}"
    );
    let attempts = world.events_of("reasked", "node-dispatched").len();
    assert!(attempts > 1, "{}", world.dump());
    assert_eq!(
        invocations(&world, "reasked"),
        vec!["build"; attempts],
        "{}",
        world.dump()
    );
}

/// After the hook's additions, every `env_from` source the launch's configs
/// name — the config after `--node-set` overrides, as it is on disk now — has to
/// be there: one missing refuses the launch with a detail naming the config
/// file, the harness variant and the key, and the same launch with the hook
/// supplying it dispatches.
#[test]
fn a_missing_env_from_source_refuses_the_launch_naming_the_file_the_variant_and_the_key() {
    let world = hooked_world("dispatch-env-sources");
    let hook = hook(&world);
    let outcome = contract_block()["refusal_outcome"]
        .as_str()
        .expect("the block names the outcome")
        .to_string();
    let worker = world.graphs().join("oneharness-worker.toml");
    std::fs::write(&worker, hooked_worker_config()).expect("the worker config is written");
    // And a second config, for a launch that moves the worker onto it.
    let other = world.graphs().join("oneharness-other.toml");
    std::fs::write(
        &other,
        "harnesses = [\"claude-code:alt\"]\n\n[harness.claude-code.variant.alt.env_from]\n\
         ONEPIPELINE_E2E_ALT = \"ONEPIPELINE_E2E_OTHER_SOURCE\"\n",
    )
    .expect("the other config is written");
    world.script("dispatch.report-env", &format!("{SUPPLIED}\n"));

    // The hook prints an empty environment, so the source stays missing.
    prints(&world, "lacking", &json!({"version": 1, "env": {}}));
    attached(
        &world,
        "lacking",
        vec![agent("build", &[])],
        &["--dispatch-env-hook", &hook],
    )
    .settled();
    let settled = settlement(&world, "lacking", "build");
    assert_eq!(settled["payload"]["outcome"], outcome, "{settled}");
    let detail = settled["payload"]["detail"].as_str().unwrap_or_default();
    for named in [
        worker.to_string_lossy().as_ref(),
        "claude-code:hooked",
        SUPPLIED,
        "build",
    ] {
        assert!(
            detail.contains(named),
            "the refusal does not name {named:?}: {detail}"
        );
    }
    assert!(
        !world.was_invoked("oneagentgraph", &["--label", "onepipeline.node=build"]),
        "a launch missing an env_from source was dispatched anyway: {:?}",
        world.invocations()
    );

    // The same config read after a `--node-set` that moves the worker: the
    // other file's source is what is missing now, and the refusal names that
    // file, that variant and that key rather than the first's.
    prints(&world, "moved", &adding(SUPPLIED, "supplied"));
    attached(
        &world,
        "moved",
        vec![agent("build", &[])],
        &[
            "--dispatch-env-hook",
            &hook,
            "--node-set",
            "members.worker.oneharness_config=./oneharness-other.toml",
        ],
    )
    .settled();
    let settled = settlement(&world, "moved", "build");
    assert_eq!(settled["payload"]["outcome"], outcome, "{settled}");
    let detail = settled["payload"]["detail"].as_str().unwrap_or_default();
    for named in [
        other.to_string_lossy().as_ref(),
        "claude-code:alt",
        "ONEPIPELINE_E2E_OTHER_SOURCE",
    ] {
        assert!(
            detail.contains(named),
            "the refusal does not name {named:?}: {detail}"
        );
    }
    assert!(
        !detail.contains(SUPPLIED),
        "the refusal names a source the hook supplied: {detail}"
    );

    // Supplied by the hook, the launch goes ahead and the child holds it.
    let sentinel = "supplied-sentinel-5c0e4a";
    prints(&world, "supplied", &adding(SUPPLIED, sentinel));
    attached(
        &world,
        "supplied",
        vec![agent("build", &[])],
        &["--dispatch-env-hook", &hook],
    )
    .exited(0)
    .settled();
    assert_eq!(
        settlement(&world, "supplied", "build")["payload"]["status"],
        "done"
    );
    assert!(
        dispatch_env(&world)
            .iter()
            .any(|(who, name, state, value)| {
                who == "build" && name == SUPPLIED && state == "set" && value == sentinel
            }),
        "the dispatch was not handed what the hook supplied: {:?}",
        dispatch_env(&world)
    );
    nothing_kept_holds(&world, "supplied", sentinel);
}

/// Against the **real** `oneagentgraph`: a node whose oneharness config names an
/// `env_from` variable absent from the driver's environment and supplied only
/// by the hook dispatches, and the harness process — where an `env_from` target
/// lands — is handed that variable; a second dispatch after the hook's output
/// changes is handed the new value. Nothing either value reaches is anything the
/// run keeps.
#[test]
fn the_real_harness_is_handed_the_env_from_target_the_hook_alone_supplied() {
    let world = hooked_world("dispatch-env-real");
    let hook = hook(&world);
    std::fs::write(
        world.graphs().join("oneharness-worker.toml"),
        hooked_worker_config(),
    )
    .expect("the worker config is written");
    world.script("turn.report-env", &format!("{HANDED}\n{SUPPLIED}\n"));
    let run = "realhanded";
    let first = "real-first-sentinel-b71e2c";
    let second = "real-second-sentinel-d94a06";
    prints(&world, run, &adding(SUPPLIED, first));
    // The first turn is held, so the hook's document can change between the two
    // dispatches.
    world.script("turn.hold", "");
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), agent("ship", &["build"])]),
    );
    let mut command =
        world.agentgraph_cmd(&["start", &path, "--detach", "--dispatch-env-hook", &hook]);
    command.current_dir(&world.project);
    world.run_on(command, "start realhanded").exited(0);
    world.until("the first turn to be inside its hold", |world| {
        world
            .turns()
            .iter()
            .any(|turn| turn.prompt.contains("Do build."))
    });
    prints(&world, run, &adding(SUPPLIED, second));
    let _ = std::fs::remove_file(world.fakes.join("turn.hold"));
    world.release("turn.go");
    world.release("turn.settle");
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });
    assert_eq!(
        world.run_json(run, "result.json")["state"],
        "complete",
        "{}",
        world.dump()
    );
    assert_eq!(
        invocations(&world, run),
        ["build", "ship"],
        "{}",
        world.dump()
    );

    // What each turn was handed, off the harness process: the target the
    // variant sourced, and the source itself, which oneharness passes on too.
    let reported: Vec<Value> = world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "turn-env")
        .collect();
    let of = |node: &str, name: &str| -> String {
        reported
            .iter()
            .find(|row| {
                row["args"][0]
                    .as_str()
                    .is_some_and(|prompt| prompt.contains(&format!("Do {node}.")))
                    && row["args"][1] == name
            })
            .map(|row| {
                assert_eq!(row["args"][2], "set", "{row}");
                row["args"][3].as_str().unwrap_or_default().to_string()
            })
            .unwrap_or_else(|| panic!("{node}'s turn reported nothing about {name}: {reported:?}"))
    };
    assert_eq!(of("build", HANDED), first);
    assert_eq!(of("ship", HANDED), second);
    for sentinel in [first, second] {
        nothing_kept_holds(&world, run, sentinel);
    }
}

/// A lifecycle node's worker launch runs the hook, and so does the drafting
/// dispatch its `--pr-author-graph` makes — whose refusal takes that dispatch's
/// own ending: the change request publishes with no drafted body,
/// `body-not-drafted` names the hook's ending, and the node settles on its
/// publication as it would have.
#[test]
fn a_lifecycle_nodes_worker_launch_runs_the_hook_and_a_refused_drafting_dispatch_publishes_with_no_body(
) {
    let world = hooked_world("dispatch-env-lifecycle");
    let hook = hook(&world);
    world.repository("change-open", &[]);
    world.script("service.work", "the worker wrote this\n");
    world.script("pr-author.body", "## What\nRead off the branch's diff.\n");
    let drafting = world.pr_author_graph();
    let run = "drafted";
    // The worker's launch is the first invocation and is admitted; the drafting
    // dispatch's is the second and is refused.
    std::fs::write(records(&world).join(format!("{run}.exit")), "7").expect("the exit is scripted");
    std::fs::write(records(&world).join(format!("{run}.exit-from-nth")), "2")
        .expect("the invocation to refuse from is scripted");
    let mut node = lifecycle("service", &[]);
    node["title"] = json!("feat: land it with no drafted body");
    attached(
        &world,
        run,
        vec![node],
        &["--dispatch-env-hook", &hook, "--pr-author-graph", &drafting],
    )
    .settled();

    assert_eq!(
        invocations(&world, run),
        ["service", "service"],
        "{}",
        world.dump()
    );
    for (nth, persona) in [(1, "engineer"), (2, "pr-author")] {
        assert_eq!(handed(&world, run, nth).env["node_id"], "service");
        assert!(
            world.was_invoked(
                "oneagentgraph",
                &["--label", &format!("onepipeline.persona={persona}")]
            ) == (persona == "engineer"),
            "the {persona} dispatch was {}: {:?}",
            if persona == "engineer" {
                "refused"
            } else {
                "launched past the hook's refusal"
            },
            world.invocations()
        );
    }
    let undrafted = world.events_of(run, "body-not-drafted");
    assert_eq!(undrafted.len(), 1, "{undrafted:?}\n{}", world.dump());
    assert_eq!(undrafted[0]["payload"]["ending"], "dispatch-failed");
    let detail = undrafted[0]["payload"]["detail"]
        .as_str()
        .unwrap_or_default();
    assert!(
        detail.contains("dispatch-env hook") && detail.contains("exit 7"),
        "the recorded ending does not name the hook's: {detail}"
    );
    let opened = world.changes_opened();
    assert_eq!(opened.len(), 1, "{opened:?}\n{}", world.dump());
    assert_eq!(opened[0]["title"], "feat: land it with no drafted body");
    assert_eq!(opened[0]["body"], "", "{opened:?}");
    let settled = settlement(&world, run, "service");
    assert_eq!(settled["payload"]["status"], "done", "{settled}");
    assert_eq!(
        world.run_json(run, "result.json")["state"],
        "complete",
        "{}",
        world.dump()
    );
}

/// The flag beats the launch config — a blank one naming none — a timeout of
/// zero is refused before a run is minted by whichever spelling carried it, the
/// keys are refused by name under an earlier schema, both are retained, a launch
/// naming none writes the record it always wrote, and `adopt` takes neither flag
/// and replays what the record retained.
#[test]
fn a_launch_resolves_the_hook_flag_over_config_refuses_a_zero_timeout_and_adopt_replays_it() {
    let world = hooked_world("dispatch-env-launch");
    let hook = hook(&world);
    let block = contract_block();
    let at = block["config_schema_version"]
        .as_u64()
        .expect("the block states the version the keys arrived at");
    let config = world.root.join("launch.yaml");
    std::fs::write(
        &config,
        format!(
            "schema_version: {at}\ndispatch_env_hook: {hook:?}\ndispatch_env_hook_timeout: 5\n"
        ),
    )
    .expect("the config is written");
    let config = config.to_string_lossy().into_owned();

    // A blank flag names none over the config's hook, and a flag's timeout beats
    // the config's — retained only beside a hook.
    let run = "blanked";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &[
            "--launch-config",
            &config,
            "--dispatch-env-hook",
            "",
            "--dispatch-env-hook-timeout",
            "9",
        ],
    )
    .exited(0)
    .settled();
    let launch = world.run_json(run, "launch.json");
    for key in ["dispatch_env_hook", "dispatch_env_hook_timeout"] {
        assert!(launch.get(key).is_none(), "{launch}");
    }
    assert!(invocations(&world, run).is_empty());

    // The config alone names the hook, and its timeout.
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
    assert_eq!(launch["dispatch_env_hook"], hook);
    assert_eq!(launch["dispatch_env_hook_timeout"], 5);
    assert_eq!(invocations(&world, run), ["build"]);

    // And a flag's timeout over the config's hook.
    let run = "flagtimeout";
    attached(
        &world,
        run,
        vec![agent("build", &[])],
        &[
            "--launch-config",
            &config,
            "--dispatch-env-hook-timeout",
            "9",
        ],
    )
    .exited(0)
    .settled();
    let launch = world.run_json(run, "launch.json");
    assert_eq!(launch["dispatch_env_hook"], hook);
    assert_eq!(launch["dispatch_env_hook_timeout"], 9);

    // A blank key names none, as a blank flag does; naming none writes neither
    // key and runs nothing.
    let blank = world.root.join("blank.yaml");
    std::fs::write(
        &blank,
        format!("schema_version: {at}\ndispatch_env_hook: \"  \"\n"),
    )
    .expect("the config is written");
    for (run, extra) in [
        (
            "blankkey",
            vec!["--launch-config", blank.to_str().expect("utf-8")],
        ),
        ("unhooked", vec![]),
    ] {
        attached(&world, run, vec![agent("build", &[])], &extra)
            .exited(0)
            .settled();
        let launch = world.run_json(run, "launch.json");
        for key in ["dispatch_env_hook", "dispatch_env_hook_timeout"] {
            assert!(
                launch.get(key).is_none(),
                "{key} reached the record of {run}: {launch}"
            );
        }
        assert!(invocations(&world, run).is_empty());
        assert!(
            !world.runs.join(run).join("hooks").exists(),
            "a launch naming no hook kept a hook log"
        );
    }

    // Zero is refused by the spelling that carried it, before a run exists, and a
    // key is refused by name under a schema that never had it.
    let path = world.plan("zeroed", &plan_of("zeroed", vec![agent("build", &[])]));
    world
        .run_from(
            &world.project,
            &[
                "start",
                &path,
                "--dispatch-env-hook",
                &hook,
                "--dispatch-env-hook-timeout",
                "0",
            ],
        )
        .exited(REFUSED)
        .err_has("--dispatch-env-hook-timeout")
        .err_has("zero");
    let zero = world.root.join("zero.yaml");
    std::fs::write(
        &zero,
        format!("schema_version: {at}\ndispatch_env_hook_timeout: 0\n"),
    )
    .expect("the config is written");
    world
        .run_from(
            &world.project,
            &["start", &path, "--launch-config", &zero.to_string_lossy()],
        )
        .exited(REFUSED)
        .err_has("`dispatch_env_hook_timeout`")
        .err_has("zero");
    let early = world.root.join("early.yaml");
    std::fs::write(
        &early,
        format!("schema_version: {}\ndispatch_env_hook: {hook:?}\n", at - 1),
    )
    .expect("the config is written");
    world
        .run_from(
            &world.project,
            &["start", &path, "--launch-config", &early.to_string_lossy()],
        )
        .exited(REFUSED)
        .err_has(&format!("`dispatch_env_hook` is a schema {at} key"));
    assert!(!world.run_file("zeroed", "launch.json").is_file());

    // `adopt` takes neither flag, and replays what the record retained: a run
    // paused on a human gate is adopted by a fresh driver, and the dispatch that
    // driver makes runs the hook.
    world
        .run(&["adopt", "configured", "--dispatch-env-hook", &hook])
        .exited(USAGE_ERROR)
        .err_has("--dispatch-env-hook");
    let run = "adopted";
    attached(
        &world,
        run,
        vec![human("approve", &[]), agent("build", &["approve"])],
        &["--launch-config", &config],
    )
    .settled();
    assert!(invocations(&world, run).is_empty(), "{}", world.dump());
    world.run(&["attest", run, "approve"]).exited(0);
    world.run(&["adopt", run]).exited(0);
    assert_eq!(invocations(&world, run), ["build"], "{}", world.dump());
    assert_eq!(handed(&world, run, 1).env["node_id"], "build");
    assert_eq!(
        settlement(&world, run, "build")["payload"]["status"],
        "done"
    );
}
