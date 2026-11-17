//! The runtime *mock/spy responder*: given a harness's pre-tool hook event on
//! stdin and a caller-supplied ruleset, decide whether to intercept the call and
//! render that harness's native verdict — a **deny** (the model reads the
//! message as tool feedback) or an **input rewrite** (the call runs with
//! substituted arguments, so a shell command can be swapped for a stub that
//! prints canned output, or a file read redirected to a fixture).
//!
//! This is the read-write sibling of [`crate::domain::gate`] and rides the same
//! installed-hook loop: `oneharness sync` installs a hook that invokes
//! `oneharness mock <id> --rules <file>`; this module is what that invocation
//! runs. It is pure — rules in, verdict out — so the per-harness wire protocol
//! is unit-testable without a real harness. The thin stdin/stdout wrapper (and
//! the spy-log append, the one I/O) lives in the binary (`src/commands/mock.rs`).
//!
//! Verdict shapes are per-harness registry data
//! ([`crate::domain::harness::HarnessSpec::mock_rewrite`] for rewrites;
//! [`DenyShape`] for denies), sourced from each CLI's published hook protocol,
//! never guessed — and **loud when absent**: a ruleset asking for an action a
//! harness cannot express is a usage error, never a silent allow. Which
//! harnesses honor a rewrite live is drift-alarmed by the `oh_mock_enforce`
//! e2e phases; the `explore-hooks` probe is how a new shape gets sourced (see
//! `docs/mock-spy-design.md`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain::gate::DenyShape;

/// How a harness expresses "allow this call, but with these rewritten
/// arguments" from a pre-tool hook. Every variant is live-verified (the
/// `explore-hooks` probe and/or an `oh_mock_enforce` phase — see
/// `docs/mock-spy-design.md` for the per-harness evidence); absent for a
/// harness whose protocol has no rewrite verdict (Goose), whose hooks never
/// fire headlessly (Copilot), or whose documented rewrite was live-refuted
/// (Qwen).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewriteShape {
    /// Claude Code / Codex: `hookSpecificOutput.updatedInput` beside an
    /// `allow` permission decision. (Qwen documents this shape too but was
    /// live-refuted — see the registry.)
    ClaudeNested,
    /// Crush: a flat `{"version":1,"decision":"allow","updated_input":{…}}`
    /// (its `updated_input` is a shallow-merge patch of the tool input).
    CrushFlat,
    /// OpenCode: the oneharness plugin shim applies a flat
    /// `{"decision":"allow","updated_input":{…}}` reply by merging it into the
    /// tool's mutable `args` before execution.
    OpencodeShim,
    /// Cursor: a flat `{"permission":"allow","updated_input":{…}}` on its
    /// `preToolUse` event. Exactly the probe-verified reply — no reason slot
    /// (extra fields are unverified against its parser, so none are sent).
    CursorPermission,
}

impl RewriteShape {
    /// Stable token for JSON surfaces (`oneharness list`).
    pub fn as_str(self) -> &'static str {
        match self {
            RewriteShape::ClaudeNested => "claude-nested",
            RewriteShape::CrushFlat => "crush-flat",
            RewriteShape::OpencodeShim => "opencode-shim",
            RewriteShape::CursorPermission => "cursor-permission",
        }
    }
}

/// How `run --mock-rules` / `run --spy-file` delivers the mock hook to this
/// harness **for one invocation** — the single-flag ephemeral path. Every
/// variant is live-verified; `None` (qwen: project hooks don't fire headlessly;
/// copilot: hooks never fire headlessly at all) makes the flag a loud usage
/// error for the harness, never a silently inert install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockDelivery {
    /// The hook rides the argv via a per-run settings flag (Claude Code's
    /// `--settings <file>`, probe-verified to load hooks headlessly): zero
    /// workspace mutation — existing project/user config still applies, the
    /// mock hook is layered on top for this invocation only.
    SettingsFlag { flag: &'static str },
    /// The hook is installed into the project-scope config in the working
    /// directory via the non-destructive merge (existing keys and hooks
    /// preserved — layered on top), with every touched file snapshotted first
    /// and restored after the run. `extra_args` are appended to the harness's
    /// argv — how Codex's hooks engine is opted in per invocation.
    ProjectHooks { extra_args: &'static [&'static str] },
}

/// Render the hook command an installed mock hook runs: this binary's `mock`
/// verb with the ruleset and spy log wired in. Paths are embedded verbatim, so
/// the caller must have refused whitespace-bearing ones first (the OpenCode
/// shim tokenizes the command on spaces, and shell-run hooks would split too).
pub fn hook_command(exe: &str, id: &str, rules: Option<&str>, spy: Option<&str>) -> String {
    let mut command = format!("{exe} mock {id}");
    if let Some(rules) = rules {
        command.push_str(" --rules ");
        command.push_str(rules);
    }
    if let Some(spy) = spy {
        command.push_str(" --spy-file ");
        command.push_str(spy);
    }
    command
}

