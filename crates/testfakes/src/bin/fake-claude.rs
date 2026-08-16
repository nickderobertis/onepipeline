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

/// What a scripted `harness.work` turn writes into the worktree it was given.
pub const WORK_FILE: &str = "work.md";

/// The variable a member's own oneharness config stamps its name into.
///
/// The turn is no longer a process this crate's suite launched, so there is no
/// argv to read a member's identity off and oneharness hands the harness no
/// name of its own. What it does hand over is the member config's `[env]`
/// block, which `World::write_graphs_with` writes one of per member — so the
/// attribution rides the same seam the binary override does, and a journey can
/// still say *which* member was given which job.
const MEMBER_ENV: &str = "ONEPIPELINE_FAKE_MEMBER";

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

/// One headless turn.
fn turn(args: &[String], dir: &std::path::Path) -> ExitCode {
    let Some(prompt) = prompt(args) else {
        return fake::refuse("claude -p was given no prompt, on the argv or on stdin");
    };
    // Both are things `oneharness` decided rather than passed through: the mode
    // comes off the member's resolved config and the format off whether its run
    // streams. A double that ran without them would settle a member whose turn
    // was prepared against neither.
    for flag in ["--permission-mode", "--output-format"] {
        if fake::flag(args, flag).is_none() {
            return fake::refuse(&format!("claude -p requires {flag}"));
        }
    }
    let streaming = fake::flag(args, "--output-format").as_deref() == Some("stream-json");
    // The working directory is `oneharness run --cwd` as it now reaches the
    // harness: the member's own worktree, entered by the process that spawned
    // this one. A turn that wrote its work anywhere else would leave a
    // publication with nothing to publish.
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => return fake::refuse(&format!("claude has no working directory: {error}")),
    };
    let member = std::env::var(MEMBER_ENV).unwrap_or_default();
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
    let failed = fake::node_script(dir, "harness", "fail").is_some()
        || (observing && fake::observe(dir) != ExitCode::SUCCESS);
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
    result(failed);
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
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

/// The turn's visible answer, as the last assistant message.
fn assistant_text() {
    println!(
        "{}",
        serde_json::json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "text", "text": "Ran what the task asked for."},
            ]},
        })
    );
}

/// The terminal document a headless run ends on.
///
/// `is_error` and the non-zero exit are the pair a caller settles a failed turn
/// on; `session_id` is what a continuation resumes, and `usage` is what an
/// accounting reader adds up.
fn result(failed: bool) {
    println!(
        "{}",
        serde_json::json!({
            "type": "result",
            "subtype": if failed { "error_during_execution" } else { "success" },
            "is_error": failed,
            "result": if failed {
                "The turn did the work and did not get there."
            } else {
                "Ran what the task asked for."
            },
            "session_id": "fake-claude-session",
            "num_turns": 1,
            "total_cost_usd": 0,
            "usage": {"input_tokens": 1, "output_tokens": 1},
        })
    );
}
