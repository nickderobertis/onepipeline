//! A real `oneagentgraph` executable, scripted from a directory.
//!
//! It speaks the sibling's command surface — `run`, `reset-timer`, `health` —
//! emits envelope NDJSON on stdout, and records every invocation so a test can
//! assert on what `onepipeline` actually asked for.
//!
//! It stands in for the real `oneagentgraph` so a journey can *state* a scenario
//! — a node that fails its gate, a dispatch held open, a driver that dies —
//! rather than arrange one out of real agent turns. That makes it an oracle, and
//! an oracle is only worth what it refuses: where the real CLI says no, this one
//! says no the same way. `tests/e2e/dispatch.rs` drives the real binary.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

/// The exit code the real CLI answers an invalid configuration with — its own
/// constant, so a double cannot answer a refusal with a code the sibling stopped
/// using.
fn invalid_config() -> ExitCode {
    ExitCode::from(u8::try_from(oneagentgraph::error::EXIT_INVALID_CONFIG).unwrap_or(1))
}

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
    // Every `--label` goes through the sibling's *own* parser, so this double
    // refuses exactly what the real CLI refuses — a key it stamps itself, one
    // that is not `k=v`, one that is not an identifier — and cannot drift from
    // it. A double that accepted a label the sibling reserves was the weak
    // oracle that let every dispatch be refused while this suite stayed green.
    for label in fake::flags(args, "--label") {
        if let Err(refusal) = oneagentgraph::run::parse_label(&label) {
            eprintln!("oneagentgraph: {refusal}");
            return invalid_config();
        }
    }

    // A refusal a launcher cannot catch by glancing: the real CLI validates
    // before it announces itself, and how long that takes is the host's
    // business — a config fetched over the network, a loaded machine. Scripted
    // in milliseconds so a journey can put one past any window a launcher might
    // have waited instead of waiting for an answer.
    if let Some(delay) = fake::node_script(dir, "run", "refuse-after") {
        let Ok(millis) = delay.trim().parse::<u64>() else {
            fake::fail(&format!(
                "run.refuse-after holds {delay:?}, which is not a number of milliseconds"
            ));
        };
        std::thread::sleep(std::time::Duration::from_millis(millis));
        eprintln!("oneagentgraph: invalid config: the graph names a member that does not exist");
        return invalid_config();
    }

    // A graph that neither announces itself nor exits — the third answer, and
    // the only one a launcher cannot wait out. The rendezvous is bounded by the
    // double's own timeout, so a launcher that failed to end this process leaves
    // a test failing on that rather than a stray one behind.
    if dir.join("run.hang").exists() {
        fake::wait_for(&dir.join("run.go"));
    }

    // A graph that finishes before it announces anything: it did what it was
    // given, so the launch succeeded even though nothing is driving anything
    // afterwards.
    if dir.join("run.exit-quietly").exists() {
        return ExitCode::SUCCESS;
    }

    // The dag-scope graph is the driver: its orchestrator member is what runs
    // the engine verbs. Acting that out is how a test exercises `start` end to
    // end rather than only the launch.
    if graph.contains("dag-scope") {
        // Announced before any work, as the real CLI announces a run. This is
        // the line the launcher's startup handshake waits for; a double that
        // stayed silent until it settled would make every launch wait out the
        // whole run. A node's dispatch is not waited on that way, and scripting
        // one to produce *nothing* is a scenario this suite needs, so the
        // announcement belongs to the launched graph rather than to every run.
        announce(args, &graph);
        return fake::drive(dir);
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

/// Announce the run, as the sibling's first line does.
fn announce(args: &[String], graph: &str) {
    println!(
        "{}",
        serde_json::json!({
            "v": 1,
            "ts": fake::now(),
            "stream": stream(),
            "seq": 0,
            "source": "agentgraph",
            "kind": "graph-started",
            "labels": stamped(args),
            "payload": {"graph": graph},
        })
    );
}

/// The labels the sibling stamps: its own run and member, and every `--label`
/// the caller passed, carried through verbatim beside them. The two `run_id`s on
/// one line are the point — a graph run is not the pipeline run that dispatched
/// it.
fn stamped(args: &[String]) -> serde_json::Map<String, serde_json::Value> {
    let mut labels = serde_json::Map::new();
    labels.insert("run_id".to_string(), graph_run().into());
    for pair in fake::flags(args, "--label") {
        if let Some((key, value)) = pair.split_once('=') {
            labels.insert(key.to_string(), value.into());
        }
    }
    labels
}

/// This graph run's own id, which is not the id of the run that started it.
fn graph_run() -> String {
    format!("fake-graph-{}", std::process::id())
}

/// The stream every envelope of this run carries.
fn stream() -> String {
    format!("fake-oneagentgraph-{}", std::process::id())
}

/// Emit the turn's envelopes, as the sibling would: the tool summary from
/// inside the turn, what the turn consumed, and the settlement that stores the
/// member's full report.
///
/// Three kinds the real CLI really emits. A double that answered a dispatch
/// with a kind of its own would be an oracle for a stream nothing produces —
/// the same weakness that let every dispatch be refused while this suite stayed
/// green.
fn emit(args: &[String], node: &str, step: Option<&str>, task: &str) {
    let mut labels = stamped(args);
    labels.insert("member".to_string(), "worker".into());
    // The node and the step are what the run being acted out is, so they are
    // stated rather than echoed: a node with no `--label` of its own still
    // belongs to one.
    labels.insert("onepipeline.node".to_string(), node.into());
    if let Some(step) = step {
        labels.insert("onepipeline.step".to_string(), step.into());
    }
    let envelope = |seq: u64, kind: &str, payload: serde_json::Value| {
        println!(
            "{}",
            serde_json::json!({
                "v": 1,
                "ts": fake::now(),
                "stream": stream(),
                "seq": seq,
                "source": "agentgraph",
                "kind": kind,
                "labels": labels,
                "payload": payload,
            })
        );
    };

    envelope(
        1,
        "turn-activity",
        serde_json::json!({
            "kind": "tool_call",
            "name": "bash",
            "detail": "echo the turn ran",
            "message": "the dispatch ran",
            // Echoed so a test can assert the task prose the node was given,
            // including the rendered planner-context section.
            "task": task,
            "dir": fake::flag(args, "--dir"),
            // A pr-author dispatch answers with the title it drafted.
            "title": drafted_title(task),
        }),
    );
    let report = report_of(task);
    envelope(
        2,
        "turn-completed",
        serde_json::json!({"usage": report["usage"]}),
    );
    // The report is *stored*, and the settlement says where — the sibling's own
    // contract, and the only reason a turn's tools and words survive the
    // process that produced them. Under a directory of this member's own,
    // named exactly as the real library names it: a consumer only reads a
    // `report_path` that names the file `oneagentgraph` itself writes.
    let path = fake::script_dir()
        .join("reports")
        .join(format!("{}-{}", fake::segment(node), std::process::id()))
        .join(oneagentgraph::member::REPORT_FILE);
    // Where the settlement says the report went. A member that settled on a
    // machine whose scratch this reader cannot reach still *names* it, so
    // `report.missing` publishes the path and writes nothing: the evidence is
    // missing, not the settlement. `report.elsewhere` names a file the real
    // library never writes, which a consumer must refuse rather than open.
    let scripted = |name: &str| fake::script_dir().join(name).exists();
    let named = if scripted("report.elsewhere") {
        Some(path.with_file_name("notes.json"))
    } else if scripted("report.missing") {
        Some(path.clone())
    } else {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // A report document a harness produced without a transcript: the
        // verdict fields the settlement reads inline, and nothing else.
        let document = if scripted("report.bare") {
            serde_json::json!({"completion_reason": "done_when_met", "verdicts": []})
        } else {
            report.clone()
        };
        std::fs::write(&path, document.to_string())
            .is_ok()
            .then(|| path.clone())
    };
    envelope(
        3,
        "member-settled",
        serde_json::json!({
            "completed": true,
            "verdict": [],
            "completion_reason": "done_when_met",
            "report_path": named.map(|path| path.display().to_string()),
        }),
    );
}

/// The onejudge report a settled member stores.
///
/// Two-party, because the shipped node-scope graph's `worker` is a two-party
/// member: its agent side does the work and its judge side supervises it, and
/// the split between what each spent is what the report carries and nothing on
/// the wire does.
fn report_of(task: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 7,
        "transcript": {"messages": [
            {"role": "user", "content": task},
            {"role": "assistant", "content": "Ran what the task asked for.", "events": [
                {"kind": "tool_call", "name": "bash",
                 "input": {"command": "echo the turn ran"}, "index": 0},
            ]},
        ]},
        "verdicts": [],
        "completion_reason": "done_when_met",
        "usage": {
            "input_tokens": 1_200, "output_tokens": 340,
            "cache_read_tokens": 900, "cache_write_tokens": 120, "cost_usd": 0.42,
        },
        "telemetry": {
            "wall_ms": 1_000,
            "agent": {"model_ms": 800, "usage": {
                "input_tokens": 1_000, "output_tokens": 300,
                "cache_read_tokens": 900, "cache_write_tokens": 120, "cost_usd": 0.40,
            }},
            "judge": {"model_ms": 100, "usage": {
                "input_tokens": 200, "output_tokens": 40, "cost_usd": 0.02,
            }},
            "orchestration_ms": 100,
            "sessions": [],
        },
    })
}

/// The title a `pr-author` dispatch drafts, when it is one.
fn drafted_title(task: &str) -> Option<String> {
    task.starts_with("Read this branch's diff")
        .then(|| "feat: drafted from the diff".to_string())
}