/// The settings JSON a [`MockDelivery::SettingsFlag`] harness receives: a
/// PreToolUse hook (no matcher — every tool) invoking `command`. Exactly the
/// shape the explore-hooks probe verified Claude Code loads from a per-run
/// `--settings` file.
pub fn settings_hooks_json(command: &str) -> String {
    json!({
        "hooks": {
            "PreToolUse": [
                { "hooks": [ { "type": "command", "command": command } ] }
            ]
        }
    })
    .to_string()
}

/// Compile a [`Action::Stub`] into the substituted shell-tool input: a
/// `printf` of the declared output (single-quoted with POSIX escaping, so any
/// text — quotes, `$`, backticks, newlines — is emitted verbatim and nothing
/// is ever interpreted), plus an `exit` when a non-zero code fakes a failure.
pub fn stub_input(output: &str, exit_code: i32) -> Value {
    let quoted = format!("'{}'", output.replace('\'', "'\\''"));
    let mut command = format!("printf '%s\\n' {quoted}");
    if exit_code != 0 {
        command.push_str(&format!("; exit {exit_code}"));
    }
    json!({ "command": command })
}

/// A parsed mock ruleset: the first rule whose `match` covers the event wins.
/// Deserialized from the JSON file `oneharness mock --rules <path>` reads;
/// unknown fields are rejected loudly (a typo must never become a silent
/// allow-everything).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MockRules {
    pub rules: Vec<MockRule>,
}

/// One rule: match criteria plus the action to take when they hold.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MockRule {
    #[serde(rename = "match")]
    pub matcher: MatchSpec,
    pub action: Action,
}

/// What a rule matches on. At least one criterion is required (an empty match
/// would silently intercept everything); when several are given, **all** must
/// hold (AND). Criteria: the tool name (exact or regex), the raw event JSON
/// (substring or regex), and per-field predicates on the tool's input.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchSpec {
    /// Case-insensitive exact match on the event's tool name. Tool names are
    /// per-harness (`Bash` on Claude Code, `bash` on OpenCode/Crush,
    /// `run_shell_command` on Qwen), so cross-harness rules usually prefer
    /// `event_contains` or an `input` predicate.
    #[serde(default)]
    pub tool: Option<String>,
    /// Regex (RE2, unanchored) on the tool name — e.g. `"^(Bash|bash)$"` to
    /// span a harness's casing. Anchor with `^…$` for an exact match.
    #[serde(default)]
    pub tool_regex: Option<String>,
    /// Substring match over the raw event JSON — harness-agnostic, because the
    /// tool's command/args always serialize into the event (the same principle
    /// as [`crate::domain::gate::should_deny`]).
    #[serde(default)]
    pub event_contains: Option<String>,
    /// Regex (RE2, unanchored) over the raw event JSON — e.g. `"git\\s+push"`.
    #[serde(default)]
    pub event_regex: Option<String>,
    /// Per-field predicates on the tool's input arguments (`tool_input`, or
    /// Copilot's `toolArgs`): the map key is the argument name (`command`,
    /// `file_path`, …) and the value a [`StringPredicate`]. All listed fields
    /// must match, and a field absent from the event fails the rule. A field
    /// whose value is not a string is compared against its compact JSON form,
    /// so a predicate can still target an array/object argument.
    #[serde(default)]
    pub input: std::collections::BTreeMap<String, StringPredicate>,
}

/// A predicate on one string value (a `tool_input` field, matched by
/// [`MatchSpec::input`]). At least one form must be set; when several are, all
/// must hold (AND). `equals` is an exact match (an empty string is allowed —
/// it matches only an empty value, not everything); `contains` is a substring;
/// `regex` is an unanchored RE2 (linear-time) match. `contains`/`regex` reject
/// an empty pattern (it would match everything).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringPredicate {
    #[serde(default)]
    pub equals: Option<String>,
    #[serde(default)]
    pub contains: Option<String>,
    #[serde(default)]
    pub regex: Option<String>,
}

impl StringPredicate {
    /// Whether every set form holds for `value`. A regex that somehow fails to
    /// compile (validated at parse time, so unreachable in practice) is treated
    /// as a non-match — never an accidental match-everything.
    fn matches(&self, value: &str) -> bool {
        if let Some(want) = &self.equals {
            if value != want {
                return false;
            }
        }
        if let Some(needle) = &self.contains {
            if !value.contains(needle.as_str()) {
                return false;
            }
        }
        if let Some(pattern) = &self.regex {
            match regex::Regex::new(pattern) {
                Ok(re) if re.is_match(value) => {}
                _ => return false,
            }
        }
        true
    }

