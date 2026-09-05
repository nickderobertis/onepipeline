//! A real `oneharness` executable, at `ONEAGENTGRAPH_ONEHARNESS_BIN`.
//!
//! Only a **two-party (`kind: onejudge`) member** comes through here. A
//! single-sided member's turn is an `oneharness_core` library call in the
//! sibling's own process, and the only process under it is `fake-claude`.
//!
//! The non-obvious part is why a two-party member still spawns anything:
//! `oneagentgraph` hands onejudge a spawn hook — the seam it reaps a paid harness
//! through — and installing one is what puts onejudge on its spawning seam. So
//! every turn of the conversation, on both sides, is one `oneharness run`, and
//! this is that process. Nothing above it is stood in for: onejudge decides each
//! turn, composes both prompts, parses both answers and settles the member.
//!
//! The two sides take one argv and answer differently, which is what [`Side`]
//! decides: the agent side streams (`--events --stream`, NDJSON events then a
//! terminal result line) and the judge side answers in one buffered report.
//! Both reports are `oneharness_core`'s own [`RunReport`], serialized by the
//! library that declares it rather than copied — at the copy **onejudge** links,
//! which is the pin the workspace manifest explains.

use oneharness_core::domain::events::ActionEvent;
use oneharness_core::domain::fallback::RunWork;
use oneharness_core::domain::mode::PermissionMode;
use oneharness_core::domain::report::{
    OutputFormat, RunReport, RunResult, RunStreamEnvelope, Status, SCHEMA_VERSION,
};
use oneharness_core::domain::signals::{FailureKind, Usage};
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
/// Decided by `--events`, which onejudge sets on the agent side and only there.
/// Deciding it from the *prompt* would be a double reading the conversation
/// instead of the argv it was invoked with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Agent,
    Judge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Takes {
    Nothing,
    AValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Occurs {
    Once,
    Repeatedly,
}

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
/// This is the only thing a double is worth: an argument waved through here is
/// one no journey can catch — onejudge grows a flag oneharness does not take,
/// every member settles green, and the first thing to say otherwise is a real
/// `oneharness`.
///
/// The refusals are the ones `fake::flag` cannot report: it answers with the
/// first occurrence and cannot tell a flag sent empty from one never sent.
fn declared(args: &[String]) -> Result<(), String> {
    let known = |arg: &String| FLAGS.iter().find(|(name, _, _)| name == arg);
    let mut seen: Vec<&str> = Vec::new();
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
    let identity = match ran(&config_text) {
        Ok(identity) => identity,
        Err(refusal) => return fake::refuse(&format!("--config {config}: {refusal}")),
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
            Some(cwd) => agent_turn(&prompt, &cwd, dir, &identity),
            None => fake::refuse("oneharness run requires --cwd for the side that does the work"),
        },
        Side::Judge => judge_turn(&prompt, dir, &identity),
    }
}

/// An `oneharness` config as this process needs to read it: which identity a turn
/// run under it would be taken by.
///
/// Nothing else is read, because nothing else changes what this double does — but
/// the document is *parsed*, so a config the sibling composed that is not TOML is
/// refused here rather than answered around.
// llmlint: ignore[boundary_inputs_validated] deliberately **not**
// `deny_unknown_fields`, and it is the one place in this repository where that would be
// wrong: this is oneharness's schema rather than this crate's, and every config the
// suite writes carries keys that are none of this process's business — `run_mode`,
// `schema_file`, an `[env]` table. Denying them would refuse every real config. The
// invariant in `AGENTS.md` is about the documents *this crate* owns; a foreign one is
// read for the field it needs and left alone.
#[derive(serde::Deserialize)]
struct Config {
    /// The chain the config names, already resolved to the identity it would run
    /// as. Absent when the config names none and leaves the selection to
    /// oneharness's own discovery.
    harnesses: Option<Chain>,
}

/// One harness identity, which is a name.
///
/// A type rather than a `String` because the empty one is not an identity and
/// must not be able to reach a report: `harness` and `harness_id` are how a
/// consumer selects the same candidate again, and a result naming `""` is a turn
/// attributed to nothing. Constructed only by [`Identity::named`], so a value of
/// this type has a name in it by construction.
struct Identity(String);

