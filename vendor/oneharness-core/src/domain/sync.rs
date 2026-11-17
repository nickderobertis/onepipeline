//! Pure logic for `oneharness sync`: building the JSON fragment each harness's
//! project config file must contain, the non-destructive deep merge that folds
//! it into whatever the file already holds, the `--exact` list replacement, and
//! the translation of the unified rules into a Codex execpolicy file. All I/O
//! (reading and writing the actual files) lives in `src/io/sync.rs`.

use serde_json::{Map, Value};

use crate::domain::config::FileConfig;
use crate::domain::harness::{HarnessSpec, SyncFormat, SyncSpec};

/// Which unified rule list an entry belongs to.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, schemars::JsonSchema, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RuleList {
    /// `allowed_tools`.
    AllowedTools,
    /// `denied_tools`.
    DeniedTools,
}

impl RuleList {
    /// The config field this list is read from.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllowedTools => "allowed_tools",
            Self::DeniedTools => "denied_tools",
        }
    }

    /// The Codex execpolicy `decision` this list's rules render to.
    fn exec_policy_decision(self) -> &'static str {
        match self {
            Self::AllowedTools => "allow",
            Self::DeniedTools => "forbidden",
        }
    }
}

/// A configured rule the harness cannot express, left out of what was written.
#[derive(Debug, Clone, PartialEq, Eq, schemars::JsonSchema, serde::Serialize)]
#[non_exhaustive]
pub struct UnmappedRule {
    pub list: RuleList,
    /// The rule exactly as configured.
    pub rule: String,
    /// Why the harness cannot express it.
    pub reason: &'static str,
}

impl UnmappedRule {
    #[must_use]
    pub fn new(list: RuleList, rule: impl Into<String>, reason: &'static str) -> Self {
        Self {
            list,
            rule: rule.into(),
            reason,
        }
    }
}

/// One entry an `--exact` sync added to, or removed from, a harness's rules.
#[derive(Debug, Clone, PartialEq, Eq, schemars::JsonSchema, serde::Serialize)]
#[non_exhaustive]
pub struct RuleChange {
    /// The list the entry belongs to; absent for a statement in a Codex rules
    /// file that no unified list renders (a hand-written `decision="prompt"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<RuleList>,
    /// The entry as it appears in the harness's file: the rule string for a
    /// JSON list, the whole `prefix_rule(...)` statement for a Codex rules file.
    pub rule: String,
}

impl RuleChange {
    #[must_use]
    pub fn new(list: Option<RuleList>, rule: impl Into<String>) -> Self {
        Self {
            list,
            rule: rule.into(),
        }
    }
}

/// What `sync` should do for one harness, computed purely from the unified
/// config and the registry's [`crate::domain::harness::SyncSpec`].
pub struct SyncPlan {
    /// The JSON object to merge into the harness's config file; `None` when
    /// nothing is configured for it (the harness is then reported `skipped`).
    pub fragment: Option<Value>,
    /// Top-level settings that exist but have no mapping for this harness
    /// (e.g. a top-level `allowed_tools` while the harness has no allow-list
    /// concept). Surfaced in the report and on stderr so a rule that did not
    /// land is always visible — never silently dropped. Per-harness fields
    /// can't end up here; those are rejected at config parse time.
    pub unmapped: Vec<&'static str>,
}

/// Build the sync plan for one harness. Pure.
///
/// The fragment starts from the harness's raw `[harness.<id>.settings]` table
/// (converted to JSON), then the unified rule lists and hooks are inserted at
/// the registry's key paths — an explicit `allowed_tools`/`denied_tools`/
/// `hooks` wins over the same key in the raw table.
pub fn plan(cfg: &FileConfig, spec: &HarnessSpec) -> Result<SyncPlan, String> {
    plan_for(cfg, spec, spec.id)
}

