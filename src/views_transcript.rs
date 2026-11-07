//! How a relayed turn record's content is resolved and described.
//!
//! `oneagentgraph` stopped copying a turn's content into its records once
//! history is on: a `turn-started`, `turn-message` or `turn-activity` carries a
//! `transcript` reference into the oneharness history file the turn was
//! recorded in, and none of the bounded copies beside it. Every view this crate
//! draws a turn's content on reads it through [`resolve`], so `monitor`,
//! `watch`, `transcript`, `status` and note confirmation say the same thing
//! about the same record. A record that carries a bounded copy and no reference
//! — a journal written before references, or a turn with history off — goes
//! through the same call and answers that copy with its cut flag kept.
//!
//! The resolution rules are `oneagentgraph`'s, stated in its
//! `docs/contract.md`: an activity is the `type: event` line whose `run_id` is
//! the reference's `history_id` and whose `event.index` is its `index`; an
//! instruction is the `type: run-started` line with that `history_id`; an
//! assistant message is the closing `type: run` line's `text`. The `history_id`
//! is always matched — `index` restarts at 0 for every fallback candidate
//! written into the same file, so an index alone names two entries.

use std::io::BufRead;
use std::path::PathBuf;

use oneagentgraph::event::{ActivityTranscript, Transcript};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The widest a [`Resolved::description`] is, in characters.
pub const DESCRIPTION_MAX_CHARS: usize = 120;

/// What one turn record says its content is: a reference into a history file,
/// or the bounded copy a record without one carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[non_exhaustive]
pub enum TranscriptRef {
    /// A `turn-started`'s reference: the candidate's prompt.
    Instruction(Transcript),
    /// A `turn-message`'s reference: the candidate's closing text.
    Message(Transcript),
    /// A supervisor's `turn-message` (`role: user`): the next instruction its
    /// decision carried. The candidate's closing text is the whole decision
    /// document onejudge parsed, and what the supervisor *said* is the
    /// `message` that decision handed the agent — the text a copy carried.
    Decision(Transcript),
    /// A `turn-activity`'s reference: one event of the candidate.
    Activity(ActivityTranscript),
    /// A record carrying a bounded copy and no reference.
    Copy {
        /// The copy, as the record carried it.
        text: Text,
        /// Whether the producer cut the copy to its bound.
        cut: bool,
    },
}

/// The whole of one entry's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Text {
    /// The message a turn was opened with.
    Instruction(String),
    /// A party's own words: a `turn-message`, or a `message` activity.
    Message(String),
    /// The agent's `reasoning` activity.
    Reasoning(String),
    /// A tool call, or the observation that answered one.
    Tool {
        /// Which of the two it is.
        kind: ToolKind,
        /// The tool, where the harness named one.
        name: Option<String>,
        /// The call's input as the harness recorded it: a structured value, or
        /// the text a bounded copy summarized it as.
        input: Option<Value>,
        /// What the tool returned, where the entry carries it.
        output: Option<String>,
    },
}

/// Which half of a tool exchange an entry is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ToolKind {
    /// `tool_call`: the model invoked a tool.
    Call,
    /// `tool_result`: the observation returned to it.
    Result,
    /// A kind oneharness added after this build, kept as it spelled it: its
    /// `kind` is an open vocabulary, and an entry of a new kind is still a
    /// tool's entry to show rather than a reason to show nothing.
    Other(String),
}

impl ToolKind {
    fn of(kind: &str) -> Self {
        match kind {
            "tool_call" => Self::Call,
            "tool_result" => Self::Result,
            other => Self::Other(other.to_string()),
        }
    }
}

/// One entry's content, whole, and one line describing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The entry's whole text — or, for a bounded copy, the copy.
    pub text: Text,
    /// One line of at most [`DESCRIPTION_MAX_CHARS`] characters: a shell tool's
    /// command's first line, a file tool's verb and path, a search tool's
    /// pattern, any other tool's name and first argument, a message's first
    /// line.
    pub description: String,
    /// Whether the text is a copy its producer cut short. Never set for text
    /// read out of a history file, which holds it whole.
    pub cut: bool,
}

/// Why a reference resolved to nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Why {
    /// No history file at the reference's address.
    MissingFile,
    /// The file holds no entry the reference names.
    MissingEntry,
    /// The reference, or the file it names, could not be read.
    Unreadable,
}

impl Why {
    /// This answer's spelling on a rendered line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingFile => "missing-file",
            Self::MissingEntry => "missing-entry",
            Self::Unreadable => "unreadable",
        }
    }
}

impl std::fmt::Display for Why {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(self.as_str())
    }
}

/// A reference that did not resolve: the file it names, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    /// The history file the reference names.
    pub path: PathBuf,
    /// Why nothing was read from it.
    pub why: Why,
}

/// `transcript unavailable: <path>: <why>` — how every view renders a
/// reference it could not resolve, so none of them renders one as a blank.
impl std::fmt::Display for Unresolved {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "transcript unavailable: {}: {}",
            self.path.display(),
            self.why
        )
    }
}

impl std::error::Error for Unresolved {}

impl TranscriptRef {
    /// What a relayed record of `kind` says its content is: `None` for a kind
    /// that carries no turn content, `Err` for a reference this build cannot
    /// read.
    ///
    /// # Errors
    ///
    /// [`Unresolved`] with [`Why::Unreadable`] for a `transcript` that is not
    /// the shape `oneagentgraph` writes.
    pub fn of(kind: &str, payload: &Map<String, Value>) -> Option<Result<Self, Unresolved>> {
        if !matches!(kind, "turn-started" | "turn-message" | "turn-activity") {
            return None;
        }
        Some(match payload.get("transcript") {
            Some(reference) => {
                let read = match kind {
                    "turn-started" => {
                        serde_json::from_value(reference.clone()).map(Self::Instruction)
                    }
                    "turn-message"
                        if payload.get("role").and_then(Value::as_str) == Some("user") =>
                    {
                        serde_json::from_value(reference.clone()).map(Self::Decision)
                    }
                    "turn-message" => serde_json::from_value(reference.clone()).map(Self::Message),
                    _ => serde_json::from_value(reference.clone()).map(Self::Activity),
                };
                read.map_err(|_| Unresolved {
                    path: address(reference),
                    why: Why::Unreadable,
                })
            }
            None => Self::copy(kind, payload),
        })
    }

