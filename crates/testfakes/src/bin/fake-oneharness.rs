//! A real `oneharness` executable, at `ONEAGENTGRAPH_ONEHARNESS_BIN`.
//!
//! **A single-sided member's turn does not come through here.** From
//! `oneagentgraph 0.2.18` that turn is an `oneharness_core` library call inside
//! the sibling's own process, and the only process left below it is the harness
//! the member's identity chain selected — which is `fake-claude`, at
//! oneharness's own `ONEHARNESS_BIN_CLAUDE_CODE`.
//!
//! What still names *this* one is a **two-party (`kind: onejudge`) member**.
//! `oneagentgraph` drives onejudge's own run driver in process and hands it a
//! spawn hook — so that it can reap a paid harness nothing else can reach — and
//! installing a hook is what puts onejudge on its spawning seam. So every turn of
//! a two-party conversation, on both sides, is one `oneharness run` process, and
//! this is that process.
//!
//! # The surface, and why it is two
//!
//! onejudge renders one turn as `run --compact [--events] --history [--system S]
//! [--config C] [--cwd D] --prompt-file - [--stream] [--history-name N]
//! [--session S]`, with the prompt on **stdin**. The two sides differ by what
//! they ask for, and this double answers each in the shape onejudge reads back:
//!
//! * the **agent** side asks for `--events --stream`, and gets the NDJSON
//!   streamed protocol — a `{"type":"event",…}` line the instant each tool event
//!   is observed, then one terminal `{"type":"result","report":{…}}`;
//! * the **judge** side asks for neither, and gets the bare report document on
//!   stdout, which is what a buffered `oneharness run` writes.
//!
//! Both reports are `oneharness_core`'s own [`RunReport`], built here and
//! serialized by that library — not a copy of its wire shape, which would go on
//! parsing after the producer changed it. The pin is the copy **onejudge** links;
//! see the workspace manifest.
//!
//! What this double is *not* is a stand-in for the conversation. onejudge decides
//! every turn, composes both prompts, parses both answers and settles the member;
//! `oneagentgraph` publishes each observation as an envelope. All of that is real.

use oneharness_core::domain::events::ActionEvent;
use oneharness_core::domain::mode::PermissionMode;
use oneharness_core::domain::report::{
    OutputFormat, RunReport, RunResult, RunStreamEnvelope, Status, SCHEMA_VERSION,
};
use oneharness_core::domain::signals::Usage;
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

/// Which side of a two-party member's conversation this turn is.
///
/// Decided by `--events`, which is the flag onejudge's own turn description sets
/// on the agent side and only there: the agent's turn is the one whose tool
/// activity a caller wants normalized, and a judgement call has none. Deciding it
/// from the *prompt* would be a double reading the conversation instead of the
/// argv it was invoked with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// The side that does the work, streaming as it goes.
    Agent,
    /// The side that supervises it, answering in one buffered document.
    Judge,
}

/// `oneharness run …` — one turn of one side.
fn run(args: &[String], dir: &std::path::Path) -> ExitCode {
    // Read **before** anything is refused. onejudge writes the prompt onto this
    // process's stdin after spawning it, so a refusal that exited without draining
    // it reaches the caller as `could not write prompt: Broken pipe` — a transport
    // failure hiding whichever refusal this double actually made.
    let Some(prompt) = prompt(args) else {
        return fake::refuse(
            "oneharness run requires the prompt, on `--prompt-file -` or on `--prompt`",
        );
    };
    let side = if args.iter().any(|arg| arg == "--events") {
        Side::Agent
    } else {
        Side::Judge
    };
    // Streaming is the agent side's, and only its: onejudge asks for it there and
    // reads the buffered document everywhere else, so a double that streamed a
    // judgement would put NDJSON where one report was expected.
    let streaming = args.iter().any(|arg| arg == "--stream");
    if (side == Side::Agent) != streaming {
        return fake::refuse(&format!(
            "oneharness run was asked for {side:?} work and {}--stream",
            if streaming { "" } else { "no " }
        ));
    }
    // The config is something `oneagentgraph` composed rather than passed
    // through, and both sides carry one: the judge side names its own, and the
    // agent side's rides the spawn hook the sibling installs. A double that ran
    // without reading it would answer a turn whose launch was prepared against
    // nothing.
    let Some(config) = fake::flag(args, "--config") else {
        return fake::refuse("oneharness run requires --config");
    };
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
    // The worktree, which is the agent side's whole working context — a turn that
    // wrote its work anywhere else would leave a publication with nothing to
    // publish. The judge side carries one only when its question is *about* the
    // worktree: the supervisor's is, and a stateless verdict over the finished
    // transcript is not, so it is required here rather than everywhere.
    let cwd = fake::flag(args, "--cwd");
    if let Some(cwd) = &cwd {
        if !std::path::Path::new(cwd).is_dir() {
            return fake::refuse(&format!(
                "oneharness run was given --cwd {cwd}, which is not a directory"
            ));
        }
    }

    match side {
        Side::Agent => match cwd {
            Some(cwd) => agent_turn(&prompt, &cwd, dir),
            None => fake::refuse("oneharness run requires --cwd for the side that does the work"),
        },
        Side::Judge => judge_turn(&prompt, dir),
    }
}

