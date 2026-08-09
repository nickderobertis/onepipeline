//! A real `oneharness` executable, for the journeys that drive the **real**
//! `oneagentgraph`.
//!
//! This double stands one layer further out than the other two. Where
//! `fake-oneagentgraph` replaces a whole sibling, this one replaces only the
//! thing a gate genuinely cannot run: the paid model turn. Real `oneagentgraph`
//! resolves the graph, prepares the member, spawns this process at its own
//! documented `ONEAGENTGRAPH_ONEHARNESS_BIN` seam, reads the two NDJSON lines
//! below off its stdout, and settles the member on them.
//!
//! It speaks the surface `oneagentgraph` invokes — `run --config C --cwd D
//! --events --stream --prompt P` — and answers as onejudge's `docs/streaming.md`
//! describes: `{"type":"event",…}` lines, then one terminal
//! `{"type":"result","report":{…}}`.
//!
//! What it *does* with the turn is what a real agent would do with the same
//! prompt: an orchestrator member is told to drive a run with the engine verbs,
//! so this one runs them.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "oneharness", &args);

    match args.first().map(String::as_str) {
        Some("run") => run(&args, &dir),
        Some(other) => fake::refuse(&format!("unknown oneharness command '{other}'")),
        None => fake::refuse("oneharness takes a command"),
    }
}

/// `oneharness run --config C --cwd D --events --stream --prompt P`
fn run(args: &[String], dir: &std::path::Path) -> ExitCode {
    // Every one of these is something `oneagentgraph` sends today. A double that
    // accepted an invocation missing one would let a member be prepared wrongly
    // and still settle, which is the failure this whole seam exists to catch.
    let Some(config) = fake::flag(args, "--config") else {
        return fake::refuse("oneharness run requires --config");
    };
    let Some(cwd) = fake::flag(args, "--cwd") else {
        return fake::refuse("oneharness run requires --cwd");
    };
    let Some(prompt) = fake::flag(args, "--prompt") else {
        return fake::refuse("oneharness run requires --prompt");
    };
    if !std::path::Path::new(&config).is_file() {
        return fake::refuse(&format!(
            "oneharness run was given --config {config}, which is not a file"
        ));
    }

    // The turn itself. The orchestrator member's prompt names the engine verbs
    // it is to drive the run with, so this turn drives them — the same work the
    // `oneagentgraph` double does when it is standing in for the whole sibling.
    let drove = prompt.contains("onepipeline round run");
    if drove && fake::drive(dir) != ExitCode::SUCCESS {
        report(dir, &prompt, &cwd, false);
        return ExitCode::from(1);
    }
    report(dir, &prompt, &cwd, true);
    ExitCode::SUCCESS
}

/// The two lines a member is settled on: what the turn did, then its report.
fn report(dir: &std::path::Path, prompt: &str, cwd: &str, completed: bool) {
    println!(
        "{}",
        serde_json::json!({
            "type": "event",
            "turn": 1,
            "event": {"kind": "tool_call", "name": "bash", "input": {"command": "echo the turn ran"}},
        })
    );
    // Recorded as well as streamed: `oneagentgraph` renders the turn into its
    // own envelopes, so the prompt and the directory the turn was given are only
    // assertable from this side of the boundary.
    fake::append(
        &dir.join("turns.jsonl"),
        &serde_json::json!({"prompt": prompt, "cwd": cwd, "completed": completed}).to_string(),
    );
    println!(
        "{}",
        serde_json::json!({
            "type": "result",
            "report": {
                "completion_reason": if completed { "done_when_met" } else { "turn_failed" },
                "identity": "fake-harness",
                "usage": {"input_tokens": 1, "output_tokens": 1},
                "verdicts": [],
            },
        })
    );
}