    // llmlint: ignore-block[changed_behavior_has_e2e] this module's public interface is
    // the library call `resolve` itself, and its tests below drive that call over real
    // history files — one worker session recorded on this host, and sessions oneharness's
    // own writer writes — so each variant is proven at the interface rather than by a
    // stand-in. What reaches it through the CLI is proven there too: `tests/e2e/
    // transcript_refs.rs` drives `monitor`, `watch`, `status` and `transcript` over a real
    // dispatch for a shell call's description, whole input, output and prompt, and for a
    // missing file and a missing entry. The rest — file, search, argv and unknown-tool
    // descriptions, the bound, a fallback candidate's repeated index, a legacy closing
    // prompt, half-written and wrongly typed lines, a malformed copy — no real dispatch in
    // this suite can produce: its harness double runs one shell tool, writes the current
    // history schema whole, and the sibling writes only well-formed records.
    /// A record's bounded copy, held to the shape its producer writes: every
    /// text a string where present, every flag a boolean. A copy that is not is
    /// unreadable rather than coerced — read as text, an object would render as
    /// though the producer had said it, and a flag read as `false` would claim a
    /// copy whole that nobody said was.
    fn copy(kind: &str, payload: &Map<String, Value>) -> Result<Self, Unresolved> {
        let unreadable = || Unresolved {
            path: PathBuf::from(format!("<{kind} copy>")),
            why: Why::Unreadable,
        };
        // Omitted is the producer saying nothing; anything present has to be
        // the type it writes. `name` alone is written `null`, on a result,
        // which names no tool.
        let text = |key: &str| match payload.get(key) {
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(Value::Null) if key == "name" => Ok(None),
            None => Ok(None),
            Some(_) => Err(unreadable()),
        };
        let flag = |key: &str| match payload.get(key) {
            Some(Value::Bool(flag)) => Ok(*flag),
            None => Ok(false),
            Some(_) => Err(unreadable()),
        };
        Ok(match kind {
            "turn-started" => Self::Copy {
                text: Text::Instruction(text("instruction")?.unwrap_or_default()),
                cut: flag("instruction_truncated")?,
            },
            "turn-message" => Self::Copy {
                text: Text::Message(text("text")?.unwrap_or_default()),
                cut: flag("truncated")?,
            },
            _ => {
                let activity = text("kind")?.ok_or_else(unreadable)?;
                let output = text("output")?;
                let cut = flag("output_truncated")?;
                let detail_cut = flag("truncated")?;
                match activity.as_str() {
                    "message" => Self::Copy {
                        text: Text::Message(output.unwrap_or_default()),
                        cut,
                    },
                    "reasoning" => Self::Copy {
                        text: Text::Reasoning(output.unwrap_or_default()),
                        cut,
                    },
                    _ => {
                        let kind = ToolKind::of(&activity);
                        Self::Copy {
                            cut: cut || (kind == ToolKind::Call && detail_cut),
                            text: Text::Tool {
                                name: text("name")?,
                                input: text("detail")?
                                    .filter(|detail| !detail.is_empty())
                                    .map(Value::String),
                                output,
                                kind,
                            },
                        }
                    }
                }
            }
        })
    }

    /// Whether this is a reference into a history file rather than a copy.
    #[must_use]
    pub fn is_reference(&self) -> bool {
        !matches!(self, Self::Copy { .. })
    }
}

/// Resolve one record's content, and describe it.
///
/// A reference is read out of its history file through the linked
/// `oneharness-core`'s own lookup, and a file still being written is read up to
/// its last complete line. A bounded copy answers itself, cut flag kept.
///
/// # Errors
///
/// [`Unresolved`] naming the file and why: [`Why::MissingFile`] where no file
/// is at the address, [`Why::MissingEntry`] where the file holds no entry the
/// reference names, and [`Why::Unreadable`] where the address or the file could
/// not be read.
pub fn resolve(reference: &TranscriptRef) -> Result<Resolved, Unresolved> {
    let (location, index) = match reference {
        TranscriptRef::Copy { text, cut } => {
            return Ok(Resolved {
                description: describe(text),
                text: text.clone(),
                cut: *cut,
            })
        }
        TranscriptRef::Instruction(location)
        | TranscriptRef::Message(location)
        | TranscriptRef::Decision(location) => (location, None),
        TranscriptRef::Activity(activity) => (&activity.location, Some(activity)),
    };
    let path = PathBuf::from(&location.history_dir)
        .join(&location.history_project)
        .join(format!("{}.jsonl", location.history_session));
    let unresolved = |why| Unresolved {
        path: path.clone(),
        why,
    };
    let found = oneharness_core::io::history::find_session_path(
        std::path::Path::new(&location.history_dir),
        Some(&location.history_project),
        &location.history_session,
    );
    let (found, file) = match found {
        Ok(Some(found)) => {
            let file = std::fs::File::open(&found).map_err(|error| {
                unresolved(if error.kind() == std::io::ErrorKind::NotFound {
                    Why::MissingFile
                } else {
                    Why::Unreadable
                })
            })?;
            (found, file)
        }
        // The lookup answers nothing for an address that names no plain file
        // under the store as well as for one that is absent: the first is a
        // reference this build cannot follow, the second a file that is gone.
        Ok(None) if plain(&location.history_project) && plain(&location.history_session) => {
            return Err(unresolved(if path.exists() {
                Why::Unreadable
            } else {
                Why::MissingFile
            }));
        }
        Ok(None) | Err(_) => return Err(unresolved(Why::Unreadable)),
    };
    let history_id = location.history_id.to_string();
    if let Some(wanted) = index {
        return session_index::of_file(&found, file)
            .ok_or_else(|| unresolved(Why::Unreadable))?
            .activity(&history_id, wanted, &unresolved);
    }
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    let mut unreadable = false;
    let mut prompt_at_close = None;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return Err(unresolved(Why::Unreadable)),
        }
        // The last line of a file still being written may be half of one: it
        // is not an entry yet, and not a reason to call the file unreadable —
        // nor one to read, however much of it happens to parse.
        if !line.ends_with('\n') {
            break;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line.trim_end()) else {
            unreadable = true;
            continue;
        };
        let field = |key: &str| entry.get(key).and_then(Value::as_str);
        // A text an entry has to carry: a string, or the entry is unreadable.
        // `null` only where the history schema allows it — a run that closed
        // without saying anything.
        let carried = |key: &str, nullable: bool| match entry.get(key) {
            Some(Value::String(text)) => Ok(text.clone()),
            Some(Value::Null) if nullable => Ok(String::new()),
            _ => Err(unresolved(Why::Unreadable)),
        };
        match (reference, field("type")) {
            (TranscriptRef::Instruction(_), Some("run-started"))
                if field("history_id") == Some(&history_id) =>
            {
                return Ok(described(Text::Instruction(carried("prompt", false)?)));
            }
            // A closing line carries the prompt too, and is the only line that
            // does in a file written before openings were.
            (TranscriptRef::Instruction(_), Some("run"))
                if field("history_id") == Some(&history_id) =>
            {
                prompt_at_close = Some(carried("prompt", false)?);
            }
            (TranscriptRef::Message(_), Some("run"))
                if field("history_id") == Some(&history_id) =>
            {
                return Ok(described(Text::Message(carried("text", true)?)));
            }
            // Read by onejudge's own parser, the one that handed the agent this
            // message: only a decision to continue carries one, so any other
            // answer is no message this record could have been.
            (TranscriptRef::Decision(_), Some("run"))
                if field("history_id") == Some(&history_id) =>
            {
                return match onejudge::parse_supervisor(&carried("text", true)?) {
                    onejudge::SupervisorOutcome::Continue { message, .. } => {
                        Ok(described(Text::Message(message)))
                    }
                    _ => Err(unresolved(Why::Unreadable)),
                };
            }
            _ => {}
        }
    }
    if let Some(prompt) = prompt_at_close {
        return Ok(described(Text::Instruction(prompt)));
    }
    Err(unresolved(if unreadable {
        Why::Unreadable
    } else {
        Why::MissingEntry
    }))
}

/// Read a record's content straight off its payload: [`TranscriptRef::of`]
/// then [`resolve`], with a malformed reference answered as unresolved.
#[must_use]
pub fn of_record(kind: &str, payload: &Map<String, Value>) -> Option<Result<Resolved, Unresolved>> {
    TranscriptRef::of(kind, payload).map(|reference| reference.and_then(|r| resolve(&r)))
}

