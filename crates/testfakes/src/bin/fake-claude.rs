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
//! [--json-schema S] [--verbose]` — and answers in the shape oneharness
//! normalizes: for a streaming run, Anthropic content-block lines and a terminal
//! `{"type":"result",…}`; for a buffered one, that result document alone. A run
//! that named a schema gets its validated answer in that document's
//! `structured_output`, which is where the change request body a `pr-author`
//! dispatch drafts comes from.
//!
//! What it *does* with the turn is what a real agent would do with the same
//! prompt: an orchestrator member is told to drive a run with the engine verbs,
//! so this one runs them.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

pub const WORK_FILE: &str = "work.md";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(unix)]
    if args.first().map(String::as_str) == Some("outlive-the-graph") {
        let [_, pid_file] = args.as_slice() else {
            return fake::refuse("outlive-the-graph takes exactly one pid file");
        };
        // SAFETY: this fixture process owns its signal disposition; SIGKILL
        // remains available to the graph's real final reaper and test cleanup.
        if unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) } == libc::SIG_ERR {
            fake::fail(&format!(
                "cannot ignore SIGTERM: {}",
                std::io::Error::last_os_error()
            ));
        }
        std::fs::write(pid_file, format!("{}\n", std::process::id()))
            .unwrap_or_else(|error| fake::fail(&format!("cannot record outliving pid: {error}")));
        loop {
            std::thread::park();
        }
    }
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

    // Recorded first, so an argv this double refuses is still readable by the
    // journey that has to explain why the member died.
    if let Err(refusal) = declared(&args) {
        return fake::refuse(&refusal);
    }
    if !args.iter().any(|arg| arg == "-p") {
        return fake::refuse("claude is only driven headless here, which is `-p`");
    }
    turn(&args, &dir)
}

/// What follows a flag on the argv, as Claude Code's own grammar has it.
///
/// Three cases rather than "takes a value or does not", because `-p` is neither:
/// the prompt after it is optional, and oneharness sends it on stdin whenever a
/// node's task prose is too large for an argv. Named, the reader of [`declared`]
/// no longer has to recognise that one flag by its spelling to parse the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Takes {
    /// Nothing: the flag is the whole of it.
    Nothing,
    /// A value, which the real CLI refuses to be without.
    AValue,
    /// The prompt, when it came on the argv rather than on stdin.
    ThePromptIfItIsHere,
}

/// The flags the headless surface above is made of, and what follows each.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the same gate as
// `MODES` below, and for the same reason.
const FLAGS: [(&str, Takes); 6] = [
    ("-p", Takes::ThePromptIfItIsHere),
    ("--input-format", Takes::AValue),
    ("--permission-mode", Takes::AValue),
    ("--output-format", Takes::AValue),
    // What a structured-output run is: oneharness names the schema file the
    // member's own config declared, and prefers the answer validated against it
    // over anything it could extract from the turn's text.
    ("--json-schema", Takes::AValue),
    ("--verbose", Takes::Nothing),
];

