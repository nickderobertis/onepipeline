//! A real `claude` executable, for the journeys that drive the **real**
//! `oneagentgraph`.
//!
//! This double stands one layer further out than the other two, and it stands
//! at the innermost boundary of the whole stack: the paid model turn. Real
//! `oneagentgraph` resolves the graph, prepares the member and runs its turn
//! through `oneharness`'s **library**, and oneharness spawns the harness the
//! member's identity chain selected. That harness is this process, named at
//! oneharness's own `ONEHARNESS_BIN_CLAUDE_CODE` seam.
//!
//! It speaks Claude Code's headless surface — `claude -p [PROMPT]
//! [--input-format text] --permission-mode M --output-format json|stream-json
//! [--verbose]` — and answers in the shape oneharness normalizes: for a
//! streaming run, Anthropic content-block lines and a terminal
//! `{"type":"result",…}`; for a buffered one, that result document alone.
//!
//! What it *does* with the turn is what a real agent would do with the same
//! prompt: an orchestrator member is told to drive a run with the engine verbs,
//! so this one runs them.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

pub const WORK_FILE: &str = "work.md";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Before the script directory is required: oneharness probes a resolved
    // binary with `--version` to decide whether the identity is installed, and
    // that probe is not a turn — a double that refused it would report every
    // member's only candidate as `not-installed`.
    if args.iter().any(|arg| arg == "--version") {
        println!("1.0.0 (fake-claude)");
        return ExitCode::SUCCESS;
    }
    let dir = fake::script_dir();
    fake::record(&dir, "claude", &args);

    if !args.iter().any(|arg| arg == "-p") {
        return fake::refuse("claude is only driven headless here, which is `-p`");
    }
    turn(&args, &dir)
}

/// The `--permission-mode` values oneharness maps its own modes onto.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] this is a copy of a
// *provider's* CLI, which no crate in this dependency graph declares as data —
// oneharness's mapping onto it is a private function in `domain::harness`, and the
// values themselves are Claude Code's. The reconciling gate is not a shared
// constant but `tests/e2e/dispatch.rs`: it drives the real `oneagentgraph` and the
// real `oneharness_core` against this binary, so a mode oneharness starts sending
// that is not below is a refusal there rather than a double that reads differently.
const MODES: [&str; 5] = [
    "plan",
    "dontAsk",
    "acceptEdits",
    "bypassPermissions",
    "auto",
];

/// The `--output-format` values oneharness names for this harness: the buffered
/// document its `output_format` selects, and the stream its `events_format` does.
///
/// `text` is not one of them and is refused: oneharness never asks Claude Code
/// for it, and a double that accepted it would have to answer in a format nothing
/// here produces.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the same gate as
// `MODES` above, and for the same reason.
const FORMATS: [&str; 2] = ["json", "stream-json"];

fn turn(args: &[String], dir: &std::path::Path) -> ExitCode {
    let Some(prompt) = prompt(args) else {
        return fake::refuse("claude -p was given no prompt, on the argv or on stdin");
    };
    // Both are decisions `oneharness` made rather than values it passed through:
    // the mode comes off the member's resolved config and the format off whether
    // its run streams. Checked against what the real CLI accepts, not merely for
    // presence — a value the real `claude` refuses has to be refused here too, or
    // a member prepared with one settles green against a double and dies against
    // a provider.
    let Some(mode) = fake::flag(args, "--permission-mode") else {
        return fake::refuse("claude -p requires --permission-mode");
    };
    if !MODES.contains(&mode.as_str()) {
        return fake::refuse(&format!("claude takes no --permission-mode {mode:?}"));
    }
    let Some(format) = fake::flag(args, "--output-format") else {
        return fake::refuse("claude -p requires --output-format");
    };
    if !FORMATS.contains(&format.as_str()) {
        return fake::refuse(&format!("claude takes no --output-format {format:?}"));
    }
    let streaming = format == "stream-json";
    // The working directory is `oneharness run --cwd` as it now reaches the
    // harness: the member's own worktree, entered by the process that spawned
    // this one. A turn that wrote its work anywhere else would leave a
    // publication with nothing to publish.
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => return fake::refuse(&format!("claude has no working directory: {error}")),
    };
    let member = std::env::var(fake::MEMBER_ENV).unwrap_or_default();
    fake::record(
        dir,
        "claude-turn",
        &[prompt.clone(), cwd.display().to_string(), member],
    );

    // A worker turn that leaves something behind in the worktree it was given.
    // Only when scripted, and only for the worker: a publication needs a diff —
    // `onevcs publish` on a clean tree publishes nothing and says so — and a
    // journey that means to reach a real change request has to have a turn that
    // made one. Every other journey here is about the dispatch, and a file
    // appearing in a worktree unasked would be a change nobody made.
    let observing = prompt.contains("Observe this run");
    if !observing {
        if let Some(body) = fake::node_script(dir, "harness", "work") {
            let path = cwd.join(WORK_FILE);
            if let Err(error) = std::fs::write(&path, format!("{body}\n")) {
                return fake::refuse(&format!("cannot write {}: {error}", path.display()));
            }
            // Recorded where every other thing a double did is recorded: a
            // publication that turns out to have had nothing to publish is
            // asked, first, whether the turn before it wrote anything.
            fake::record(dir, "claude-work", &[path.display().to_string()]);
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
    // the pair of a non-zero exit and an errored result document, and that pair
    // is what a caller reading the graph's settlement sees. Without it the only
    // failing member this suite could produce was one that refused on the way
    // in, which never reaches a settlement at all.
    let outcome = if fake::node_script(dir, "harness", "fail").is_some()
        || (observing && fake::observe(dir) != ExitCode::SUCCESS)
    {
        Outcome::TurnFailed
    } else {
        Outcome::Answered
    };
    if streaming {
        tool_call(1, "echo the turn ran");
    }
    // A worker turn that reports again after a hold: what a live readout of a
    // running dispatch is read against. The first event proves the stream
    // arrives; a second, released while the node is still in flight, proves the
    // readout *advances*. The driver's own turn is left alone — it is the
    // member running the engine verbs, not the one being watched.
    if !observing && dir.join("turn.hold").exists() {
        fake::wait_for(&dir.join("turn.go"));
        if streaming {
            tool_call(2, "cargo llvm-cov --workspace");
        }
        // Held again, so the node is *still in flight* when the second reading
        // is taken: a readout that only advances as the dispatch ends proves
        // nothing about supervising a live one.
        fake::wait_for(&dir.join("turn.settle"));
    }
    if streaming {
        assistant_text();
    }
    result(outcome);
    outcome.exit_code()
}

/// The prompt this turn was given.
///
/// Claude Code takes it as the positional after `-p`, and off **stdin** when
/// oneharness decides the prompt is too large for an argv — which a node's task
/// prose reaches easily, so both forms are live here rather than only the one
/// that is easier to read.
fn prompt(args: &[String]) -> Option<String> {
    let at = args.iter().position(|arg| arg == "-p")?;
    match args.get(at + 1) {
        Some(next) if !next.starts_with('-') => Some(next.clone()),
        _ => {
            use std::io::Read;
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text).ok()?;
            (!text.trim().is_empty()).then_some(text)
        }
    }
}

