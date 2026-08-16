//! A real harness CLI, for the journeys that drive the **real**
//! `oneagentgraph`.
//!
//! This double stands one layer further out than the other two. Where
//! `fake-oneagentgraph` replaces a whole sibling, this one replaces only the
//! thing a gate genuinely cannot run: the paid model turn. Real `oneagentgraph`
//! resolves the graph and prepares the member, real `oneharness` layers the
//! member's config, selects the identity, spawns this process, and normalizes
//! what it writes; nothing between them is stubbed.
//!
//! # Why it is the *harness* rather than `oneharness`
//!
//! It used to stand in for the `oneharness` binary, at `oneagentgraph`'s own
//! `ONEAGENTGRAPH_ONEHARNESS_BIN`. From `oneagentgraph` 0.2.16 a `kind:
//! oneharness` member's turn is `oneharness_core::io::run::run_supervised`
//! called **in this process** — there is no `oneharness` subprocess left for
//! that member, so that variable reaches only the two-party members onejudge
//! still spawns one for. The seam underneath it is oneharness's own
//! `ONEHARNESS_BIN_<ID>`, which names the provider CLI, and this is what that
//! names.
//!
//! So it speaks **Claude Code's headless wire shape**, because that is what
//! `oneharness` asks a `claude-code` candidate for:
//!
//! ```jsonl
//! {"type":"system","subtype":"init","session_id":"…"}
//! {"type":"assistant","message":{"content":[{"type":"tool_use",…}]}}
//! {"type":"result","subtype":"success","result":"…","usage":{…}}
//! ```
//!
//! …unless the run asked for another one. `oneharness` picks the format per run
//! and parses this process's stdout under it, so the format is read off the argv
//! rather than assumed — see [`Shape`], which is what a structured-output run
//! (`--output-format json`, one document carrying `structured_output`) needs.
//!
//! What it *does* with the turn is what a real agent would do with the same
//! prompt: a monitor member is told to observe a run, so this one observes it.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

/// What a scripted `harness.work` turn writes into the worktree it was given.
pub const WORK_FILE: &str = "work.md";

/// What a turn says it did, which reaches a reader as the report's own text.
pub const ANSWER: &str = "Ran what the task asked for.";

/// The command the first tool call of every turn runs.
pub const FIRST_TOOL: &str = "echo the turn ran";

/// The command the second one runs, after a held turn is released.
pub const SECOND_TOOL: &str = "cargo llvm-cov --workspace";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "harness", &args);
    run(&args, &dir)
}

/// One headless turn: `claude -p PROMPT … --output-format FORMAT`.
fn run(args: &[String], dir: &std::path::Path) -> ExitCode {
    // Print mode and a prompt are what a headless launch *is*. A double that
    // answered an invocation missing either would let a member be prepared
    // wrongly and still settle, which is the failure this whole seam exists to
    // catch.
    let Some(prompt) = prompt(args) else {
        return fake::refuse("a headless harness run requires -p with a prompt");
    };
    let Some(shape) = Shape::asked_for(args) else {
        return fake::refuse(&format!(
            "--output-format must be json or stream-json, got {:?}",
            fake::flag(args, "--output-format")
        ));
    };
    // The directory the turn works in is the one this process was started in:
    // `oneharness` runs a candidate in the run's own working directory, which
    // for a member is the worktree `oneagentgraph` resolved for it. A double
    // that wrote anywhere else would leave a dispatch's work outside the branch
    // that publishes it.
    let worktree = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(error) => return fake::refuse(&format!("a turn has no working directory: {error}")),
    };

    // The monitor member's prompt says it is watching the run, so this turn
    // watches it — the same work the `oneagentgraph` double does when it stands
    // in for the whole sibling. It changes nothing.
    let observing = prompt.contains("Observe this run");
    if !observing {
        // A worker turn that leaves something behind in the worktree it was
        // given. Only when scripted, and only for the worker: a publication
        // needs a diff — `onevcs publish` on a clean tree publishes nothing and
        // says so — and a journey that means to reach a real change request has
        // to have a turn that made one.
        if let Some(body) = fake::node_script(dir, "harness", "work") {
            let path = worktree.join(WORK_FILE);
            if let Err(error) = std::fs::write(&path, format!("{body}\n")) {
                return fake::refuse(&format!("cannot write {}: {error}", path.display()));
            }
            // Recorded where every other thing a double did is recorded: a
            // publication that turns out to have had nothing to publish is
            // asked, first, whether the turn before it wrote anything.
            fake::record(dir, "harness-work", &[path.display().to_string()]);
        }
    }

    // `harness.fail` is a turn that did the work and did not get there: a
    // non-zero exit having published its stream, which is the pair `oneharness`
    // records as `nonzero` and `oneagentgraph` settles a member's death on.
    // Without it the only failing member this suite could produce was one that
    // refused on the way in, which never reaches a turn at all.
    let failing = fake::node_script(dir, "harness", "fail").is_some()
        || (observing && fake::observe(dir) != ExitCode::SUCCESS);

    if shape == Shape::Stream {
        emit(&serde_json::json!({
            "type": "system", "subtype": "init", "session_id": session(),
        }));
        tool_call(FIRST_TOOL);
    }
    // A worker turn that reports again after a hold: what a live readout of a
    // running dispatch is read against. The first event proves the stream
    // arrives; a second, released while the node is still in flight, proves the
    // readout *advances*. The observing turn is left alone — it is the member
    // watching, not the one being watched.
    if !observing && dir.join("turn.hold").exists() {
        fake::wait_for(&dir.join("turn.go"));
        if shape == Shape::Stream {
            tool_call(SECOND_TOOL);
        }
        // Held again, so the node is *still in flight* when the second reading
        // is taken: a readout that only advances as the dispatch ends proves
        // nothing about supervising a live one.
        fake::wait_for(&dir.join("turn.settle"));
    }

    if failing {
        // A turn that failed says so on both channels a harness has: the
        // terminal document, and the exit code `oneharness` classifies it by.
        emit(&result(&prompt, args, true));
        eprintln!("the turn did not get there");
        return ExitCode::from(1);
    }
    emit(&result(&prompt, args, false));
    ExitCode::SUCCESS
}