impl Identity {
    /// The identity `raw` names, or the reason it names none.
    fn named(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("an identity chain candidate has no name".to_string());
        }
        Ok(Self(raw.to_string()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// A config's identity chain, as the identity it would run the turn as: the
/// first candidate, which is the one a run that stepped past nothing reports.
///
/// Every candidate is checked, not only that one — a nameless entry anywhere is a
/// config a real `oneharness` would refuse, and this turn not reaching it is no
/// reason to wave it through.
struct Chain(Identity);

impl<'de> serde::Deserialize<'de> for Chain {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let candidates = Vec::<String>::deserialize(deserializer)?;
        let mut named = candidates
            .iter()
            .map(|candidate| Identity::named(candidate))
            .collect::<Result<Vec<_>, _>>()
            .map_err(D::Error::custom)?
            .into_iter();
        named.next().map(Self).ok_or_else(|| {
            D::Error::custom("its identity chain names no candidate to run the turn")
        })
    }
}

/// The identity a turn under `config` ran as.
///
/// Read off the config rather than assumed, so a report names the identity the
/// launch selected. Which candidate that is, and what makes a chain resolvable at
/// all, is [`Chain`]'s — what is left here is the one case a chain cannot answer.
fn ran(config: &str) -> Result<Identity, String> {
    let config: Config = toml::from_str(config)
        .map_err(|error| format!("this is not a config oneharness could run: {error}"))?;
    match config.harnesses {
        Some(chain) => Ok(chain.0),
        // What oneharness does with a config that names no chain: discover one.
        // There is one harness in this suite, so that is what it discovers.
        None => Identity::named(DISCOVERED),
    }
}

/// The identity oneharness discovers when a config names no chain — the only one
/// this suite provides a binary for, at `ONEHARNESS_BIN_CLAUDE_CODE`.
const DISCOVERED: &str = "claude-code";

/// The prompt this turn was given.
///
/// onejudge sends it on **stdin**, behind `--prompt-file -`: that is what keeps a
/// transcript that grows with every turn under the OS argument ceiling. `--prompt`
/// is taken too, because it is the same value by another spelling and a double
/// that refused it would be refusing a legal `oneharness run`.
///
/// Blank is no prompt, whichever spelling it arrived in: a turn with nothing to
/// answer is a caller that composed one wrongly, and both ways in are held to
/// that so a journey cannot pass on the argv what it would be refused on stdin.
fn prompt(args: &[String]) -> Option<String> {
    if let Some(prompt) = fake::flag(args, "--prompt") {
        return (!prompt.trim().is_empty()).then_some(prompt);
    }
    if fake::flag(args, "--prompt-file").as_deref() != Some("-") {
        return None;
    }
    use std::io::Read;
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

fn agent_turn(prompt: &str, cwd: &str, dir: &std::path::Path, identity: &Identity) -> ExitCode {
    match work(prompt, cwd, dir, identity) {
        Ok(outcome) => outcome.exit_code(),
        Err(refusal) => fake::refuse(&refusal),
    }
}

/// The turn itself, answering with how it *ended* — which is not the same as this
/// process having refused, and is why the two are separate results: a turn that
/// did the work and did not get there still streamed and still reported, and a
/// refusal never started.
fn work(
    prompt: &str,
    cwd: &str,
    dir: &std::path::Path,
    identity: &Identity,
) -> Result<Outcome, String> {
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
    let outcome = if fake::node_script(dir, "harness", "rejects").is_some() {
        Outcome::ProviderRejected
    } else if fake::node_script(dir, "harness", "fail").is_some()
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
        // A journey that needs the turn *after* this one held too asks for it
        // with `turn.hold-each`, and the gates are consumed here rather than
        // re-armed by the journey afterwards. Re-arming from outside is a race
        // against the next turn, which starts as soon as this one ends — and
        // the one journey that needs it is about what a note did to a node that
        // is still in flight, which is exactly the window that race loses.
        //
        // The script's own text narrows it to the member whose prompt carries
        // that text, because the gates are one pair for the whole world: a
        // second node's turn consuming them would take the gate the first one is
        // still waiting on, and hang it.
        if fake::node_script(dir, "turn", "hold-each")
            .is_some_and(|marker| marker.trim().is_empty() || prompt.contains(marker.trim()))
        {
            for gate in ["turn.go", "turn.settle"] {
                let _ = std::fs::remove_file(dir.join(gate));
            }
        }
    }

    stream(&RunStreamEnvelope::Result {
        report: report(outcome.text(), Some(events), outcome, identity),
    })?;
    Ok(outcome)
}

/// The side that supervises: one buffered document carrying the answer.
///
/// Two questions reach it over one conversation and both arrive as the same argv,
/// so the prompt is the only thing that tells them apart. A third is refused
/// rather than guessed at: onejudge reads each answer strictly, and one given to
/// the wrong question is a protocol failure it reports as the *member* dying.
fn judge_turn(prompt: &str, dir: &std::path::Path, identity: &Identity) -> ExitCode {
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
    match answer.and_then(|answer| document(&report(&answer, None, Outcome::Answered, identity))) {
        Ok(document) => {
            print!("{document}");
            ExitCode::SUCCESS
        }
        Err(refusal) => fake::refuse(&refusal),
    }
}

/// The supervisor's decision: send the agent back when a journey scripted an
/// instruction for it to send, and otherwise call the work done.
///
/// **A bounded number of times**, counted rather than remembered by a marker:
/// each turn is its own process, so without a bound the conversation would only
/// ever end at its turn ceiling. Counted before the instruction is handed over,
/// so a later ask reads it whether or not the turn it asked for got anywhere.
///
/// The bound is one unless a journey names another with `judge.asks-again-times`,
/// which exists for the one shape a single ask cannot state: a supervisor
/// decision *re-taken* — as a note delivered into a live judge turn re-takes it —
/// that is still not the conversation's last, so the note rides a response that
/// opens another worker turn rather than one that ends the work.
fn supervision(dir: &std::path::Path) -> Result<String, String> {
    hold_the_supervisor(dir);
    let asked_again = dir.join("judge.asked-again");
    let nth = fake::count(dir, "judge-supervision");
    let times = asks_again_times(dir)?;
    match fake::node_script(dir, "judge", "asks-again") {
        Some(instruction) if nth <= times => {
            fake::record(dir, "judge-asks-again", std::slice::from_ref(&instruction));
            // Refused rather than unwrapped: a marker this process could not write
            // is a journey reading a conversation whose shape it cannot see.
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

/// How many supervisor decisions send the agent back before one calls it done.
///
/// One, unless `judge.asks-again-times` names another. A script holding anything
/// that is not a count is a scenario nobody wrote, and reading it as the default
/// would run a journey against a conversation it did not ask for.
fn asks_again_times(dir: &std::path::Path) -> Result<usize, String> {
    match fake::node_script(dir, "judge", "asks-again-times") {
        None => Ok(1),
        Some(text) => text.parse().map_err(|error| {
            format!("judge.asks-again-times holds {text:?}, which is not a count: {error}")
        }),
    }
}

/// Hold this supervisor turn open, when a journey asked for one that is.
///
/// The judge's side of `turn.hold`, and the only way a journey can offer anything
/// — a manager's note, in particular — while the **supervisor** is the party
/// taking a turn: a worker hold reaches the other party and holds the wrong one.
/// The marker is written before the wait so the journey can tell a supervisor turn
/// that is live from one that has not opened yet, and the gate is a file rather
/// than a clock so the note really arrives inside the turn.
///
/// The gate is not consumed, so a decision re-taken *because* of what arrived
/// runs straight through: what a journey holds is the turn it is delivering into,
/// not every turn after it.
fn hold_the_supervisor(dir: &std::path::Path) {
    if fake::node_script(dir, "judge", "hold").is_none() {
        return;
    }
    let holding = dir.join("judge.holding");
    if let Err(error) = std::fs::write(&holding, "holding") {
        fake::fail(&format!(
            "cannot say the supervisor turn is live at {}: {error}",
            holding.display()
        ));
    }
    fake::wait_for(&dir.join("judge.go"));
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
/// One value rather than a boolean beside four uses of it: held apart, the words,
/// the `status`, the exit code, the `error` and this process's own exit could each
/// say something the others do not — and a result reporting success beside a
/// non-zero exit is exactly the pair a caller settles a member on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The turn reached what it was asked for.
    Answered,
    /// It did not, and says so in the report as well as with a non-zero exit.
    TurnFailed,
    /// The turn **ran, answered and was billed**, and the provider then rejected
    /// it — declared in the same terminal record while the process exits 0. What
    /// oneharness writes down for such a turn is a record that contradicts its own
    /// classification: `status: ok`, `exit_code: 0` and billed usage beside
    /// `failure_kind: rate_limit`. This double writes exactly that record, in
    /// oneharness's own types, because it is the pair a supervisor has to
    /// reconcile and no other outcome here can produce it.
    ProviderRejected,
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
            Self::Answered | Self::ProviderRejected => "Ran what the task asked for.",
            Self::TurnFailed => "The turn did the work and did not get there.",
        }
    }

    /// The `status` this turn's result carries.
    fn status(self) -> Status {
        match self {
            // `Ok` on the rejection too, and that is the whole point: the process
            // ran to completion, so the record says so however the turn was
            // classified.
            Self::Answered | Self::ProviderRejected => Status::Ok,
            Self::TurnFailed => Status::Nonzero,
        }
    }

    /// The harness's own exit code, which the result reports and this process
    /// answers with.
    fn code(self) -> i32 {
        match self {
            Self::Answered | Self::ProviderRejected => 0,
            Self::TurnFailed => 1,
        }
    }

    /// The problem the result names, which is `null` on a turn that had none.
    fn error(self) -> Option<String> {
        match self {
            Self::Answered => None,
            Self::TurnFailed => Some(self.text().to_string()),
            Self::ProviderRejected => Some("API Error: 429 rate limit exceeded".to_string()),
        }
    }

    /// What the harness classified this turn as, which is the field a supervisor
    /// publishes a death on. `None` where the record has nothing to say.
    fn failure_kind(self) -> Option<FailureKind> {
        match self {
            Self::Answered | Self::TurnFailed => None,
            Self::ProviderRejected => Some(FailureKind::RateLimit),
        }
    }

    /// What the provider billed for this turn.
    ///
    /// One token each is the ordinary reading; the rejection's is real money on a
    /// real turn, because *billed* is the half of the record that says a turn was
    /// paid for and got — the reading a reconciliation asks about.
    fn usage(self) -> Usage {
        match self {
            Self::Answered | Self::TurnFailed => Usage {
                input_tokens: Some(1),
                output_tokens: Some(1),
                cache_read_tokens: None,
                cache_write_tokens: None,
                cost_usd: None,
            },
            Self::ProviderRejected => Usage {
                input_tokens: Some(41233),
                output_tokens: Some(9812),
                cache_read_tokens: None,
                cache_write_tokens: None,
                cost_usd: Some(12.11),
            },
        }
    }

    /// What this run has to show for itself, which the report carries only where
    /// `failure_kind` has nothing to say — a success needs no such reading.
    ///
    /// A failing turn here is one that ran: it billed usage and it answers in its
    /// own words. Reported `none` instead, it would read to onejudge's fallback
    /// verdict as a candidate that never got started — which is a chain this
    /// double would fall through rather than the turn it is acting out.
    fn work(self) -> Option<RunWork> {
        match self {
            Self::Answered => None,
            Self::TurnFailed | Self::ProviderRejected => Some(RunWork::Done),
        }
    }

    fn exit_code(self) -> ExitCode {
        match self {
            Self::Answered | Self::ProviderRejected => ExitCode::SUCCESS,
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

/// The report one turn answers with, where `said` is what onejudge reads back as
/// the turn's reply.
///
/// Free text rather than [`Outcome::text`] because the two sides do not answer the
/// same way: the agent's words *are* its outcome, and the supervisor's are a
/// decision it reached over a turn that succeeded.
///
/// `fallback` is absent, which is what says this run had one candidate rather than
/// a chain — a report with a chain and no `ran` is an exhausted chain, and this
/// turn ran.
fn report(
    said: &str,
    events: Option<Vec<ActionEvent>>,
    outcome: Outcome,
    identity: &Identity,
) -> RunReport {
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
            harness: identity.as_str().into(),
            variant: None,
            harness_id: identity.as_str().into(),
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
            usage: outcome.usage(),
            usage_source: Some("json".into()),
            session_id: Some("fake-oneharness-session".into()),
            events,
            events_source: Some("json".into()),
            structured: None,
            schema_valid: None,
            schema_attempts: None,
            schema_error: None,
            failure_kind: outcome.failure_kind(),
            failure_kind_source: outcome.failure_kind().map(|_| "stdout".to_string()),
            work: outcome.work(),
            stdout: String::new(),
            stderr: String::new(),
            error: outcome.error(),
        }],
    }
}
