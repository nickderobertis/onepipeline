//! The onejudge report a settled member left behind.
//!
//! `oneagentgraph` stores each member's full report and puts its `report_path`
//! on the `member-settled` it relays here, so the evidence behind a dispatch —
//! every turn, its tools, its text, and what the two sides of the conversation
//! spent — is retained rather than summarised away. This module is the reader:
//! which reports a run's merged store names, and what one says.
//!
//! It reads the document **structurally**, by field name, rather than into the
//! producing library's own types. The report is a sibling's artifact and this
//! crate is a consumer of it: a stricter read would refuse a whole report over
//! one field it did not recognise and report nothing at all, which for evidence
//! is the wrong direction to fail in. Every other cross-library read here is
//! lenient for the same reason.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::event::{Envelope, Source};

/// The kind `oneagentgraph` settles a member with.
pub const MEMBER_SETTLED: &str = "member-settled";

/// The payload key naming where the member's report was stored.
pub const REPORT_PATH: &str = "report_path";

/// One member's retained report, as its settlement named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retained {
    /// The node whose dispatch produced it, when the envelope named one.
    pub node: Option<String>,
    /// The member within that dispatch, when the producer stamped one.
    pub member: Option<String>,
    /// Where the producing library stored it.
    pub path: PathBuf,
}

/// Every report a `member-settled` in this store named, in settlement order.
///
/// A settlement that stored no report is absent rather than listed with an
/// empty path: the producer says so with a null `report_path`, and a consumer
/// that invented a path for it would send a reader looking for a file nobody
/// wrote.
pub fn retained(events: &[Envelope]) -> Vec<Retained> {
    events
        .iter()
        .filter(|event| event.source == Source::Agentgraph && event.kind.0 == MEMBER_SETTLED)
        .filter_map(|event| {
            let path = event
                .payload
                .get(REPORT_PATH)
                .and_then(Value::as_str)
                .filter(|path| !path.is_empty())?;
            Some(Retained {
                node: event.labels.node.clone(),
                member: event
                    .labels
                    .extra
                    .get("member")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                path: PathBuf::from(path),
            })
        })
        .collect()
}

/// Read one report, or `None` when it is not there to read.
///
/// A report the machine that ran the dispatch stored elsewhere, or that its
/// scratch has since been swept of, is simply absent — a caller says so rather
/// than reporting a dispatch as having produced nothing.
pub fn read(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// The turns a report's transcript carries, in order.
///
/// Empty for a report that carries no transcript, which is a report this build
/// can say nothing further about rather than a conversation that never happened.
pub fn turns(document: &Value) -> Vec<Turn> {
    document
        .get("transcript")
        .and_then(|transcript| transcript.get("messages"))
        .and_then(Value::as_array)
        .map(|messages| messages.iter().map(Turn::of).collect())
        .unwrap_or_default()
}

/// One turn of a retained transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// Who produced it, as the report names them.
    pub role: String,
    /// What they said.
    pub text: String,
    /// The tools the turn used, in the order it used them.
    pub tools: Vec<Tool>,
}

impl Turn {
    fn of(message: &Value) -> Self {
        Self {
            role: string(message, "role"),
            text: string(message, "content"),
            tools: message
                .get("events")
                .and_then(Value::as_array)
                .map(|events| events.iter().map(Tool::of).collect())
                .unwrap_or_default(),
        }
    }
}

/// One tool call a turn made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    /// `tool_call` or `tool_result`, as the report names it.
    pub kind: String,
    /// The tool, where the harness named one.
    pub name: String,
    /// What it acted on, rendered compactly.
    pub detail: String,
}

impl Tool {
    fn of(event: &Value) -> Self {
        Self {
            kind: string(event, "kind"),
            name: string(event, "name"),
            detail: match event.get("input") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(text)) => text.clone(),
                Some(input) => input.to_string(),
            },
        }
    }
}

fn string(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, Labels, ENVELOPE_VERSION};
    use serde_json::json;

    fn settled(node: Option<&str>, path: Option<&str>) -> Envelope {
        let mut labels = Labels {
            node: node.map(str::to_string),
            ..Labels::default()
        };
        labels.extra.insert("member".into(), "worker".into());
        Envelope {
            v: ENVELOPE_VERSION,
            ts: "2026-08-08T00:00:00.000Z".into(),
            stream: "oneagentgraph-1".into(),
            seq: 4,
            source: Source::Agentgraph,
            kind: EventKind(MEMBER_SETTLED.into()),
            labels,
            payload: crate::journal::payload(&[(REPORT_PATH, json!(path))]),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn a_settlement_that_stored_a_report_names_where_it_went() {
        let retained = retained(&[settled(Some("build"), Some("/tmp/report.json"))]);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].node.as_deref(), Some("build"));
        assert_eq!(retained[0].member.as_deref(), Some("worker"));
        assert_eq!(retained[0].path, PathBuf::from("/tmp/report.json"));
    }

    /// A `null` or empty `report_path` is the producer saying it stored none.
    #[test]
    fn a_settlement_that_stored_none_is_not_listed_with_an_invented_path() {
        assert!(retained(&[settled(Some("build"), None)]).is_empty());
        assert!(retained(&[settled(Some("build"), Some(""))]).is_empty());
    }

    #[test]
    fn a_pipeline_event_of_the_same_shape_is_not_a_members_report() {
        let mut ours = settled(Some("build"), Some("/tmp/report.json"));
        ours.source = Source::Pipeline;
        assert!(retained(&[ours]).is_empty());
    }

    #[test]
    fn a_transcripts_turns_carry_their_text_and_their_tools() {
        let document = json!({
            "transcript": {"messages": [
                {"role": "user", "content": "## What\nship it"},
                {"role": "assistant", "content": "Ran the gate.", "events": [
                    {"kind": "tool_call", "name": "bash",
                     "input": {"command": "just check"}, "index": 0},
                    {"kind": "tool_result", "output": "ok", "index": 1},
                ]},
            ]},
        });
        let turns = turns(&document);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert!(turns[0].tools.is_empty());
        assert_eq!(turns[1].text, "Ran the gate.");
        assert_eq!(turns[1].tools[0].name, "bash");
        assert!(turns[1].tools[0].detail.contains("just check"));
        // A result names no tool, and is not given one.
        assert_eq!(turns[1].tools[1].kind, "tool_result");
        assert!(turns[1].tools[1].name.is_empty());
    }

    #[test]
    fn a_report_carrying_no_transcript_has_no_turns_rather_than_a_refusal() {
        assert!(turns(&json!({"usage": {"input_tokens": 1}})).is_empty());
        assert!(turns(&json!({"transcript": {}})).is_empty());
        assert!(turns(&Value::Null).is_empty());
    }

    #[test]
    fn a_report_that_is_not_there_to_read_is_absent() {
        assert!(read(Path::new("/nowhere/onepipeline/report.json")).is_none());
    }
}
