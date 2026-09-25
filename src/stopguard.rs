//! `onepipeline stop-guard` — the general stop guard over
//! [`crate::unwatched`]: one verdict on whether a session's turn may end.
//!
//! What the verb promises is entry 85 of `docs/contract-divergences.md`; it is
//! not restated here, on the terms [`crate::unwatched`] keeps beside its own
//! entry. What belongs beside the code is the one rule the endings are built
//! on: **only a positively determined unwatched run blocks, and only once the
//! block has been written down.** For *reporting* a run every unknown resolves
//! toward reporting it, because being wrong there costs one re-armed watch. For
//! *blocking* the irreversible act is the block itself — it holds the caller's
//! own session open — so every unknown resolves toward standing aside: a
//! question that could not be asked, a memory that could not be read or kept,
//! an engine error in place of an answer. Each of those is a [`Verdict::Warn`],
//! never a block and never silence, because a guard that fails silent is worse
//! than none.
//!
//! Declared `--source`s are combined into the one verdict by [`combined`]; a
//! source that cannot be consulted blocks rather than warns, the one exception
//! to the rule above.
//!
//! The verb is harness-neutral in and out. What it reads is a session and
//! whether this stop continues a block it made; what it answers is one verdict.
//! A harness's payload and decision shape are a *rendering* of that, chosen by
//! [`Format`], and the decision is made in [`guard`] alone.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::cli::{StopGuardArgs, StopGuardFormat as Format};
use crate::error::Error;

/// The directory under the state root the per-session memories are kept in.
const MEMORY_DIR: &str = "onepipeline/stop-guard";

/// What a reported run's line tells a caller to do about it, spelled as the
/// command a person types.
const ARM_A_WATCH: &str = "onepipeline watch";

/// The most a declared source may write on standard output. A verdict object
/// is a few lines of text; anything past this is not one, and is not read into
/// memory to find out.
const SOURCE_ANSWER_LIMIT: u64 = 1024 * 1024;

/// A session id that names somebody: not blank, and free of the NUL no
/// argument vector could carry. Built only by [`Session::named`], so a session
/// the guard asks about, keys a memory by, or names in a warning is one that
/// was checked at the boundary it arrived over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Session(String);

impl Session {
    /// `text` as a session, or nothing when it names nobody.
    fn named(text: String) -> Option<Self> {
        (!text.trim().is_empty() && !text.contains('\0')).then_some(Self(text))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which stop this is: the first of a turn, or one following a block this guard
/// made — the one case the memory is consulted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// A stop nothing has refused yet.
    First,
    /// A stop following a block this guard made.
    Continuation,
}

impl Stop {
    const fn of(continuation: bool) -> Self {
        if continuation {
            Self::Continuation
        } else {
            Self::First
        }
    }
}

/// What this verb was asked: whose stop, and which stop it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asked {
    pub session: Session,
    pub stop: Stop,
}

/// The one neutral input object the verb reads off standard input when no
/// `--session` is given: its two field names are this verb's own.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NeutralInput {
    session: Option<String>,
    #[serde(default)]
    continuation: bool,
}

/// The Stop payload the two hook-driven harnesses hand a stop hook, read for
/// the two fields this verb needs and nothing else.
///
/// Claude Code's `Stop` hook and Codex's `Stop` hook share these two names —
/// Codex's own schema says it mirrors Claude's — so one reader serves both
/// renderings. Every other field of the payload is ignored, because a payload
/// that grows a field is not a reason to refuse a stop.
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] these two names are the
// harnesses' own and exist in no machine-readable form this tree can reach: Claude Code
// publishes its Stop payload only as prose, and Codex's schema is compiled into its
// binary. The drift gate is `tests/e2e/stop_guard.rs`, which feeds full Stop payloads
// through the wiring read out of `docs/stop-guard.md`, and that page records how the
// names were read off `codex-cli 0.154.0` so the next reader can re-check them.
#[derive(Debug, Deserialize)]
struct HookInput {
    session_id: Option<String>,
    #[serde(default)]
    stop_hook_active: bool,
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// What the guard decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Refuse the stop; the reason is the report — `unwatched`'s own lines.
    Block(String),
    /// Let the stop through and put one sentence in front of the person: what
    /// could not be answered, and the command to ask it by hand.
    Warn(String),
    /// Nothing to say.
    None,
}

