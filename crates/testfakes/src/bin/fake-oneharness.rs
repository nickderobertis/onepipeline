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
    for flag in ["--events", "--stream"] {
        if !args.iter().any(|arg| arg == flag) {
            return fake::refuse(&format!("oneharness run requires {flag}"));
        }
    }
    // Both paths are this process's external input, and both are things
    // `oneagentgraph` composed rather than passed through: a config it wrote and
    // a worktree it resolved. A double that ran anyway would settle a member
    // whose launch was prepared against neither.
    if !std::path::Path::new(&config).is_file() {
        return fake::refuse(&format!(
            "oneharness run was given --config {config}, which is not a file"
        ));
    }
    if !std::path::Path::new(&cwd).is_dir() {
        return fake::refuse(&format!(
            "oneharness run was given --cwd {cwd}, which is not a directory"
        ));
    }

    // The turn itself. The orchestrator member's prompt names the engine verbs
    // it is to drive the run with, so this turn drives them — the same work the
    // `oneagentgraph` double does when it is standing in for the whole sibling.
    let driving = prompt.contains("onepipeline round run");
    let outcome = if driving && fake::drive(dir) != ExitCode::SUCCESS {
        Outcome::TurnFailed
    } else {
        Outcome::DoneWhenMet
    };
    tool_event(1, "echo the turn ran");
    // A worker turn that reports again after a hold: what a live readout of a
    // running dispatch is read against. The first event proves the stream
    // arrives; a second, released while the node is still in flight, proves the
    // readout *advances*. The driver's own turn is left alone — it is the
    // member running the engine verbs, not the one being watched.
    if !driving && dir.join("turn.hold").exists() {
        fake::wait_for(&dir.join("turn.go"));
        tool_event(2, "cargo llvm-cov --workspace");
        // Held again, so the node is *still in flight* when the second reading
        // is taken: a readout that only advances as the dispatch ends proves
        // nothing about supervising a live one.
        fake::wait_for(&dir.join("turn.settle"));
    }
    report(outcome);
    match outcome {
        Outcome::DoneWhenMet => ExitCode::SUCCESS,
        Outcome::TurnFailed => ExitCode::from(1),
    }
}

/// One streamed tool event, in the shape onejudge's `docs/streaming.md` fixes.
fn tool_event(index: u64, command: &str) {
    println!(
        "{}",
        serde_json::json!({
            "type": "event",
            "turn": index,
            "event": {"kind": "tool_call", "name": "bash", "input": {"command": command}},
        })
    );
}

/// How the turn ended, in the vocabulary a report names it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The turn reached what it was asked for.
    DoneWhenMet,
    /// It did not, and says so with a non-zero exit as well as with the
    /// reason below: `oneagentgraph` settles a member on the pair.
    TurnFailed,
}

impl Outcome {
    /// The `completion_reason` a report carries.
    fn reason(self) -> &'static str {
        match self {
            Self::DoneWhenMet => "done_when_met",
            Self::TurnFailed => "turn_failed",
        }
    }
}

/// The terminal line a member is settled on: the turn's report.
fn report(outcome: Outcome) {
    println!(
        "{}",
        serde_json::json!({
            "type": "result",
            "report": {
                "completion_reason": outcome.reason(),
                "identity": "fake-harness",
                "usage": {"input_tokens": 1, "output_tokens": 1},
                "verdicts": [],
            },
        })
    );
}
