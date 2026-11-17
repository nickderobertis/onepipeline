//! Best-effort extraction of a harness's final assistant text from its raw
//! stdout. Pure: no I/O. The execution envelope (exit code, stdout, stderr,
//! duration) is always guaranteed; `text` is a convenience, and its method is
//! recorded so a consumer can tell extraction apart from raw passthrough.

use crate::domain::report::OutputFormat;
use serde_json::Value;

/// A successfully extracted final message and the method used to find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub text: String,
    pub source: String,
}

/// Object keys, in priority order, that commonly hold a harness's final text.
const TEXT_KEYS: &[&str] = &["result", "text", "message", "output", "content", "response"];

/// Extract the final text from `stdout` according to the harness's format.
/// Returns `None` when nothing usable can be found (the caller then leaves
/// `text` null and consumers fall back to the raw `stdout`).
pub fn extract(stdout: &str, fmt: OutputFormat) -> Option<Extracted> {
    match fmt {
        OutputFormat::Text => extract_text(stdout),
        OutputFormat::Json => extract_json(stdout),
        OutputFormat::StreamJson => extract_stream_json(stdout),
    }
}

fn extract_text(stdout: &str) -> Option<Extracted> {
    let t = stdout.trim();
    (!t.is_empty()).then(|| Extracted {
        text: t.to_string(),
        source: "raw".to_string(),
    })
}

fn extract_json(stdout: &str) -> Option<Extracted> {
    // The common case is a single JSON document carrying the final answer in a
    // known key (Claude Code's terminal `result`). Try that first.
    if let Ok(value) = serde_json::from_str::<Value>(stdout.trim()) {
        if let Some((text, key)) = json_text(&value) {
            return Some(Extracted {
                text,
                source: format!("json:{key}"),
            });
        }
    }
    // OpenCode also requests `--format json` but emits *line-delimited* events,
    // not one document, so the single-parse above fails (or finds no top-level
    // text key). Its visible answer lives in `text` parts; recover it from those.
    extract_opencode_parts(stdout)
        // Codex's default `exec --json` emits JSONL whose final answer is an
        // `agent_message` item.
        .or_else(|| extract_codex_agent_message(stdout))
        // Qwen `--output-format json` emits one JSON *array* of Anthropic-style
        // messages; the answer is the last assistant message's `text` blocks.
        .or_else(|| extract_content_block_text(stdout))
}

/// Codex `exec --json`: the final assistant text is the last `item.completed`
/// whose `item.type == "agent_message"`, under `item.text`. `None` when no such
/// item is present. Sourced from a real codex transcript.
fn extract_codex_agent_message(stdout: &str) -> Option<Extracted> {
    let mut last: Option<String> = None;
    for value in json_lines(stdout) {
        if value.get("type").and_then(Value::as_str) != Some("item.completed") {
            continue;
        }
        let Some(item) = value.get("item") else {
            continue;
        };
        if item.get("type").and_then(Value::as_str) != Some("agent_message") {
            continue;
        }
        if let Some(t) = item.get("text").and_then(Value::as_str) {
            if !t.trim().is_empty() {
                last = Some(t.to_string());
            }
        }
    }
    last.map(|text| Extracted {
        text,
        source: "json:codex-agent-message".to_string(),
    })
}