/// The prompt this turn was given.
///
/// onejudge sends it on **stdin**, behind `--prompt-file -`: that is what keeps a
/// transcript that grows with every turn under the OS argument ceiling. `--prompt`
/// is taken too, because it is the same value by another spelling and a double
/// that refused it would be refusing a legal `oneharness run`.
fn prompt(args: &[String]) -> Option<String> {
    if let Some(prompt) = fake::flag(args, "--prompt") {
        return Some(prompt);
    }
    if fake::flag(args, "--prompt-file").as_deref() != Some("-") {
        return None;
    }
    use std::io::Read;
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

/// The side that does the work: a tool exchange as it happens, then the report.
fn agent_turn(prompt: &str, cwd: &str, dir: &std::path::Path) -> ExitCode {
    // A worker turn that leaves something behind in the worktree it was given.
    // Only when scripted, and only for the worker: a publication needs a diff —
    // `onevcs publish` on a clean tree publishes nothing and says so — and a
    // journey that means to reach a real change request has to have a turn that
    // made one. Every other journey here is about the dispatch, and a file
    // appearing in a worktree unasked would be a change nobody made.
    let observing = prompt.contains("Observe this run");
    if !observing {
        if let Some(body) = fake::node_script(dir, "harness", "work") {
            let path = std::path::Path::new(cwd).join(WORK_FILE);
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
    // the pair of a non-zero exit and a report that says the run failed, and that
    // pair is what a caller reading the graph's settlement sees. Without it the
    // only failing member this suite could produce was one that refused on the
    // way in, which never reaches a settlement at all.
    let failed = fake::node_script(dir, "harness", "fail").is_some()
        || (observing && fake::observe(dir) != ExitCode::SUCCESS);

    let mut events = vec![call(0, "echo the turn ran"), observation(1)];
    for event in &events {
        stream(&RunStreamEnvelope::Event {
            event: event.clone(),
        });
    }
    // A worker turn that reports again after a hold: what a live readout of a
    // running dispatch is read against. The first event proves the stream
    // arrives; a second, released while the node is still in flight, proves the
    // readout *advances*. The driver's own turn is left alone — it is the
    // member running the engine verbs, not the one being watched.
    if !observing && dir.join("turn.hold").exists() {
        fake::wait_for(&dir.join("turn.go"));
        let held = [call(2, "cargo llvm-cov --workspace"), observation(3)];
        for event in &held {
            stream(&RunStreamEnvelope::Event {
                event: event.clone(),
            });
        }
        events.extend(held);
        // Held again, so the node is *still in flight* when the second reading
        // is taken: a readout that only advances as the dispatch ends proves
        // nothing about supervising a live one.
        fake::wait_for(&dir.join("turn.settle"));
    }

    let text = if failed { TURN_FAILED } else { ANSWER };
    stream(&RunStreamEnvelope::Result {
        report: report(text, Some(events), failed),
    });
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// The side that supervises: one buffered document carrying the answer.
///
/// Two questions reach it over one conversation, and they are told apart by the
/// prompt onejudge composed, because that is the only thing that distinguishes
/// them — both arrive as the same argv. Anything else is refused rather than
/// answered: a double that returned its supervisor decision to an evaluator would
/// be a protocol failure onejudge reports as the *member* dying.
///
/// Both answers are affirmative, which is what keeps a two-party journey to one
/// agent turn. A supervisor that asked for more would be a second paid turn and a
/// second of everything downstream reads; a `done_when` scored false would settle
/// the member as incomplete. A journey that wants either has no way to say so
/// through this double, which is a limit worth stating rather than a shape worth
/// guessing at.
fn judge_turn(prompt: &str, dir: &std::path::Path) -> ExitCode {
    let answer = if prompt.contains(SUPERVISOR_OPENING) {
        supervision(dir)
    } else if prompt.contains(EVALUATOR_OPENING) {
        CRITERION_MET.to_string()
    } else {
        return fake::refuse(&format!(
            "the judge side was asked something this double does not answer; it speaks the \
             supervisor decision ({SUPERVISOR_OPENING:?}) and the boolean verdict \
             ({EVALUATOR_OPENING:?}), and nothing else"
        ));
    };
    print!("{}", document(&report(&answer, None, false)));
    ExitCode::SUCCESS
}

/// The supervisor's decision: send the agent back once when a journey scripted
/// an instruction for it to send, and otherwise call the work done.
///
/// **Once**, and the marker file is how: each turn of the conversation is its own
/// process, so a double with no memory beyond the script directory would send the
/// agent back on every ask and the conversation would only ever end at its turn
/// ceiling. The marker is written before the instruction is handed over, so a
/// second ask reads it whether or not the turn it asked for got anywhere.
///
/// It is what makes a *conversation* observable at all: a supervisor that
/// completes on the first ask relays one party's words, and nothing a journey
/// reads can then tell the two parties' turns apart.
fn supervision(dir: &std::path::Path) -> String {
    let asked_again = dir.join("judge.asked-again");
    match fake::node_script(dir, "judge", "asks-again") {
        Some(instruction) if !asked_again.exists() => {
            fake::record(dir, "judge-asks-again", &[instruction.clone()]);
            std::fs::write(&asked_again, &instruction).expect("the supervisor's ask is recorded");
            document(&serde_json::json!({
                "completion": false,
                "message": instruction,
                "reason": "the work is not there yet",
            }))
        }
        _ => SUPERVISED_COMPLETE.to_string(),
    }
}

/// How onejudge opens the prompt it hands its supervisor side.
///
/// Matched on the opening rather than on the whole prompt, which carries the
/// task, the persona and the transcript so far. It is the *supervisor* prompt
/// specifically: onejudge builds this one for the decision that continues or ends
/// the conversation, and its answer is the discriminated JSON below rather than
/// the `value`/`reason` pair a boolean verdict takes.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] onejudge composes this
// prompt in a private function and declares no constant for it, so there is nothing to
// share. Its gate is the refusal above: the real onejudge builds the prompt and this
// process reads it, so an opening it stops writing is a loud refusal in
// `tests/e2e/turns.rs` rather than a double that quietly answers the wrong question.
const SUPERVISOR_OPENING: &str = "You are the simulated USER and completion supervisor";

/// What the supervisor answers: this turn's work is done.
///
/// onejudge's own discriminated shape — a `completion: true` carries a non-empty
/// `reason` and no `message`, and anything else it refuses as a protocol failure.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the same prompt contract as
// `SUPERVISOR_OPENING` above and gated the same way: onejudge's own `parse_supervisor`
// reads this document, so a shape it stops accepting is a member that dies in
// `tests/e2e/turns.rs` naming the protocol failure.
const SUPERVISED_COMPLETE: &str =
    "{\"completion\":true,\"reason\":\"the turn did what the task asked\"}";

/// How onejudge opens the prompt it hands its **evaluator**: the boolean verdict
/// that scores a criterion over the finished transcript, which is what a member's
/// `done_when` is re-judged as once the conversation ends.
///
/// A different question from the supervisor's, asked at a different point and
/// answered in a different shape, so it is matched separately rather than folded
/// into one "the judge side" case.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the same prompt contract as
// `SUPERVISOR_OPENING` above, gated the same way.
const EVALUATOR_OPENING: &str = "You are a strict, careful evaluator";

/// What the evaluator answers: the criterion is satisfied.
///
/// onejudge's own verdict shape — a `value` of the type the query asked for, and
/// the reason beside it. This double is only ever asked the boolean form, because
/// a member's `done_when` is the one criterion `oneagentgraph` scores.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the same prompt contract as
// `SUPERVISOR_OPENING` above, gated the same way.
const CRITERION_MET: &str = "{\"value\":true,\"reason\":\"the turn did what the task asked\"}";

/// What a turn that reached its task answers with.
const ANSWER: &str = "Ran what the task asked for.";

/// What a turn that did the work and did not get there answers with.
const TURN_FAILED: &str = "The turn did the work and did not get there.";

/// What the tool below returned. Recognisable, and the same every time, so a
/// journey can assert on the observation a turn was given rather than on the fact
/// that it was given one.
const OBSERVED: &str = "the turn ran";

/// One tool call, in oneharness's own normalized event shape.
fn call(index: usize, command: &str) -> ActionEvent {
    ActionEvent {
        kind: "tool_call".into(),
        name: Some("bash".into()),
        input: Some(serde_json::json!({"command": command})),
        output: None,
        index,
        tool_call_id: Some(format!("toolu_{index}")),
        started_at: None,
        finished_at: None,
        duration_ms: None,
        status: None,
        timing_source: None,
    }
}

/// The observation that answered the call before it, joined to it by the same
/// call id.
///
/// Emitted with every call rather than scripted, because a real turn has no other
/// way to go: a model that asked for a tool and was told nothing could not take
/// another step. It carries **no tool name**, which is the shape rather than an
/// omission — a result answers a call already named.
fn observation(index: usize) -> ActionEvent {
    ActionEvent {
        kind: "tool_result".into(),
        name: None,
        input: None,
        output: Some(OBSERVED.into()),
        index,
        tool_call_id: Some(format!("toolu_{}", index - 1)),
        started_at: None,
        finished_at: None,
        duration_ms: None,
        status: None,
        timing_source: None,
    }
}

/// One line of the streamed protocol, in oneharness's own envelope.
fn stream(envelope: &RunStreamEnvelope) {
    println!("{}", document(envelope));
}

/// One document, serialized by the library that declares it.
fn document<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("an oneharness report serializes")
}

/// The report one turn answers with: `text` is what the side said, and it is what
/// onejudge reads back as that turn's reply.
///
/// Every other field is what a real single-candidate run carries. `fallback` is
/// absent, which is what says this run had one candidate rather than a chain — a
/// report with a chain and no `ran` is an exhausted chain, and this turn ran.
fn report(text: &str, events: Option<Vec<ActionEvent>>, failed: bool) -> RunReport {
    RunReport {
        schema_version: SCHEMA_VERSION.into(),
        oneharness_version: "0.0.0-fake".into(),
        prompt: String::new(),
        model: None,
        models: None,
        resume: None,
        fork: false,
        session: None,
        permission_mode: PermissionMode::Bypass,
        bypass_permissions: true,
        dry_run: false,
        schema: None,
        schema_max_retries: None,
        batch: None,
        fallback: None,
        mock_rules: None,
        spy_file: None,
        history_file: None,
        config_files: Vec::new(),
        control: None,
        results: vec![RunResult {
            harness: "claude-code".into(),
            variant: None,
            harness_id: "claude-code".into(),
            bin: "fake-claude".into(),
            available: true,
            status: if failed { Status::Nonzero } else { Status::Ok },
            prompt: None,
            model: None,
            exit_code: Some(i32::from(failed)),
            duration_ms: Some(1),
            telemetry: None,
            command: vec!["fake-claude".into()],
            output_format: OutputFormat::StreamJson,
            text: Some(text.into()),
            text_source: Some("json:result".into()),
            usage: Usage {
                input_tokens: Some(1),
                output_tokens: Some(1),
                cache_read_tokens: None,
                cache_write_tokens: None,
                cost_usd: None,
            },
            usage_source: Some("json".into()),
            session_id: Some("fake-oneharness-session".into()),
            events,
            events_source: Some("json".into()),
            structured: None,
            schema_valid: None,
            schema_attempts: None,
            schema_error: None,
            failure_kind: None,
            failure_kind_source: None,
            stdout: String::new(),
            stderr: String::new(),
            error: failed.then(|| TURN_FAILED.to_string()),
        }],
    }
}
