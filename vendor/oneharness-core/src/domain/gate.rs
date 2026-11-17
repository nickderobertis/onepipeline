//! The runtime *gate*: given a harness's pre-tool hook event on stdin, decide
//! whether to block the call and render that harness's native deny verdict.
//!
//! This is the counterpart to hook *installation* (`io::hooks`): installation
//! writes a hook that invokes `oneharness gate <id>`; this module is what that
//! invocation runs. It is pure — bytes in, bytes out — so the per-harness wire
//! protocol is unit-testable without a real harness. The thin stdin/stdout
//! wrapper lives in the binary (`src/commands/gate.rs`).
//!
//! Only a **deny** is ever emitted; any non-deny is empty stdout, the universal
//! fall-through every harness reads as "no objection" (under bypass the call then
//! runs). The deny shapes were sourced verbatim from the
//! `nickderobertis/allowlister` harness adapters — each harness's published hook
//! protocol — never guessed. The decision itself is intentionally trivial (a
//! substring match): oneharness's gate exists to *prove the installed hook is
//! honored* end to end, not to be a policy engine (that is allowlister's job).

use serde_json::json;

/// How a harness expresses a pre-tool *deny* on stdout. The exit code is always
/// `0`; the verdict is the JSON alone (see the allowlister adapters).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyShape {
    /// Claude Code / Codex / Qwen: nested under `hookSpecificOutput` with a
    /// `PreToolUse` event name and `permissionDecision: "deny"`.
    ClaudeNested,
    /// Copilot: the same `permissionDecision` field, flat (no `hookSpecificOutput`
    /// envelope).
    CopilotFlat,
    /// Cursor: `permission: "deny"`, with the reason under both `agentMessage`
    /// (its published types) and `agent_message` (its docs) since builds disagree.
    CursorPermission,
    /// Crush / OpenCode / Goose: a flat `decision` field with a `reason` — the
    /// value is `"deny"` for Crush and OpenCode, `"block"` for Goose.
    Decision(&'static str),
}

/// Render the stdout a gate writes to *block* a call, carrying `reason`. Pure:
/// the returned string is exactly the JSON the harness reads (the caller appends
/// the trailing newline).
pub fn render_deny(shape: DenyShape, reason: &str) -> String {
    let value = match shape {
        DenyShape::ClaudeNested => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }),
        DenyShape::CopilotFlat => json!({
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }),
        DenyShape::CursorPermission => json!({
            "permission": "deny",
            "agentMessage": reason,
            "agent_message": reason,
        }),
        DenyShape::Decision(verdict) => json!({
            "decision": verdict,
            "reason": reason,
        }),
    };
    value.to_string()
}

/// Decide whether a hook event should be blocked: `true` iff `event` (the raw
/// hook JSON the harness piped to stdin) contains `deny_substr`. Extraction is
/// deliberately harness-agnostic — the marker lives in the tool's command/args,
/// which always serialize into the event — so a gate needs no per-harness field
/// knowledge to *decide*, only to *render* the verdict. An empty needle never
/// matches (a gate with no deny marker allows everything).
pub fn should_deny(event: &str, deny_substr: &str) -> bool {
    !deny_substr.is_empty() && event.contains(deny_substr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn parsed(shape: DenyShape) -> Value {
        serde_json::from_str(&render_deny(shape, "oneharness: blocked")).unwrap()
    }

    #[test]
    fn claude_nested_shape_matches_the_protocol() {
        let v = parsed(DenyShape::ClaudeNested);
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            v["hookSpecificOutput"]["permissionDecisionReason"],
            "oneharness: blocked"
        );
    }

    #[test]
    fn copilot_is_flat() {
        let v = parsed(DenyShape::CopilotFlat);
        assert_eq!(v["permissionDecision"], "deny");
        assert!(v.get("hookSpecificOutput").is_none());
    }

    #[test]
    fn cursor_carries_both_message_spellings() {
        let v = parsed(DenyShape::CursorPermission);
        assert_eq!(v["permission"], "deny");
        assert_eq!(v["agentMessage"], "oneharness: blocked");
        assert_eq!(v["agent_message"], "oneharness: blocked");
    }

    #[test]
    fn decision_field_carries_deny_or_block() {
        assert_eq!(parsed(DenyShape::Decision("deny"))["decision"], "deny");
        assert_eq!(parsed(DenyShape::Decision("block"))["decision"], "block");
        assert_eq!(
            parsed(DenyShape::Decision("block"))["reason"],
            "oneharness: blocked"
        );
    }

    #[test]
    fn should_deny_substring_matches_command_in_event() {
        let event = r#"{"tool_name":"Bash","tool_input":{"command":"touch DENYME-123.txt"}}"#;
        assert!(should_deny(event, "DENYME-123"));
        assert!(!should_deny(event, "OTHER-MARKER"));
        assert!(!should_deny(event, ""), "an empty needle must never match");
    }
}
