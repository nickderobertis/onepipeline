//! Pure rendering of a normalized hook into each harness's native hook shape.
//!
//! oneharness models a pre-tool gate as one harness-agnostic [`HookSpec`] — a
//! command to run before a tool call, optionally scoped by a `matcher` — and the
//! registry tags each harness with the [`HookShape`] its config file expects.
//! This module turns the two into the exact JSON a harness reads. It is pure: no
//! file is touched here, so the same render feeds an in-file merge, a dedicated
//! hooks file, or a plugin's bundled `hooks.json` — delivery (which file,
//! seeding, plugin shims) lives in `src/io/sync.rs`.
//!
//! The shapes mirror the known-good formats each real CLI accepts, sourced from
//! the `nickderobertis/allowlister` adapters — never guessed; pinned
//! byte-for-byte in the tests below so a drift in a harness's schema fails here.

use serde_json::{json, Map, Value};

/// A normalized, harness-agnostic pre-tool hook: run `command` before a tool
/// call, optionally only when the tool matches `matcher` (each harness
/// interprets the matcher in its own dialect — a regex on the tool name for most
/// of them), with an optional `timeout` in seconds for the harnesses whose
/// schema carries one. The command is supplied by the caller, never invented
/// here, so this stays a generic installer rather than any one tool's wiring.
#[derive(Debug, Clone, PartialEq)]
pub struct HookSpec {
    pub command: String,
    pub matcher: Option<String>,
    pub timeout: Option<u64>,
    /// Identity for the plugin-delivered harnesses (Goose, OpenCode): it names
    /// the plugin directory/file/manifest and the dedicated Copilot hook file,
    /// so two tools installing hooks never clobber each other's plugin. Ignored
    /// by harnesses that merge into a shared config file. The delivery layer
    /// falls back to `oneharness` when this is `None`.
    pub plugin_name: Option<String>,
    /// Human-facing description for the harnesses that carry plugin metadata
    /// (currently Goose's `plugin.json`), so a consuming tool can brand its own
    /// plugin rather than inherit oneharness's default text. `None` keeps the
    /// default; ignored by harnesses whose hook carries no description field.
    pub description: Option<String>,
}

impl HookSpec {
    /// A bare command hook with no matcher, timeout, plugin name, or description.
    pub fn command(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            matcher: None,
            timeout: None,
            plugin_name: None,
            description: None,
        }
    }
}

/// How one harness's config file spells a pre-tool hook. Pure registry data: the
/// shape decides how [`render`] lays out the entry. Each variant carries the
/// harness's event name(s), since those differ across CLIs (`PreToolUse`,
/// `preToolUse`, Cursor's three `before*` events).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HookShape {
    /// Claude Code / Qwen / Codex / Goose: an event maps to a list of groups,
    /// each `{matcher?, hooks:[{type:"command", command, timeout?}]}`. `with_timeout`
    /// flags the harnesses whose schema accepts the per-hook `timeout` (Claude
    /// Code and Goose) — the key is still only emitted when the spec provides one.
    Nested {
        event: &'static str,
        with_timeout: bool,
    },
    /// Crush: a flatter group `{matcher?, command, timeout?}` — no inner `hooks`
    /// list, `matcher` is a regex on the tool name.
    Flat { event: &'static str },
    /// Cursor: each named event maps to a list of `{command}` entries; there is
    /// no matcher, the event itself is the scope.
    CommandOnly { events: &'static [&'static str] },
    /// Copilot: one event maps to `{type:"command", bash, powershell}` — the
    /// same command run under either shell.
    CrossShell { event: &'static str },
}

/// Render `spec` into the value that belongs at a harness's hooks key: an object
/// mapping the harness's event name(s) to its native entry list. Pure.
///
/// Delivery merges this under the harness's hooks path (an existing settings
/// file, a dedicated hooks file, or a plugin's `hooks.json`); the array-union in
/// [`crate::domain::sync::deep_merge`] makes re-rendering the same spec
/// idempotent.
pub fn render(spec: &HookSpec, shape: HookShape) -> Value {
    match shape {
        HookShape::Nested {
            event,
            with_timeout,
        } => {
            let mut hook = Map::new();
            hook.insert("type".into(), json!("command"));
            hook.insert("command".into(), json!(spec.command));
            if with_timeout {
                if let Some(timeout) = spec.timeout {
                    hook.insert("timeout".into(), json!(timeout));
                }
            }
            let mut group = Map::new();
            if let Some(matcher) = &spec.matcher {
                group.insert("matcher".into(), json!(matcher));
            }
            group.insert("hooks".into(), Value::Array(vec![Value::Object(hook)]));
            event_map(&[event], Value::Array(vec![Value::Object(group)]))
        }
        HookShape::Flat { event } => {
            let mut group = Map::new();
            if let Some(matcher) = &spec.matcher {
                group.insert("matcher".into(), json!(matcher));
            }
            group.insert("command".into(), json!(spec.command));
            if let Some(timeout) = spec.timeout {
                group.insert("timeout".into(), json!(timeout));
            }
            event_map(&[event], Value::Array(vec![Value::Object(group)]))
        }
        HookShape::CommandOnly { events } => {
            let entry = Value::Array(vec![json!({ "command": spec.command })]);
            event_map(events, entry)
        }
        HookShape::CrossShell { event } => {
            let entry = json!({
                "type": "command",
                "bash": spec.command,
                "powershell": spec.command,
            });
            event_map(&[event], Value::Array(vec![entry]))
        }
    }
}