// llmlint: ignore[invalid_states_unrepresentable] This pure API receives the serialized selector only after command-layer selection and variant lookup; borrowing it avoids a competing identity type while integration tests pin invalid selectors at the boundary.
pub fn plan_for(cfg: &FileConfig, spec: &HarnessSpec, id: &str) -> Result<SyncPlan, String> {
    if spec
        .sync
        .as_ref()
        .is_some_and(|sync| sync.format() == SyncFormat::ExecPolicyRules)
    {
        // A rules file has no JSON to merge into; its lists are planned, rule
        // by rule, by `rules_plan_for`.
        return Ok(SyncPlan {
            fragment: None,
            unmapped: Vec::new(),
        });
    }
    let Some(sync) = &spec.sync else {
        // No config surface at all: anything aimed at this harness from the
        // top level is unmapped (per-harness fields were parse-rejected).
        let mut unmapped = Vec::new();
        if !cfg.allowed_tools_for(id).is_empty() {
            unmapped.push("allowed_tools");
        }
        if !cfg.denied_tools_for(id).is_empty() {
            unmapped.push("denied_tools");
        }
        return Ok(SyncPlan {
            fragment: None,
            unmapped,
        });
    };

    let mut root = match cfg.settings_for(id) {
        Some(table) => {
            let value = serde_json::to_value(table)
                .map_err(|e| format!("settings table is not representable as JSON: {e}"))?;
            match value {
                Value::Object(map) => map,
                _ => return Err("`settings` must be a table".to_string()),
            }
        }
        None => Map::new(),
    };
    let mut unmapped = Vec::new();

    let rules = [
        ("allowed_tools", cfg.allowed_tools_for(id), sync.allow_path),
        ("denied_tools", cfg.denied_tools_for(id), sync.deny_path),
    ];
    for (name, values, path) in rules {
        if values.is_empty() {
            continue;
        }
        match path {
            Some(path) => {
                let list = Value::Array(values.iter().cloned().map(Value::String).collect());
                insert_at(&mut root, path, list);
            }
            None => unmapped.push(name),
        }
    }

    // Complete the schema for any top-level key the fragment touches (e.g.
    // Cursor requires both permissions arrays); untouched keys stay unseeded.
    if let Some(seed) = sync.schema_seed {
        let seed: Value =
            serde_json::from_str(seed).expect("registry schema_seed is valid JSON (test-pinned)");
        if let Value::Object(seed_map) = seed {
            for (key, seed_value) in seed_map {
                if let Some(current) = root.get(&key) {
                    let completed = deep_merge(&seed_value, current);
                    root.insert(key, completed);
                }
            }
        }
    }

    if let Some(hooks) = cfg.hooks_for(id) {
        // Parse-time validation guarantees hooks only appear where a path
        // exists, so this expect cannot fire on user input.
        let path = sync.hooks_path.expect("hooks validated against hooks_path");
        let value = serde_json::to_value(hooks)
            .map_err(|e| format!("hooks table is not representable as JSON: {e}"))?;
        insert_at(&mut root, path, value);
    }

    Ok(SyncPlan {
        fragment: if root.is_empty() {
            None
        } else {
            Some(Value::Object(root))
        },
        unmapped,
    })
}

/// Insert `value` at a key path, creating intermediate objects. An explicit
/// unified field overwrites the same key from the raw settings table (the
/// dedicated field is the more specific intent).
fn insert_at(root: &mut Map<String, Value>, path: &[&str], value: Value) {
    let (last, parents) = path.split_last().expect("key paths are non-empty");
    let mut node = root;
    for key in parents {
        let entry = node
            .entry(key.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        node = entry.as_object_mut().expect("just ensured an object");
    }
    node.insert(last.to_string(), value);
}

/// The list at `path` in `value`, or `None` when the path does not exist.
fn get_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |node, key| node.get(key))
}

/// What an `--exact` sync makes of one JSON file.
#[derive(Debug, Clone, PartialEq)]
pub struct ExactMerge {
    /// The file's new content.
    pub value: Value,
    /// Entries the source has that the file's lists lacked.
    pub added: Vec<RuleChange>,
    /// Entries the file's lists had that the source does not.
    pub removed: Vec<RuleChange>,
}

/// Merge `fragment` into `existing` as [`deep_merge`] does, then hold the list
/// at each of the spec's `allow_path`/`deny_path` to exactly the fragment's
/// list there (an absent one is empty), in source order: stale entries are
/// removed and hand-added ones dropped. Every other key — hooks, `env`, any
/// non-permission setting — is merged exactly as the add-only sync merges it.
///
/// A list the source leaves empty is only written where the file already has
/// one, so an exact sync never invents a permissions block. `fragment` is
/// `None` when nothing is configured for the harness, which empties whatever
/// lists the file holds.
#[must_use]
pub fn merge_exact(existing: &Value, fragment: Option<&Value>, sync: &SyncSpec) -> ExactMerge {
    let mut value = match fragment {
        Some(fragment) => deep_merge(existing, fragment),
        None => existing.clone(),
    };
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let lists = [
        (RuleList::AllowedTools, sync.allow_path),
        (RuleList::DeniedTools, sync.deny_path),
    ];
    for (list, path) in lists {
        let Some(path) = path else { continue };
        let source: Vec<Value> = fragment
            .and_then(|fragment| get_at(fragment, path))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let current = get_at(existing, path);
        if source.is_empty() && current.is_none() {
            continue;
        }
        let current: Vec<Value> = current
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let entry = |item: &Value| RuleChange::new(Some(list), entry_text(item));
        added.extend(source.iter().filter(|i| !current.contains(i)).map(entry));
        removed.extend(current.iter().filter(|i| !source.contains(i)).map(entry));
        if let Value::Object(root) = &mut value {
            insert_at(root, path, Value::Array(source));
        }
    }
    ExactMerge {
        value,
        added,
        removed,
    }
}