impl Verdict {
    /// The verdict as a caller reads it: one object under `format`, or nothing
    /// at all for a harness whose "proceed" is silence.
    pub(crate) fn render(&self, format: Format) -> String {
        let object = match (format, self) {
            (Format::Neutral, Self::Block(reason)) => json!({"verdict": "block", "reason": reason}),
            (Format::Neutral, Self::Warn(message)) => {
                json!({"verdict": "warn", "message": message})
            }
            (Format::Neutral, Self::None) => json!({"verdict": "none"}),
            // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] the decision
            // shape is the harnesses', with no machine-readable source this tree can reach;
            // held by the Claude Code journey in `tests/e2e/stop_guard.rs`, and read off
            // Codex's compiled schema as `docs/stop-guard.md` records.
            (Format::ClaudeCode | Format::Codex, Self::Block(reason)) => {
                json!({"decision": "block", "reason": reason})
            }
            (Format::ClaudeCode | Format::Codex, Self::Warn(message)) => {
                json!({"systemMessage": message})
            }
            (Format::ClaudeCode | Format::Codex, Self::None) => return String::new(),
            // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
        };
        format!("{object}\n")
    }
}

/// Read what the verb was asked: the flags, or the one object on standard
/// input the format names.
///
/// `None` is a silent ending rather than a refusal — an input this verb cannot
/// read, or one naming no session, knows nothing about whether a run is
/// watched, and saying so on a terminal at the end of every turn would be noise
/// about the guard rather than about the runs. A blank session is that same
/// nothing: it is *not* filled from the environment, because the environment
/// carries the launching session of whatever process runs the hook, which for a
/// dispatched worker is its manager's — the one session this verb must never
/// answer about unasked.
pub(crate) fn asked(args: &StopGuardArgs) -> Option<Asked> {
    if let Some(session) = &args.session {
        return named(session.clone(), args.continuation);
    }
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).ok()?;
    // One object and nothing else: serde would read a struct out of an array
    // by position, and an array is not the input either contract names.
    let object: serde_json::Value = serde_json::from_str(&text).ok()?;
    if !object.is_object() {
        return None;
    }
    match args.format {
        Format::Neutral => {
            let input: NeutralInput = serde_json::from_value(object).ok()?;
            named(input.session?, input.continuation || args.continuation)
        }
        Format::ClaudeCode | Format::Codex => {
            let input: HookInput = serde_json::from_value(object).ok()?;
            named(
                input.session_id?,
                input.stop_hook_active || args.continuation,
            )
        }
    }
}

/// A session that names somebody, or nothing.
fn named(session: String, continuation: bool) -> Option<Asked> {
    Some(Asked {
        session: Session::named(session)?,
        stop: Stop::of(continuation),
    })
}

/// Decide one stop: ask `unwatched` and every declared source about the
/// session, and answer with one verdict combining them, remembering each block
/// — per source — before it is made.
///
/// The sources are consulted concurrently with `unwatched` and with each other,
/// so what they add to the stop is at most one `timeout` rather than the sum of
/// theirs; `unwatched` itself is bounded as the verb always was, by its cost.
///
/// `unresolved` is what the ordinary verb would have written on standard error
/// for the same question, handed back so the caller writes exactly that and
/// nothing else there.
pub(crate) fn guard(
    root: &Path,
    asked: &Asked,
    sources: &[String],
    timeout: Duration,
) -> (Verdict, Vec<String>) {
    let mut declared: Vec<&str> = Vec::new();
    for command in sources {
        if !declared.contains(&command.as_str()) {
            declared.push(command);
        }
    }
    let ((own, unresolved), answered) = std::thread::scope(|scope| {
        let consulting: Vec<_> = declared
            .iter()
            .map(|&command| scope.spawn(move || consult(command, asked, timeout)))
            .collect();
        let own = own(root, asked);
        let answered: Vec<Result<Answer, String>> = consulting
            .into_iter()
            .map(|consulting| {
                consulting
                    .join()
                    .unwrap_or_else(|_| Err("the guard's own consultation of it failed".to_owned()))
            })
            .collect();
        (own, answered)
    });
    let mut verdicts = vec![own];
    verdicts.extend(
        declared
            .iter()
            .zip(answered)
            .map(|(command, answer)| settle(asked, command, answer)),
    );
    (combined(verdicts), unresolved)
}