/// Anthropic content-block text (Qwen / Claude stream-json): the last assistant
/// message's `text` blocks, joined — the visible final answer when there is no
/// terminal `result` string. `message.content[]` (or a top-level `content[]`)
/// entries of `type:"text"` with a non-empty `text`. `None` when none is found.
fn extract_content_block_text(stdout: &str) -> Option<Extracted> {
    let mut last: Option<String> = None;
    for value in json_lines(stdout) {
        let content = value
            .get("message")
            .and_then(|m| m.get("content"))
            .or_else(|| value.get("content"))
            .and_then(Value::as_array);
        let Some(blocks) = content else { continue };
        let joined = blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .filter(|t| !t.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !joined.is_empty() {
            last = Some(joined);
        }
    }
    last.map(|text| Extracted {
        text,
        source: "content-blocks:text".to_string(),
    })
}

/// JSON values in `stdout`: a top-level array flattened to its elements (Qwen's
/// `json` mode), else each parseable line (JSONL / stream-json). Mirrors
/// `events::json_candidates` so the text and event paths see the same shapes.
fn json_lines(stdout: &str) -> Vec<Value> {
    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(stdout.trim()) {
        return items;
    }
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect()
}

/// OpenCode's `run --format json` streams one JSON event per line. The assistant's
/// visible answer is carried by its `text` parts: events whose `part` object has
/// `type: "text"` and a `text` string. Other parts — `step-start`/`step-finish`,
/// `reasoning`, and `tool` — are not the final answer and are skipped. A single
/// turn can emit several text parts across steps (e.g. a line of prose, a tool
/// call, then more prose), so they are joined in document order. The source is
/// recorded as `opencode-parts` so a consumer can tell this reconstruction apart
/// from a single-document `result`. `None` when no text part is present.
fn extract_opencode_parts(stdout: &str) -> Option<Extracted> {
    let mut texts: Vec<String> = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(part) = value.get("part").and_then(Value::as_object) else {
            continue;
        };
        if part.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            if !text.trim().is_empty() {
                texts.push(text.to_string());
            }
        }
    }
    (!texts.is_empty()).then(|| Extracted {
        text: texts.join("\n"),
        source: "json:opencode-parts".to_string(),
    })
}

/// Scan line-delimited JSON events and return the last usable text, preferring
/// a terminal `result` event (the shape harnesses use for their final answer).
fn extract_stream_json(stdout: &str) -> Option<Extracted> {
    let mut last: Option<(String, &'static str)> = None;
    let mut last_result: Option<String> = None;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some((text, key)) = json_text(&value) {
            if key == "result" {
                last_result = Some(text.clone());
            }
            last = Some((text, key));
        }
    }
    if let Some(text) = last_result {
        return Some(Extracted {
            text,
            source: "stream-json:result".to_string(),
        });
    }
    // Qwen's stream-json carries no terminal `result` string; its answer is the
    // last assistant message's Anthropic content-block `text`. Prefer that over a
    // stray top-level text key from an intermediate event.
    if let Some(extracted) = extract_content_block_text(stdout) {
        return Some(extracted);
    }
    last.map(|(text, key)| Extracted {
        text,
        source: format!("stream-json:{key}"),
    })
}