/// Each session file's activity entries, from one parse of the file.
///
/// A dispatch's transcript holds thousands of references into a few files, and
/// answering each by parsing its file from the top was most of what reading a long
/// dispatch cost. What is kept is the file's text and, per `(history id, index)`, the
/// lines holding that entry, so what stays in memory is bounded by the file itself
/// rather than by a parsed tree several times its size.
///
/// Process-wide, because `onepipeline-ui` reads the same files for every request, and
/// bounded by [`SESSIONS`]: the sessions used least recently go first. An entry stands
/// only while its file's [`Stamp`](crate::appendstamp::Stamp) does, so a file that grew
/// or changed is read again, and a removed one never reaches here.
mod session_index {
    use std::collections::{HashMap, HashSet};
    use std::io::Read as _;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use serde_json::Value;

    use super::{described, ActivityTranscript, Resolved, Text, ToolKind, Unresolved, Why};
    use crate::appendstamp::Stamp;

    /// How many session files are held.
    pub(super) const SESSIONS: usize = 256;

    #[derive(Debug)]
    pub(super) struct SessionFile {
        text: String,
        /// Where each entry's lines are in `text`, in the file's order.
        entries: HashMap<(String, u64), Vec<(usize, usize)>>,
        /// A whole line of the file did not parse, which makes a miss an unreadable
        /// file rather than a missing entry: it may have been the entry asked for.
        malformed: bool,
        /// The file stopped being readable as text here: nothing after it was seen.
        cut_short: bool,
        /// History ids holding an entry whose index is no integer, which may be the
        /// one asked for and cannot be told apart.
        indexless: HashSet<String>,
    }

    impl SessionFile {
        fn of(bytes: Vec<u8>) -> Self {
            // Up to a line that is not text, as a line-by-line read of the file stops
            // there, and only ever through finished lines: the last line of a file
            // still being written may be half of one, however much of it parses.
            let mut cut_short = false;
            let mut valid = match std::str::from_utf8(&bytes) {
                Ok(_) => bytes.len(),
                Err(error) => {
                    cut_short = true;
                    error.valid_up_to()
                }
            };
            valid = bytes[..valid]
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |newline| newline + 1);
            let mut bytes = bytes;
            bytes.truncate(valid);
            let text = String::from_utf8(bytes).unwrap_or_default();
            let mut session = Self {
                text,
                entries: HashMap::new(),
                malformed: false,
                cut_short,
                indexless: HashSet::new(),
            };
            let mut start = 0;
            let mut lines = Vec::new();
            for line in session.text.split_inclusive('\n') {
                lines.push((start, start + line.len()));
                start += line.len();
            }
            for (from, to) in lines {
                session.index_line(from, to);
            }
            session
        }

        fn index_line(&mut self, from: usize, to: usize) {
            let Ok(entry) = serde_json::from_str::<Value>(self.text[from..to].trim_end()) else {
                self.malformed = true;
                return;
            };
            if entry.get("type").and_then(Value::as_str) != Some("event") {
                return;
            }
            let Some(run) = entry.get("run_id").and_then(Value::as_str) else {
                return;
            };
            let raw = entry.get("event").unwrap_or(&Value::Null);
            match raw.get("index").and_then(Value::as_u64) {
                Some(at) => self
                    .entries
                    .entry((run.to_owned(), at))
                    .or_default()
                    .push((from, to)),
                None => {
                    self.indexless.insert(run.to_owned());
                }
            }
        }

        /// What this file answers for an activity reference.
        ///
        /// In the file's order, the first entry of that index whose tool call is the
        /// one asked for: an entry naming another call is not this one, and an entry
        /// of that index that does not read as an event makes the file unreadable.
        pub(super) fn activity(
            &self,
            history_id: &str,
            wanted: &ActivityTranscript,
            unresolved: &dyn Fn(Why) -> Unresolved,
        ) -> Result<Resolved, Unresolved> {
            let lines = self
                .entries
                .get(&(history_id.to_owned(), wanted.index))
                .map_or(&[][..], Vec::as_slice);
            for (from, to) in lines {
                let raw = serde_json::from_str::<Value>(self.text[*from..*to].trim_end())
                    .ok()
                    .and_then(|mut entry| entry.get_mut("event").map(Value::take))
                    .unwrap_or(Value::Null);
                // Read through oneharness's own event type, so a field of the wrong
                // type is an unreadable entry rather than empty content.
                let event: oneharness_core::domain::events::ActionEvent =
                    serde_json::from_value(raw).map_err(|_| unresolved(Why::Unreadable))?;
                if let (Some(expected), Some(found)) = (
                    wanted.tool_call_id.as_deref(),
                    event.tool_call_id.as_deref(),
                ) {
                    if expected != found {
                        continue;
                    }
                }
                return Ok(described(match event.kind.as_str() {
                    "message" => Text::Message(event.output.unwrap_or_default()),
                    "reasoning" => Text::Reasoning(event.output.unwrap_or_default()),
                    kind => Text::Tool {
                        kind: ToolKind::of(kind),
                        name: event.name,
                        input: event.input.filter(|input| !input.is_null()),
                        output: event.output,
                    },
                }));
            }
            Err(unresolved(
                if self.malformed || self.cut_short || self.indexless.contains(history_id) {
                    Why::Unreadable
                } else {
                    Why::MissingEntry
                },
            ))
        }
    }

    #[cfg(test)]
    pub(super) fn held() -> usize {
        let held = HELD.lock().unwrap_or_else(|held| held.into_inner());
        held.as_ref().map_or(0, |(files, _)| files.len())
    }

    /// One held file: what it was stamped when read, and when it was last used.
    type Held = (Stamp, Arc<SessionFile>, u64);

    /// The held files, and the clock their use is ordered by.
    static HELD: Mutex<Option<(HashMap<PathBuf, Held>, u64)>> = Mutex::new(None);

    /// The file at `path`, open as `file`, as it stands now: `None` where it cannot be
    /// read.
    ///
    /// The stamp is taken before the read, so a file that changes during it is read
    /// again next time rather than held as what it no longer is. The lock is not held
    /// across the parse, which is the one slow step: two readers of one file may both
    /// parse it, and neither makes the other wait behind an unrelated file.
    pub(super) fn of_file(path: &Path, mut file: std::fs::File) -> Option<Arc<SessionFile>> {
        let stamp = Stamp::read(path)?;
        {
            let mut held = HELD.lock().unwrap_or_else(|held| held.into_inner());
            let (files, clock) = held.get_or_insert_with(Default::default);
            *clock += 1;
            if let Some((held_stamp, session, used)) = files.get_mut(path) {
                if *held_stamp == stamp {
                    *used = *clock;
                    return Some(Arc::clone(session));
                }
            }
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).ok()?;
        let session = Arc::new(SessionFile::of(bytes));
        let mut held = HELD.lock().unwrap_or_else(|held| held.into_inner());
        let (files, clock) = held.get_or_insert_with(Default::default);
        *clock += 1;
        files.insert(path.to_path_buf(), (stamp, Arc::clone(&session), *clock));
        while files.len() > SESSIONS {
            let Some(oldest) = files
                .iter()
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            files.remove(&oldest);
        }
        Some(session)
    }
}

fn described(text: Text) -> Resolved {
    Resolved {
        description: describe(&text),
        text,
        cut: false,
    }
}

/// Whether a value can name one file directly under a directory — the same
/// test the linked lookup applies before it opens anything.
fn plain(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.contains(['/', '\\', ':'])
}

/// The file a reference names, as far as its fields can be read.
fn address(reference: &Value) -> PathBuf {
    let part = |key: &str| reference.get(key).and_then(Value::as_str).unwrap_or("?");
    PathBuf::from(part("history_dir"))
        .join(part("history_project"))
        .join(format!("{}.jsonl", part("history_session")))
}