/// The verb's own answer: `unwatched` about the session, remembered under the
/// session's own memory.
fn own(root: &Path, asked: &Asked) -> (Verdict, Vec<String>) {
    let session = asked.session.as_str();
    let unwatched = match crate::unwatched::unwatched(root, session) {
        Ok(unwatched) => unwatched,
        // The memory is left as it is: a question that could not be asked says
        // nothing about whether the condition it records moved, and a kept
        // memory changes no later verdict but an identical continuation's.
        Err(error) => return (unguarded(session, &error), Vec::new()),
    };
    let name = own_memory(session);
    if unwatched.reported.is_empty() {
        return match forget(&name) {
            Ok(()) => (Verdict::None, unwatched.unresolved),
            Err(why) => (unforgotten(session, &why), unwatched.unresolved),
        };
    }
    // The verb's own lines, byte for byte: they are what name the runs to watch
    // and the command that watches each, and nothing here reads a run out of
    // them.
    let report = crate::verbs::render_unwatched(&unwatched);
    let unresolved = unwatched.unresolved;
    let digest = hex(&Sha256::digest(report.as_bytes()));
    if asked.stop == Stop::Continuation {
        match remembered(&name) {
            Err(why) => {
                return (
                    stood_aside(
                        session,
                        &report,
                        &format!("could not read what it last blocked on ({why})"),
                    ),
                    unresolved,
                )
            }
            // The manager was told this and did nothing, so a second identical
            // block would hold the session open on a condition that has not
            // moved. What ends the guard is the condition changing, never a
            // count of how often it fired.
            Ok(Some(last)) if last == digest => return (Verdict::None, unresolved),
            Ok(_) => {}
        }
    }
    // Recorded *before* the block, because the record is what makes the block
    // safe to make: without it the next continuation cannot tell an unchanged
    // condition from a moved one, and would block again on the same answer for
    // ever.
    if let Err(why) = remember(&name, &digest) {
        return (
            stood_aside(
                session,
                &report,
                &format!("could not record what it would block on ({why})"),
            ),
            unresolved,
        );
    }
    (Verdict::Block(report), unresolved)
}

/// A declared source's answer once it has been read and checked: kept apart
/// from [`Verdict`] because a source's words are not yet this stop's — they
/// are still to be labelled with the source and held to its continuation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    Block(String),
    Warn(String),
    None,
}

/// The neutral verdict object as a source writes it, read closed: each word
/// with exactly the field `--format neutral` renders beside it, and no other.
#[derive(Debug, Deserialize)]
#[serde(tag = "verdict", rename_all = "lowercase", deny_unknown_fields)]
enum Answered {
    Block { reason: String },
    Warn { message: String },
    None {},
}

