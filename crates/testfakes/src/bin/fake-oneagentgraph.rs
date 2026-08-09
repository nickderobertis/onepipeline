//! A real `oneagentgraph` executable, scripted from a directory.
//!
//! It speaks the sibling's command surface — `run`, `reset-timer`, `health` —
//! emits envelope NDJSON on stdout, and records every invocation so a test can
//! assert on what `onepipeline` actually asked for. It is a stand-in for
//! `oneagentgraph`, which is itself interface-only and refuses everything; the
//! composition on this side of the boundary is what the tests exercise.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

/// The label keys the real `oneagentgraph` stamps itself, and so refuses on a
/// `--label`. Its own list, restated here because a double stands in for the
/// sibling's *behaviour* — a crate-level test drives the same keys through
/// `oneagentgraph::run::parse_label`, which is the copy that cannot drift.
const RESERVED_LABELS: &[&str] = &["run_id", "member", "persona"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "oneagentgraph", &args);

    match args.first().map(String::as_str) {
        Some("run") => run(&args, &dir),
        Some("reset-timer") => reset_timer(&args, &dir),
        Some("health") => {
            println!("fake-provider: 1 identity bound, 0% utilized");
            ExitCode::SUCCESS
        }
        Some(other) => fake::refuse(&format!("unknown oneagentgraph command '{other}'")),
        None => fake::refuse("oneagentgraph takes a command"),
    }
}