/// A list entry as the report names it: a string as itself, anything else a
/// hand edit put there as its JSON text.
fn entry_text(item: &Value) -> String {
    match item {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Why a rule has no Codex translation. Each names the shape and why Codex
/// cannot express it, since the rule is reported and never widened.
mod reason {
    pub const NOT_BASH: &str =
        "not a `Bash(...)` rule: Codex execpolicy rules govern only shell commands";
    pub const WHOLE_TOOL: &str =
        "names the whole Bash tool: a Codex rule needs at least one literal command token";
    pub const NO_COMMAND: &str =
        "names no command: a Codex rule needs at least one literal command token";
    pub const EXACT: &str = "an exact command: a Codex prefix rule cannot bound the command's \
         length, and oneharness never widens an exact rule into a prefix";
    pub const MID_GLOB: &str = "a wildcard before the end of the pattern: Codex patterns match \
         literal tokens only";
    pub const GLUED_GLOB: &str = "a wildcard glued to a token (e.g. `status*`): Codex patterns \
         match whole tokens only";
    pub const QUOTE: &str = "an unterminated quote: the rule's shell words cannot be split";
    pub const SHELL_SYNTAX: &str = "shell syntax (an operator, expansion, or comment): Codex \
         matches a rule against one command's literal tokens";
    pub const ASSIGNMENT: &str = "starts with an environment assignment: Codex does not match \
         rules against assignments";
    pub const CONTROL: &str = "a control character: it cannot be written as a Codex pattern token";
}

/// One shell word of a rule, as a POSIX shell would split it.
struct Word {
    text: String,
    /// Whether any `*`, `?`, or `[` appeared outside quotes.
    glob: bool,
    /// The word is exactly one unquoted `*`.
    bare_star: bool,
    /// The word ends in an unquoted `*` and has no other unquoted glob.
    trailing_star_only: bool,
    /// An `=` appeared outside quotes (an assignment when it is the first word).
    assignment: bool,
}

/// Split `text` into words the way a POSIX shell does: whitespace separates,
/// single quotes are literal, double quotes are literal but for `\` escaping
/// `$`, `` ` ``, `"`, `\` and a newline, and a backslash outside quotes makes
/// the next character literal. Anything the shell would expand or treat as an
/// operator is refused, since Codex compares literal tokens.
fn shell_words(text: &str) -> Result<Vec<Word>, &'static str> {
    if text
        .chars()
        .any(|c| c.is_control() && c != '\t' && c != ' ')
    {
        return Err(if text.contains('\n') {
            reason::SHELL_SYNTAX
        } else {
            reason::CONTROL
        });
    }
    let mut words = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| *c == ' ' || *c == '\t').is_some() {}
        if chars.peek().is_none() {
            return Ok(words);
        }
        let mut word = Word {
            text: String::new(),
            glob: false,
            bare_star: false,
            trailing_star_only: false,
            assignment: false,
        };
        let mut unquoted_globs = 0usize;
        let mut last_was_unquoted_star = false;
        let mut quoted_any = false;
        while let Some(&c) = chars.peek() {
            if c == ' ' || c == '\t' {
                break;
            }
            chars.next();
            last_was_unquoted_star = false;
            match c {
                '\'' => {
                    quoted_any = true;
                    loop {
                        match chars.next() {
                            Some('\'') => break,
                            Some(inner) => word.text.push(inner),
                            None => return Err(reason::QUOTE),
                        }
                    }
                }
                '"' => {
                    quoted_any = true;
                    loop {
                        match chars.next() {
                            Some('"') => break,
                            Some('$' | '`') => return Err(reason::SHELL_SYNTAX),
                            Some('\\') => match chars.next() {
                                Some(escaped @ ('$' | '`' | '"' | '\\')) => {
                                    word.text.push(escaped);
                                }
                                Some(other) => {
                                    word.text.push('\\');
                                    word.text.push(other);
                                }
                                None => return Err(reason::QUOTE),
                            },
                            Some(inner) => word.text.push(inner),
                            None => return Err(reason::QUOTE),
                        }
                    }
                }
                '\\' => match chars.next() {
                    Some(escaped) => {
                        quoted_any = true;
                        word.text.push(escaped);
                    }
                    None => return Err(reason::QUOTE),
                },
                '|' | '&' | ';' | '<' | '>' | '(' | ')' | '$' | '`' => {
                    return Err(reason::SHELL_SYNTAX)
                }
                '#' | '~' if word.text.is_empty() && !quoted_any => {
                    return Err(reason::SHELL_SYNTAX)
                }
                '*' | '?' | '[' => {
                    unquoted_globs += 1;
                    last_was_unquoted_star = c == '*';
                    word.text.push(c);
                }
                '=' => {
                    word.assignment = true;
                    word.text.push(c);
                }
                other => word.text.push(other),
            }
        }
        word.glob = unquoted_globs > 0;
        word.bare_star = word.text == "*" && !quoted_any && unquoted_globs == 1;
        word.trailing_star_only = unquoted_globs == 1 && last_was_unquoted_star;
        words.push(word);
    }
}