/// What a source's standard output says, or why it says nothing this verb can
/// use: one verdict object and nothing else, carrying exactly the field its
/// word documents, and that field saying something.
fn answer_of(stdout: &[u8]) -> Result<Answer, String> {
    let text = std::str::from_utf8(stdout)
        .map_err(|_| "it answered with bytes that are not text".to_owned())?
        .trim();
    if text.is_empty() {
        return Err("it exited 0 and wrote nothing on standard output".to_owned());
    }
    let outside = |why: &str| {
        let shown: String = text.chars().take(200).collect();
        format!(
            "it answered outside the vocabulary ({why}): `{shown}{}`",
            if shown.len() < text.len() { "…" } else { "" }
        )
    };
    // One object and nothing else, asked first because serde would read a
    // struct out of an array by position. The text itself is then read into
    // the closed type — not the parsed value, which would have kept only the
    // last of a key written twice.
    if !serde_json::from_str::<serde_json::Value>(text).is_ok_and(|value| value.is_object()) {
        return Err(outside("not one JSON object"));
    }
    let answered: Answered =
        serde_json::from_str(text).map_err(|error| outside(&error.to_string()))?;
    let said = |text: String, field: &str| {
        if text.trim().is_empty() {
            Err(outside(&format!("`{field}` is blank")))
        } else {
            Ok(text)
        }
    };
    match answered {
        Answered::Block { reason } => said(reason, "reason").map(Answer::Block),
        Answered::Warn { message } => said(message, "message").map(Answer::Warn),
        Answered::None {} => Ok(Answer::None),
    }
}

/// The input a source is handed. It is this verb's own neutral input rather
/// than the harness's payload, so a source reads one shape under every
/// `--format` and can be driven by hand with the same bytes.
fn source_input(asked: &Asked) -> String {
    json!({
        "session": asked.session.as_str(),
        "continuation": asked.stop == Stop::Continuation,
    })
    .to_string()
}

#[cfg(unix)]
fn shell(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.arg("-c").arg(command);
    shell
}

// llmlint: ignore-block[changed_behavior_has_e2e] this arm is `#[cfg(windows)]`, and the
// journeys that drive a declared source write it as a POSIX shell script, which `cmd` does
// not run; every ending past the spawn is the same code on both platforms and is driven by
// `tests/e2e/stop_guard.rs`. `raw_arg` is what hands `cmd /C` the command line unquoted, as
// a hook command line is written.
#[cfg(windows)]
fn shell(command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut shell = Command::new("cmd");
    shell.arg("/C").raw_arg(command);
    shell
}
// llmlint: ignore-end[changed_behavior_has_e2e]