/// One streamed tool call, as an Anthropic content block.
///
/// The shape oneharness's own normalizer reads a `tool_call` out of — a
/// `tool_use` block on an assistant message — rather than any envelope of this
/// suite's own devising.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] a provider's wire
// shape, which no crate here declares as data. Its gate is
// `tests/e2e/dispatch.rs`'s `transcript_renders_a_real_dispatched_turns_tools_and_words`:
// the real `oneharness_core` normalizes these lines, so a shape it stops reading
// is a transcript missing its tools rather than a double nobody checks.
fn tool_call(index: u64, command: &str) {
    println!(
        "{}",
        serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use",
                "id": format!("toolu_{index}"),
                "name": "bash",
                "input": {"command": command},
            }]},
        })
    );
}

fn assistant_text() {
    println!(
        "{}",
        serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": ANSWER}]},
        })
    );
}

/// How the turn ended.
///
/// One value rather than a boolean beside four uses of it: the terminal
/// document's `subtype`, its `is_error`, the text it carries and this process's
/// exit status are four spellings of one fact, and held apart any of them could
/// say something the others do not — a turn reporting success on a non-zero exit
/// is exactly the pair a caller settles a member on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The turn reached what it was asked for.
    Answered,
    /// It did not, and says so with a non-zero exit as well as in the document.
    TurnFailed,
}

impl Outcome {
    /// The `subtype` the terminal document carries.
    fn subtype(self) -> &'static str {
        match self {
            Self::Answered => "success",
            Self::TurnFailed => "error_during_execution",
        }
    }

    /// The visible answer, which is also the last assistant message's text.
    fn text(self) -> &'static str {
        match self {
            Self::Answered => ANSWER,
            Self::TurnFailed => "The turn did the work and did not get there.",
        }
    }

    fn exit_code(self) -> ExitCode {
        match self {
            Self::Answered => ExitCode::SUCCESS,
            Self::TurnFailed => ExitCode::from(1),
        }
    }
}

/// What a turn that reached its task answers with, on the stream and in the
/// terminal document alike — a real one says the same thing twice, and a reader
/// of the retained report meets whichever of them oneharness kept.
const ANSWER: &str = "Ran what the task asked for.";

/// The terminal document a headless run ends on.
///
/// `session_id` is what a continuation resumes, and `usage` is what an accounting
/// reader adds up.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the same provider
// wire shape as `tool_call` above, gated the same way: `oneharness_core` reads
// `is_error`, `result`, `session_id` and `usage` out of this document, so a field
// it stops reading settles a member differently in `tests/e2e/dispatch.rs`.
fn result(outcome: Outcome) {
    println!(
        "{}",
        serde_json::json!({
            "type": "result",
            "subtype": outcome.subtype(),
            "is_error": outcome == Outcome::TurnFailed,
            "result": outcome.text(),
            "session_id": "fake-claude-session",
            "num_turns": 1,
            "total_cost_usd": 0,
            "usage": {"input_tokens": 1, "output_tokens": 1},
        })
    );
}