/// Translate one Claude-dialect rule into the literal tokens of a Codex
/// `prefix_rule` pattern, or say why Codex cannot express it.
///
/// `Bash(<cmd>:*)` — and its equivalent `Bash(<cmd> *)`, a trailing `*` word —
/// is a prefix of `<cmd>`'s shell words. `Bash(<cmd>)` is an exact command,
/// which Codex cannot express (a prefix rule matches any longer command too),
/// so it is refused rather than widened.
///
/// # Errors
///
/// The reason the rule has no Codex translation.
pub fn exec_policy_pattern(rule: &str) -> Result<Vec<String>, &'static str> {
    if rule.trim() == "Bash" {
        return Err(reason::WHOLE_TOOL);
    }
    let inner = rule
        .strip_prefix("Bash(")
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or(reason::NOT_BASH)?;
    let (command, mut prefix) = match inner.strip_suffix(":*") {
        Some(command) => (command, true),
        None => (inner, false),
    };
    let mut words = shell_words(command)?;
    if !prefix && words.last().is_some_and(|word| word.bare_star) {
        words.pop();
        prefix = true;
    }
    if words.is_empty() {
        return Err(reason::NO_COMMAND);
    }
    if let Some(index) = words.iter().position(|word| word.glob) {
        let glued = index + 1 == words.len() && words[index].trailing_star_only;
        return Err(if glued {
            reason::GLUED_GLOB
        } else {
            reason::MID_GLOB
        });
    }
    if words[0].assignment {
        return Err(reason::ASSIGNMENT);
    }
    if !prefix {
        return Err(reason::EXACT);
    }
    Ok(words.into_iter().map(|word| word.text).collect())
}

/// The comment every rules file oneharness writes opens with.
pub const RULES_FILE_HEADER: &str = "# Generated by `oneharness sync` from the unified \
`allowed_tools` / `denied_tools`.\n# Do not edit: every sync that changes it rewrites this \
file whole.\n";

/// What `sync` writes for a harness whose target is a Codex rules file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RulesPlan {
    /// The whole file.
    pub text: String,
    /// Whether any rule is configured for the harness. With none, the file is
    /// written only if it already exists, so a stale file is emptied and a
    /// project that never asked for one gains nothing.
    pub configured: bool,
    /// Every configured rule Codex cannot express, in source order.
    pub unmapped: Vec<UnmappedRule>,
}

/// Plan the Codex rules file for `id`: one `prefix_rule` per translatable
/// rule, allowed rules first, each list in source order, and every other rule
/// reported in [`RulesPlan::unmapped`]. `None` when the harness's target is
/// not a rules file. Pure.
#[must_use]
pub fn rules_plan_for(cfg: &FileConfig, spec: &HarnessSpec, id: &str) -> Option<RulesPlan> {
    spec.sync
        .as_ref()
        .filter(|sync| sync.format() == SyncFormat::ExecPolicyRules)?;
    let mut text = String::from(RULES_FILE_HEADER);
    let mut unmapped = Vec::new();
    let mut configured = false;
    let lists = [
        (RuleList::AllowedTools, cfg.allowed_tools_for(id)),
        (RuleList::DeniedTools, cfg.denied_tools_for(id)),
    ];
    for (list, rules) in lists {
        let decision = list.exec_policy_decision();
        for rule in rules {
            configured = true;
            match exec_policy_pattern(rule) {
                Ok(tokens) => {
                    text.push_str(&format!("\n# {}: {rule}\n", list.as_str()));
                    text.push_str(&prefix_rule(&tokens, decision));
                    text.push('\n');
                }
                Err(why) => unmapped.push(UnmappedRule::new(list, rule.clone(), why)),
            }
        }
    }
    Some(RulesPlan {
        text,
        configured,
        unmapped,
    })
}

/// One `prefix_rule(...)` statement. A token never holds a control character
/// (`shell_words` refuses them), so a JSON string literal is also a valid
/// Starlark one: the only escapes it can contain are `\"` and `\\`.
fn prefix_rule(tokens: &[String], decision: &str) -> String {
    let pattern = tokens
        .iter()
        .map(|token| Value::String(token.clone()).to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("prefix_rule(pattern=[{pattern}], decision=\"{decision}\")")
}

/// The statements of a rules file — every line that is neither blank nor a
/// comment — each with the list its decision renders from, when one does.
fn rules_statements(text: &str) -> Vec<(Option<RuleList>, &str)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let list = [RuleList::AllowedTools, RuleList::DeniedTools]
                .into_iter()
                .find(|list| {
                    line.contains(&format!("decision=\"{}\"", list.exec_policy_decision()))
                });
            (list, line)
        })
        .collect()
}