/// Ask one declared source about this stop, and read its answer — or say, in
/// words that name what went wrong, why it could not be read.
///
/// The session is handed to it twice over and both say the same one: on
/// standard input, and as `ONEPIPELINE_LAUNCHER_SESSION`, which otherwise
/// carries whichever session launched the harness — for a dispatched worker,
/// its manager's. Its standard error is discarded, because this verb's own
/// carries exactly what `unwatched` would write and nothing else.
fn consult(command: &str, asked: &Asked, timeout: Duration) -> Result<Answer, String> {
    let mut spawning = shell(command);
    // Its own group, so what it leaves behind when it exits can still be ended
    // at the deadline, after descent from it is lost.
    crate::sys::in_own_process_group(&mut spawning);
    spawning
        .env(crate::sys::LAUNCHER_SESSION_ENV, asked.session.as_str())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = spawning
        .spawn()
        .map_err(|error| format!("it could not be started ({error})"))?;
    let mut input = source_input(asked);
    input.push('\n');
    // Written on a thread of its own, so a source that never reads its input
    // cannot hold this wait on a full pipe, and reported back rather than
    // discarded: a source that did not receive the whole of its input answered
    // without knowing whose stop it was, and nothing it says is taken.
    let (wrote, delivered) = std::sync::mpsc::channel();
    match child.stdin.take() {
        Some(mut stdin) => {
            std::thread::spawn(move || {
                let _ = wrote.send(stdin.write_all(input.as_bytes()));
            });
        }
        None => {
            let _ = wrote.send(Err(std::io::Error::other("no pipe to its standard input")));
        }
    }
    // Read on a thread too, and waited for no longer than the deadline: a
    // source that leaves something behind it holding standard output would
    // otherwise hold this stop open past its own exit.
    let (sent, answer) = std::sync::mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut stdout = stdout;
            // Past the limit the rest is drained rather than the pipe closed,
            // so an overlong answer is reported as that and not as the source
            // dying of a write nobody read.
            let read = (&mut stdout)
                .take(SOURCE_ANSWER_LIMIT + 1)
                .read_to_end(&mut bytes)
                .and_then(|_| std::io::copy(&mut stdout, &mut std::io::sink()))
                .map(|_| bytes);
            let _ = sent.send(read);
        });
    }
    // `--source-timeout` is bounded at the flag, so this is an instant the
    // clock holds; were it not, the stop is refused rather than left unbounded.
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| format!("its timeout of {timeout:?} is past what the clock can count"))?;
    let late = || format!("it did not answer within {} second(s)", timeout.as_secs());
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {}
            waited => {
                let _ = crate::sys::stop(child.id(), crate::sys::Stop::Now);
                end_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err(match waited {
                    Err(error) => format!("whether it had finished could not be read ({error})"),
                    Ok(_) => late(),
                });
            }
        }
        std::thread::sleep(crate::hooks::POLL);
    };
    if !status.success() {
        return Err(match status.code() {
            // What POSIX shells exit with for a command they could not find.
            Some(127) => {
                "it exited with status 127, the shell's status for a command it could not find"
                    .to_owned()
            }
            Some(code) => format!("it exited with status {code}"),
            None => "it was ended by a signal".to_owned(),
        });
    }
    // Asked after it exited, when every reader of the pipe has gone, so a
    // write it never took has by now been refused rather than left pending —
    // unless something it left behind still holds the pipe, which the deadline
    // bounds as it does the answer.
    delivered
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| {
            end_group(child.id());
            late()
        })?
        .map_err(|error| format!("its input could not be delivered to it in full ({error})"))?;
    let bytes = answer
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| {
            end_group(child.id());
            late()
        })?;
    // llmlint: ignore-block[changed_behavior_has_e2e] no source can provoke this: `read_to_end`
    // retries `EINTR`, and a pipe whose writer exits or is ended reads as end of file, not as
    // an error, so no script a journey runs reaches it. It is here so that an error the kernel
    // does raise still refuses the stop naming the source, rather than being read as an empty
    // answer — the `wrote nothing` refusal the journeys in `tests/e2e/stop_guard.rs` do drive.
    let bytes =
        bytes.map_err(|error| format!("its standard output could not be read ({error})"))?;
    // llmlint: ignore-end[changed_behavior_has_e2e]
    if bytes.len() as u64 > SOURCE_ANSWER_LIMIT {
        return Err(format!(
            "it answered more than {SOURCE_ANSWER_LIMIT} bytes, which is no verdict object"
        ));
    }
    answer_of(&bytes)
}