impl Text {
    /// The whole of what a tool call acted on, as text: a string input as it
    /// is, a structured one as its JSON.
    #[must_use]
    pub fn input_text(&self) -> Option<String> {
        match self {
            Self::Tool {
                input: Some(input), ..
            } => Some(match input {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            }),
            _ => None,
        }
    }
}

/// One line describing `text`, at most [`DESCRIPTION_MAX_CHARS`] characters.
fn describe(text: &Text) -> String {
    let line = match text {
        Text::Instruction(said) | Text::Message(said) | Text::Reasoning(said) => {
            first_line(said).to_string()
        }
        Text::Tool {
            kind,
            name,
            input,
            output,
        } => {
            if *kind == ToolKind::Result {
                first_line(output.as_deref().unwrap_or_default()).to_string()
            } else {
                describe_call(name.as_deref(), input.as_ref())
            }
        }
    };
    bounded(&line)
}

/// A tool call, by what kind of tool it is.
fn describe_call(name: Option<&str>, input: Option<&Value>) -> String {
    let name = name.unwrap_or("tool");
    let lowered = name.to_ascii_lowercase();
    let object = input.and_then(Value::as_object);
    let field =
        |keys: &[&str]| object.and_then(|object| keys.iter().find_map(|key| object.get(*key)));
    // A shell tool: the command it ran, its wrapper taken off.
    if let Some(command) = field(&["command", "cmd"]).and_then(command_text) {
        return first_line(&command).to_string();
    }
    let shell = matches!(
        lowered.as_str(),
        "bash"
            | "shell"
            | "exec_command"
            | "local_shell"
            | "command_execution"
            | "run_shell_command"
    );
    if let (true, Some(Value::String(command))) = (shell, input) {
        return first_line(&unwrap_shell(command)).to_string();
    }
    // A search tool: its pattern.
    let search = matches!(
        lowered.as_str(),
        "grep" | "glob" | "search" | "rg" | "find" | "search_files" | "file_search"
    );
    let pattern = match field(&["pattern"]) {
        Some(pattern) => Some(pattern),
        None if search => field(&["query", "regex"]),
        None => None,
    };
    if let Some(pattern) = pattern.and_then(Value::as_str) {
        return first_line(pattern).to_string();
    }
    // A file tool: its verb and the path.
    if let Some(path) =
        field(&["file_path", "path", "notebook_path", "filePath"]).and_then(Value::as_str)
    {
        return format!("{name} {}", first_line(path));
    }
    if let Some(changes) = field(&["changes"]) {
        let paths: Vec<String> = match changes {
            Value::Array(changes) => changes
                .iter()
                .filter_map(|change| change.get("path").and_then(Value::as_str))
                .map(str::to_string)
                .collect(),
            Value::Object(changes) => changes.keys().cloned().collect(),
            _ => Vec::new(),
        };
        if let Some(first) = paths.first() {
            let more = match paths.len() {
                1 => String::new(),
                n => format!(" (+{} more)", n - 1),
            };
            return format!("{name} {}{more}", first_line(first));
        }
    }
    // Anything else: its name and its first argument.
    let argument = match input {
        Some(Value::Object(object)) => object.values().next().map(|value| match value {
            Value::String(text) => first_line(text).to_string(),
            other => other.to_string(),
        }),
        Some(Value::String(text)) => Some(first_line(text).to_string()),
        Some(Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    };
    match argument.filter(|argument| !argument.is_empty()) {
        Some(argument) => format!("{name} {argument}"),
        None => name.to_string(),
    }
}

/// The command a call ran: a string, or an argv list.
fn command_text(command: &Value) -> Option<String> {
    match command {
        Value::String(text) => Some(unwrap_shell(text)),
        Value::Array(words) => {
            let words: Vec<&str> = words.iter().map(Value::as_str).collect::<Option<_>>()?;
            match words.as_slice() {
                [shell, flag, script] if is_shell(shell) && is_command_flag(flag) => {
                    Some((*script).to_string())
                }
                words => Some(words.join(" ")),
            }
        }
        _ => None,
    }
}

/// `/usr/bin/bash -lc 'cat x'` → `cat x`: the harness's wrapper around what
/// the agent asked to run. Anything not plainly that is left as it was.
fn unwrap_shell(command: &str) -> String {
    let trimmed = command.trim_start();
    let mut words = trimmed.splitn(3, char::is_whitespace);
    let (Some(shell), Some(flag), Some(script)) = (words.next(), words.next(), words.next()) else {
        return command.to_string();
    };
    if !is_shell(shell) || !is_command_flag(flag) {
        return command.to_string();
    }
    let script = script.trim();
    for quote in ['\'', '"'] {
        if let Some(inner) = script
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.to_string();
        }
    }
    script.to_string()
}

fn is_shell(word: &str) -> bool {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    matches!(
        base,
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "pwsh"
    )
}

fn is_command_flag(word: &str) -> bool {
    matches!(word, "-c" | "-lc" | "-cl")
}