    /// The forms that are set (for validation): `(any_set, has_empty_needle)`.
    fn forms(&self) -> (bool, bool) {
        let any = self.equals.is_some() || self.contains.is_some() || self.regex.is_some();
        let empty_needle =
            self.contains.as_deref() == Some("") || self.regex.as_deref() == Some("");
        (any, empty_needle)
    }
}

/// The interception to perform when a rule matches.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Block the call; the harness surfaces `message` to the model as the
    /// tool's feedback. Expressible wherever `oneharness gate` works.
    Deny { message: String },
    /// Allow the call with `input` substituted for the tool's arguments — the
    /// general rewrite: swap a shell command, or redirect a read's `file_path`
    /// to a fixture. `input` is passed to the harness verbatim (each applies
    /// its own semantics; Crush shallow-merges).
    Rewrite {
        input: Value,
        /// Optional reason surfaced where the harness's shape carries one.
        #[serde(default)]
        message: Option<String>,
    },
    /// Fake a SHELL call's result by declaring only the output (and optional
    /// exit code): oneharness generates a safely-quoted `printf` stub itself
    /// and delivers it as an input rewrite, so no user-authored command — and
    /// nothing real — ever executes. The model receives `output` (plus a
    /// trailing newline, like real command output) as the tool's genuine
    /// result. Sugar over [`Action::Rewrite`], so it needs the same
    /// `mock_rewrite` capability; shell tools only (their input is a `command`
    /// field on every rewrite-capable harness) — mock file reads with a
    /// `rewrite` of `file_path` instead.
    Stub {
        output: String,
        /// The stub's exit code (default 0). Non-zero fakes a failing command.
        #[serde(default)]
        exit_code: i32,
    },
}

impl Action {
    /// Stable token for the spy log and error messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Action::Deny { .. } => "deny",
            Action::Rewrite { .. } => "rewrite",
            Action::Stub { .. } => "stub",
        }
    }
}

/// Parse and validate a ruleset. Loud on any fault — an unparseable or invalid
/// ruleset must abort the run (usage error), never degrade to allow-everything.
pub fn parse_rules(text: &str) -> Result<MockRules, String> {
    let rules: MockRules = serde_json::from_str(text).map_err(|e| e.to_string())?;
    for (i, rule) in rules.rules.iter().enumerate() {
        let m = &rule.matcher;
        // Empty top-level needles would match everything — refuse each loudly.
        for (field, value) in [
            ("tool", &m.tool),
            ("tool_regex", &m.tool_regex),
            ("event_contains", &m.event_contains),
            ("event_regex", &m.event_regex),
        ] {
            if value.as_deref() == Some("") {
                return Err(format!(
                    "rule {i}: empty `match.{field}` is not allowed (it would match everything)"
                ));
            }
        }
        // Compile the regex criteria so an invalid pattern is a loud parse
        // error, never a silent match-nothing at runtime.
        for (field, pattern) in [
            ("tool_regex", &m.tool_regex),
            ("event_regex", &m.event_regex),
        ] {
            if let Some(pattern) = pattern {
                regex::Regex::new(pattern)
                    .map_err(|e| format!("rule {i}: invalid `match.{field}` regex: {e}"))?;
            }
        }
        // Per-field input predicates: each needs a form, no empty needle, valid
        // regex.
        for (key, pred) in &m.input {
            let (any, empty_needle) = pred.forms();
            if !any {
                return Err(format!(
                    "rule {i}: `match.input.{key}` needs one of `equals`/`contains`/`regex`"
                ));
            }
            if empty_needle {
                return Err(format!(
                    "rule {i}: empty `contains`/`regex` in `match.input.{key}` is not allowed (it would match everything)"
                ));
            }
            if let Some(pattern) = &pred.regex {
                regex::Regex::new(pattern).map_err(|e| {
                    format!("rule {i}: invalid `match.input.{key}.regex` regex: {e}")
                })?;
            }
        }
        // At least one criterion, or the rule intercepts everything.
        let no_criteria = m.tool.is_none()
            && m.tool_regex.is_none()
            && m.event_contains.is_none()
            && m.event_regex.is_none()
            && m.input.is_empty();
        if no_criteria {
            return Err(format!(
                "rule {i}: `match` needs at least one of `tool`, `tool_regex`, `event_contains`, `event_regex`, or `input`"
            ));
        }
        if let Action::Rewrite { input, .. } = &rule.action {
            if !input.is_object() {
                return Err(format!(
                    "rule {i}: `rewrite.input` must be a JSON object (the substituted tool arguments)"
                ));
            }
        }
    }
    Ok(rules)
}

/// The action a ruleset uses that the harness cannot express, if any — checked
/// up front so an unrenderable ruleset is a loud usage error before any event
/// is read, never a silent downgrade at match time.
pub fn unsupported_action(
    rules: &MockRules,
    gate_deny: Option<DenyShape>,
    rewrite: Option<RewriteShape>,
) -> Option<&'static str> {
    for rule in &rules.rules {
        match rule.action {
            Action::Deny { .. } if gate_deny.is_none() => return Some("deny"),
            Action::Rewrite { .. } if rewrite.is_none() => return Some("rewrite"),
            // A stub is delivered as a rewrite, so it needs the same shape.
            Action::Stub { .. } if rewrite.is_none() => return Some("stub"),
            _ => {}
        }
    }
    None
}