/// Refuses an argv the real `claude` would not take.
///
/// The value checks in `turn` are about a flag oneharness chose the *wrong*
/// value for; this is about one it should not be sending at all. Both are the
/// same property, which is the only thing a double is worth: where the real CLI
/// says no, this one has to. An argument waved through here is one no journey
/// can catch — oneharness grows a flag Claude Code does not take, every member
/// settles green, and the first thing that ever says otherwise is a provider.
fn declared(args: &[String]) -> Result<(), String> {
    let mut at = 0;
    while at < args.len() {
        let arg = &args[at];
        let Some((_, takes)) = FLAGS.iter().find(|(name, _)| name == arg) else {
            return Err(format!("claude takes no argument {arg:?}"));
        };
        at += 1;
        match takes {
            Takes::Nothing => {}
            // Refused here rather than left to the read in `turn`: an option
            // with nothing after it is indistinguishable, to every `fake::flag`
            // below, from one that was never sent — so an optional one would be
            // waved through, and the required ones would end the process on a
            // misconfiguration exit that says the *test* was set up wrongly
            // rather than that oneharness sent this.
            Takes::AValue if at == args.len() => {
                return Err(format!(
                    "claude's {arg} takes a value, and nothing followed it"
                ))
            }
            Takes::AValue => at += 1,
            Takes::ThePromptIfItIsHere => {
                if args.get(at).is_some_and(|next| !next.starts_with('-')) {
                    at += 1;
                }
            }
        }
    }
    Ok(())
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

/// The `--input-format` values oneharness names: `text` alone, which is how it
/// says the prompt it is about to send — on the argv or on stdin — is prose. The
/// other value the real CLI takes, `stream-json`, arrives as a stream of
/// envelopes this double neither reads nor answers in.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the same gate as
// `MODES` above, and for the same reason.
const INPUT_FORMATS: [&str; 1] = ["text"];

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
    // Optional, unlike the two above: oneharness sends it only when it has
    // decided how the prompt travels, so its absence is a live case and only a
    // value it did send is checked.
    if let Some(input) = fake::flag(args, "--input-format") {
        if !INPUT_FORMATS.contains(&input.as_str()) {
            return fake::refuse(&format!("claude takes no --input-format {input:?}"));
        }
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
    // The scratch directory the engine promised this dispatch, as it reaches the
    // process that actually runs the turn: this one is the harness child, the
    // deepest thing in the stack, so what it holds is what an agent would hold
    // however the graph above it was started.
    //
    // Empty is a real answer and the only lenient one: this double also runs the
    // observer graph's turns, which are not node dispatches and are promised
    // nothing. A value that is *there* is held to the whole promise here rather
    // than in a journey's assertion, because a turn is where the promise is made
    // and a double that recorded an unusable path would have a journey assert on
    // it a page later.
    let scratch = std::env::var("ONEPIPELINE_NODE_SCRATCH_DIR").unwrap_or_default();
    if !scratch.is_empty() {
        let at = std::path::Path::new(&scratch);
        if !at.is_absolute() || !at.is_dir() {
            fake::fail(&format!(
                "this turn was given the scratch directory {scratch:?}, which is not an \
                 absolute path to a directory that exists"
            ));
        }
        if let Err(error) = std::fs::write(at.join("turn"), &prompt) {
            fake::fail(&format!(
                "this turn cannot write to the scratch directory {scratch:?} it was given: \
                 {error}"
            ));
        }
    }
    fake::record(
        dir,
        "claude-turn",
        &[prompt.clone(), cwd.display().to_string(), member, scratch],
    );

    // Stamp a process to the graph-run root, above this member's scratch. The
    // member's own teardown therefore leaves it alone and the graph's final
    // teardown is the first real reaper that owns it.
    #[cfg(unix)]
    if dir.join("harness.outlives-graph").exists() && !prompt.contains("Observe this run") {
        let scratch = std::env::var("ONEAGENTGRAPH_SCRATCH_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|error| fake::fail(&format!("the turn has no scratch: {error}")));
        let run_root = scratch
            .parent()
            .and_then(std::path::Path::parent)
            .unwrap_or_else(|| fake::fail("the member scratch has no graph-run root"));
        let pid_file = dir.join("harness.outlives-graph.pid");
        let exe = std::env::current_exe()
            .unwrap_or_else(|error| fake::fail(&format!("fake claude has no image: {error}")));
        std::process::Command::new(exe)
            .args(["outlive-the-graph", &pid_file.to_string_lossy()])
            .env("ONEAGENTGRAPH_SCRATCH_DIR", run_root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap_or_else(|error| fake::fail(&format!("cannot outlive the graph: {error}")));
        fake::wait_for(&pid_file);
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
            let path = cwd.join(WORK_FILE);
            if let Err(error) = std::fs::write(&path, format!("{body}\n")) {
                return fake::refuse(&format!("cannot write {}: {error}", path.display()));
            }
            // Recorded where every other thing a double did is recorded: a
            // publication that turns out to have had nothing to publish is
            // asked, first, whether the turn before it wrote anything.
            fake::record(dir, "claude-work", &[path.display().to_string()]);
        }
        // The turn that stops and asks its manager, through the operator's
        // `ask-manager` wrapper — which runs *inside this process*, and reads the
        // run it addresses out of this process's own environment. Scripted, and
        // never for the observer: a monitor member watches and asks nothing.
        if let Some(question) = fake::node_script(dir, "harness", "asks") {
            fake::ask_manager(&question);
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
    result(outcome, &prompt, args, dir);
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
    observation(index);
}

/// The observation that answered the call above, as the same wire shape carries
/// it: a `user` message holding a `tool_result` block, joined back to the call
/// by `tool_use_id`.
///
/// Emitted with every call rather than scripted, because a real turn has no
/// other way to go: a model that asked for a tool and was told nothing could not
/// take another step. Streaming only the ask was a double that stopped halfway
/// through the exchange it stands in for, and a consumer reading its stream saw
/// every dispatch reach for a tool and none of them ever learn anything.
///
/// It carries **no tool name**, which is the shape rather than an omission: a
/// result answers a call already named, so `oneharness_core` normalizes it to an
/// event whose `name` is absent and whose `output` is what came back.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the same provider wire shape
// as `tool_call` above, gated the same way: the real `oneharness_core` normalizes these
// lines, so a shape it stops reading is `tests/e2e/turns.rs` finding a dispatch that
// relayed an ask and never an answer.
fn observation(index: u64) {
    println!(
        "{}",
        serde_json::json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result",
                "tool_use_id": format!("toolu_{index}"),
                "content": OBSERVED,
            }]},
        })
    );
}

