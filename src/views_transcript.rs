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
        /// `tool_call` or `tool_result`.
        kind: String,
        /// The tool, where the harness named one.
        name: Option<String>,
        /// The call's input as the harness recorded it: a structured value, or
        /// the text a bounded copy summarized it as.
        input: Option<Value>,
        /// What the tool returned, where the entry carries it.
        output: Option<String>,
    },
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
        let reference = payload.get("transcript");
        let malformed = |reference: &Value| Unresolved {
            path: address(reference),
            why: Why::Unreadable,
        };
        let string = |key: &str| match payload.get(key) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let flag = |key: &str| payload.get(key).and_then(Value::as_bool) == Some(true);
        Some(match kind {
            "turn-started" => match reference {
                Some(reference) => serde_json::from_value(reference.clone())
                    .map(Self::Instruction)
                    .map_err(|_| malformed(reference)),
                None => Ok(Self::Copy {
                    text: Text::Instruction(string("instruction")),
                    cut: flag("instruction_truncated"),
                }),
            },
            "turn-message" => match reference {
                Some(reference) => serde_json::from_value(reference.clone())
                    .map(Self::Message)
                    .map_err(|_| malformed(reference)),
                None => Ok(Self::Copy {
                    text: Text::Message(string("text")),
                    cut: flag("truncated"),
                }),
            },
            "turn-activity" => match reference {
                Some(reference) => serde_json::from_value(reference.clone())
                    .map(Self::Activity)
                    .map_err(|_| malformed(reference)),
                None => {
                    let activity = string("kind");
                    let output = payload
                        .get("output")
                        .filter(|value| !value.is_null())
                        .map(|_| string("output"));
                    Ok(Self::Copy {
                        cut: flag("output_truncated")
                            || (activity == "tool_call" && flag("truncated")),
                        text: match activity.as_str() {
                            "message" => Text::Message(output.unwrap_or_default()),
                            "reasoning" => Text::Reasoning(output.unwrap_or_default()),
                            _ => Text::Tool {
                                name: payload
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                input: payload
                                    .get("detail")
                                    .and_then(Value::as_str)
                                    .filter(|detail| !detail.is_empty())
                                    .map(|detail| Value::String(detail.to_string())),
                                output,
                                kind: activity,
                            },
                        },
                    })
                }
            },
            _ => return None,
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
        TranscriptRef::Instruction(location) | TranscriptRef::Message(location) => {
            (location, None)
        }
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
    let file = match found {
        Ok(Some(found)) => std::fs::File::open(found).map_err(|error| {
            unresolved(if error.kind() == std::io::ErrorKind::NotFound {
                Why::MissingFile
            } else {
                Why::Unreadable
            })
        })?,
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
        // is not an entry yet, and not a reason to call the file unreadable.
        let complete = line.ends_with('\n');
        let Ok(entry) = serde_json::from_str::<Value>(line.trim_end()) else {
            unreadable |= complete;
            continue;
        };
        let text = |key: &str| entry.get(key).and_then(Value::as_str);
        match (reference, text("type")) {
            (TranscriptRef::Instruction(_), Some("run-started"))
                if text("history_id") == Some(&history_id) =>
            {
                if let Some(prompt) = text("prompt") {
                    return Ok(described(Text::Instruction(prompt.to_string())));
                }
            }
            // A closing line carries the prompt too, and is the only line that
            // does in a file written before openings were.
            (TranscriptRef::Instruction(_), Some("run"))
                if text("history_id") == Some(&history_id) =>
            {
                prompt_at_close = text("prompt").map(str::to_string);
            }
            (TranscriptRef::Message(_), Some("run")) if text("history_id") == Some(&history_id) => {
                return Ok(described(Text::Message(
                    text("text").unwrap_or_default().to_string(),
                )));
            }
            (TranscriptRef::Activity(_), Some("event")) if text("run_id") == Some(&history_id) => {
                let Some(wanted) = index else { continue };
                let event = entry.get("event").unwrap_or(&Value::Null);
                if event.get("index").and_then(Value::as_u64) != Some(wanted.index) {
                    continue;
                }
                let field = |key: &str| event.get(key).and_then(Value::as_str);
                if let (Some(expected), Some(found)) =
                    (wanted.tool_call_id.as_deref(), field("tool_call_id"))
                {
                    if expected != found {
                        continue;
                    }
                }
                let output = field("output").map(str::to_string);
                return Ok(described(match field("kind").unwrap_or_default() {
                    "message" => Text::Message(output.unwrap_or_default()),
                    "reasoning" => Text::Reasoning(output.unwrap_or_default()),
                    kind => Text::Tool {
                        kind: kind.to_string(),
                        name: field("name").map(str::to_string),
                        input: event.get("input").filter(|input| !input.is_null()).cloned(),
                        output,
                    },
                }));
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
            if kind == "tool_result" {
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
    let field = |keys: &[&str]| {
        object.and_then(|object| keys.iter().find_map(|key| object.get(*key)))
    };
    // A shell tool: the command it ran, its wrapper taken off.
    if let Some(command) = field(&["command", "cmd"]).and_then(command_text) {
        return first_line(&command).to_string();
    }
    let shell = matches!(
        lowered.as_str(),
        "bash" | "shell" | "exec_command" | "local_shell" | "command_execution" | "run_shell_command"
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
    if let Some(path) = field(&["file_path", "path", "notebook_path", "filePath"]).and_then(Value::as_str)
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
    matches!(base, "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "pwsh")
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