/// Decide which rule (if any) intercepts `event` — the raw hook JSON the
/// harness piped to stdin. First match wins; no match means allow-through
/// (empty stdout, the universal "no objection").
pub fn decide<'r>(event: &str, rules: &'r MockRules) -> Option<(usize, &'r Action)> {
    // Parse the event once (the process handles a single hook call), so every
    // rule sees the same tool name and input object without re-parsing.
    let parsed: Option<Value> = serde_json::from_str(event.trim()).ok();
    let tool = parsed.as_ref().and_then(tool_name_of);
    rules
        .rules
        .iter()
        .enumerate()
        .find(|(_, rule)| rule_matches(rule, event, tool.as_deref(), parsed.as_ref()))
        .map(|(i, rule)| (i, &rule.action))
}

fn rule_matches(rule: &MockRule, event: &str, tool: Option<&str>, parsed: Option<&Value>) -> bool {
    let m = &rule.matcher;
    if let Some(want) = m.tool.as_deref() {
        // A `tool` criterion can only match an event that names its tool; an
        // empty want never matches (also rejected at parse time).
        match tool {
            Some(name) if !want.is_empty() && name.eq_ignore_ascii_case(want) => {}
            _ => return false,
        }
    }
    if let Some(pattern) = m.tool_regex.as_deref() {
        match (tool, regex::Regex::new(pattern)) {
            (Some(name), Ok(re)) if re.is_match(name) => {}
            _ => return false,
        }
    }
    if let Some(needle) = m.event_contains.as_deref() {
        if needle.is_empty() || !event.contains(needle) {
            return false;
        }
    }
    if let Some(pattern) = m.event_regex.as_deref() {
        match regex::Regex::new(pattern) {
            Ok(re) if re.is_match(event) => {}
            _ => return false,
        }
    }
    if !m.input.is_empty() {
        let args = parsed.and_then(tool_input_of);
        for (key, pred) in &m.input {
            // A field absent from the event fails the rule (never fabricated).
            let Some(value) = args.and_then(|a| a.get(key)) else {
                return false;
            };
            // Match a string field directly; coerce anything else to its
            // compact JSON so a predicate can still target a non-string arg.
            let owned;
            let text = match value.as_str() {
                Some(s) => s,
                None => {
                    owned = serde_json::to_string(value).unwrap_or_default();
                    &owned
                }
            };
            if !pred.matches(text) {
                return false;
            }
        }
    }
    true
}

/// Best-effort tool name from a hook event: the field every gated harness (and
/// the oneharness OpenCode shim) uses is `tool_name`; `toolName` (Copilot) and
/// `tool` are accepted for robustness. `None` when the event is not JSON or
/// names no tool — a `tool` matcher then simply cannot match (never fabricated).
pub fn extract_tool_name(event: &str) -> Option<String> {
    let value: Value = serde_json::from_str(event.trim()).ok()?;
    tool_name_of(&value)
}

/// The tool name from an already-parsed event value.
fn tool_name_of(value: &Value) -> Option<String> {
    for key in ["tool_name", "toolName", "tool"] {
        if let Some(name) = value.get(key).and_then(Value::as_str) {
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// The tool's input-arguments object from a parsed event: `tool_input` (every
/// gated harness + the OpenCode shim) or `toolArgs` (Copilot).
fn tool_input_of(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    for key in ["tool_input", "toolArgs"] {
        if let Some(obj) = value.get(key).and_then(Value::as_object) {
            return Some(obj);
        }
    }
    None
}

/// Render the stdout that substitutes `input` for the tool's arguments,
/// carrying `reason` where the harness's shape has a slot for one. Pure: the
/// returned string is exactly the JSON the harness (or the OpenCode shim)
/// reads; the caller appends the trailing newline.
pub fn render_rewrite(shape: RewriteShape, input: &Value, reason: &str) -> String {
    let value = match shape {
        RewriteShape::ClaudeNested => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": reason,
                "updatedInput": input,
            }
        }),
        RewriteShape::CrushFlat => json!({
            "version": 1,
            "decision": "allow",
            "reason": reason,
            "updated_input": input,
        }),
        RewriteShape::OpencodeShim => json!({
            "decision": "allow",
            "reason": reason,
            "updated_input": input,
        }),
        // Cursor's probe-verified reply carries no reason slot; sending only
        // what was verified keeps its parser from rejecting the verdict.
        RewriteShape::CursorPermission => json!({
            "permission": "allow",
            "updated_input": input,
        }),
    };
    value.to_string()
}