/// End what a source left running once descent from it is lost: a process
/// it started and then exited from is reparented, so its group is the one
/// handle left on it. `SIGKILL`, because this runs only past a deadline the
/// source was already given to finish in.
#[cfg(unix)]
fn end_group(leader: u32) {
    if let Ok(group) = i32::try_from(leader) {
        // SAFETY: `kill` takes no pointers; a negative pid names the process
        // group `consult` put the source in, whose id is the source's own pid,
        // and a group already gone is an error the result is discarded for.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
}

// llmlint: ignore-block[changed_behavior_has_e2e] Windows has no process group a signal can
// end; its `CREATE_NEW_PROCESS_GROUP` only keeps a console's Ctrl-C off the source. What a
// source leaves running there after it exits is out of reach, which `docs/stop-guard.md`
// states. The Unix arm is driven by the left-behind journey in `tests/e2e/stop_guard.rs`.
#[cfg(not(unix))]
fn end_group(_leader: u32) {}
// llmlint: ignore-end[changed_behavior_has_e2e]

/// One source's contribution to the verdict, under its own memory. Why a
/// failed consultation blocks rather than warns is `docs/stop-guard.md`'s
/// "Declared sources".
fn settle(asked: &Asked, command: &str, answer: Result<Answer, String>) -> Verdict {
    let session = asked.session.as_str();
    let name = source_memory(session, command);
    let report = match answer {
        Ok(Answer::Block(reason)) => {
            let mut report = format!("stop-guard: `{command}` refuses this stop:\n{reason}");
            if !report.ends_with('\n') {
                report.push('\n');
            }
            report
        }
        // The input shown is a first stop's whatever this stop is: the report
        // is what a continuation is compared against, so it must not move with
        // the one field that differs between a block and its continuation.
        Err(why) => format!(
            "stop-guard: `{command}` could not be consulted, so this stop is refused rather than \
             let through unanswered: {why}. Ask it by hand by handing it `{}` on standard input, \
             and fix or remove its `--source`.\n",
            source_input(&Asked {
                session: asked.session.clone(),
                stop: Stop::First,
            })
        ),
        Ok(answer) => {
            // Nothing to block on leaves no block for a continuation to
            // continue, so what was last blocked on goes.
            let forgot = forget(&name).err().map(|why| {
                Verdict::Warn(format!(
                    "stop-guard: `{command}` refuses nothing, but the guard could not remove what \
                     it last blocked on for it ({why}), so a later continuation over that same \
                     report would be let through unrefused; remove it by hand."
                ))
            });
            let said = match answer {
                Answer::Warn(message) => {
                    Verdict::Warn(format!("stop-guard: `{command}` warns: {message}"))
                }
                _ => Verdict::None,
            };
            return combined(forgot.into_iter().chain([said]).collect());
        }
    };
    let digest = hex(&Sha256::digest(report.as_bytes()));
    let aside = |because: String| {
        Verdict::Warn(format!(
            "stop-guard: `{command}` would refuse this stop and it was not refused, because the \
             guard {because}; what it would have refused on:\n{}",
            report.trim_end()
        ))
    };
    if asked.stop == Stop::Continuation {
        match remembered(&name) {
            Err(why) => return aside(format!("could not read what it last blocked on ({why})")),
            Ok(Some(last)) if last == digest => return Verdict::None,
            Ok(_) => {}
        }
    }
    match remember(&name, &digest) {
        Ok(()) => Verdict::Block(report),
        Err(why) => aside(format!("could not record what it would block on ({why})")),
    }
}

/// Every verdict of one stop as the one the harness receives: the strongest
/// wins, and nothing any of them said is dropped — every block's reason, in
/// the order declared, and every warning after them, since a block's rendering
/// has no second field to carry one in.
fn combined(verdicts: Vec<Verdict>) -> Verdict {
    let mut reasons: Option<String> = None;
    let mut warnings = Vec::new();
    for verdict in verdicts {
        match verdict {
            Verdict::Block(reason) => {
                let reasons = reasons.get_or_insert_with(String::new);
                if !reasons.is_empty() && !reasons.ends_with('\n') {
                    reasons.push('\n');
                }
                reasons.push_str(&reason);
            }
            Verdict::Warn(message) => warnings.push(message),
            Verdict::None => {}
        }
    }
    match reasons {
        Some(mut reasons) => {
            for warning in warnings {
                if !reasons.ends_with('\n') {
                    reasons.push('\n');
                }
                reasons.push_str(&warning);
                reasons.push('\n');
            }
            Verdict::Block(reasons)
        }
        None if !warnings.is_empty() => Verdict::Warn(warnings.join("\n")),
        None => Verdict::None,
    }
}

/// The warning for a question this guard could not ask.
fn unguarded(session: &str, error: &Error) -> Verdict {
    Verdict::Warn(format!(
        "stop-guard: this stop is unguarded, because whether a run this session owns is \
         unwatched could not be answered ({error}); ask it by hand with `onepipeline unwatched \
         --session {session}`."
    ))
}

/// The warning for runs that *are* unwatched over a memory this guard could
/// not keep: the runs, and why it refused nothing over them.
fn stood_aside(session: &str, report: &str, because: &str) -> Verdict {
    let lines: Vec<&str> = report.lines().collect();
    Verdict::Warn(format!(
        "stop-guard: {} run(s) this session owns are unwatched and this stop was not refused, \
         because whether it had already been refused for the same runs could not be answered \
         — the guard {because}; ask it by hand with `onepipeline unwatched --session {session}` \
         and arm `{ARM_A_WATCH} <run>` on each:\n{}",
        lines.len(),
        lines.join("\n")
    ))
}

/// The warning for a session with nothing unwatched whose memory this guard
/// could not remove: what was answered, what was not, and how to finish it.
fn unforgotten(session: &str, why: &str) -> Verdict {
    Verdict::Warn(format!(
        "stop-guard: nothing this session owns is unwatched, but the guard could not remove \
         what it last blocked on ({why}), so a later continuation over that same report would \
         be let through unrefused; remove it by hand, and ask again with `onepipeline unwatched \
         --session {session}`."
    ))
}

/// The file name the verb's own memory for `session` is kept under: a digest
/// of the session id rather than the id, which is a stranger's string arriving
/// on standard input and is never joined onto a path.
fn own_memory(session: &str) -> String {
    hex(&Sha256::digest(session.as_bytes()))
}

/// The file name a declared source's memory for `session` is kept under: the
/// session's own name and a digest of the source's command line, so each
/// source's continuation is held apart from the verb's and every other's.
fn source_memory(session: &str, command: &str) -> String {
    format!(
        "{}.{}",
        own_memory(session),
        hex(&Sha256::digest(command.as_bytes()))
    )
}

/// Where the memory file `name` is kept, under the state root.
///
/// `XDG_STATE_HOME` when it is set to an absolute path, else `~/.local/state`.
/// A relative `XDG_STATE_HOME` is ignored, as the specification says, and it is
/// a boundary check as well: a relative one would put the memory under whatever
/// directory the harness happened to run this in, and the runs root is a
/// relative default of exactly that shape.
fn memory_named(name: &str) -> Result<PathBuf, String> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let root = match state {
        Some(state) => state,
        None => home()
            .ok_or_else(|| {
                "neither XDG_STATE_HOME nor a home directory names a state root".to_owned()
            })?
            .join(".local")
            .join("state"),
    };
    Ok(root.join(MEMORY_DIR).join(name))
}

