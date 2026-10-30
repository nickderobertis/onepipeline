//! The one effective `human_items` configuration every consumer reads: the
//! human-items verbs, an embedding SDK, and a launch.
//!
//! Two layers, the second overriding the first key by key:
//!
//! 1. **Repository-tracked shared defaults**: `.onepipeline/config.yaml` in the
//!    checkout a command runs in (the nearest ancestor of the directory holding
//!    one, stopping at the repository root), so every host that checks the
//!    repository out reads the same defaults.
//! 2. **Host-local overrides**: the file `ONEPIPELINE_CONFIG` names, else
//!    `onepipeline/config.yaml` under `XDG_CONFIG_HOME` (or `~/.config`).
//!
//! Both files hold one top-level `human_items` mapping. Absent everywhere,
//! human items are disabled. Every hook is validated here, once, when the
//! configuration is loaded; a malformed file refuses every consumer alike.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{HumanItemKind, Priority};

/// The repository-tracked file, relative to a checkout.
pub const REPOSITORY_CONFIG: &str = ".onepipeline/config.yaml";
/// The environment variable naming the host-local override file.
pub const HOST_CONFIG_ENV: &str = "ONEPIPELINE_CONFIG";

/// What a configured hook holds when it raises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "scope", rename_all = "kebab-case", deny_unknown_fields)]
pub enum HookBlock {
    /// Holds nothing.
    Nothing,
    /// Holds the context's node in the owning run, and what depends on it.
    Work,
    /// Holds the owning plan.
    Plan,
    /// Holds all work on the host.
    Host,
}

/// A configured approval reference: the name of a trusted adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HookApproval {
    /// The adapter in `approval_adapters` a completion invokes.
    // llmlint: ignore[invalid_states_unrepresentable] a key of the configured `approval_adapters` map, which `validate` requires every hook's adapter to name before any consumer sees the configuration; a name type would restate that map's own key.
    pub adapter: String,
}

/// A configured, trusted approval authority: the command a completion runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalAdapter {
    /// The program and its arguments. It reads `{board_copy,key,actor,on_behalf_of}`
    /// on standard input and, approving, answers `{board_copy,key}` on standard output.
    #[serde(deserialize_with = "command")]
    // llmlint: ignore[invalid_states_unrepresentable] the configured shape is the task's `command: [program, args...]` list; its decoder refuses an empty list or a blank program, so no adapter without a program is ever constructed from configuration.
    pub command: Vec<String>,
}

fn command<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let command = Vec::<String>::deserialize(d)?;
    if command
        .first()
        .is_none_or(|program| program.trim().is_empty())
    {
        return Err(serde::de::Error::custom(
            "an approval adapter's command names a program",
        ));
    }
    Ok(command)
}

/// Aggregations a hook may select.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookAggregate {
    /// One review task over the context's tickets, its priority derived from theirs.
    TicketReview,
}

/// One configured phase hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemHook {
    /// The phase name a raise selects it by.
    // llmlint: ignore[invalid_states_unrepresentable] the task's configuration shape is `phase: string`; `validate` refuses a blank or `:`-bearing phase, and a duplicate, before any consumer sees the loaded configuration.
    pub phase: String,
    /// Task or question.
    pub kind: HumanItemKind,
    // llmlint: ignore-block[invalid_states_unrepresentable] the task's hook shape is `title: string, body: string`; the loader refuses a blank or unparseable template, and `phases::expand` renders each strictly and validates the result as any raised item is, so an in-process hook that skipped the loader is refused at expansion rather than raised.
    /// The title template; `{{ name }}` expands a context variable.
    pub title: String,
    /// The body template.
    pub body: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// The priority, unless an aggregation derives one.
    #[serde(deserialize_with = "priority")]
    #[schemars(schema_with = "super::priority_schema")]
    // llmlint: ignore[invalid_states_unrepresentable] the imported store Priority carries `none`, which this crate cannot remove from the sibling's enum; the decoder and `phases::expand` both refuse it, and the schema states the four usable values.
    pub priority: Priority,
    /// What the item holds while open.
    pub block: HookBlock,
    // llmlint: ignore-block[invalid_states_unrepresentable] the task prescribes the flat hook `{phase, kind, title, body, priority, block, approval}` and selects `ticket_review` on it; the loader's `validate` refuses a question carrying an approval or an aggregation before any consumer sees the configuration, and `phases::expand` refuses the same again for an in-process hook.
    /// The trusted approval adapter a completion of this task runs, if any.
    #[serde(default)]
    pub approval: Option<HookApproval>,
    /// An aggregation over the context, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<HookAggregate>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

fn priority<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Priority, D::Error> {
    let value = Priority::deserialize(d)?;
    if value == Priority::None {
        return Err(serde::de::Error::custom("a hook states its priority"));
    }
    Ok(value)
}