/// One spy-log line: the observed hook event plus what the responder did with
/// it. Appended as JSONL by the command layer for every invocation — with or
/// without a ruleset — so a consumer gets the *original* tool call (pre-rewrite
/// intent), which the harness's own transcript `events` cannot show once an
/// input was substituted.
#[derive(Debug, Serialize)]
pub struct SpyRecord<'a> {
    pub harness: &'a str,
    /// The raw hook event, parsed when it is JSON (else the raw string, never
    /// dropped).
    pub event: Value,
    /// `allow` (no rule matched — the fall-through), `deny`, or `rewrite`.
    pub action: &'static str,
    /// Index of the matched rule in the ruleset; `null` on the fall-through.
    pub rule: Option<usize>,
}

/// Render the spy-log line for one invocation (no trailing newline).
pub fn spy_line(harness: &str, event: &str, decision: Option<(usize, &Action)>) -> String {
    let record = SpyRecord {
        harness,
        event: serde_json::from_str(event.trim())
            .unwrap_or_else(|_| Value::String(event.to_string())),
        action: decision.map_or("allow", |(_, action)| action.kind()),
        rule: decision.map(|(i, _)| i),
    };
    serde_json::to_string(&record).expect("SpyRecord serialization cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(json: &str) -> MockRules {
        parse_rules(json).expect("test ruleset must parse")
    }

    const REWRITE_RULES: &str = r#"{
        "rules": [
            {
                "match": {"tool": "Bash", "event_contains": "git push"},
                "action": {"deny": {"message": "pushes are mocked"}}
            },
            {
                "match": {"event_contains": "git status"},
                "action": {"rewrite": {"input": {"command": "printf clean"}}}
            }
        ]
    }"#;

    #[test]
    fn parse_accepts_valid_rules_and_rejects_faults_loudly() {
        assert_eq!(rules(REWRITE_RULES).rules.len(), 2);
        // Unknown fields are a loud parse error, not a silent skip.
        assert!(parse_rules(r#"{"rules":[],"extra":1}"#).is_err());
        assert!(parse_rules(
            r#"{"rules":[{"match":{"typo_contains":"x"},"action":{"deny":{"message":"m"}}}]}"#
        )
        .is_err());
        // An empty match would intercept everything — refused.
        let err = parse_rules(r#"{"rules":[{"match":{},"action":{"deny":{"message":"m"}}}]}"#)
            .unwrap_err();
        assert!(err.contains("at least one of"), "{err}");
        // Empty strings are refused too (an empty needle matches everything).
        for m in [
            r#"{"tool": ""}"#,
            r#"{"event_contains": ""}"#,
            r#"{"tool": "", "event_contains": "x"}"#,
        ] {
            let text =
                format!(r#"{{"rules":[{{"match":{m},"action":{{"deny":{{"message":"m"}}}}}}]}}"#);
            assert!(parse_rules(&text).is_err(), "{m} must be rejected");
        }
        // A rewrite's input must be the substituted arguments object.
        let err = parse_rules(
            r#"{"rules":[{"match":{"tool":"Bash"},"action":{"rewrite":{"input":"echo"}}}]}"#,
        )
        .unwrap_err();
        assert!(err.contains("must be a JSON object"), "{err}");
        // Not JSON at all.
        assert!(parse_rules("not json").is_err());
    }

    #[test]
    fn decide_first_match_wins_and_falls_through() {
        let r = rules(REWRITE_RULES);
        // Rule 0: tool + substring both hold.
        let event = r#"{"tool_name":"Bash","tool_input":{"command":"git push origin"}}"#;
        let (i, action) = decide(event, &r).expect("must match");
        assert_eq!(i, 0);
        assert_eq!(action.kind(), "deny");
        // Rule 1 matches on substring alone (no tool criterion).
        let event = r#"{"tool_name":"shell","tool_input":{"command":"git status"}}"#;
        let (i, action) = decide(event, &r).expect("must match");
        assert_eq!(i, 1);
        assert_eq!(action.kind(), "rewrite");
        // Neither: fall through.
        assert!(decide(r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#, &r).is_none());
        // Rule 0 requires BOTH criteria: right tool, wrong substring -> rule 1
        // doesn't match either -> fall through.
        assert!(decide(r#"{"tool_name":"Bash","tool_input":{"command":"rm"}}"#, &r).is_none());
        // Wrong tool with rule 0's substring: rule 0 misses, no other rule
        // carries "git push" -> fall through (tool criterion is enforced).
        assert!(decide(
            r#"{"tool_name":"Edit","tool_input":{"command":"git push"}}"#,
            &r
        )
        .is_none());
    }

    #[test]
    fn tool_regex_and_event_regex_match() {
        // One cross-harness rule spanning `Bash`/`bash` and asserting a push.
        let r = rules(
            r#"{"rules":[{"match":{"tool_regex":"^(?i)bash$","event_regex":"git\\s+push"},"action":{"deny":{"message":"m"}}}]}"#,
        );
        assert!(decide(
            r#"{"tool_name":"Bash","tool_input":{"command":"git    push"}}"#,
            &r
        )
        .is_some());
        assert!(decide(
            r#"{"tool_name":"bash","tool_input":{"command":"git push"}}"#,
            &r
        )
        .is_some());
        // Wrong tool, or no push, or no tool name → no match.
        assert!(decide(
            r#"{"tool_name":"Edit","tool_input":{"command":"git push"}}"#,
            &r
        )
        .is_none());
        assert!(decide(
            r#"{"tool_name":"bash","tool_input":{"command":"git status"}}"#,
            &r
        )
        .is_none());
        assert!(decide(r#"{"tool_input":{"command":"git push"}}"#, &r).is_none());
    }

    #[test]
    fn input_field_predicates_match_and_coerce() {
        // Regex on the command field; substring + equals on others.
        let r = rules(
            r#"{"rules":[{"match":{"input":{"command":{"regex":"rm\\s+-rf"}}},"action":{"deny":{"message":"m"}}}]}"#,
        );
        assert!(decide(
            r#"{"tool_name":"bash","tool_input":{"command":"rm  -rf /tmp/x"}}"#,
            &r
        )
        .is_some());
        assert!(decide(r#"{"tool_name":"bash","tool_input":{"command":"ls"}}"#, &r).is_none());
        // An absent field fails the rule (never fabricated).
        assert!(decide(
            r#"{"tool_name":"bash","tool_input":{"other":"rm -rf"}}"#,
            &r
        )
        .is_none());

        // `equals` is exact; multiple criteria on one field AND together.
        let r = rules(
            r#"{"rules":[{"match":{"input":{"file_path":{"equals":"/etc/passwd"}}},"action":{"deny":{"message":"m"}}}]}"#,
        );
        assert!(decide(
            r#"{"tool_name":"Read","tool_input":{"file_path":"/etc/passwd"}}"#,
            &r
        )
        .is_some());
        assert!(decide(
            r#"{"tool_name":"Read","tool_input":{"file_path":"/etc/passwd.bak"}}"#,
            &r
        )
        .is_none());

        // A non-string field is coerced to compact JSON, so a predicate can
        // still target an array/object argument.
        let r = rules(
            r#"{"rules":[{"match":{"input":{"args":{"contains":"--force"}}},"action":{"deny":{"message":"m"}}}]}"#,
        );
        assert!(decide(
            r#"{"tool_name":"exec","tool_input":{"args":["git","push","--force"]}}"#,
            &r
        )
        .is_some());
        assert!(decide(
            r#"{"tool_name":"exec","tool_input":{"args":["git","status"]}}"#,
            &r
        )
        .is_none());

        // Copilot's `toolArgs` is accepted as the input object too.
        let r = rules(
            r#"{"rules":[{"match":{"input":{"command":{"contains":"push"}}},"action":{"deny":{"message":"m"}}}]}"#,
        );
        assert!(decide(
            r#"{"toolName":"shell","toolArgs":{"command":"git push"}}"#,
            &r
        )
        .is_some());
    }

    #[test]
    fn input_and_tool_criteria_are_anded() {
        // Both a tool pin and an input predicate must hold.
        let r = rules(
            r#"{"rules":[{"match":{"tool":"Bash","input":{"command":{"contains":"push"}}},"action":{"deny":{"message":"m"}}}]}"#,
        );
        assert!(decide(
            r#"{"tool_name":"Bash","tool_input":{"command":"git push"}}"#,
            &r
        )
        .is_some());
        // Right command, wrong tool → no match (AND).
        assert!(decide(
            r#"{"tool_name":"Edit","tool_input":{"command":"git push"}}"#,
            &r
        )
        .is_none());
    }

    #[test]
    fn regex_and_input_validation_is_loud() {
        // Invalid regex in each location is a loud parse error.
        for m in [
            r#"{"tool_regex":"("}"#,
            r#"{"event_regex":"["}"#,
            r#"{"input":{"command":{"regex":"("}}}"#,
        ] {
            let text =
                format!(r#"{{"rules":[{{"match":{m},"action":{{"deny":{{"message":"m"}}}}}}]}}"#);
            let err = parse_rules(&text).unwrap_err();
            assert!(
                err.contains("invalid") && err.contains("regex"),
                "{m}: {err}"
            );
        }
        // An input predicate with no form set is refused.
        let err = parse_rules(
            r#"{"rules":[{"match":{"input":{"command":{}}},"action":{"deny":{"message":"m"}}}]}"#,
        )
        .unwrap_err();
        assert!(err.contains("needs one of"), "{err}");
        // Empty contains/regex in an input predicate is refused; empty `equals`
        // is allowed (it matches only an empty value, not everything).
        assert!(parse_rules(
            r#"{"rules":[{"match":{"input":{"c":{"contains":""}}},"action":{"deny":{"message":"m"}}}]}"#
        )
        .is_err());
        let ok = parse_rules(
            r#"{"rules":[{"match":{"input":{"c":{"equals":""}}},"action":{"deny":{"message":"m"}}}]}"#,
        )
        .unwrap();
        assert!(decide(r#"{"tool_name":"x","tool_input":{"c":""}}"#, &ok).is_some());
        assert!(decide(r#"{"tool_name":"x","tool_input":{"c":"nonempty"}}"#, &ok).is_none());
        // Empty tool_regex/event_regex refused (would match everything).
        for f in ["tool_regex", "event_regex"] {
            let text = format!(
                r#"{{"rules":[{{"match":{{"{f}":""}},"action":{{"deny":{{"message":"m"}}}}}}]}}"#
            );
            assert!(parse_rules(&text).is_err(), "{f}");
        }
    }

    #[test]
    fn tool_matching_is_case_insensitive_and_needs_a_named_tool() {
        let r = rules(r#"{"rules":[{"match":{"tool":"bash"},"action":{"deny":{"message":"m"}}}]}"#);
        assert!(decide(r#"{"tool_name":"Bash"}"#, &r).is_some());
        assert!(decide(r#"{"toolName":"BASH"}"#, &r).is_some());
        assert!(decide(r#"{"tool":"bash"}"#, &r).is_some());
        // No tool name in the event: a tool criterion cannot match (honest, not
        // fabricated) — and a non-JSON event can never satisfy it either.
        assert!(decide(r#"{"command":"bash stuff"}"#, &r).is_none());
        assert!(decide("not json", &r).is_none());
    }

    #[test]
    fn extract_tool_name_reads_known_fields_only() {
        assert_eq!(
            extract_tool_name(r#"{"tool_name":"Bash"}"#).as_deref(),
            Some("Bash")
        );
        assert_eq!(
            extract_tool_name(r#"{"toolName":"shell"}"#).as_deref(),
            Some("shell")
        );
        assert_eq!(
            extract_tool_name(r#"{"tool":"bash"}"#).as_deref(),
            Some("bash")
        );
        // Precedence: tool_name first (the field every gated harness uses).
        assert_eq!(
            extract_tool_name(r#"{"tool":"b","tool_name":"a"}"#).as_deref(),
            Some("a")
        );
        assert!(extract_tool_name(r#"{"tool_name":""}"#).is_none());
        assert!(extract_tool_name(r#"{"tool_name":42}"#).is_none());
        assert!(extract_tool_name("nope").is_none());
    }

    #[test]
    fn unsupported_action_reports_the_first_unrenderable_verb() {
        let r = rules(REWRITE_RULES);
        // Everything renderable: no complaint.
        assert!(unsupported_action(
            &r,
            Some(DenyShape::ClaudeNested),
            Some(RewriteShape::ClaudeNested)
        )
        .is_none());
        // No rewrite shape: the rewrite rule is unrenderable.
        assert_eq!(
            unsupported_action(&r, Some(DenyShape::ClaudeNested), None),
            Some("rewrite")
        );
        // No deny shape either: deny reported (it is the first offending rule).
        assert_eq!(unsupported_action(&r, None, None), Some("deny"));
        // A deny-only ruleset needs no rewrite shape.
        let deny_only =
            rules(r#"{"rules":[{"match":{"tool":"Bash"},"action":{"deny":{"message":"m"}}}]}"#);
        assert!(unsupported_action(&deny_only, Some(DenyShape::CopilotFlat), None).is_none());
    }

    #[test]
    fn render_rewrite_matches_each_protocol() {
        let input = json!({"command": "printf mocked"});
        let claude: Value =
            serde_json::from_str(&render_rewrite(RewriteShape::ClaudeNested, &input, "r")).unwrap();
        assert_eq!(claude["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(claude["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(
            claude["hookSpecificOutput"]["permissionDecisionReason"],
            "r"
        );
        assert_eq!(
            claude["hookSpecificOutput"]["updatedInput"]["command"],
            "printf mocked"
        );
        let crush: Value =
            serde_json::from_str(&render_rewrite(RewriteShape::CrushFlat, &input, "r")).unwrap();
        assert_eq!(crush["version"], 1);
        assert_eq!(crush["decision"], "allow");
        assert_eq!(crush["updated_input"]["command"], "printf mocked");
        let oc: Value =
            serde_json::from_str(&render_rewrite(RewriteShape::OpencodeShim, &input, "r")).unwrap();
        assert_eq!(oc["decision"], "allow");
        assert_eq!(oc["updated_input"]["command"], "printf mocked");
        assert!(oc.get("hookSpecificOutput").is_none());
        // Cursor: permission + updated_input ONLY — the probe-verified reply
        // carries no reason field, so none may be added.
        let cursor: Value =
            serde_json::from_str(&render_rewrite(RewriteShape::CursorPermission, &input, "r"))
                .unwrap();
        assert_eq!(
            cursor,
            json!({"permission": "allow", "updated_input": {"command": "printf mocked"}})
        );
    }

    #[test]
    fn hook_command_wires_rules_and_spy() {
        assert_eq!(
            hook_command("/bin/oh", "crush", Some("/w/r.json"), Some("/w/s.jsonl")),
            "/bin/oh mock crush --rules /w/r.json --spy-file /w/s.jsonl"
        );
        // Spy-only (no rules): a pure observer hook.
        assert_eq!(
            hook_command("/bin/oh", "goose", None, Some("/w/s.jsonl")),
            "/bin/oh mock goose --spy-file /w/s.jsonl"
        );
        assert_eq!(
            hook_command("oh", "claude-code", Some("r.json"), None),
            "oh mock claude-code --rules r.json"
        );
    }

    #[test]
    fn settings_hooks_json_matches_the_probe_verified_shape() {
        let v: Value = serde_json::from_str(&settings_hooks_json("oh mock claude-code")).unwrap();
        assert_eq!(
            v,
            json!({
                "hooks": {
                    "PreToolUse": [
                        { "hooks": [ { "type": "command", "command": "oh mock claude-code" } ] }
                    ]
                }
            })
        );
    }

    #[test]
    fn stub_input_quotes_any_output_safely() {
        // Plain text: a printf of the output, trailing newline like a real
        // command, exit 0 implied (no exit suffix).
        assert_eq!(
            stub_input("nothing to commit", 0)["command"],
            "printf '%s\\n' 'nothing to commit'"
        );
        // Nothing is ever shell-interpreted: quotes, $, backticks, newlines all
        // ride inside the single-quoted argument (POSIX quote escaping).
        assert_eq!(
            stub_input("it's `x` a $HOME\nline2", 0)["command"],
            "printf '%s\\n' 'it'\\''s `x` a $HOME\nline2'"
        );
        // A non-zero exit code fakes a failing command.
        assert_eq!(
            stub_input("boom", 3)["command"],
            "printf '%s\\n' 'boom'; exit 3"
        );
    }

    #[test]
    fn stub_action_parses_and_requires_the_rewrite_shape() {
        let rules = parse_rules(
            r#"{"rules":[{"match":{"event_contains":"git status"},"action":{"stub":{"output":"clean"}}}]}"#,
        )
        .unwrap();
        let (_, action) = decide(r#"{"tool_input":{"command":"git status"}}"#, &rules).unwrap();
        assert_eq!(action.kind(), "stub");
        assert_eq!(
            *action,
            Action::Stub {
                output: "clean".into(),
                exit_code: 0
            }
        );
        // exit_code is optional sugar with an explicit form.
        let rules = parse_rules(
            r#"{"rules":[{"match":{"tool":"Bash"},"action":{"stub":{"output":"e","exit_code":2}}}]}"#,
        )
        .unwrap();
        assert!(matches!(
            rules.rules[0].action,
            Action::Stub { exit_code: 2, .. }
        ));
        // A stub compiles to a rewrite, so it needs the same capability.
        assert_eq!(
            unsupported_action(&rules, Some(DenyShape::Decision("block")), None),
            Some("stub")
        );
        assert!(unsupported_action(
            &rules,
            Some(DenyShape::Decision("block")),
            Some(RewriteShape::CrushFlat)
        )
        .is_none());
    }

    #[test]
    fn rewrite_shape_tokens_are_stable() {
        assert_eq!(RewriteShape::ClaudeNested.as_str(), "claude-nested");
        assert_eq!(RewriteShape::CrushFlat.as_str(), "crush-flat");
        assert_eq!(RewriteShape::OpencodeShim.as_str(), "opencode-shim");
        assert_eq!(RewriteShape::CursorPermission.as_str(), "cursor-permission");
    }

    #[test]
    fn spy_line_records_event_action_and_rule() {
        let r = rules(REWRITE_RULES);
        let event = r#"{"tool_name":"Bash","tool_input":{"command":"git push"}}"#;
        let decision = decide(event, &r);
        let line: Value = serde_json::from_str(&spy_line("claude-code", event, decision)).unwrap();
        assert_eq!(line["harness"], "claude-code");
        assert_eq!(line["action"], "deny");
        assert_eq!(line["rule"], 0);
        // The event is embedded as parsed JSON, so consumers query it directly.
        assert_eq!(line["event"]["tool_input"]["command"], "git push");
        // Fall-through: action `allow`, rule null; a non-JSON event is kept as
        // a string, never dropped.
        let line: Value = serde_json::from_str(&spy_line("goose", "raw text", None)).unwrap();
        assert_eq!(line["action"], "allow");
        assert!(line["rule"].is_null());
        assert_eq!(line["event"], "raw text");
    }
}