/// The statements `planned` adds to, and removes from, the rules file
/// `existing` — what an `--exact` sync reports for a rules target.
#[must_use]
pub fn rules_changes(existing: &str, planned: &str) -> (Vec<RuleChange>, Vec<RuleChange>) {
    let before = rules_statements(existing);
    let after = rules_statements(planned);
    let change = |(list, line): &(Option<RuleList>, &str)| RuleChange::new(*list, *line);
    let added = after
        .iter()
        .filter(|(_, line)| !before.iter().any(|(_, old)| old == line))
        .map(change)
        .collect();
    let removed = before
        .iter()
        .filter(|(_, line)| !after.iter().any(|(_, new)| new == line))
        .map(change)
        .collect();
    (added, removed)
}

/// Merge `fragment` into `existing`, non-destructively:
///
/// - objects merge per key — keys absent from the fragment are never touched;
/// - arrays union — existing entries keep their order, fragment entries not
///   already present are appended (so re-syncing is idempotent);
/// - anything else (scalars, or a type mismatch) takes the fragment's value —
///   for keys oneharness manages, the unified config is the source of truth.
pub fn deep_merge(existing: &Value, fragment: &Value) -> Value {
    match (existing, fragment) {
        (Value::Object(a), Value::Object(b)) => {
            let mut merged = a.clone();
            for (key, frag) in b {
                let entry = match a.get(key) {
                    Some(prev) => deep_merge(prev, frag),
                    None => frag.clone(),
                };
                merged.insert(key.clone(), entry);
            }
            Value::Object(merged)
        }
        (Value::Array(a), Value::Array(b)) => {
            let mut merged = a.clone();
            for item in b {
                if !merged.contains(item) {
                    merged.push(item.clone());
                }
            }
            Value::Array(merged)
        }
        _ => fragment.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{config, harness};
    use serde_json::json;

    fn cfg(text: &str) -> FileConfig {
        config::parse(text).expect("config should parse")
    }

    fn plan_for(text: &str, id: &str) -> SyncPlan {
        plan(&cfg(text), harness::by_id(id).unwrap()).expect("plan should build")
    }

    #[test]
    fn nothing_configured_means_no_fragment() {
        let p = plan_for("model = \"x\"", "claude-code");
        assert!(p.fragment.is_none());
        assert!(p.unmapped.is_empty());
    }

    #[test]
    fn rules_and_hooks_land_at_the_registry_paths() {
        let p = plan_for(
            concat!(
                "allowed_tools = [\"Bash(git log:*)\"]\n",
                "denied_tools = [\"Bash(rm:*)\"]\n",
                "[harness.claude-code.hooks]\n",
                "PreToolUse = []\n",
            ),
            "claude-code",
        );
        assert_eq!(
            p.fragment.unwrap(),
            json!({
                "permissions": {
                    "allow": ["Bash(git log:*)"],
                    "deny": ["Bash(rm:*)"],
                },
                "hooks": { "PreToolUse": [] },
            })
        );
        assert!(p.unmapped.is_empty());
    }

    #[test]
    fn crush_deny_maps_to_disabled_tools() {
        let p = plan_for("denied_tools = [\"bash\"]", "crush");
        assert_eq!(
            p.fragment.unwrap(),
            json!({ "options": { "disabled_tools": ["bash"] } })
        );
    }

    #[test]
    fn settings_table_passes_through_and_explicit_rules_win() {
        let p = plan_for(
            concat!(
                "[harness.opencode.settings.permission]\n",
                "edit = \"deny\"\n",
                "[harness.opencode.settings.permission.bash]\n",
                "\"git *\" = \"allow\"\n",
            ),
            "opencode",
        );
        assert_eq!(
            p.fragment.unwrap(),
            json!({ "permission": { "edit": "deny", "bash": { "git *": "allow" } } })
        );

        // An explicit rule list overwrites the same key from the raw table.
        let p = plan_for(
            concat!(
                "[harness.claude-code]\n",
                "allowed_tools = [\"Read\"]\n",
                "[harness.claude-code.settings.permissions]\n",
                "allow = [\"stale\"]\n",
                "defaultMode = \"acceptEdits\"\n",
            ),
            "claude-code",
        );
        assert_eq!(
            p.fragment.unwrap(),
            json!({ "permissions": { "allow": ["Read"], "defaultMode": "acceptEdits" } })
        );
    }

    #[test]
    fn top_level_rules_without_a_mapping_are_reported_unmapped() {
        // opencode has a config file but no list-shaped permission concept;
        // goose has no config surface at all. Both must say so, loudly.
        let text = "allowed_tools = [\"x\"]\ndenied_tools = [\"y\"]";
        let p = plan_for(text, "opencode");
        assert!(p.fragment.is_none());
        assert_eq!(p.unmapped, ["allowed_tools", "denied_tools"]);
        let p = plan_for(text, "goose");
        assert!(p.fragment.is_none());
        assert_eq!(p.unmapped, ["allowed_tools", "denied_tools"]);
    }

    #[test]
    fn a_rules_target_declares_no_json_shape_and_carries_both_lists() {
        // The only fields a rules target may set are its file; everything a
        // JSON merge reads stays empty, so no half-JSON rules target exists.
        let mut rules_targets = 0;
        for spec in harness::all() {
            let Some(sync) = &spec.sync else { continue };
            if sync.format() != SyncFormat::ExecPolicyRules {
                continue;
            }
            rules_targets += 1;
            assert!(sync.alt_files.is_empty(), "{}", spec.id);
            assert!(
                sync.allow_path.is_none() && sync.deny_path.is_none(),
                "{}",
                spec.id
            );
            assert!(
                sync.hooks_path.is_none() && sync.schema_seed.is_none(),
                "{}",
                spec.id
            );
            assert!(sync.carries_allowed_tools() && sync.carries_denied_tools());
        }
        assert_eq!(rules_targets, 1, "codex is the one rules target today");
    }

    #[test]
    fn codex_lists_are_planned_as_a_rules_file_never_a_json_fragment() {
        let text = "allowed_tools = [\"Bash(git status:*)\"]";
        let p = plan_for(text, "codex");
        assert!(p.fragment.is_none());
        assert!(p.unmapped.is_empty());
        let spec = harness::by_id("codex").unwrap();
        assert_eq!(
            spec.sync.as_ref().unwrap().format(),
            SyncFormat::ExecPolicyRules
        );
        // Every other target stays a JSON merge, and has no rules plan.
        for other in harness::all().iter().filter(|s| s.id != "codex") {
            if let Some(sync) = &other.sync {
                assert_eq!(sync.format(), SyncFormat::Json, "{}", other.id);
            }
            assert!(rules_plan_for(&cfg(text), other, other.id).is_none());
        }
    }

    fn rules(text: &str) -> RulesPlan {
        let spec = harness::by_id("codex").unwrap();
        rules_plan_for(&cfg(text), spec, "codex").expect("codex has a rules target")
    }

    #[test]
    fn prefix_rules_translate_to_one_prefix_rule_each_in_source_order() {
        let p = rules(concat!(
            "allowed_tools = [\"Bash(git status:*)\", \"Bash(cargo test *)\"]\n",
            "denied_tools = [\"Bash(rm -rf:*)\"]\n",
        ));
        assert!(p.configured);
        assert!(p.unmapped.is_empty(), "{:?}", p.unmapped);
        assert_eq!(
            p.text,
            format!(
                "{RULES_FILE_HEADER}\n\
                 # allowed_tools: Bash(git status:*)\n\
                 prefix_rule(pattern=[\"git\", \"status\"], decision=\"allow\")\n\n\
                 # allowed_tools: Bash(cargo test *)\n\
                 prefix_rule(pattern=[\"cargo\", \"test\"], decision=\"allow\")\n\n\
                 # denied_tools: Bash(rm -rf:*)\n\
                 prefix_rule(pattern=[\"rm\", \"-rf\"], decision=\"forbidden\")\n"
            )
        );
        assert!(p.text.starts_with("# Generated by `oneharness sync`"));
    }

    #[test]
    fn a_per_harness_list_replaces_the_top_level_one_for_codex() {
        let p = rules(concat!(
            "allowed_tools = [\"Bash(ls:*)\"]\n",
            "[harness.codex]\n",
            "allowed_tools = [\"Bash(just check:*)\"]\n",
        ));
        assert!(p.text.contains("[\"just\", \"check\"]"), "{}", p.text);
        assert!(!p.text.contains("\"ls\""), "{}", p.text);
    }

    #[test]
    fn tokens_split_like_a_posix_shell() {
        let ok = |rule: &str| exec_policy_pattern(rule).unwrap();
        assert_eq!(ok("Bash(git   log:*)"), ["git", "log"]);
        assert_eq!(
            ok("Bash(git commit -m 'a b':*)"),
            ["git", "commit", "-m", "a b"]
        );
        assert_eq!(ok("Bash(echo \"x \\\"y\\\"\":*)"), ["echo", "x \"y\""]);
        assert_eq!(ok("Bash(echo a\\ b:*)"), ["echo", "a b"]);
        assert_eq!(ok("Bash(echo \"a\\nb\":*)"), ["echo", "a\\nb"]);
        // A quoted wildcard is a literal token, not a glob.
        assert_eq!(ok("Bash(echo '*':*)"), ["echo", "*"]);
        // `=` is only an assignment in the command position.
        assert_eq!(ok("Bash(make CC=clang:*)"), ["make", "CC=clang"]);
    }

    #[test]
    fn every_untranslatable_shape_is_refused_with_its_reason() {
        let refused = |rule: &str| exec_policy_pattern(rule).unwrap_err();
        assert_eq!(refused("Bash(git -C * log*)"), reason::MID_GLOB);
        assert_eq!(refused("Bash(git * log:*)"), reason::MID_GLOB);
        assert_eq!(refused("Bash(ls -[al]:*)"), reason::MID_GLOB);
        assert_eq!(refused("Bash(git status*)"), reason::GLUED_GLOB);
        assert_eq!(refused("Bash(git status*:*)"), reason::GLUED_GLOB);
        assert_eq!(refused("Read(./src/**)"), reason::NOT_BASH);
        assert_eq!(refused("Edit"), reason::NOT_BASH);
        assert_eq!(refused("WebFetch(domain:example.com)"), reason::NOT_BASH);
        assert_eq!(refused("Bash"), reason::WHOLE_TOOL);
        assert_eq!(refused("Bash(*)"), reason::NO_COMMAND);
        assert_eq!(refused("Bash(:*)"), reason::NO_COMMAND);
        assert_eq!(refused("Bash(just check)"), reason::EXACT);
        assert_eq!(refused("Bash(npm run build)"), reason::EXACT);
        assert_eq!(refused("Bash(git log | head:*)"), reason::SHELL_SYNTAX);
        assert_eq!(refused("Bash(echo $HOME:*)"), reason::SHELL_SYNTAX);
        assert_eq!(refused("Bash(echo \"$HOME\":*)"), reason::SHELL_SYNTAX);
        assert_eq!(refused("Bash(ls ~/x:*)"), reason::SHELL_SYNTAX);
        assert_eq!(refused("Bash(a\nb:*)"), reason::SHELL_SYNTAX);
        assert_eq!(refused("Bash(echo 'open:*)"), reason::QUOTE);
        assert_eq!(refused("Bash(FOO=1 make:*)"), reason::ASSIGNMENT);
        assert_eq!(refused("Bash(echo \u{1b}x:*)"), reason::CONTROL);
    }

    #[test]
    fn unmapped_rules_are_left_out_of_the_file_and_reported_in_order() {
        let p = rules(concat!(
            "allowed_tools = [\"Bash(git -C * log*)\", \"Bash(git log:*)\", \"Read\"]\n",
            "denied_tools = [\"Bash(git status*)\"]\n",
        ));
        assert_eq!(
            p.unmapped,
            [
                UnmappedRule::new(
                    RuleList::AllowedTools,
                    "Bash(git -C * log*)",
                    reason::MID_GLOB
                ),
                UnmappedRule::new(RuleList::AllowedTools, "Read", reason::NOT_BASH),
                UnmappedRule::new(
                    RuleList::DeniedTools,
                    "Bash(git status*)",
                    reason::GLUED_GLOB
                ),
            ]
        );
        assert!(p.text.contains("[\"git\", \"log\"]"));
        assert!(!p.text.contains("status"), "{}", p.text);
        assert!(!p.text.contains("Read"), "{}", p.text);
    }

    #[test]
    fn nothing_configured_plans_an_empty_owned_file() {
        let p = rules("model = \"x\"");
        assert!(!p.configured);
        assert_eq!(p.text, RULES_FILE_HEADER);
        // Every rule unmapped is still configured: the file is written, empty.
        let p = rules("allowed_tools = [\"Read\"]");
        assert!(p.configured);
        assert_eq!(p.text, RULES_FILE_HEADER);
    }

    #[test]
    fn rules_changes_name_each_statement_and_its_list() {
        let old = concat!(
            "# header\n",
            "prefix_rule(pattern=[\"ls\"], decision=\"allow\")\n",
            "prefix_rule(pattern=[\"curl\"], decision=\"prompt\")\n",
        );
        let new = concat!(
            "prefix_rule(pattern=[\"ls\"], decision=\"allow\")\n",
            "prefix_rule(pattern=[\"rm\"], decision=\"forbidden\")\n",
        );
        let (added, removed) = rules_changes(old, new);
        assert_eq!(
            added,
            [RuleChange::new(
                Some(RuleList::DeniedTools),
                "prefix_rule(pattern=[\"rm\"], decision=\"forbidden\")"
            )]
        );
        assert_eq!(
            removed,
            [RuleChange::new(
                None,
                "prefix_rule(pattern=[\"curl\"], decision=\"prompt\")"
            )]
        );
    }

    #[test]
    fn exact_merge_holds_each_list_to_the_source_and_keeps_everything_else() {
        let sync = harness::by_id("claude-code")
            .unwrap()
            .sync
            .as_ref()
            .unwrap();
        let existing = json!({
            "permissions": {
                "allow": ["Bash(ls:*)", "Bash(stale:*)", "Bash(hand-added:*)"],
                "deny": ["Bash(rm:*)"],
                "defaultMode": "plan",
            },
            "hooks": { "SessionStart": [{ "hooks": [] }] },
            "env": { "FOO": "bar" },
        });
        let fragment = json!({ "permissions": { "allow": ["Bash(new:*)", "Bash(ls:*)"] } });
        let out = merge_exact(&existing, Some(&fragment), sync);
        assert_eq!(
            out.value,
            json!({
                "permissions": {
                    "allow": ["Bash(new:*)", "Bash(ls:*)"],
                    "deny": [],
                    "defaultMode": "plan",
                },
                "hooks": { "SessionStart": [{ "hooks": [] }] },
                "env": { "FOO": "bar" },
            })
        );
        let names = |changes: &[RuleChange]| {
            changes
                .iter()
                .map(|c| (c.list.unwrap(), c.rule.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&out.added),
            [(RuleList::AllowedTools, "Bash(new:*)".to_string())]
        );
        assert_eq!(
            names(&out.removed),
            [
                (RuleList::AllowedTools, "Bash(stale:*)".to_string()),
                (RuleList::AllowedTools, "Bash(hand-added:*)".to_string()),
                (RuleList::DeniedTools, "Bash(rm:*)".to_string()),
            ]
        );
        // Idempotent: the exact result is already exact.
        let again = merge_exact(&out.value, Some(&fragment), sync);
        assert_eq!(again.value, out.value);
        assert!(again.added.is_empty() && again.removed.is_empty());
    }

    #[test]
    fn exact_merge_never_invents_a_list_the_file_lacks() {
        let sync = harness::by_id("claude-code")
            .unwrap()
            .sync
            .as_ref()
            .unwrap();
        let existing = json!({ "env": { "A": "1" } });
        let out = merge_exact(&existing, None, sync);
        assert_eq!(out.value, existing);
        assert!(out.added.is_empty() && out.removed.is_empty());
        // A non-string entry a hand edit left is named by its JSON text.
        let existing = json!({ "permissions": { "allow": [7] } });
        let out = merge_exact(&existing, None, sync);
        assert_eq!(out.value, json!({ "permissions": { "allow": [] } }));
        assert_eq!(out.removed[0].rule, "7");
    }

    #[test]
    fn every_registry_schema_seed_is_valid_json() {
        for spec in harness::all() {
            if let Some(seed) = spec.sync.as_ref().and_then(|s| s.schema_seed) {
                let value: Value = serde_json::from_str(seed)
                    .unwrap_or_else(|e| panic!("{}: schema_seed is not JSON: {e}", spec.id));
                assert!(
                    value.is_object(),
                    "{}: schema_seed must be an object",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn cursor_allow_only_is_completed_with_an_empty_deny() {
        // Cursor's CLI rejects a permissions block missing either array
        // (observed live: "permissions.deny Required"), so a partial write
        // must be seeded into a schema-valid shape.
        let p = plan_for(
            "[harness.cursor]\nallowed_tools = [\"Shell(touch)\"]",
            "cursor",
        );
        assert_eq!(
            p.fragment.unwrap(),
            json!({ "permissions": { "allow": ["Shell(touch)"], "deny": [] } })
        );
        // And the seed never invents a permissions block out of nothing.
        let p = plan_for(
            "[harness.cursor.settings]\neditor = { vimMode = true }",
            "cursor",
        );
        assert_eq!(
            p.fragment.unwrap(),
            json!({ "editor": { "vimMode": true } })
        );
    }

    #[test]
    fn deep_merge_preserves_unrelated_keys_and_unions_arrays() {
        let existing = json!({
            "permissions": { "allow": ["Read", "Bash(ls *)"], "defaultMode": "plan" },
            "env": { "FOO": "bar" },
        });
        let fragment = json!({
            "permissions": { "allow": ["Bash(ls *)", "Edit"], "deny": ["Bash(rm *)"] },
        });
        let merged = deep_merge(&existing, &fragment);
        assert_eq!(
            merged,
            json!({
                "permissions": {
                    "allow": ["Read", "Bash(ls *)", "Edit"],
                    "defaultMode": "plan",
                    "deny": ["Bash(rm *)"],
                },
                "env": { "FOO": "bar" },
            })
        );
    }

    #[test]
    fn deep_merge_is_idempotent_and_scalars_take_the_fragment() {
        let fragment = json!({ "a": { "b": [1, 2], "c": "new" } });
        let once = deep_merge(&json!({ "a": { "c": "old" } }), &fragment);
        assert_eq!(once, json!({ "a": { "b": [1, 2], "c": "new" } }));
        let twice = deep_merge(&once, &fragment);
        assert_eq!(twice, once, "re-syncing must change nothing");
    }
}