/// `oneagentgraph reset-timer RUN MEMBER`
fn reset_timer(args: &[String], dir: &std::path::Path) -> ExitCode {
    for (at, name) in [(1, "RUN"), (2, "MEMBER")] {
        if let Err(refusal) = fake::required(args, at, name) {
            return refusal;
        }
    }
    if dir.join("reset-timer.fail").exists() {
        eprintln!("no resettable schedule named that member");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

/// `oneagentgraph run GRAPH --task T --output json [--label k=v]...`
fn run(args: &[String], dir: &std::path::Path) -> ExitCode {
    let graph = match fake::required(args, 1, "GRAPH") {
        Ok(graph) => graph,
        Err(refusal) => return refusal,
    };
    // The real CLI requires both, so a caller that stopped sending either would
    // otherwise go unnoticed here and fail only against the sibling itself.
    let Some(task) = fake::flag(args, "--task") else {
        return fake::refuse("oneagentgraph run requires --task");
    };
    if fake::flag(args, "--output").as_deref() != Some("json") {
        return fake::refuse("oneagentgraph run requires --output json");
    }
    // The real CLI refuses a `--label` naming a key it stamps itself, and exits
    // 2 doing it. A double that accepted one would be the weak oracle that let
    // every dispatch be refused by the real sibling while this suite stayed
    // green — which is exactly what happened.
    for label in fake::flags(args, "--label") {
        let key = label.split('=').next().unwrap_or_default();
        if RESERVED_LABELS.contains(&key) {
            eprintln!(
                "oneagentgraph: invalid config: --label {label:?}: {key:?} is a reserved label \
                 this run stamps itself; pick another name so a consumer can tell the two apart"
            );
            return ExitCode::from(2);
        }
    }

    // The dag-scope graph is the driver: its orchestrator member is what runs
    // the engine verbs. Acting that out is how a test exercises `start` end to
    // end rather than only the launch.
    if graph.contains("dag-scope") {
        return drive(dir);
    }

    let node = fake::label(args, "onepipeline.node").unwrap_or_else(|| "unknown".into());
    let step = fake::label(args, "onepipeline.step");
    let persona = fake::label(args, "onepipeline.persona");
    // A node dispatches under more than one persona — its own worker, and the
    // `pr-author` that drafts its change request — so a script may name either
    // the persona or the node/step it applies to.
    let key = match (&step, &persona) {
        (_, Some(persona)) if dir.join(format!("{node}.{persona}.fail")).exists() => {
            format!("{node}.{persona}")
        }
        (Some(step), _) => format!("{node}.{step}"),
        (None, _) => node.clone(),
    };

    // Hold the dispatch open until the test releases it, so an edit, a stall
    // watch, or a driver death can happen while a node is genuinely in flight.
    if dir.join(format!("{key}.wait")).exists() {
        fake::wait_for(&dir.join(format!("{key}.go")));
    }

    // A dispatch that produces nothing and fails is the case boundary retry
    // exists for: the failure carries no work to lose.
    if dir.join(format!("{key}.silent")).exists() {
        let attempts = dir.join(format!("{key}.attempts"));
        fake::append(&attempts, "attempt");
        let so_far = std::fs::read_to_string(&attempts)
            .unwrap_or_default()
            .lines()
            .count();
        // A scripted count that is not a count is a test that means something
        // other than what it says: read leniently it becomes "never recovers",
        // so the scenario written to prove recovery would prove the opposite.
        let recover_after: usize = match fake::node_script(dir, &key, "recover-after") {
            None => usize::MAX,
            Some(text) => match text.trim().parse() {
                Ok(after) => after,
                Err(_) => fake::fail(&format!(
                    "{key}.recover-after holds {text:?}, which is not an attempt count"
                )),
            },
        };
        if so_far < recover_after {
            eprintln!("provider refused before the first turn");
            return ExitCode::from(1);
        }
    }

    // A line from a build whose envelope shape this one cannot read. It is
    // emitted *before* the good one, so a reader that stopped at it would lose
    // the turn that follows.
    if dir.join(format!("{key}.unreadable")).exists() {
        println!("{{\"from\":\"a newer oneagentgraph\"}}");
    }
    emit(args, &node, step.as_deref(), &task);

    if let Some(code) = fake::node_script(dir, &key, "fail") {
        // A scripted code that is not a code is a test that means something
        // other than what it says. Defaulting it to 1 would quietly pass the
        // scenario the author did not write.
        let Ok(code) = code.parse::<u8>() else {
            fake::fail(&format!(
                "{key}.fail holds {code:?}, which is not an exit code"
            ));
        };
        eprintln!("the node failed its gate");
        return ExitCode::from(code);
    }
    ExitCode::SUCCESS
}

/// Emit one envelope, as the sibling would.
///
/// The labels are stamped the way the real CLI stamps them: the graph run's own
/// identity under the keys it reserves, and every `--label` the caller passed
/// carried through verbatim beside them. The two `run_id`s on one line are the
/// point — a graph run is not the pipeline run that dispatched it.
fn emit(args: &[String], node: &str, step: Option<&str>, task: &str) {
    let mut labels = serde_json::Map::new();
    labels.insert(
        "run_id".to_string(),
        format!("fake-graph-{}", std::process::id()).into(),
    );
    labels.insert("member".to_string(), "worker".into());
    for pair in fake::flags(args, "--label") {
        if let Some((key, value)) = pair.split_once('=') {
            labels.insert(key.to_string(), value.into());
        }
    }
    // The node and the step are what the run being acted out is, so they are
    // stated rather than echoed: a node with no `--label` of its own still
    // belongs to one.
    labels.insert("onepipeline.node".to_string(), node.into());
    if let Some(step) = step {
        labels.insert("onepipeline.step".to_string(), step.into());
    }
    println!(
        "{}",
        serde_json::json!({
            "v": 1,
            "ts": fake::now(),
            "stream": format!("fake-oneagentgraph-{}", std::process::id()),
            "seq": 0,
            "source": "agentgraph",
            "kind": "turn-finished",
            "labels": labels,
            "payload": {
                "message": "the dispatch ran",
                // Echoed so a test can assert the task prose the node was given,
                // including the rendered planner-context section.
                "task": task,
                "dir": fake::flag(args, "--dir"),
                // A pr-author dispatch answers with the title it drafted.
                "title": drafted_title(task),
            },
        })
    );
}

/// The title a `pr-author` dispatch drafts, when it is one.
fn drafted_title(task: &str) -> Option<String> {
    task.starts_with("Read this branch's diff")
        .then(|| "feat: drafted from the diff".to_string())
}

/// Act as the dag-scope graph's orchestrator member: drive the engine verbs.
///
/// Exactly what the shipped `orchestrator` persona instructs — `round run` then
/// `round next`, and nothing else that changes run state — so `start`'s journey
/// is exercised through the same commands a real orchestrator would use.
fn drive(dir: &std::path::Path) -> ExitCode {
    // Required, not defaulted: an empty run id would send every engine verb at
    // a run named by nothing, and the refusals that came back would read as the
    // engine's fault rather than as the launcher never having said which run
    // this driver is for.
    let run = match std::env::var("ONEPIPELINE_RUN_ID") {
        Ok(run) if !run.is_empty() => run,
        _ => fake::fail("ONEPIPELINE_RUN_ID is unset: no run to drive"),
    };
    let binary = match std::env::var("ONEPIPELINE_FAKE_DRIVER_BIN") {
        Ok(binary) if !binary.is_empty() => binary,
        _ => fake::fail("ONEPIPELINE_FAKE_DRIVER_BIN is unset: nothing to drive the run with"),
    };
    // The first thing a real orchestrator member does is read the run's ledger,
    // so the first thing this one records is whether that ledger was there to
    // be read. A launcher that started its driver before writing the launch
    // record leaves this `false` — and the driver then dies on a file its own
    // launcher had not written yet, with the run stuck at `run-started`.
    fake::append(
        &dir.join("driver-saw.jsonl"),
        &serde_json::json!({
            "run": run,
            "launch_record": launch_record(&run).is_some_and(|path| path.is_file()),
        })
        .to_string(),
    );

    if dir.join("driver.wait").exists() {
        fake::wait_for(&dir.join("driver.go"));
    }
    // Bounded: a graph that never settles ends this driver rather than
    // spinning, so a test fails on its own assertion instead of hanging.
    for _ in 0..8 {
        // Inherited on purpose: an attached launcher relays what the engine
        // verb says, and a real orchestrator member does not swallow it. What
        // it *exited* with is recorded, because that is the fact this driver
        // then acts on and nothing else in the tree writes it down.
        let ran = std::process::Command::new(&binary)
            .args(["round", "run", &run])
            .status();
        let Ok(ran) = ran else {
            return ExitCode::from(1);
        };
        fake::append(
            &dir.join("driver-saw.jsonl"),
            &serde_json::json!({"run": run, "round_run": ran.code()}).to_string(),
        );
        // A round that settled unfinished is the planner's move, not the
        // orchestrator's: it never re-dispatches a failed node on its own
        // judgement. The shipped persona says the same thing in prose.
        if !ran.success() {
            return ExitCode::SUCCESS;
        }
        let next = std::process::Command::new(&binary)
            .args(["round", "next", &run])
            .output();
        let Ok(next) = next else {
            return ExitCode::from(1);
        };
        if !next.status.success() {
            fake::append(
                &dir.join("driver-saw.jsonl"),
                &serde_json::json!({
                    "run": run,
                    "round_next": next.status.code(),
                    "stderr": String::from_utf8_lossy(&next.stderr),
                })
                .to_string(),
            );
            return ExitCode::from(1);
        }
        // `continuing` is the only answer that means there is another round to
        // run. Complete, and every gated state, ends the driver.
        if !String::from_utf8_lossy(&next.stdout).contains("\"continuing\"") {
            return ExitCode::SUCCESS;
        }
    }
    ExitCode::SUCCESS
}

/// The launch record of the run this driver was started for.
///
/// Resolved from the same two variables the launcher hands every driver, so the
/// probe above reads exactly the file `onepipeline round run` opens first.
fn launch_record(run: &str) -> Option<std::path::PathBuf> {
    let root = std::env::var("ONEPIPELINE_RUNS_DIR").ok()?;
    (!root.is_empty() && !run.is_empty())
        .then(|| std::path::PathBuf::from(root).join(run).join("launch.json"))
}