/// What the tool above returned. Recognisable, and the same every time, so a
/// journey can assert on the observation a turn was given rather than on the
/// fact that it was given one.
const OBSERVED: &str = "the turn ran";

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
/// reader adds up. `structured_output` is the field a native structured-output
/// run answers in — `--json-schema` is on the argv, and oneharness prefers that
/// field over anything it could extract from the text. Only a turn that
/// *answered* carries one: a turn that did not get there has nothing the schema
/// could have accepted, and a failure carrying an answer is a shape no harness
/// produces.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the same provider
// wire shape as `tool_call` above, gated the same way: `oneharness_core` reads
// `is_error`, `result`, `session_id` and `usage` out of this document, so a field
// it stops reading settles a member differently in `tests/e2e/dispatch.rs`.
fn result(outcome: Outcome, prompt: &str, args: &[String], dir: &std::path::Path) {
    let mut document = serde_json::json!({
        "type": "result",
        "subtype": outcome.subtype(),
        "is_error": outcome == Outcome::TurnFailed,
        "result": outcome.text(),
        "session_id": "fake-claude-session",
        "num_turns": 1,
        "total_cost_usd": 0,
        "usage": {"input_tokens": 1, "output_tokens": 1},
    });
    if outcome == Outcome::Answered && fake::flag(args, "--json-schema").is_some() {
        document["structured_output"] = structured(prompt, dir);
    }
    println!("{document}");
}

/// The validated answer a structured-output run is asked for.
///
/// One shape, because one dispatch in this stack asks for one: the change
/// request body, which the schema its graph names requires. Scripted with
/// `harness.body` where a journey wants to read its own words back out of the
/// change request.
fn structured(prompt: &str, dir: &std::path::Path) -> serde_json::Value {
    let body = fake::node_script(dir, "harness", "body").unwrap_or_else(|| {
        format!(
            "## What\nWhat the diff did.\n\n## Why\n{}",
            prompt.lines().next().unwrap_or_default()
        )
    });
    serde_json::json!({"body": body})
}
