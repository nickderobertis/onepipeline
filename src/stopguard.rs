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
//! The verb is harness-neutral in and out. What it reads is a session and
//! whether this stop continues a block it made; what it answers is one verdict.
//! A harness's payload and decision shape are a *rendering* of that, chosen by
//! [`Format`], and the decision is made in [`guard`] alone.

use std::io::Read;
use std::path::{Path, PathBuf};

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

/// What this verb was asked: whose stop, and whether it continues a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asked {
    /// The session whose stop this is.
    pub session: String,
    /// Whether this stop follows a block this guard made.
    pub continuation: bool,
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
#[derive(Debug, Deserialize)]
struct HookInput {
    session_id: Option<String>,
    #[serde(default)]
    stop_hook_active: bool,
}

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
            (Format::ClaudeCode | Format::Codex, Self::Block(reason)) => {
                json!({"decision": "block", "reason": reason})
            }
            (Format::ClaudeCode | Format::Codex, Self::Warn(message)) => {
                json!({"systemMessage": message})
            }
            (Format::ClaudeCode | Format::Codex, Self::None) => return String::new(),
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
    (!session.trim().is_empty() && !session.contains('\0')).then_some(Asked {
        session,
        continuation,
    })
}

/// Decide one stop: ask `unwatched` about the session and answer with one
/// verdict, remembering a block before it is made.
///
/// `unresolved` is what the ordinary verb would have written on standard error
/// for the same question, handed back so the caller writes exactly that and
/// nothing else there.
pub(crate) fn guard(root: &Path, asked: &Asked) -> (Verdict, Vec<String>) {
    let unwatched = match crate::unwatched::unwatched(root, &asked.session) {
        Ok(unwatched) => unwatched,
        Err(error) => {
            forget(&asked.session);
            return (unguarded(&asked.session, &error), Vec::new());
        }
    };
    if unwatched.reported.is_empty() {
        forget(&asked.session);
        return (Verdict::None, unwatched.unresolved);
    }
    // The verb's own lines, byte for byte: they are what name the runs to watch
    // and the command that watches each, and nothing here reads a run out of
    // them.
    let report = crate::verbs::render_unwatched(&unwatched);
    let unresolved = unwatched.unresolved;
    let digest = hex(&Sha256::digest(report.as_bytes()));
    if asked.continuation {
        match remembered(&asked.session) {
            Err(why) => {
                return (
                    stood_aside(
                        &asked.session,
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
    if let Err(why) = remember(&asked.session, &digest) {
        return (
            stood_aside(
                &asked.session,
                &report,
                &format!("could not record what it would block on ({why})"),
            ),
            unresolved,
        );
    }
    (Verdict::Block(report), unresolved)
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

/// Where this session's memory is kept: one file per session under the state
/// root, named by a digest of the session id rather than by the id, which is a
/// stranger's string arriving on standard input and is never joined onto a
/// path.
///
/// `XDG_STATE_HOME` when it is set to an absolute path, else `~/.local/state`.
/// A relative `XDG_STATE_HOME` is ignored, as the specification says, and it is
/// a boundary check as well: a relative one would put the memory under whatever
/// directory the harness happened to run this in, and the runs root is a
/// relative default of exactly that shape.
fn memory(session: &str) -> Result<PathBuf, String> {
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
    Ok(root
        .join(MEMORY_DIR)
        .join(hex(&Sha256::digest(session.as_bytes()))))
}

/// The home directory the state root is derived from when nothing names one.
fn home() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|key| std::env::var_os(key).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

/// What this last blocked `session` on: `None` when nothing did, and an error
/// when it cannot say — which are different answers, because a memory that is
/// *absent* is a session nothing has blocked yet, which is safe to block, and
/// one that *cannot be read* leaves the guard unable to say whether the
/// condition moved.
fn remembered(session: &str) -> Result<Option<String>, String> {
    let path = memory(session)?;
    match std::fs::read(&path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(|text| Some(text.trim().to_owned()))
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Record what the guard is about to block on, or say why it could not.
fn remember(session: &str, digest: &str) -> Result<(), String> {
    let path = memory(session)?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    std::fs::write(&path, format!("{digest}\n"))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Drop what was remembered for `session`, this stop having nothing to block
/// on.
///
/// Every ending that is not a block, because what the memory is *for* is
/// telling a continuation whether the condition moved, and a turn that ended
/// without blocking leaves no block for the next one to continue. A memory that
/// cannot be removed changes nothing: the next ordinary stop is not a
/// continuation, so it blocks on what it finds whether or not anything was
/// remembered.
fn forget(session: &str) {
    if let Ok(path) = memory(session) {
        let _ = std::fs::remove_file(path);
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
                session: "s-1".into(),
                continuation: true
            })
        );
    }

    /// The memory lives under an absolute `XDG_STATE_HOME` and never under a
    /// relative one.
    #[test]
    fn the_memory_is_keyed_by_a_digest_under_an_absolute_state_root() {
        let path = memory("session-x").expect("a memory path");
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
