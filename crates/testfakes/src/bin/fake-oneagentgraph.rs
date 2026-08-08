//! A real `oneagentgraph` executable, scripted from a directory.
//!
//! It speaks the sibling's command surface — `run`, `reset-timer`, `health` —
//! emits envelope NDJSON on stdout, and records every invocation so a test can
//! assert on what `onepipeline` actually asked for. It is a stand-in for
//! `oneagentgraph`, which is itself interface-only and refuses everything; the
//! composition on this side of the boundary is what the tests exercise.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "oneagentgraph", &args);

    match args.first().map(String::as_str) {
        Some("run") => run(&args, &dir),
        Some("reset-timer") if dir.join("reset-timer.fail").exists() => {
            eprintln!("no resettable schedule named that member");
            ExitCode::from(2)
        }
        Some("reset-timer") => ExitCode::SUCCESS,
        Some("health") => {
            println!("fake-provider: 1 identity bound, 0% utilized");
            ExitCode::SUCCESS
        }
        Some(other) => fake::refuse(&format!("unknown oneagentgraph command '{other}'")),
        None => fake::refuse("oneagentgraph takes a command"),
    }
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

    // The dag-scope graph is the driver: its orchestrator member is what runs
    // the engine verbs. Acting that out is how a test exercises `start` end to
    // end rather than only the launch.
    if graph.contains("dag-scope") {
        return drive(dir);
    }

    let node = fake::label(args, "node").unwrap_or_else(|| "unknown".into());
    let step = fake::label(args, "step");
    let persona = fake::label(args, "persona");
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
        let recover_after: usize = fake::node_script(dir, &key, "recover-after")
            .and_then(|text| text.parse().ok())
            .unwrap_or(usize::MAX);
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
        eprintln!("the node failed its gate");
        return ExitCode::from(code.parse::<u8>().unwrap_or(1));
    }
    ExitCode::SUCCESS
}

/// Emit one envelope, as the sibling would.
fn emit(args: &[String], node: &str, step: Option<&str>, task: &str) {
    let mut labels = serde_json::Map::new();
    for (key, value) in [
        ("run_id", fake::label(args, "run_id")),
        ("round", fake::label(args, "round")),
        ("node", Some(node.to_string())),
        ("step", step.map(str::to_string)),
        ("persona", fake::label(args, "persona")),
    ] {
        if let Some(value) = value {
            // `round` is a number on the wire; everything else is a string.
            let value = if key == "round" {
                value
                    .parse::<u64>()
                    .map(serde_json::Value::from)
                    .unwrap_or(value.into())
            } else {
                value.into()
            };
            labels.insert(key.to_string(), value);
        }
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
    let run = std::env::var("ONEPIPELINE_RUN_ID").unwrap_or_default();
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
        let ran = std::process::Command::new(&binary)
            .args(["round", "run", &run])
            .status();
        let Ok(ran) = ran else {
            return ExitCode::from(1);
        };
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
