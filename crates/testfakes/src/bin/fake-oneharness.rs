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

/// What follows a flag on a `oneharness run` argv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Takes {
    /// Nothing: the flag is the whole of it.
    Nothing,
    /// A value, which the real CLI refuses to be without.
    AValue,
}

/// How many times one flag may appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Occurs {
    /// Once. A second is a caller that has changed its mind mid-argv, and which
    /// of the two the real CLI keeps is not something a double may guess at.
    Once,
    /// As often as the caller has values for it.
    Repeatedly,
}

/// The flags one onejudge turn renders, what follows each, and how often it may.
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] this is a copy of the
// **CLI**'s grammar, which no crate in this dependency graph declares as data:
// onejudge renders it in a private function and oneharness parses it in a binary this
// build does not link. The reconciling gate is `tests/e2e/turns.rs`, which drives the
// real onejudge against this process — so a flag it starts sending that is not below is
// a refusal there rather than a double that quietly waves it through.
const FLAGS: [(&str, Takes, Occurs); 13] = [
    ("--compact", Takes::Nothing, Occurs::Once),
    ("--events", Takes::Nothing, Occurs::Once),
    ("--history", Takes::Nothing, Occurs::Once),
    ("--stream", Takes::Nothing, Occurs::Once),
    ("--control", Takes::Nothing, Occurs::Once),
    ("--system", Takes::AValue, Occurs::Once),
    ("--config", Takes::AValue, Occurs::Once),
    // The one repeatable flag: onejudge's renderer emits one per harness id whose
    // provider process oneharness is to replace with its own responder.
    ("--mock-harness", Takes::AValue, Occurs::Repeatedly),
    ("--cwd", Takes::AValue, Occurs::Once),
    ("--prompt", Takes::AValue, Occurs::Once),
    ("--prompt-file", Takes::AValue, Occurs::Once),
    ("--history-name", Takes::AValue, Occurs::Once),
    ("--session", Takes::AValue, Occurs::Once),
];

/// Refuses an argv the real `oneharness run` would not take.
///
/// The checks in [`run`] are about a flag onejudge chose the *wrong value* for;
/// this is about one it should not be sending at all. Both are the same property,
/// which is the only thing a double is worth: where the real CLI says no, this
/// one has to. An argument waved through here is one no journey can catch —
/// onejudge grows a flag oneharness does not take, every member settles green,
/// and the first thing that ever says otherwise is a real `oneharness`.
///
/// Three ways an argv is refused, and each is a different mistake:
///
/// * an argument that is not a flag this verb takes, which covers a positional
///   after the verb as well as an unknown option;
/// * a second occurrence of a flag that may appear once. Every reader below is
///   `fake::flag`, which answers with the **first**, so a second is a value this
///   process would silently ignore — and which of the two a real `oneharness`
///   keeps is not something a double may guess at;
/// * a value-taking flag with nothing after it, or with the next *flag* after it.
///   Both are the flag arriving empty, and left alone both read to `fake::flag`
///   as the flag never having been sent — so the refusal would name the wrong
///   thing, or the following flag would be eaten as this one's value.
fn declared(args: &[String]) -> Result<(), String> {
    let known = |arg: &String| FLAGS.iter().find(|(name, _, _)| name == arg);
    let mut seen: Vec<&str> = Vec::new();
    // Past the verb, which `main` matched to get here.
    let mut at = 1;
    while at < args.len() {
        let arg = &args[at];
        let Some((name, takes, occurs)) = known(arg) else {
            return Err(format!("oneharness run takes no argument {arg:?}"));
        };
        if *occurs == Occurs::Once && seen.contains(name) {
            return Err(format!(
                "oneharness run was given {arg} more than once, and it takes one"
            ));
        }
        seen.push(name);
        at += 1;
        if *takes == Takes::AValue {
            match args.get(at) {
                None => {
                    return Err(format!(
                        "oneharness run's {arg} takes a value, and nothing followed it"
                    ))
                }
                Some(next) if known(next).is_some() => {
                    return Err(format!(
                        "oneharness run's {arg} takes a value, and {next} followed it"
                    ))
                }
                Some(_) => at += 1,
            }
        }
    }
    Ok(())
}

/// `oneharness run …` — one turn of one side.
fn run(args: &[String], dir: &std::path::Path) -> ExitCode {
    if let Err(refusal) = declared(args) {
        return fake::refuse(&refusal);
    }
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
    match work(prompt, cwd, dir) {
        Ok(outcome) => outcome.exit_code(),
        Err(refusal) => fake::refuse(&refusal),
    }
}