/// The first line of `text` that says anything, trimmed.
fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// `line` with every control character flattened, cut to
/// [`DESCRIPTION_MAX_CHARS`] with an ellipsis where it was cut.
fn bounded(line: &str) -> String {
    let flat: String = line
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if flat.chars().count() <= DESCRIPTION_MAX_CHARS {
        return flat;
    }
    let mut cut: String = flat.chars().take(DESCRIPTION_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}
// llmlint: ignore-end[changed_behavior_has_e2e]

#[cfg(test)]
mod tests {
    use super::*;
    use oneharness_core::domain::events::ActionEvent;
    use oneharness_core::domain::history::HistoryLabels;
    use oneharness_core::io::history::HistoryWriter;
    use serde_json::json;

    /// The store the recorded session sits in: one real worker turn of this
    /// host's, trimmed and with its paths redacted, whose first candidate fell
    /// through on a quota and whose second ran — so index 0 names an entry of
    /// each.
    fn recorded_store() -> String {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/transcript/history"
        )
        .to_string()
    }

    const RECORDED_PROJECT: &str = "home-operator-worktree";
    const RECORDED_SESSION: &str = "node-scope-recorded-worker-skill";
    const FELL_THROUGH: &str = "01a13e8e-1251-7680-b71d-ee89e3a3dc97";
    const RAN: &str = "01a13e8e-594b-7761-b9bb-7b5975a478e5";

    fn location(dir: &str, project: &str, session: &str, history_id: &str) -> Value {
        json!({
            "history_dir": dir,
            "history_project": project,
            "history_session": session,
            "history_id": history_id,
        })
    }

    fn recorded(history_id: &str) -> Value {
        location(
            &recorded_store(),
            RECORDED_PROJECT,
            RECORDED_SESSION,
            history_id,
        )
    }

    /// A turn record of `kind` carrying `reference`, as the sibling relays one.
    fn record(reference: Value, extra: &[(&str, Value)]) -> Map<String, Value> {
        let mut payload = Map::new();
        payload.insert("transcript".into(), reference);
        for (key, value) in extra {
            payload.insert((*key).to_string(), value.clone());
        }
        payload
    }

    fn activity(reference: Value, index: u64) -> Map<String, Value> {
        let mut reference = reference;
        reference["index"] = json!(index);
        record(reference, &[("index", json!(index))])
    }

    fn resolved(kind: &str, payload: &Map<String, Value>) -> Resolved {
        match of_record(kind, payload) {
            Some(Ok(resolved)) => resolved,
            other => panic!("{kind} did not resolve: {other:?}"),
        }
    }

    fn unresolved(kind: &str, payload: &Map<String, Value>) -> Unresolved {
        match of_record(kind, payload) {
            Some(Err(unresolved)) => unresolved,
            other => panic!("{kind} resolved where it should not have: {other:?}"),
        }
    }

    /// What the recorded session's candidate that ran holds as one event's
    /// output, read straight off the fixture.
    fn recorded_output(index: u64) -> String {
        let path = PathBuf::from(recorded_store())
            .join(RECORDED_PROJECT)
            .join(format!("{RECORDED_SESSION}.jsonl"));
        std::fs::read_to_string(path)
            .expect("the fixture reads")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a fixture line parses"))
            .find(|line| line["run_id"] == RAN && line["event"]["index"] == index)
            .and_then(|line| line["event"]["output"].as_str().map(str::to_string))
            .expect("the fixture holds that event's output")
    }

    /// Every description is one line inside the bound, whatever it describes.
    fn assert_one_line(description: &str) {
        assert!(
            !description.contains(['\n', '\r']) && !description.chars().any(char::is_control),
            "a description is more than one line: {description:?}"
        );
        assert!(
            description.chars().count() <= DESCRIPTION_MAX_CHARS,
            "a description is past {DESCRIPTION_MAX_CHARS} characters: {description:?}"
        );
    }

    fn tool(resolved: &Resolved) -> (Option<String>, Option<String>) {
        match &resolved.text {
            Text::Tool { output, .. } => (resolved.text.input_text(), output.clone()),
            other => panic!("not a tool's entry: {other:?}"),
        }
    }

    /// The recorded session's shell commands, file read and file edit, by their
    /// whole input and one-line description.
    #[test]
    fn a_recorded_sessions_tool_calls_resolve_whole_and_are_described_by_what_they_did() {
        let shell = resolved("turn-activity", &activity(recorded(RAN), 0));
        let (input, output) = tool(&shell);
        let input = input.expect("a call carries its input");
        assert!(
            input.contains("git log --oneline -3 && git status --short | head;")
                && input.len() > DESCRIPTION_MAX_CHARS,
            "the shell call's input is not the whole command: {input}"
        );
        assert_eq!(output, None, "a call carries no observation of its own");
        assert!(
            shell
                .description
                .starts_with("git log --oneline -3 && git status --short | head;"),
            "a shell call is not described by its command: {}",
            shell.description
        );
        assert!(shell.description.ends_with('…'), "{}", shell.description);
        assert!(!shell.cut, "text read out of a history file is whole");

        // A heredoc: the command's first line, and the rest kept in the input.
        let heredoc = resolved("turn-activity", &activity(recorded(RAN), 8));
        assert_eq!(heredoc.description, "python3 - <<'EOF'");
        let (input, _) = tool(&heredoc);
        let input = input.expect("a call carries its input");
        assert!(
            input.contains("tests/unwatched/test_flow_stop_hook_e2e.py") && input.contains("\\n"),
            "the heredoc's body is not in the call's whole input: {input}"
        );

        let read = resolved("turn-activity", &activity(recorded(RAN), 11));
        assert!(
            read.description
                .starts_with("Read /home/operator/.onevcs/workspaces/"),
            "a file read is not described by its verb and path: {}",
            read.description
        );
        let edit = resolved("turn-activity", &activity(recorded(RAN), 13));
        assert!(
            edit.description
                .starts_with("Edit /home/operator/.onevcs/workspaces/"),
            "a file edit is not described by its verb and path: {}",
            edit.description
        );
        let (input, _) = tool(&edit);
        assert!(
            input.is_some_and(|input| input.contains("old_string")),
            "the edit's whole input is not kept"
        );

        // The observation answering the first call, whole.
        let result = resolved("turn-activity", &activity(recorded(RAN), 1));
        let (_, output) = tool(&result);
        let output = output.expect("a result carries what it returned");
        assert_eq!(
            output,
            recorded_output(1),
            "the recorded output is not served whole"
        );
        assert!(result.description.starts_with("5af78730 Merge origin/main"));

        for described in [&shell, &heredoc, &read, &edit, &result] {
            assert_one_line(&described.description);
        }
    }

    /// `index` restarts for every candidate written into one file, so the same
    /// index names a different entry under each `history_id` — and the one the
    /// reference names is the one it gets.
    #[test]
    fn an_index_repeated_across_two_fallback_candidates_resolves_by_its_history_id() {
        let fell_through = resolved("turn-activity", &activity(recorded(FELL_THROUGH), 0));
        assert_eq!(
            fell_through.text,
            Text::Message("You've hit your weekly limit · resets 1pm (UTC)".into())
        );
        let ran = resolved("turn-activity", &activity(recorded(RAN), 0));
        assert!(
            matches!(&ran.text, Text::Tool { kind: ToolKind::Call, name, .. } if name.as_deref() == Some("Bash")),
            "index 0 of the candidate that ran resolved to another candidate's entry: {ran:?}"
        );
    }

    /// The recorded messages, instruction and closing text, whole, each
    /// described by its first line.
    #[test]
    fn a_recorded_sessions_messages_and_prompt_resolve_whole() {
        let said = resolved("turn-activity", &activity(recorded(RAN), 10));
        assert!(matches!(&said.text, Text::Message(text) if text.starts_with("Fixing a race")));
        assert!(said
            .description
            .starts_with("Fixing a race in the flow stop-hook test"));

        let closing = resolved("turn-message", &record(recorded(RAN), &[]));
        let Text::Message(text) = &closing.text else {
            panic!("a turn message resolved to {closing:?}");
        };
        assert_eq!(text.len(), 2453, "the closing text is not served whole");
        assert!(closing
            .description
            .starts_with("I found and fixed the test race"));

        // Recorded before opening lines existed: the prompt is on the closing
        // line, and it is past the bound a journal copy is cut to.
        let opened = resolved("turn-started", &record(recorded(RAN), &[]));
        let Text::Instruction(prompt) = &opened.text else {
            panic!("a turn opening resolved to {opened:?}");
        };
        assert!(
            prompt.len() > crate::event::MAX_PAYLOAD_TEXT_BYTES,
            "{}",
            prompt.len()
        );
        assert_eq!(opened.description, "## What");
        for described in [&said, &closing, &opened] {
            assert_one_line(&described.description);
        }
    }

    /// A missing file and a missing entry, each named with its path.
    #[test]
    fn a_missing_file_and_a_missing_entry_name_the_file_and_why() {
        let gone = unresolved(
            "turn-activity",
            &activity(
                location(&recorded_store(), RECORDED_PROJECT, "never-written", RAN),
                0,
            ),
        );
        let path = PathBuf::from(recorded_store())
            .join(RECORDED_PROJECT)
            .join("never-written.jsonl");
        assert_eq!(
            gone,
            Unresolved {
                path: path.clone(),
                why: Why::MissingFile
            }
        );
        assert_eq!(
            gone.to_string(),
            format!("transcript unavailable: {}: missing-file", path.display())
        );

        let absent = unresolved("turn-activity", &activity(recorded(RAN), 99));
        assert_eq!(absent.why, Why::MissingEntry);
        assert!(absent
            .path
            .ends_with("home-operator-worktree/node-scope-recorded-worker-skill.jsonl"));
        assert!(absent.to_string().ends_with(": missing-entry"), "{absent}");

        // A candidate this file never held, at an index every candidate has.
        let stranger = unresolved(
            "turn-activity",
            &activity(recorded("01a13e8e-0000-7000-8000-000000000000"), 0),
        );
        assert_eq!(stranger.why, Why::MissingEntry);
    }

    /// A store the test writes through oneharness's own writer, and the
    /// reference it hands out for one begun candidate.
    struct Written {
        root: PathBuf,
        writer: HistoryWriter,
        run_id: oneharness_core::domain::history::HistoryId,
        reference: Value,
    }

    impl Written {
        fn new(name: &str, prompt: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "onepipeline-transcript-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let project = root.join("project");
            std::fs::create_dir_all(&project).expect("a project directory");
            let writer = HistoryWriter::open(
                &root.join("store"),
                &project,
                "worker-skill",
                HistoryLabels::default(),
            )
            .expect("a history session opens");
            let identity = "claude-code".parse().expect("a harness identity");
            let run_id = writer.begin_harness_run(&identity, prompt);
            let begun = writer.run_begun(&identity, run_id);
            let reference = serde_json::to_value(&begun).expect("a begun candidate serializes");
            let reference = location(
                reference["history_dir"].as_str().expect("a store"),
                reference["history_project"].as_str().expect("a project"),
                reference["history_session"].as_str().expect("a session"),
                &run_id.to_string(),
            );
            Self {
                root,
                writer,
                run_id,
                reference,
            }
        }

        fn event(
            &self,
            index: usize,
            kind: &str,
            name: Option<&str>,
            input: Option<Value>,
            output: Option<&str>,
        ) {
            self.writer
                .append_event(
                    self.run_id,
                    "claude-code",
                    ActionEvent {
                        kind: kind.into(),
                        name: name.map(str::to_string),
                        input,
                        output: output.map(str::to_string),
                        index,
                        tool_call_id: Some(format!("call-{index}")),
                        started_at: None,
                        finished_at: None,
                        duration_ms: None,
                        status: None,
                        timing_source: None,
                    },
                )
                .expect("an event appends");
        }
    }

    impl Drop for Written {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A search, a tool this build has no reading of, and a long multi-line
    /// command, each written live by oneharness's own writer into a session
    /// whose run has not closed — the file a dispatch in flight is writing.
    #[test]
    fn a_search_an_unknown_tool_and_a_long_multiline_command_are_described_in_one_bounded_line() {
        let prompt = format!("## What\n\n{}\nthe note sits here", "a".repeat(5_000));
        let written = Written::new("described", &prompt);
        written.event(
            0,
            "tool_call",
            Some("Grep"),
            Some(json!({"pattern": "fn resolve\\(", "path": "src", "-n": true})),
            None,
        );
        let body = "line two of the body\n".repeat(40);
        written.event(
            1,
            "tool_call",
            Some("mcp__tracker__create_issue"),
            Some(json!({"title": "Transcript readers resolve references", "body": body})),
            None,
        );
        let long_first_line = format!("cargo nextest run {}", "--package onepipeline ".repeat(12));
        written.event(
            2,
            "tool_call",
            Some("Bash"),
            Some(json!({"command": format!("{long_first_line}\necho done\n")})),
            None,
        );
        written.event(
            3,
            "tool_call",
            Some("Glob"),
            Some(json!({"pattern": "src/**/*.rs"})),
            None,
        );

        let search = resolved("turn-activity", &activity(written.reference.clone(), 0));
        assert_eq!(
            search.description, "fn resolve\\(",
            "a search is not described by its pattern"
        );
        let glob = resolved("turn-activity", &activity(written.reference.clone(), 3));
        assert_eq!(glob.description, "src/**/*.rs");

        let other = resolved("turn-activity", &activity(written.reference.clone(), 1));
        assert_eq!(
            other.description, "mcp__tracker__create_issue Transcript readers resolve references",
            "any other tool is described by its name and first argument"
        );
        let (input, _) = tool(&other);
        let input: Value =
            serde_json::from_str(&input.expect("its input")).expect("structured input stays JSON");
        assert_eq!(
            input["body"],
            json!(body),
            "an unknown tool's input is not kept whole"
        );

        let long = resolved("turn-activity", &activity(written.reference.clone(), 2));
        assert!(long
            .description
            .starts_with("cargo nextest run --package onepipeline"));
        assert!(long.description.ends_with('…'), "{}", long.description);
        assert_eq!(long.description.chars().count(), DESCRIPTION_MAX_CHARS);
        let (input, _) = tool(&long);
        assert!(input.is_some_and(|input| input.contains("echo done")));

        // The opening line carries the whole prompt before the run closes.
        let opened = resolved("turn-started", &record(written.reference.clone(), &[]));
        assert_eq!(opened.text, Text::Instruction(prompt.clone()));

        for described in [&search, &glob, &other, &long, &opened] {
            assert_one_line(&described.description);
        }
    }

    /// A file still being written ends in half a line: that is not an entry yet,
    /// and not a reason to call the file unreadable.
    #[test]
    fn a_file_still_being_written_resolves_up_to_its_last_complete_line() {
        let written = Written::new("partial", "## What\nwrite it");
        written.event(
            0,
            "tool_call",
            Some("Bash"),
            Some(json!({"command": "just check"})),
            None,
        );
        let path = written.writer.path().to_path_buf();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("the session file");
        std::io::Write::write_all(&mut file, br#"{"type":"event","run_id":"#).expect("half a line");

        let call = resolved("turn-activity", &activity(written.reference.clone(), 0));
        assert_eq!(call.description, "just check");
        let pending = unresolved("turn-activity", &activity(written.reference.clone(), 1));
        assert_eq!(
            pending.why,
            Why::MissingEntry,
            "half a line read as an unreadable file"
        );
    }

    /// A file holding a line that is not JSON, and an address that names no
    /// plain file, both answer `unreadable` naming where they looked.
    #[test]
    fn unreadable_history_names_the_file_and_says_so() {
        let written = Written::new("unreadable", "## What\nread it");
        let path = written.writer.path().to_path_buf();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("the session file");
        std::io::Write::write_all(&mut file, b"not a history line\n").expect("a corrupt line");

        let corrupt = unresolved("turn-activity", &activity(written.reference.clone(), 0));
        assert_eq!(
            corrupt,
            Unresolved {
                path: path.clone(),
                why: Why::Unreadable
            }
        );
        assert_eq!(
            corrupt.to_string(),
            format!("transcript unavailable: {}: unreadable", path.display())
        );
        // The entry the file does hold still resolves past the corrupt line.
        let opened = resolved("turn-started", &record(written.reference.clone(), &[]));
        assert_eq!(opened.text, Text::Instruction("## What\nread it".into()));

        // A session that is a directory rather than a file.
        let dir = written.root.join("store").join("dir-project");
        std::fs::create_dir_all(dir.join("a-directory.jsonl")).expect("a directory");
        let not_a_file = unresolved(
            "turn-activity",
            &activity(
                location(
                    written.root.join("store").to_str().expect("a UTF-8 path"),
                    "dir-project",
                    "a-directory",
                    &written.run_id.to_string(),
                ),
                0,
            ),
        );
        assert_eq!(not_a_file.why, Why::Unreadable);

        // A reference that is not the shape the sibling writes.
        let malformed = unresolved(
            "turn-activity",
            &record(
                json!({"history_dir": "/state", "history_project": "p"}),
                &[],
            ),
        );
        assert_eq!(malformed.why, Why::Unreadable);
        assert_eq!(malformed.path, PathBuf::from("/state/p/?.jsonl"));
    }

    /// A record carrying a copy and no reference answers that copy, its cut
    /// flag kept — which is how a journal written before references reads.
    #[test]
    fn a_bounded_copy_resolves_to_itself_with_its_cut_flag_kept() {
        let mut payload = Map::new();
        payload.insert("instruction".into(), json!("## What\nthe head of it"));
        payload.insert("instruction_truncated".into(), json!(true));
        let opened = resolved("turn-started", &payload);
        assert_eq!(
            opened.text,
            Text::Instruction("## What\nthe head of it".into())
        );
        assert!(opened.cut);
        assert_eq!(opened.description, "## What");

        let mut call = Map::new();
        call.insert("kind".into(), json!("tool_call"));
        call.insert("name".into(), json!("command_execution"));
        call.insert("detail".into(), json!("/usr/bin/bash -lc 'just check'"));
        let call = resolved("turn-activity", &call);
        assert_eq!(call.description, "just check");
        assert!(!call.cut);

        let mut result = Map::new();
        result.insert("kind".into(), json!("tool_result"));
        result.insert("name".into(), Value::Null);
        result.insert("output".into(), json!("tail of it\n"));
        result.insert("output_truncated".into(), json!(true));
        let result = resolved("turn-activity", &result);
        assert!(result.cut);
        assert_eq!(result.description, "tail of it");

        assert!(of_record("turn-completed", &Map::new()).is_none());
    }

    /// A reference whose `tool_call_id` disagrees with the entry at its index
    /// names a different call, and is answered as a missing entry rather than as
    /// that call.
    #[test]
    fn a_conflicting_tool_call_id_is_not_the_entry_the_reference_names() {
        let written = Written::new("call-id", "## What\ncall it");
        written.event(
            0,
            "tool_call",
            Some("Bash"),
            Some(json!({"command": "just check"})),
            None,
        );
        let mut agreeing = written.reference.clone();
        agreeing["tool_call_id"] = json!("call-0");
        assert_eq!(
            resolved("turn-activity", &activity(agreeing, 0)).description,
            "just check"
        );
        let mut conflicting = written.reference.clone();
        conflicting["tool_call_id"] = json!("call-from-another-turn");
        let missing = unresolved("turn-activity", &activity(conflicting, 0));
        assert_eq!(missing.why, Why::MissingEntry);
    }

    fn session_file(reference: &Value) -> PathBuf {
        address(reference)
    }

    /// What a whole read of a session file holds at `index` for `run`: its `output`.
    fn whole_read_output(path: &std::path::Path, run: &str, index: u64) -> Option<String> {
        std::fs::read_to_string(path)
            .expect("the session file")
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|entry| entry["run_id"] == run && entry["event"]["index"] == index)
            .and_then(|entry| entry["event"]["output"].as_str().map(str::to_string))
    }

    fn output_of(reference: &Value, index: u64) -> Option<String> {
        match resolved("turn-activity", &activity(reference.clone(), index)).text {
            Text::Tool { output, .. } => output,
            other => panic!("a tool entry, not {other:?}"),
        }
    }

    /// Every reference answers what a whole read of the file holds, and a file whose
    /// bytes changed answers its new content on the next read: grown, rewritten at the
    /// same length with a new modification time, or removed.
    #[test]
    fn one_parse_of_a_session_answers_what_the_file_holds_until_the_file_changes() {
        let written = Written::new("indexed", "## What\nread it");
        for index in 0..200 {
            written.event(
                index,
                "tool_result",
                Some("Bash"),
                None,
                Some(&format!("output of {index} \u{1F980}")),
            );
        }
        let path = session_file(&written.reference);
        let run = written.run_id.to_string();
        for index in (0..200).rev() {
            assert_eq!(
                output_of(&written.reference, index),
                whole_read_output(&path, &run, index),
                "entry {index}"
            );
        }

        // It grew: what was not there is, and what was still is.
        assert_eq!(
            unresolved("turn-activity", &activity(written.reference.clone(), 200)).why,
            Why::MissingEntry
        );
        written.event(200, "tool_result", Some("Bash"), None, Some("grown"));
        assert_eq!(output_of(&written.reference, 200).as_deref(), Some("grown"));
        assert_eq!(
            output_of(&written.reference, 3),
            whole_read_output(&path, &run, 3)
        );

        // Rewritten in place, one byte for another, so only its stamp says so.
        let text = std::fs::read_to_string(&path).expect("the session file");
        std::fs::write(&path, text.replace("output of 7 ", "OUTPUT of 7 ")).expect("rewritten");
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(later))
            .expect("a new modification time");
        assert_eq!(
            output_of(&written.reference, 7).as_deref(),
            Some("OUTPUT of 7 \u{1F980}")
        );

        std::fs::remove_file(&path).expect("removed");
        assert_eq!(
            unresolved("turn-activity", &activity(written.reference.clone(), 7)).why,
            Why::MissingFile
        );
    }

    /// A tool-call identity mismatch and a malformed line answer as they do when the
    /// file is scanned: the first entry of the index naming that call, a miss that may
    /// have been the unreadable line, and a hit that no malformed line can reach.
    #[test]
    fn a_mismatched_call_and_a_malformed_line_answer_what_a_scan_of_the_file_answers() {
        let written = Written::new("indexed-malformed", "## What\nread it");
        written.event(0, "tool_result", Some("Bash"), None, Some("the first"));
        written.event(1, "tool_result", Some("Bash"), None, Some("the second"));
        let path = session_file(&written.reference);

        // Another call at that index, later in the file, is the one a reference names.
        let text = std::fs::read_to_string(&path).expect("the session file");
        let repeated = text
            .lines()
            .find(|line| line.contains("\"call-0\""))
            .expect("the first entry")
            .replace("call-0", "call-again")
            .replace("the first", "the entry again");
        std::fs::write(&path, format!("{text}{repeated}\n")).expect("appended");
        let mut named = written.reference.clone();
        named["tool_call_id"] = json!("call-again");
        assert_eq!(
            output_of(&named, 0).as_deref(),
            Some("the entry again"),
            "the later entry naming this call"
        );
        let mut other = written.reference.clone();
        other["tool_call_id"] = json!("call-elsewhere");
        assert_eq!(
            unresolved("turn-activity", &activity(other.clone(), 0)).why,
            Why::MissingEntry,
            "a call this file never held at that index"
        );

        // A malformed line: what is there still answers, and a miss may be that line.
        let text = std::fs::read_to_string(&path).expect("the session file");
        std::fs::write(&path, format!("{text}{{\"type\":\"event\",\n")).expect("appended");
        assert_eq!(
            output_of(&written.reference, 1).as_deref(),
            Some("the second")
        );
        assert_eq!(
            unresolved("turn-activity", &activity(written.reference.clone(), 9)).why,
            Why::Unreadable
        );
        assert_eq!(
            unresolved("turn-activity", &activity(other, 0)).why,
            Why::Unreadable
        );

        // Bytes that are not text end the read there: before them still answers.
        let mut bytes = std::fs::read(&path).expect("the session file");
        bytes.extend_from_slice(b"\xff\xfe\n");
        std::fs::write(&path, bytes).expect("appended");
        assert_eq!(
            output_of(&written.reference, 1).as_deref(),
            Some("the second")
        );
        assert_eq!(
            unresolved("turn-activity", &activity(written.reference.clone(), 9)).why,
            Why::Unreadable
        );
    }

    /// The index holds at most its bound of sessions, and a session it let go of reads
    /// the same as one it holds.
    #[test]
    fn the_session_index_holds_no_more_than_its_bound_and_forgets_nothing_it_answers() {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-transcript-bound-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("a project directory");
        let history = "01a13e8e-0000-7000-8000-00000000b0dd".to_string();
        let reference_of = |session: usize| {
            location(
                root.to_str().expect("a path"),
                "project",
                &format!("session-{session}"),
                &history,
            )
        };
        let sessions = session_index::SESSIONS + 8;
        for session in 0..sessions {
            let line = json!({
                "type": "event",
                "run_id": history,
                "event": {"kind": "tool_result", "index": 0, "output": format!("of {session}")},
            });
            std::fs::write(
                project.join(format!("session-{session}.jsonl")),
                format!("{line}\n"),
            )
            .expect("a session file");
        }
        for session in (0..sessions).chain(0..sessions) {
            assert_eq!(
                output_of(&reference_of(session), 0),
                Some(format!("of {session}"))
            );
            assert!(session_index::held() <= session_index::SESSIONS);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A command recorded as an argv list: the harness's shell wrapper taken
    /// off, and any other argv joined as it would be typed.
    #[test]
    fn an_argv_command_is_described_by_the_command_it_ran() {
        let written = Written::new("argv", "## What\nrun it");
        written.event(
            0,
            "tool_call",
            Some("exec_command"),
            Some(json!({"command": ["/bin/bash", "-lc", "cargo test --workspace\necho done"]})),
            None,
        );
        written.event(
            1,
            "tool_call",
            Some("exec_command"),
            Some(json!({"command": ["git", "status", "--short"]})),
            None,
        );
        written.event(
            2,
            "tool_call",
            Some("command_execution"),
            Some(json!("/usr/bin/bash -lc 'just gate'")),
            None,
        );
        let wrapped = resolved("turn-activity", &activity(written.reference.clone(), 0));
        assert_eq!(wrapped.description, "cargo test --workspace");
        let plain = resolved("turn-activity", &activity(written.reference.clone(), 1));
        assert_eq!(plain.description, "git status --short");
        let string = resolved("turn-activity", &activity(written.reference.clone(), 2));
        assert_eq!(string.description, "just gate");
    }

    /// A history entry whose fields are not the types oneharness writes is
    /// unreadable, never empty content; and a last line still being written is
    /// not read even where what has arrived of it parses.
    #[test]
    fn a_wrongly_typed_entry_is_unreadable_and_an_unterminated_line_is_not_read() {
        let written = Written::new("typed", "## What\ntype it");
        let path = written.writer.path().to_path_buf();
        let id = written.run_id.to_string();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("the session file");
        let bad_event = json!({"type": "event", "run_id": id, "event": {"kind": 7, "index": 0}});
        let bad_close = json!({"type": "run", "history_id": id, "text": {"not": "text"}});
        let unterminated = json!({"type": "event", "run_id": id, "event": {
            "kind": "tool_call", "name": "Bash", "input": {"command": "half written"},
            "output": null, "index": 1, "tool_call_id": null, "started_at": null,
            "finished_at": null, "duration_ms": null, "status": null}});
        std::io::Write::write_all(
            &mut file,
            format!("{bad_event}\n{bad_close}\n{unterminated}").as_bytes(),
        )
        .expect("the entries are written");

        let event = unresolved("turn-activity", &activity(written.reference.clone(), 0));
        assert_eq!(
            event,
            Unresolved {
                path: path.clone(),
                why: Why::Unreadable
            }
        );
        let message = unresolved("turn-message", &record(written.reference.clone(), &[]));
        assert_eq!(message.why, Why::Unreadable);
        let pending = unresolved("turn-activity", &activity(written.reference.clone(), 1));
        assert_eq!(
            pending.why,
            Why::MissingEntry,
            "an unterminated line was read as an entry"
        );

        // An entry of this candidate whose index is not an integer could be the
        // one asked for: a miss beside it is unreadable, not missing.
        let written = Written::new("typed-index", "## What\nindex it");
        let path = written.writer.path().to_path_buf();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("the session file");
        let odd_index = json!({"type": "event", "run_id": written.run_id.to_string(),
            "event": {"kind": "tool_call", "index": "zero"}});
        std::io::Write::write_all(&mut file, format!("{odd_index}\n").as_bytes())
            .expect("the entry is written");
        let unknown = unresolved("turn-activity", &activity(written.reference.clone(), 0));
        assert_eq!(
            unknown.why,
            Why::Unreadable,
            "an unindexed entry was read as no entry"
        );
    }

    /// A copy whose text is not a string, or whose cut flag is not a boolean,
    /// is unreadable rather than coerced into a claim nobody made.
    #[test]
    fn a_malformed_copy_is_unreadable_rather_than_coerced() {
        let mut object_text = Map::new();
        object_text.insert("instruction".into(), json!({"an": "object"}));
        assert_eq!(
            unresolved("turn-started", &object_text).why,
            Why::Unreadable
        );

        let mut odd_flag = Map::new();
        odd_flag.insert("kind".into(), json!("tool_result"));
        odd_flag.insert("output".into(), json!("the tail"));
        odd_flag.insert("output_truncated".into(), json!("yes"));
        let odd = unresolved("turn-activity", &odd_flag);
        assert_eq!(odd.why, Why::Unreadable);
        assert_eq!(
            odd.to_string(),
            "transcript unavailable: <turn-activity copy>: unreadable"
        );

        let mut null_text = Map::new();
        null_text.insert("text".into(), Value::Null);
        assert_eq!(unresolved("turn-message", &null_text).why, Why::Unreadable);
        let mut null_flag = Map::new();
        null_flag.insert("instruction".into(), json!("## What"));
        null_flag.insert("instruction_truncated".into(), Value::Null);
        assert_eq!(unresolved("turn-started", &null_flag).why, Why::Unreadable);
        let mut unnamed_result = Map::new();
        unnamed_result.insert("kind".into(), json!("tool_result"));
        unnamed_result.insert("name".into(), Value::Null);
        unnamed_result.insert("output".into(), json!("ok"));
        assert_eq!(resolved("turn-activity", &unnamed_result).description, "ok");

        let mut kindless = Map::new();
        kindless.insert("output".into(), json!("said"));
        assert_eq!(unresolved("turn-activity", &kindless).why, Why::Unreadable);
    }

    /// A supervisor's message is the instruction its decision handed the agent,
    /// read by onejudge's own parser — never the decision document around it —
    /// and a decision that carried no instruction is no message at all.
    #[test]
    fn a_supervisors_message_resolves_to_the_instruction_its_decision_carried() {
        let decide = |name: &str, decision: Value| {
            let written =
                Written::new(name, "You are the simulated USER and completion supervisor");
            let closing = json!({"type": "run", "history_id": written.run_id.to_string(),
                "text": decision.to_string()});
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(written.writer.path())
                .expect("the session file");
            std::io::Write::write_all(&mut file, format!("{closing}\n").as_bytes())
                .expect("the closing line is written");
            written
        };
        let asked = decide(
            "decision",
            json!({"completion": false, "message": "Run the check again.\nThen report it.",
                "reason": "not there yet"}),
        );
        let said = resolved(
            "turn-message",
            &record(asked.reference.clone(), &[("role", json!("user"))]),
        );
        assert_eq!(
            said.text,
            Text::Message("Run the check again.\nThen report it.".into())
        );
        assert_eq!(said.description, "Run the check again.");
        // The same session read as an assistant's words is its whole closing text.
        let whole = resolved(
            "turn-message",
            &record(asked.reference.clone(), &[("role", json!("assistant"))]),
        );
        assert!(matches!(&whole.text, Text::Message(text) if text.contains("\"completion\"")));

        let completed = decide("completed", json!({"completion": true, "reason": "done"}));
        let none = unresolved(
            "turn-message",
            &record(completed.reference.clone(), &[("role", json!("user"))]),
        );
        assert_eq!(none.why, Why::Unreadable);
    }
}