/// The prompt this turn was given.
///
/// The positional after `-p` on an ordinary launch. A prompt too large for the
/// argv rides stdin instead (`-p --input-format text`, no positional), which is
/// `oneharness`'s own delivery decision — so it is read rather than refused.
fn prompt(args: &[String]) -> Option<String> {
    if fake::flag(args, "--input-format").as_deref() == Some("text") {
        use std::io::Read;
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).ok()?;
        return Some(text);
    }
    let at = args.iter().position(|arg| arg == "-p")?;
    args.get(at + 1)
        .filter(|next| !next.starts_with('-'))
        .cloned()
}

/// The output shape `oneharness` asked this turn for, on its own
/// `--output-format`.
///
/// Not a detail a journey may ignore: `oneharness` selects the format per run
/// and then *parses this process's stdout under it*. A structured-output run
/// asks for `json` — one document, because that is where the answer it
/// validates lives — and a double that answered the streamed transcript anyway
/// produces stdout no answer can be extracted from, which reads as a harness
/// that ran and said nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// `--output-format stream-json`: the content-block stream, which is where
    /// a turn's tools are.
    Stream,
    /// `--output-format json`: one document, which is where a validated answer
    /// is.
    Single,
}

impl Shape {
    /// The shape this launch asked for, or `None` for a format this double
    /// cannot answer in — which is refused rather than answered anyway, because
    /// stdout written under one format and parsed under another is a journey
    /// failing far from the flag that caused it.
    fn asked_for(args: &[String]) -> Option<Self> {
        match fake::flag(args, "--output-format").as_deref() {
            Some("stream-json") => Some(Self::Stream),
            Some("json") => Some(Self::Single),
            _ => None,
        }
    }
}

/// One streamed tool call, as a Claude Code content block.
fn tool_call(command: &str) {
    emit(&serde_json::json!({
        "type": "assistant",
        "session_id": session(),
        "message": {"id": "m1", "type": "message", "role": "assistant", "content": [
            {"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": command}},
        ]},
    }));
}

/// The terminal document a headless run ends with.
///
/// `structured_output` is the field a native structured-output run answers in —
/// `--json-schema` is on the argv, and `oneharness` prefers that field over
/// anything it could extract from the text. What goes in it is the answer this
/// double is asked for: the change request body a `pr-author` dispatch drafts.
fn result(prompt: &str, args: &[String], failed: bool) -> serde_json::Value {
    let mut document = serde_json::json!({
        "type": "result",
        "subtype": if failed { "error_during_execution" } else { "success" },
        "is_error": failed,
        "duration_ms": 5,
        "num_turns": 1,
        "result": ANSWER,
        "session_id": session(),
        "usage": {"input_tokens": 10, "output_tokens": 5,
                  "cache_read_input_tokens": 4, "cache_creation_input_tokens": 1},
        "total_cost_usd": 0.002,
    });
    if fake::flag(args, "--json-schema").is_some() && !failed {
        document["structured_output"] = structured(prompt);
    }
    document
}

/// The validated answer a structured-output run is asked for.
///
/// One shape, because one dispatch in this stack asks for one: the change
/// request body, which the schema its graph names requires. Scripted with
/// `harness.body` where a journey wants to read its own words back out of the
/// change request.
fn structured(prompt: &str) -> serde_json::Value {
    let body = fake::node_script(&fake::script_dir(), "harness", "body").unwrap_or_else(|| {
        format!(
            "## What\nWhat the diff did.\n\n## Why\n{}",
            prompt.lines().next().unwrap_or_default()
        )
    });
    serde_json::json!({"body": body})
}

/// The session id this turn reports, which `oneharness` threads into `--resume`.
fn session() -> String {
    format!("fake-session-{}", std::process::id())
}

/// One document on stdout, which is the whole of what a harness CLI answers on.
fn emit(document: &serde_json::Value) {
    println!("{document}");
}