/// The turn itself, answering with how it *ended* — which is not the same as this
/// process having refused, and is why the two are separate results: a turn that
/// did the work and did not get there still streamed and still reported, and a
/// refusal never started.
fn work(prompt: &str, cwd: &str, dir: &std::path::Path) -> Result<Outcome, String> {
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
            std::fs::write(&path, format!("{body}\n"))
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
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
    let outcome = if fake::node_script(dir, "harness", "fail").is_some()
        || (observing && fake::observe(dir) != ExitCode::SUCCESS)
    {
        Outcome::TurnFailed
    } else {
        Outcome::Answered
    };

    let mut events = vec![call(0, "echo the turn ran"), observation(1)];
    for event in &events {
        stream(&RunStreamEnvelope::Event {
            event: event.clone(),
        })?;
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
            })?;
        }
        events.extend(held);
        // Held again, so the node is *still in flight* when the second reading
        // is taken: a readout that only advances as the dispatch ends proves
        // nothing about supervising a live one.
        fake::wait_for(&dir.join("turn.settle"));
    }

    stream(&RunStreamEnvelope::Result {
        report: report(outcome.text(), Some(events), outcome),
    })?;
    Ok(outcome)
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
        Ok(CRITERION_MET.to_string())
    } else {
        Err(format!(
            "the judge side was asked something this double does not answer; it speaks the \
             supervisor decision ({SUPERVISOR_OPENING:?}) and the boolean verdict \
             ({EVALUATOR_OPENING:?}), and nothing else"
        ))
    };
    // A judgement is `Answered` whatever it decided: the turn that reached the
    // decision succeeded, and a supervisor saying the work is not done is not a
    // harness that failed to run.
    match answer.and_then(|answer| document(&report(&answer, None, Outcome::Answered))) {
        Ok(document) => {
            print!("{document}");
            ExitCode::SUCCESS
        }
        Err(refusal) => fake::refuse(&refusal),
    }
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
fn supervision(dir: &std::path::Path) -> Result<String, String> {
    let asked_again = dir.join("judge.asked-again");
    match fake::node_script(dir, "judge", "asks-again") {
        Some(instruction) if !asked_again.exists() => {
            fake::record(dir, "judge-asks-again", std::slice::from_ref(&instruction));
            // Refused rather than unwrapped: a marker this process could not write
            // is a supervisor that will ask again on every turn, which runs the
            // conversation to its ceiling and leaves the journey reading a shape
            // nobody scripted. Better the member dies saying so.
            std::fs::write(&asked_again, &instruction).map_err(|error| {
                format!(
                    "cannot record the supervisor's ask at {}: {error}",
                    asked_again.display()
                )
            })?;
            document(&serde_json::json!({
                "completion": false,
                "message": instruction,
                "reason": "the work is not there yet",
            }))
        }
        _ => Ok(SUPERVISED_COMPLETE.to_string()),
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

/// How a turn ended.
///
/// One value rather than a boolean beside four uses of it: the words the turn
/// answered with, its result's `status`, the exit code beside it, the `error` that
/// says what went wrong and this process's own exit status are five spellings of
/// one fact, and held apart any of them could say something the others do not — a
/// result reporting success beside a non-zero exit is exactly the pair a caller
/// settles a member on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The turn reached what it was asked for.
    Answered,
    /// It did not, and says so in the report as well as with a non-zero exit.
    TurnFailed,
}

impl Outcome {
    /// The visible answer, which is also the turn's last words.
    ///
    /// The same every time, so a journey can read a turn's own words back rather
    /// than assert that it had some: `tests/e2e/dispatch.rs` finds them in a
    /// rendered transcript and `tests/e2e/turns.rs` in a relayed `turn-message`,
    /// so a double that changed its mind about what a turn says fails in both
    /// places rather than passing quietly in either.
    ///
    /// A failing turn still *answers*. A double that fell silent instead would be
    /// the other failure — a producer that died mid-stream, which onejudge
    /// classifies as a broken transport rather than as a turn that did not get
    /// there.
    fn text(self) -> &'static str {
        match self {
            Self::Answered => "Ran what the task asked for.",
            Self::TurnFailed => "The turn did the work and did not get there.",
        }
    }

    /// The `status` this turn's result carries.
    fn status(self) -> Status {
        match self {
            Self::Answered => Status::Ok,
            Self::TurnFailed => Status::Nonzero,
        }
    }

    /// The harness's own exit code, which the result reports and this process
    /// answers with.
    fn code(self) -> i32 {
        match self {
            Self::Answered => 0,
            Self::TurnFailed => 1,
        }
    }

    /// The problem the result names, which is `null` on a turn that had none.
    fn error(self) -> Option<String> {
        match self {
            Self::Answered => None,
            Self::TurnFailed => Some(self.text().to_string()),
        }
    }

    fn exit_code(self) -> ExitCode {
        match self {
            Self::Answered => ExitCode::SUCCESS,
            Self::TurnFailed => ExitCode::from(1),
        }
    }
}

/// What the tool below returned. Recognisable, and the same every time, so a
/// journey can assert on the observation a turn was given rather than on the fact
/// that it was given one.
const OBSERVED: &str = "the turn ran";

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

fn stream(envelope: &RunStreamEnvelope) -> Result<(), String> {
    println!("{}", document(envelope)?);
    Ok(())
}

/// One document, serialized by the library that declares it.
///
/// Fallible rather than unwrapped, because what a refusal costs is the whole
/// difference: a panic here reaches onejudge as a producer that died mid-stream,
/// which it classifies and reports as the *member* failing. The message says
/// which document, so the journey reading that member's death is told the double
/// could not write one rather than left to infer it from a stack trace on a
/// stderr nobody kept.
fn document<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value)
        .map_err(|error| format!("cannot serialize what this turn answers with: {error}"))
}

/// The report one turn answers with: `said` is what that side said, and it is what
/// onejudge reads back as the turn's reply.
///
/// Free text rather than [`Outcome::text`] because the two sides do not answer the
/// same way: the agent's words *are* its outcome, and the supervisor's are a
/// decision it reached over a turn that succeeded. Everything the outcome does
/// decide comes off the one value, so no caller can pair a failure with a result
/// that reports success.
///
/// Every other field is what a real single-candidate run carries. `fallback` is
/// absent, which is what says this run had one candidate rather than a chain — a
/// report with a chain and no `ran` is an exhausted chain, and this turn ran.
fn report(said: &str, events: Option<Vec<ActionEvent>>, outcome: Outcome) -> RunReport {
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
            status: outcome.status(),
            prompt: None,
            model: None,
            exit_code: Some(outcome.code()),
            duration_ms: Some(1),
            telemetry: None,
            command: vec!["fake-claude".into()],
            output_format: OutputFormat::StreamJson,
            text: Some(said.into()),
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
            error: outcome.error(),
        }],
    }
}