/// The home directory the state root is derived from when nothing names one.
///
/// Absolute only, on the same ground as `XDG_STATE_HOME`: a relative home would
/// put the memory under whatever directory the harness ran this in.
fn home() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .find(|path| path.is_absolute())
}

/// What the memory `name` last blocked on: `None` when nothing did, and an error
/// when it cannot say — which are different answers, because a memory that is
/// *absent* is a session nothing has blocked yet, which is safe to block, and
/// one that *cannot be read* leaves the guard unable to say whether the
/// condition moved.
fn remembered(name: &str) -> Result<Option<String>, String> {
    let path = memory_named(name)?;
    match std::fs::read(&path) {
        // Only what `remember` writes — one SHA-256 digest in lowercase hex — is
        // a memory; anything else is a record this guard cannot vouch for, and
        // comparing a report against it would be a guess.
        Ok(bytes) => String::from_utf8(bytes)
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .map(Some)
            .ok_or_else(|| format!("{}: not a digest this guard wrote", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Record what the guard is about to block on, or say why it could not.
fn remember(name: &str, digest: &str) -> Result<(), String> {
    let path = memory_named(name)?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    std::fs::write(&path, format!("{digest}\n"))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Drop what was remembered under `name`, this stop having nothing to block
/// on, or say why it could not be.
///
/// What the memory is *for* is telling a continuation whether the condition
/// moved, and a session with nothing unwatched leaves no block for the next
/// stop to continue — while a memory left behind would let a continuation over
/// the same report through should that report return. A memory that was never
/// written is already gone.
fn forget(name: &str) -> Result<(), String> {
    let path = memory_named(name)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The neutral rendering and the hook rendering are two presentations of
    /// one verdict: the same three verdicts, and the hook's "proceed" is
    /// silence.
    #[test]
    fn every_verdict_renders_under_every_format() {
        let block = Verdict::Block("run-1  ACTIVE  nothing has recorded a watch on it\n".into());
        let warn = Verdict::Warn("stop-guard: unguarded".into());
        for format in [Format::ClaudeCode, Format::Codex] {
            assert_eq!(
                block.render(format),
                "{\"decision\":\"block\",\"reason\":\"run-1  ACTIVE  nothing has recorded a watch on it\\n\"}\n"
            );
            assert_eq!(
                warn.render(format),
                "{\"systemMessage\":\"stop-guard: unguarded\"}\n"
            );
            assert_eq!(Verdict::None.render(format), "");
        }
        assert_eq!(
            block.render(Format::Neutral),
            "{\"verdict\":\"block\",\"reason\":\"run-1  ACTIVE  nothing has recorded a watch on it\\n\"}\n"
        );
        assert_eq!(
            warn.render(Format::Neutral),
            "{\"verdict\":\"warn\",\"message\":\"stop-guard: unguarded\"}\n"
        );
        assert_eq!(
            Verdict::None.render(Format::Neutral),
            "{\"verdict\":\"none\"}\n"
        );
    }

    /// What a source answers is read in exactly the vocabulary this verb
    /// renders: every verdict's neutral rendering reads back as the same
    /// answer, so the reader and the renderer cannot drift apart.
    #[test]
    fn every_neutral_rendering_reads_back_as_a_source_answer() {
        for (rendered, read) in [
            (
                Verdict::Block("run-1\n".into()),
                Answer::Block("run-1\n".into()),
            ),
            (Verdict::Warn("stale".into()), Answer::Warn("stale".into())),
            (Verdict::None, Answer::None),
        ] {
            assert_eq!(
                answer_of(rendered.render(Format::Neutral).as_bytes()),
                Ok(read),
                "{rendered:?}"
            );
        }
    }

    /// The hook payload reader takes the two names the harnesses share and
    /// ignores the rest of their payload.
    #[test]
    fn the_hook_payload_is_read_for_its_two_fields_and_the_rest_is_ignored() {
        let input: HookInput = serde_json::from_str(
            r#"{"session_id":"s-1","stop_hook_active":true,"transcript_path":"/t","cwd":"/c","hook_event_name":"Stop","last_assistant_message":null}"#,
        )
        .expect("a payload with more fields than this reads");
        assert_eq!(input.session_id.as_deref(), Some("s-1"));
        assert!(input.stop_hook_active);
        // And the neutral object is this verb's own, read closed: a field it
        // does not name is a caller that meant another verb's input.
        assert!(serde_json::from_str::<NeutralInput>(r#"{"session_id":"s-1"}"#).is_err());
        let neutral: NeutralInput =
            serde_json::from_str(r#"{"session":"s-1"}"#).expect("the neutral object reads");
        assert_eq!(neutral.session.as_deref(), Some("s-1"));
        assert!(!neutral.continuation);
    }

    /// A blank session, and one no argument vector could carry, name nobody.
    #[test]
    fn a_blank_session_names_nobody() {
        assert_eq!(named("   ".into(), false), None);
        assert_eq!(named("a\0b".into(), false), None);
        assert_eq!(
            named("s-1".into(), true),
            Some(Asked {
                session: Session("s-1".into()),
                stop: Stop::Continuation
            })
        );
    }

    /// The memory lives under an absolute `XDG_STATE_HOME` and never under a
    /// relative one.
    #[test]
    fn the_memory_is_keyed_by_a_digest_under_an_absolute_state_root() {
        let path = memory_named(&own_memory("session-x")).expect("a memory path");
        let name = path
            .file_name()
            .expect("a file name")
            .to_string_lossy()
            .into_owned();
        assert_eq!(name.len(), 64, "{name}");
        assert!(name.bytes().all(|byte| byte.is_ascii_hexdigit()), "{name}");
        assert!(
            path.to_string_lossy().contains(MEMORY_DIR),
            "{}",
            path.display()
        );
        assert!(path.is_absolute() || home().is_none(), "{}", path.display());
    }
}