/// Pull a non-empty string out of a JSON value: a bare string, or the first
/// matching key of an object. Returns the text and the key that matched.
fn json_text(value: &Value) -> Option<(String, &'static str)> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some((s.clone(), "string")),
        Value::Object(map) => {
            for key in TEXT_KEYS {
                if let Some(Value::String(s)) = map.get(*key) {
                    if !s.trim().is_empty() {
                        return Some((s.clone(), key));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_format_trims_raw_stdout() {
        let got = extract("  hi there\n\n", OutputFormat::Text).unwrap();
        assert_eq!(got.text, "hi there");
        assert_eq!(got.source, "raw");
    }

    #[test]
    fn empty_text_yields_none() {
        assert!(extract("   \n", OutputFormat::Text).is_none());
    }

    #[test]
    fn json_prefers_result_field() {
        let raw = r#"{"type":"result","result":"the answer","is_error":false}"#;
        let got = extract(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.text, "the answer");
        assert_eq!(got.source, "json:result");
    }

    #[test]
    fn json_falls_back_to_other_known_keys() {
        let got = extract(r#"{"message":"hello"}"#, OutputFormat::Json).unwrap();
        assert_eq!(got.text, "hello");
        assert_eq!(got.source, "json:message");
    }

    #[test]
    fn bare_json_string_is_extracted() {
        let got = extract(r#""just a string""#, OutputFormat::Json).unwrap();
        assert_eq!(got.text, "just a string");
        assert_eq!(got.source, "json:string");
    }

    #[test]
    fn unparseable_json_yields_none() {
        assert!(extract("not json at all", OutputFormat::Json).is_none());
    }

    #[test]
    fn stream_json_takes_terminal_result_event() {
        let raw = concat!(
            "{\"type\":\"system\"}\n",
            "{\"type\":\"assistant\",\"text\":\"thinking\"}\n",
            "{\"type\":\"result\",\"result\":\"final\"}\n",
        );
        let got = extract(raw, OutputFormat::StreamJson).unwrap();
        assert_eq!(got.text, "final");
        assert_eq!(got.source, "stream-json:result");
    }

    #[test]
    fn stream_json_falls_back_to_last_text_without_result() {
        let raw = "{\"type\":\"assistant\",\"text\":\"first\"}\n{\"type\":\"assistant\",\"text\":\"second\"}\n";
        let got = extract(raw, OutputFormat::StreamJson).unwrap();
        assert_eq!(got.text, "second");
        assert_eq!(got.source, "stream-json:text");
    }

    // A real `opencode run --format json` transcript (captured from OpenCode
    // 1.17.3 against claude-haiku-4-5): JSONL with a `step_start`, a `text`
    // event carrying the answer under `part.text`, and a `step_finish`. This is
    // the shape that made `extract_json`'s single-document parse fail and left
    // `text` null before the `opencode-parts` fallback.
    const OPENCODE_RUN_JSONL: &str = concat!(
        r#"{"type":"step_start","timestamp":1781179518088,"sessionID":"ses_1496d7d5effenFlBINeoCqBVk8","part":{"id":"prt_eb6928c85001I7CgUE9rTv6VJ0","messageID":"msg_eb69284ee001f9gKyD7PZudPCV","sessionID":"ses_1496d7d5effenFlBINeoCqBVk8","type":"step-start"}}"#,
        "\n",
        r#"{"type":"text","timestamp":1781179518140,"sessionID":"ses_1496d7d5effenFlBINeoCqBVk8","part":{"id":"prt_eb6928c87001tfso5uqcn63dxP","messageID":"msg_eb69284ee001f9gKyD7PZudPCV","sessionID":"ses_1496d7d5effenFlBINeoCqBVk8","type":"text","text":"PING-123","time":{"start":1781179518087,"end":1781179518139}}}"#,
        "\n",
        r#"{"type":"step_finish","timestamp":1781179518188,"sessionID":"ses_1496d7d5effenFlBINeoCqBVk8","part":{"id":"prt_eb6928ce6001n1g7PPB9g6pC3D","reason":"stop","messageID":"msg_eb69284ee001f9gKyD7PZudPCV","sessionID":"ses_1496d7d5effenFlBINeoCqBVk8","type":"step-finish","tokens":{"total":8186,"input":3,"output":7,"reasoning":0,"cache":{"write":8176,"read":0}},"cost":0.010258}}"#,
        "\n",
    );

    #[test]
    fn opencode_jsonl_extracts_text_part_under_json_format() {
        // OpenCode requests `--format json` (OutputFormat::Json) but streams JSONL,
        // so this exercises the fallback inside `extract_json`, not stream-json.
        let got = extract(OPENCODE_RUN_JSONL, OutputFormat::Json).unwrap();
        assert_eq!(got.text, "PING-123");
        assert_eq!(got.source, "json:opencode-parts");
    }

    #[test]
    fn opencode_jsonl_joins_text_parts_and_skips_tool_and_step_parts() {
        // A real tool-using turn (captured the same way): two `text` parts around
        // a `tool` part, plus step-start/step-finish. The final text is the two
        // text parts joined in order; the tool/step parts are not the answer.
        let raw = concat!(
            r#"{"type":"step_start","sessionID":"ses_x","part":{"id":"p0","type":"step-start"}}"#,
            "\n",
            r#"{"type":"text","sessionID":"ses_x","part":{"id":"p1","type":"text","text":"I'll run that shell command for you."}}"#,
            "\n",
            r#"{"type":"tool_use","sessionID":"ses_x","part":{"id":"p2","type":"tool","tool":"bash","state":{"status":"completed","output":"HELLO-FROM-TOOL"}}}"#,
            "\n",
            r#"{"type":"text","sessionID":"ses_x","part":{"id":"p3","type":"text","text":"The command printed: `HELLO-FROM-TOOL`\n\nFINI-42"}}"#,
            "\n",
            r#"{"type":"step_finish","sessionID":"ses_x","part":{"id":"p4","type":"step-finish","cost":0.01,"tokens":{"input":3,"output":7}}}"#,
            "\n",
        );
        let got = extract(raw, OutputFormat::Json).unwrap();
        assert_eq!(
            got.text,
            "I'll run that shell command for you.\nThe command printed: `HELLO-FROM-TOOL`\n\nFINI-42"
        );
        assert_eq!(got.source, "json:opencode-parts");
        // The tool's output is excluded, not surfaced as the answer.
        assert!(!got.text.contains("step-finish"));
    }

    #[test]
    fn opencode_jsonl_with_no_text_parts_yields_none() {
        // A turn that produced only a tool call and step events has no visible
        // answer to extract; `text` stays null and the consumer falls back to
        // stdout rather than oneharness fabricating something.
        let raw = concat!(
            r#"{"type":"step_start","sessionID":"ses_x","part":{"id":"p0","type":"step-start"}}"#,
            "\n",
            r#"{"type":"step_finish","sessionID":"ses_x","part":{"id":"p1","type":"step-finish","cost":0.01}}"#,
            "\n",
        );
        assert!(extract(raw, OutputFormat::Json).is_none());
    }

    #[test]
    fn claude_single_document_still_wins_over_jsonl_fallback() {
        // Guard the no-regression promise: a single-document `result` (Claude
        // Code) must keep extracting via `json:result`, never the opencode path.
        let raw = r#"{"type":"result","result":"the answer","is_error":false}"#;
        let got = extract(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.text, "the answer");
        assert_eq!(got.source, "json:result");
    }

    #[test]
    fn opencode_parts_skip_blank_unparseable_and_partless_lines() {
        // The JSONL scan must skip noise without crashing or mis-extracting: a
        // blank line, a non-JSON line, a JSON object with no `part`, a `part`
        // that is not a text part, and a text part whose text is whitespace —
        // then still recover the one real answer that follows them.
        let raw = concat!(
            "\n",
            "   \n",
            "not json at all\n",
            r#"{"type":"noise"}"#,
            "\n",
            r#"{"type":"reasoning","part":{"type":"reasoning","text":"thinking out loud"}}"#,
            "\n",
            r#"{"type":"text","part":{"type":"text","text":"   "}}"#,
            "\n",
            r#"{"type":"text","part":{"type":"text","text":"REAL-ANSWER"}}"#,
            "\n",
        );
        let got = extract(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.text, "REAL-ANSWER");
        assert_eq!(got.source, "json:opencode-parts");
    }

    #[test]
    fn stream_json_skips_blank_and_unparseable_lines() {
        // stream-json events interleaved with a blank line and a non-JSON line;
        // the scan ignores both and still returns the last usable text.
        let raw = concat!(
            "\n",
            "<<< not json >>>\n",
            "{\"type\":\"assistant\",\"text\":\"only-line\"}\n",
        );
        let got = extract(raw, OutputFormat::StreamJson).unwrap();
        assert_eq!(got.text, "only-line");
        assert_eq!(got.source, "stream-json:text");
    }

    #[test]
    fn stream_json_with_no_text_yields_none() {
        // Events that carry no extractable text leave `text` null rather than
        // fabricating an answer.
        let raw = "{\"type\":\"system\"}\n{\"type\":\"ping\"}\n";
        assert!(extract(raw, OutputFormat::StreamJson).is_none());
    }

    #[test]
    fn json_text_ignores_non_string_and_empty_key_values() {
        // A known key whose value is the wrong type (number) or blank is not a
        // valid answer; a non-string/non-object document yields nothing at all.
        assert!(extract(r#"{"result":42}"#, OutputFormat::Json).is_none());
        assert!(extract(r#"{"result":"   "}"#, OutputFormat::Json).is_none());
        assert!(extract("12345", OutputFormat::Json).is_none());
    }
}