/// The effective `human_items` settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanItemsConfig {
    /// Whether the live store and its automatic items are on; false by default.
    #[serde(default)]
    pub enabled: bool,
    /// The plan-store source open tasks are projected into, if any.
    #[serde(default)]
    // llmlint: ignore[invalid_states_unrepresentable] the task's configuration shape is `task_source: string`; `validate` refuses a blank one at load, and the projection converts it into the store's own SourceName before any write, refusing a name the store does not accept.
    pub task_source: Option<String>,
    /// The configured phases.
    #[serde(default)]
    pub hooks: Vec<HumanItemHook>,
    /// The trusted approval adapters hooks may name.
    #[serde(default)]
    pub approval_adapters: BTreeMap<String, ApprovalAdapter>,
}

/// One file: a `human_items` mapping whose keys are [`HumanItemsConfig`]'s own,
/// held as a mapping so a layer above replaces exactly the keys it states.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    human_items: Option<serde_json::Map<String, serde_json::Value>>,
}

/// The effective configuration and the files it was read from, in layer order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct EffectiveConfig {
    /// The settings every consumer acts on.
    pub human_items: HumanItemsConfig,
    /// The files that supplied them, repository first.
    pub sources: Vec<PathBuf>,
}

impl EffectiveConfig {
    /// The hook a phase names.
    pub fn hook(&self, phase: &str) -> Option<&HumanItemHook> {
        self.human_items.hooks.iter().find(|h| h.phase == phase)
    }
}

/// The repository file governing `dir`: the nearest ancestor holding one, not
/// looking past the repository root.
fn repository_file(dir: &Path) -> Option<PathBuf> {
    let dir = std::path::absolute(dir).ok()?;
    for ancestor in dir.ancestors() {
        let candidate = ancestor.join(REPOSITORY_CONFIG);
        if candidate.is_file() {
            return Some(candidate);
        }
        if ancestor.join(".git").exists() {
            return None;
        }
    }
    None
}

fn host_file() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os(HOST_CONFIG_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(named));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    let file = base.join("onepipeline/config.yaml");
    file.is_file().then_some(file)
}

fn read_layer(path: &Path) -> Result<Option<serde_json::Map<String, serde_json::Value>>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: File =
        crate::yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(file.human_items)
}

/// The effective configuration for a command working in `dir`.
///
/// # Errors
///
/// A file that cannot be read or parsed, or a hook that is not valid, named by
/// its file.
pub fn effective(dir: &Path) -> Result<EffectiveConfig, String> {
    let mut merged = serde_json::Map::new();
    let mut sources = Vec::new();
    let host = host_file();
    if host.as_deref().is_some_and(|h| !h.is_file()) {
        return Err(format!(
            "{HOST_CONFIG_ENV} names {}, which is not a file",
            host.as_deref()
                .map(Path::display)
                .map(|d| d.to_string())
                .unwrap_or_default()
        ));
    }
    for path in [repository_file(dir), host].into_iter().flatten() {
        if let Some(layer) = read_layer(&path)? {
            // Each layer is read as the whole shape first, so a key it misspells or
            // a value it mistypes is named with the file it is in.
            serde_json::from_value::<HumanItemsConfig>(serde_json::Value::Object(layer.clone()))
                .map_err(|e| format!("{}: human_items: {e}", path.display()))?;
            merged.extend(layer);
        }
        sources.push(path);
    }
    let config: HumanItemsConfig = serde_json::from_value(serde_json::Value::Object(merged))
        .map_err(|e| format!("human_items: {e}"))?;
    validate(&config)?;
    Ok(EffectiveConfig {
        human_items: config,
        sources,
    })
}

/// The effective configuration for this process's working directory.
///
/// # Errors
///
/// As [`effective`].
pub fn here() -> Result<EffectiveConfig, String> {
    let dir = std::env::current_dir().map_err(|e| format!("the working directory: {e}"))?;
    effective(&dir)
}

fn validate(config: &HumanItemsConfig) -> Result<(), String> {
    if config
        .task_source
        .as_deref()
        .is_some_and(|s| s.trim().is_empty())
    {
        return Err("human_items.task_source must not be blank".into());
    }
    let mut phases = std::collections::BTreeSet::new();
    for hook in &config.hooks {
        let at = format!("human_items.hooks[{}]", hook.phase);
        if hook.phase.trim().is_empty() || hook.phase.contains(':') {
            return Err(format!("{at}: a phase is a nonblank name without ':'"));
        }
        if !phases.insert(hook.phase.as_str()) {
            return Err(format!("{at}: the phase is configured twice"));
        }
        if hook.title.trim().is_empty() || hook.body.trim().is_empty() {
            return Err(format!("{at}: title and body must not be blank"));
        }
        for template in [&hook.title, &hook.body] {
            minijinja::Environment::new()
                .template_from_str(template)
                .map_err(|e| format!("{at}: {e}"))?;
        }
        if hook.kind == HumanItemKind::Question
            && (hook.approval.is_some() || hook.aggregate.is_some())
        {
            return Err(format!(
                "{at}: a question carries no approval and no aggregation"
            ));
        }
        if let Some(approval) = &hook.approval {
            if !config.approval_adapters.contains_key(&approval.adapter) {
                return Err(format!(
                    "{at}: approval adapter {} is not configured under human_items.approval_adapters",
                    approval.adapter
                ));
            }
        }
    }
    Ok(())
}