/// Map each event name to a clone of `entries` (the same hook lands on every
/// event a harness gates with — e.g. Cursor's three `before*` events).
fn event_map(events: &[&str], entries: Value) -> Value {
    let mut map = Map::new();
    for event in events {
        map.insert((*event).to_string(), entries.clone());
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nested `{matcher, hooks:[{type, command}]}` shape Codex and Qwen read —
    /// no timeout in their schema, so a provided timeout is dropped.
    #[test]
    fn nested_without_timeout_drops_timeout() {
        let spec = HookSpec {
            command: "guard hook codex".into(),
            matcher: Some("Bash".into()),
            timeout: Some(10),
            plugin_name: None,
            description: None,
        };
        let shape = HookShape::Nested {
            event: "PreToolUse",
            with_timeout: false,
        };
        assert_eq!(
            render(&spec, shape),
            json!({
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [ { "type": "command", "command": "guard hook codex" } ],
                    }
                ]
            }),
            "timeout must be dropped for a shape whose schema has no timeout",
        );
    }

    /// Claude Code and Goose are the nested shapes that *do* carry a per-hook
    /// timeout, emitted when the spec provides one.
    #[test]
    fn nested_with_timeout_keeps_timeout() {
        let spec = HookSpec {
            command: "guard hook claude-code".into(),
            matcher: Some("Bash".into()),
            timeout: Some(10),
            plugin_name: None,
            description: None,
        };
        let shape = HookShape::Nested {
            event: "PreToolUse",
            with_timeout: true,
        };
        assert_eq!(
            render(&spec, shape),
            json!({
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "guard hook claude-code", "timeout": 10 }
                        ],
                    }
                ]
            }),
        );
    }

    /// Crush's flat group: command and timeout sit directly on the entry.
    #[test]
    fn flat_matches_crush_shape() {
        let spec = HookSpec {
            command: "guard hook crush".into(),
            matcher: Some("bash".into()),
            timeout: Some(10),
            plugin_name: None,
            description: None,
        };
        assert_eq!(
            render(
                &spec,
                HookShape::Flat {
                    event: "PreToolUse"
                }
            ),
            json!({
                "PreToolUse": [
                    { "matcher": "bash", "command": "guard hook crush", "timeout": 10 }
                ]
            }),
        );
    }

    /// Cursor fans the same command across its three `before*` events plus
    /// `preToolUse` (the one event whose reply can rewrite the tool's input),
    /// with no matcher key.
    #[test]
    fn command_only_matches_cursor_shape() {
        let spec = HookSpec::command("guard hook cursor");
        let shape = HookShape::CommandOnly {
            events: &[
                "beforeShellExecution",
                "beforeReadFile",
                "beforeMCPExecution",
                "preToolUse",
            ],
        };
        assert_eq!(
            render(&spec, shape),
            json!({
                "beforeShellExecution": [ { "command": "guard hook cursor" } ],
                "beforeReadFile": [ { "command": "guard hook cursor" } ],
                "beforeMCPExecution": [ { "command": "guard hook cursor" } ],
                "preToolUse": [ { "command": "guard hook cursor" } ],
            }),
        );
    }

    /// Copilot carries the one command under both `bash` and `powershell`.
    #[test]
    fn cross_shell_matches_copilot_shape() {
        let spec = HookSpec::command("guard hook copilot");
        assert_eq!(
            render(
                &spec,
                HookShape::CrossShell {
                    event: "preToolUse"
                }
            ),
            json!({
                "preToolUse": [
                    {
                        "type": "command",
                        "bash": "guard hook copilot",
                        "powershell": "guard hook copilot",
                    }
                ]
            }),
        );
    }

    /// A spec with no matcher omits the key entirely (the harness treats an
    /// absent matcher as match-all) rather than emitting `null` or `""`.
    #[test]
    fn absent_matcher_is_omitted_not_nulled() {
        let spec = HookSpec::command("guard hook claude-code");
        let nested = render(
            &spec,
            HookShape::Nested {
                event: "PreToolUse",
                with_timeout: false,
            },
        );
        let group = &nested["PreToolUse"][0];
        assert!(
            group.get("matcher").is_none(),
            "absent matcher must not appear: {group}",
        );
        let flat = render(
            &spec,
            HookShape::Flat {
                event: "PreToolUse",
            },
        );
        assert!(flat["PreToolUse"][0].get("matcher").is_none());
    }

    /// An absent timeout on a timeout-bearing shape omits the key, leaving the
    /// harness to apply its own default.
    #[test]
    fn absent_timeout_is_omitted() {
        let spec = HookSpec {
            command: "guard hook goose".into(),
            matcher: None,
            timeout: None,
            plugin_name: None,
            description: None,
        };
        let rendered = render(
            &spec,
            HookShape::Nested {
                event: "PreToolUse",
                with_timeout: true,
            },
        );
        assert!(rendered["PreToolUse"][0]["hooks"][0]
            .get("timeout")
            .is_none());
    }
}
