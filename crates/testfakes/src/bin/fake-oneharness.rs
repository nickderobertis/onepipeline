//! A real `oneharness` executable, at `ONEAGENTGRAPH_ONEHARNESS_BIN`.
//!
//! **A single-sided member's turn no longer comes through here.** From
//! `oneagentgraph 0.2.18` that turn is an `oneharness_core` library call inside
//! the sibling's own process, and the only process left below it is the harness
//! the member's identity chain selected — which is `fake-claude`, at
//! oneharness's own `ONEHARNESS_BIN_CLAUDE_CODE`. What still names *this* one is
//! the sibling's remaining process boundary: the provider block it composes for
//! a `kind: onejudge` member, and the `oneharness interrupt` an in-flight
//! redirection is delivered by.
//!
//! So what this double is for now is being the executable that variable names,
//! and refusing what it does not speak. The `run` surface below is the one
//! `oneagentgraph` used to invoke — `run --config C --cwd D --events --stream
//! --prompt P` — answering as onejudge's `docs/streaming.md` describes:
//! `{"type":"event",…}` lines, then one terminal
//! `{"type":"result","report":{…}}`. A two-party member's agent side is
//! `oneharness run … --prompt-file -`, which this does **not** speak and
//! deliberately refuses: no offline stand-in for that conversation exists, and
//! every two-party journey here reads the launch rather than a settlement.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

/// What a scripted `harness.work` turn writes into the worktree it was given.
pub const WORK_FILE: &str = "work.md";

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
    let config_text = match std::fs::read_to_string(&config) {
        Ok(text) => text,
        Err(error) => return fake::refuse(&format!("cannot read --config {config}: {error}")),
    };
    fake::record(dir, "oneharness-config", &[prompt.clone(), config_text]);
    if !std::path::Path::new(&cwd).is_dir() {
        return fake::refuse(&format!(
            "oneharness run was given --cwd {cwd}, which is not a directory"
        ));
    }

    // A worker turn that leaves something behind in the worktree it was given.
    // Only when scripted, and only for the worker: a publication needs a diff —
    // `onevcs publish` on a clean tree publishes nothing and says so — and a
    // journey that means to reach a real change request has to have a turn that
    // made one. Every other journey here is about the dispatch, and a file
    // appearing in a worktree unasked would be a change nobody made.
    let observing = prompt.contains("Observe this run");
    if !observing {
        if let Some(body) = fake::node_script(dir, "harness", "work") {
            let path = std::path::Path::new(&cwd).join(WORK_FILE);
            if let Err(error) = std::fs::write(&path, format!("{body}\n")) {
                return fake::refuse(&format!("cannot write {}: {error}", path.display()));
            }
            // Recorded where every other thing a double did is recorded: a
            // publication that turns out to have had nothing to publish is
            // asked, first, whether the turn before it wrote anything.
            fake::record(dir, "oneharness-work", &[path.display().to_string()]);
        }
    }

    // The turn itself. The monitor member's prompt says it is watching the run,
    // so this turn watches it — the same work the `oneagentgraph` double does
    // when it is standing in for the whole sibling. It changes nothing: no
    // engine verb exists for it to run.
    //
    // `harness.fail` is the other way a turn ends: it did the work and did not
    // get there. Scripted rather than inferred, because a turn that fails
    // *after* it has started and streamed is its own case — a member settles on
    // the pair of a non-zero exit and a `turn_failed` report, and that pair is
    // what a caller reading the graph's settlement sees. Without it the only
    // failing member this suite could produce was one that refused on the way
    // in, which never reaches a settlement at all.
    let outcome = if fake::node_script(dir, "harness", "fail").is_some() {
        Outcome::TurnFailed
    } else if observing && fake::observe(dir) != ExitCode::SUCCESS {
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
    if !observing && dir.join("turn.hold").exists() {
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
///
/// A report document, which is what `oneagentgraph` settles a member on and
/// then stores whole — so it carries the transcript a real one carries, and not
/// only the verdict fields the settlement reads inline. A single-sided member
/// has one side, so there is no two-party split here and none is invented.
fn report(outcome: Outcome) {
    println!(
        "{}",
        serde_json::json!({
            "type": "result",
            "report": {
                "schema_version": 7,
                "transcript": {"messages": [
                    {"role": "assistant", "content": "Ran what the task asked for.", "events": [
                        {"kind": "tool_call", "name": "bash",
                         "input": {"command": "echo the turn ran"}, "index": 0},
                    ]},
                ]},
                "completion_reason": outcome.reason(),
                "identity": "fake-harness",
                "usage": {"input_tokens": 1, "output_tokens": 1},
                "verdicts": [],
            },
        })
    );
}
