//! Reading and writing harness config files for `oneharness sync`. This is an
//! I/O boundary: it touches the project's filesystem. The merge itself is pure
//! (`src/domain/sync.rs`); this layer only locates, reads, compares, and
//! (atomically) writes.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::domain::harness::{self, HarnessSpec, SyncSpec};
use crate::domain::sync as sync_domain;
use crate::domain::sync::{deep_merge, RuleChange, RulesPlan, UnmappedRule};
use crate::errors::OneharnessError;

/// What applying a fragment did (or, under `check`, would do) to one file.
///
/// Serialized as the report token itself, so the wire value and the variant a
/// consumer matches on cannot drift apart.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, schemars::JsonSchema, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    /// The file did not exist and was (or would be) created.
    Created,
    /// The file existed and its content changed (or would change).
    Updated,
    /// The file already contains everything the fragment asks for.
    Unchanged,
}

/// What one harness's permission/settings sync did (or would do).
///
/// [`FileStatus`] plus the one outcome a *file* never has: a harness with no
/// permission/settings fragment to apply at all. Keeping them one closed set —
/// rather than the report's earlier free string — is what makes an unreachable
/// status unconstructible and lets the contract publish the four values.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, schemars::JsonSchema, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum SyncStatus {
    /// The file did not exist and was (or would be) created.
    Created,
    /// The file existed and its content changed (or would change).
    Updated,
    /// The file already contains everything the fragment asks for.
    Unchanged,
    /// No permission/settings fragment applies to this harness, so there was no
    /// file to write. Hook files carry their own [`FileStatus`].
    Skipped,
}

impl From<FileStatus> for SyncStatus {
    fn from(status: FileStatus) -> Self {
        match status {
            FileStatus::Created => Self::Created,
            FileStatus::Updated => Self::Updated,
            FileStatus::Unchanged => Self::Unchanged,
        }
    }
}

/// Merge `fragment` into the harness's config file under `project_dir`.
///
/// The target is the registry's `file`, unless one of the higher-precedence
/// `alt_files` already exists — merging into the file the harness actually
/// reads, rather than creating a second, shadowed one. A file that exists but
/// cannot be parsed as JSON is a loud error and is left untouched: oneharness
/// only rewrites files it can round-trip (a JSONC file with comments would
/// lose them). Under `check`, nothing is written.
pub fn apply(
    project_dir: &Path,
    spec: &SyncSpec,
    fragment: &Value,
    check: bool,
) -> Result<(PathBuf, FileStatus), OneharnessError> {
    let target = json_target(project_dir, spec);
    let existing: Option<Value> = read_optional(&target)?
        .map(|text| {
            serde_json::from_str(&text).map_err(|e| OneharnessError::HarnessConfigUnmergeable {
                path: target.display().to_string(),
                message: format!("not valid JSON ({e}); fix or remove it and re-run"),
            })
        })
        .transpose()?;

    let (merged, status) = match &existing {
        Some(existing) => {
            let merged = deep_merge(existing, fragment);
            let status = if &merged == existing {
                FileStatus::Unchanged
            } else {
                FileStatus::Updated
            };
            (merged, status)
        }
        None => (fragment.clone(), FileStatus::Created),
    };

    if !check && status != FileStatus::Unchanged {
        write_atomically(&target, &merged)?;
    }
    Ok((target, status))
}

/// What an `--exact` or rules-file apply did (or would do) to one file.
struct Applied {
    path: PathBuf,
    status: FileStatus,
    added: Vec<RuleChange>,
    removed: Vec<RuleChange>,
}

/// The JSON target an [`apply`] writes: the first existing `alt_files` entry,
/// else `file`.
fn json_target(project_dir: &Path, spec: &SyncSpec) -> PathBuf {
    spec.alt_files
        .iter()
        .map(|name| project_dir.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| project_dir.join(spec.file))
}

/// The file's text, or `None` when it does not exist.
fn read_optional(target: &Path) -> Result<Option<String>, OneharnessError> {
    match std::fs::read_to_string(target) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(OneharnessError::HarnessConfigRead {
            path: target.display().to_string(),
            source,
        }),
    }
}

/// [`apply`] under `--exact`: the same merge, then each allow/deny list held to
/// exactly the source's ([`sync_domain::merge_exact`]). `fragment` is `None`
/// when nothing is configured for the harness; the file is then only touched
/// if it exists and holds one of those lists, which the exact sync empties.
/// `Ok(None)` means there was nothing to hold (the harness is `skipped`).
fn apply_exact(
    project_dir: &Path,
    spec: &SyncSpec,
    fragment: Option<&Value>,
    check: bool,
) -> Result<Option<Applied>, OneharnessError> {
    let target = json_target(project_dir, spec);
    let existing: Option<Value> = read_optional(&target)?
        .map(|text| {
            serde_json::from_str(&text).map_err(|e| OneharnessError::HarnessConfigUnmergeable {
                path: target.display().to_string(),
                message: format!("not valid JSON ({e}); fix or remove it and re-run"),
            })
        })
        .transpose()?;
    if fragment.is_none() && existing.is_none() {
        return Ok(None);
    }
    let base = existing
        .clone()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let merged = sync_domain::merge_exact(&base, fragment, spec);
    let status = match &existing {
        None => FileStatus::Created,
        Some(existing) if &merged.value == existing => {
            if fragment.is_none() {
                // Nothing configured, and nothing at the list paths to empty.
                return Ok(None);
            }
            FileStatus::Unchanged
        }
        Some(_) => FileStatus::Updated,
    };
    if !check && status != FileStatus::Unchanged {
        write_atomically(&target, &merged.value)?;
    }
    Ok(Some(Applied {
        path: target,
        status,
        added: merged.added,
        removed: merged.removed,
    }))
}

/// Write a Codex rules file whole. oneharness owns it, so there is nothing to
/// merge: the file equals the plan or it is rewritten, in both modes. With no
/// rule configured it is written only if it already exists (emptying a stale
/// one); `Ok(None)` means neither applied. Only `exact` computes the statement
/// diff the report names.
fn apply_rules(
    project_dir: &Path,
    spec: &SyncSpec,
    plan: &RulesPlan,
    exact: bool,
    check: bool,
) -> Result<Option<Applied>, OneharnessError> {
    let target = project_dir.join(spec.file);
    let existing = read_optional(&target)?;
    let status = match &existing {
        None if !plan.configured => return Ok(None),
        None => FileStatus::Created,
        Some(text) if *text == plan.text => FileStatus::Unchanged,
        Some(_) => FileStatus::Updated,
    };
    if !check && status != FileStatus::Unchanged {
        write_text_atomically(&target, &plan.text)?;
    }
    let (added, removed) = if exact {
        sync_domain::rules_changes(existing.as_deref().unwrap_or(""), &plan.text)
    } else {
        (Vec::new(), Vec::new())
    };
    Ok(Some(Applied {
        path: target,
        status,
        added,
        removed,
    }))
}

/// Pretty-print and write via a temp file + rename, so a crash mid-write can
/// never leave a harness with a truncated config file.
pub(crate) fn write_atomically(target: &Path, value: &Value) -> Result<(), OneharnessError> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    write_text_atomically(target, &text)
}

fn write_text_atomically(target: &Path, text: &str) -> Result<(), OneharnessError> {
    let write_err = |source: std::io::Error| OneharnessError::HarnessConfigWrite {
        path: target.display().to_string(),
        source,
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(write_err)?;
    }
    let tmp = target.with_extension("oneharness.tmp");
    std::fs::write(&tmp, text).map_err(write_err)?;
    std::fs::rename(&tmp, target).map_err(write_err)
}

/// How a sync treats the permission lists it writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum SyncMode {
    /// Add what the source has and keep everything else: lists are unioned, so
    /// a rule removed from the source stays in the harness's file. The
    /// historical behavior, and the default.
    #[default]
    AddOnly,
    /// Hold the list at each harness's allow/deny path to exactly the source's
    /// rendered list, in source order — stale and hand-added entries are
    /// removed, and every other key is left as the add-only merge leaves it.
    /// Under `check`, any difference in those lists is pending, extras
    /// included, and the report names each added and removed entry. The
    /// `--exact` flag.
    Exact,
}

/// What a [`sync`] call merges, as plain data.
///
/// Every field is one thing the CLI resolves from its own flags, so an embedder
/// states them rather than inheriting them from a process it does not own.
#[derive(Debug, Clone, Default)]
pub struct SyncRequest {
    /// The project directory whose harness config files are written; also where
    /// project-config discovery starts. `None` means the process's current
    /// directory, which is what the CLI uses.
    pub cwd: Option<PathBuf>,
    /// Harness id(s) to sync. Empty falls back to the configured `harnesses`,
    /// and then to every harness that has something to sync.
    pub harness: Vec<String>,
    /// Report what would change and write nothing — the `--check` flag.
    pub check: bool,
    /// Install hooks into the user-global location instead of the project.
    pub global: bool,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery — the repeatable `--config` flag. Each later file
    /// (its own `extends` chain beneath it) overrides the earlier ones; empty
    /// means discovery. See [`crate::io::config::load_layers`].
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Vec<PathBuf>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. Takes
    /// precedence over `config`: set together, no file is loaded.
    pub no_config: bool,
}

/// The `oneharness sync` output contract.
#[derive(Debug, Clone, schemars::JsonSchema, serde::Serialize)]
#[non_exhaustive]
pub struct SyncReport {
    pub schema_version: &'static str,
    /// The oneharness config files the synced settings came from.
    pub config_files: Vec<String>,
    /// True under `--check`: statuses describe what *would* happen.
    pub check: bool,
    /// True under `--exact`: each permission list was held to exactly the
    /// source's, and each result names the entries that added and removed.
    /// Omitted when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exact: bool,
    pub results: Vec<SyncResult>,
}

impl SyncReport {
    /// A report of the current schema version, for an add-only sync.
    #[must_use]
    pub fn new(config_files: Vec<String>, check: bool, results: Vec<SyncResult>) -> Self {
        Self {
            schema_version: crate::domain::report::SCHEMA_VERSION,
            config_files,
            check,
            exact: false,
            results,
        }
    }

    /// Whether this report describes a config file that differs from the
    /// policy — created, updated, or a hook that is not already installed.
    ///
    /// This answers only "did anything differ", not "should a caller fail".
    /// Under `--check` nothing was written, so a difference is still pending
    /// and [`SyncReport::check`] is what turns it into the CLI's non-zero
    /// exit; after a real sync the same difference has just been written, so
    /// the caller is looking at what *changed*, not at work left to do.
    /// Folding `check` in here would make a write-mode report claim nothing
    /// changed when it had rewritten every file, which is the opposite of what
    /// a library consumer inspecting the report is asking.
    #[must_use]
    pub fn changes(&self) -> bool {
        self.results.iter().any(|result| {
            matches!(result.status, SyncStatus::Created | SyncStatus::Updated)
                || result
                    .hooks
                    .iter()
                    .any(|hook| hook.status != FileStatus::Unchanged)
        })
    }
}

/// What one harness's sync did (or would do).
#[derive(Debug, Clone, schemars::JsonSchema, serde::Serialize)]
#[non_exhaustive]
pub struct SyncResult {
    pub harness: &'static str,
    /// The permission/settings config file written (or that would be written);
    /// `null` when nothing of that kind is configured for this harness.
    pub file: Option<String>,
    // NO doc comment, and a `//` rather than a `///` one for the same reason: a
    // `$ref` with a sibling `description` is merged inline by
    // json-schema-to-typescript instead of resolving to the named type, so the
    // generated SDK would lose `SyncStatus` as an exported name and `zod.ts`
    // would fail to import it. `SyncStatus`'s own definition carries the
    // description, including what `skipped` means here.
    pub status: SyncStatus,
    /// Normalized `[[hooks]]` files installed into this harness (a Goose hook
    /// writes two). Empty when no `[[hooks]]` entry targets it.
    pub hooks: Vec<HookFileResult>,
    /// Top-level settings that have no mapping for this harness (e.g. a
    /// top-level `allowed_tools` while the harness has no allow-list concept)
    /// — visible here and warned on stderr, never silently dropped.
    pub unmapped: Vec<&'static str>,
    /// Individual configured rules this harness cannot express (a Codex
    /// execpolicy pattern has no wildcards and no exact-length form), each left
    /// out of the file with the reason — never widened, and never silently
    /// dropped. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmapped_rules: Vec<UnmappedRule>,
    /// Under `--exact`: each entry the sync added (or would add) to this
    /// harness's permission lists. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added_rules: Vec<RuleChange>,
    /// Under `--exact`: each entry the sync removed (or would remove) from
    /// this harness's permission lists — stale and hand-added alike. Omitted
    /// when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_rules: Vec<RuleChange>,
}

impl SyncResult {
    /// A result with no hooks, no unmapped settings, and no rule changes.
    #[must_use]
    pub fn new(harness: &'static str, file: Option<String>, status: SyncStatus) -> Self {
        Self {
            harness,
            file,
            status,
            hooks: Vec::new(),
            unmapped: Vec::new(),
            unmapped_rules: Vec::new(),
            added_rules: Vec::new(),
            removed_rules: Vec::new(),
        }
    }
}

/// One installed `[[hooks]]` file.
#[derive(Debug, Clone, schemars::JsonSchema, serde::Serialize)]
pub struct HookFileResult {
    pub file: String,
    pub status: FileStatus,
}

/// Merge the unified policy settings into each selected harness's own config
/// file and return the report — the add-only [`sync_with`].
///
/// # Errors
///
/// As [`sync_with`].
pub fn sync(request: &SyncRequest) -> Result<SyncReport, OneharnessError> {
    sync_with(request, SyncMode::AddOnly)
}

/// Merge the unified policy settings into each selected harness's own config
/// file under `mode` and return the report.
///
/// Warnings about settings with no mapping for a harness go to the host's
/// stderr, exactly as they do from the CLI, so an embedder inherits them rather
/// than losing them.
///
/// # Errors
///
/// Returns a usage error for an unknown harness id or variant, a configuration
/// that cannot be loaded or merged, two variants of one harness that disagree
/// on what to write, a permission fragment under `global` (which has no
/// user-global mapping), or a config file that cannot be written.
pub fn sync_with(request: &SyncRequest, mode: SyncMode) -> Result<SyncReport, OneharnessError> {
    let exact = mode == SyncMode::Exact;
    // Mirror `run`: the project being synced is the request's cwd (else the
    // current directory), and that is also where discovery starts.
    let project_dir = match &request.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let loaded = crate::io::config::load(&request.config, request.no_config, &project_dir)?;
    let cfg = &loaded.config;
    let selected_ids = if request.harness.is_empty() {
        cfg.harnesses.clone().unwrap_or_default()
    } else {
        request.harness.clone()
    };
    let selected_ids = crate::domain::select::dedupe_exact_ids(&selected_ids);
    for id in &selected_ids {
        if let Some((base, variant)) = id.split_once(':') {
            if cfg.variant_for(id).is_none() {
                return Err(OneharnessError::UnknownHarnessVariant {
                    id: id.clone(),
                    base: base.to_string(),
                    variant: variant.to_string(),
                });
            }
        }
    }
    for (index, first_id) in selected_ids.iter().enumerate() {
        let (base, _) = cfg.split_harness_id(first_id);
        let spec = harness::by_id(base).ok_or_else(|| OneharnessError::UnknownHarness {
            id: base.to_string(),
            valid: harness::valid_ids(),
        })?;
        let first = sync_domain::plan_for(cfg, spec, first_id).map_err(|message| {
            OneharnessError::HarnessConfigUnmergeable {
                path: format!("[harness.{base}]"),
                message,
            }
        })?;
        for second_id in selected_ids.iter().skip(index + 1) {
            let (second_base, _) = cfg.split_harness_id(second_id);
            if base != second_base {
                continue;
            }
            let second = sync_domain::plan_for(cfg, spec, second_id).map_err(|message| {
                OneharnessError::HarnessConfigUnmergeable {
                    path: format!("[harness.{base}]"),
                    message,
                }
            })?;
            if first.fragment != second.fragment
                || first.unmapped != second.unmapped
                || sync_domain::rules_plan_for(cfg, spec, first_id)
                    != sync_domain::rules_plan_for(cfg, spec, second_id)
            {
                return Err(OneharnessError::VariantSyncConflict {
                    base: base.to_string(),
                    first: first_id.clone(),
                    second: second_id.clone(),
                });
            }
        }
    }

    // With no CLI/config selection, cover every harness; those with nothing to
    // sync report `skipped`. A configured selection stays explicit and ordered.
    let specs: Vec<&'static HarnessSpec> = if selected_ids.is_empty() {
        harness::all().iter().collect()
    } else {
        crate::domain::select::select_specs(false, &selected_ids, &[])?
    };

    let mut results = Vec::with_capacity(specs.len());

    // Resolved once: the user-global base dirs a `global` hook install anchors
    // under. Unused (but harmless) for a project sync.
    let global_dirs = crate::io::hooks::GlobalDirs::from_env();

    for (index, spec) in specs.into_iter().enumerate() {
        let selected_id = selected_ids.get(index).map_or(spec.id, String::as_str);
        let plan = sync_domain::plan_for(cfg, spec, selected_id).map_err(|message| {
            OneharnessError::HarnessConfigUnmergeable {
                path: format!("[harness.{}]", spec.id),
                message,
            }
        })?;
        for setting in &plan.unmapped {
            eprintln!(
                "oneharness: warning: `{setting}` has no mapping for harness `{}` and was NOT applied to it",
                spec.id
            );
        }
        let rules = sync_domain::rules_plan_for(cfg, spec, selected_id);
        let unmapped_rules = rules
            .as_ref()
            .map(|rules| rules.unmapped.clone())
            .unwrap_or_default();
        for rule in &unmapped_rules {
            eprintln!(
                "oneharness: warning: `{}` rule `{}` has no mapping for harness `{}` and was NOT applied to it: {}",
                rule.list.as_str(),
                rule.rule,
                spec.id,
                rule.reason
            );
        }
        // A configured permission/settings fragment (or rule list) has no
        // user-global mapping, so refuse it loudly rather than silently writing
        // only the hooks and leaving the rules behind.
        let configured =
            plan.fragment.is_some() || rules.as_ref().is_some_and(|rules| rules.configured);
        if configured && request.global {
            return Err(OneharnessError::GlobalSyncOnlyHooks { id: spec.id.into() });
        }
        let applied = match (&spec.sync, &rules) {
            // `--global` writes no permission file at all, so there is none to
            // hold exact either.
            _ if request.global => None,
            (Some(sync_spec), Some(rules)) => {
                apply_rules(&project_dir, sync_spec, rules, exact, request.check)?
            }
            (Some(sync_spec), None) if exact => apply_exact(
                &project_dir,
                sync_spec,
                plan.fragment.as_ref(),
                request.check,
            )?,
            (Some(sync_spec), None) => match &plan.fragment {
                Some(fragment) => {
                    let (path, status) = apply(&project_dir, sync_spec, fragment, request.check)?;
                    Some(Applied {
                        path,
                        status,
                        added: Vec::new(),
                        removed: Vec::new(),
                    })
                }
                None => None,
            },
            (None, _) => None,
        };
        let (file, status, added_rules, removed_rules) = match applied {
            Some(applied) => (
                Some(applied.path.display().to_string()),
                applied.status.into(),
                applied.added,
                applied.removed,
            ),
            None => (None, SyncStatus::Skipped, Vec::new(), Vec::new()),
        };

        // Normalized `[[hooks]]` install into this harness's native shape,
        // independent of the permission/settings fragment above.
        let mut hooks = Vec::new();
        for hook in cfg.hook_specs_for(spec.id) {
            let scope = if request.global {
                crate::io::hooks::Scope::Global(&global_dirs)
            } else {
                crate::io::hooks::Scope::Project(&project_dir)
            };
            for write in crate::io::hooks::install(scope, spec, &hook, request.check)? {
                hooks.push(HookFileResult {
                    file: write.path.display().to_string(),
                    status: write.status,
                });
            }
        }

        results.push(SyncResult {
            harness: spec.id,
            file,
            status,
            hooks,
            unmapped: plan.unmapped,
            unmapped_rules,
            added_rules,
            removed_rules,
        });
    }

    Ok(SyncReport {
        schema_version: crate::domain::report::SCHEMA_VERSION,
        config_files: loaded.files,
        check: request.check,
        exact,
        results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::scratch::ScratchDir;
    use serde_json::json;

    fn temp_project(tag: &str) -> ScratchDir {
        ScratchDir::new(&format!("sync-{tag}")).unwrap()
    }

    const SPEC: SyncSpec = SyncSpec {
        file: "sub/config.json",
        alt_files: &[".alt.json"],
        allow_path: None,
        deny_path: None,
        hooks_path: None,
        schema_seed: None,
    };

    #[test]
    fn creates_with_parent_dirs_then_reports_unchanged() {
        let dir = temp_project("create");
        let fragment = json!({"a": 1});
        let (path, status) = apply(&dir, &SPEC, &fragment, false).unwrap();
        assert_eq!(status, FileStatus::Created);
        assert_eq!(path, dir.join("sub/config.json"));
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk, fragment);
        // Idempotent: a second apply changes nothing.
        let (_, status) = apply(&dir, &SPEC, &fragment, false).unwrap();
        assert_eq!(status, FileStatus::Unchanged);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn existing_alt_file_is_merged_into_instead() {
        let dir = temp_project("alt");
        std::fs::write(dir.join(".alt.json"), "{\"keep\": true}").unwrap();
        let (path, status) = apply(&dir, &SPEC, &json!({"a": 1}), false).unwrap();
        assert_eq!(status, FileStatus::Updated);
        assert_eq!(path, dir.join(".alt.json"));
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk, json!({"keep": true, "a": 1}));
        assert!(!dir.join("sub/config.json").exists(), "no shadow file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_mode_writes_nothing() {
        let dir = temp_project("check");
        let (_, status) = apply(&dir, &SPEC, &json!({"a": 1}), true).unwrap();
        assert_eq!(status, FileStatus::Created);
        assert!(!dir.join("sub/config.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    const LISTS: SyncSpec = SyncSpec {
        file: "sub/config.json",
        alt_files: &[],
        allow_path: Some(&["permissions", "allow"]),
        deny_path: Some(&["permissions", "deny"]),
        hooks_path: None,
        schema_seed: None,
    };

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn exact_apply_replaces_the_lists_and_keeps_every_other_key() {
        let dir = temp_project("exact");
        let path = dir.join("sub/config.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"permissions":{"allow":["old","hand"],"ask":["x"]},"env":{"A":"1"}}"#,
        )
        .unwrap();
        let fragment = json!({"permissions": {"allow": ["new"]}});
        let checked = apply_exact(&dir, &LISTS, Some(&fragment), true)
            .unwrap()
            .unwrap();
        assert_eq!(checked.status, FileStatus::Updated);
        assert_eq!(
            checked
                .removed
                .iter()
                .map(|c| c.rule.as_str())
                .collect::<Vec<_>>(),
            ["old", "hand"]
        );
        assert_eq!(
            read_json(&path)["permissions"]["allow"],
            json!(["old", "hand"])
        );

        let applied = apply_exact(&dir, &LISTS, Some(&fragment), false)
            .unwrap()
            .unwrap();
        assert_eq!(applied.added[0].rule, "new");
        assert_eq!(
            read_json(&path),
            json!({"permissions": {"allow": ["new"], "ask": ["x"]}, "env": {"A": "1"}})
        );
        let again = apply_exact(&dir, &LISTS, Some(&fragment), false)
            .unwrap()
            .unwrap();
        assert_eq!(again.status, FileStatus::Unchanged);
        // Nothing configured any more: the list the file holds is emptied.
        let emptied = apply_exact(&dir, &LISTS, None, false).unwrap().unwrap();
        assert_eq!(emptied.status, FileStatus::Updated);
        assert_eq!(read_json(&path)["permissions"]["allow"], json!([]));
        // ...and after that there is nothing left to hold.
        assert!(apply_exact(&dir, &LISTS, None, false).unwrap().is_none());
    }

    #[test]
    fn exact_apply_with_nothing_configured_and_no_file_is_nothing() {
        let dir = temp_project("exact-none");
        assert!(apply_exact(&dir, &LISTS, None, false).unwrap().is_none());
        assert!(!dir.join("sub/config.json").exists());
        let created = apply_exact(&dir, &LISTS, Some(&json!({"a": 1})), false)
            .unwrap()
            .unwrap();
        assert_eq!(created.status, FileStatus::Created);
    }

    fn codex_rules(text: &str) -> RulesPlan {
        let cfg = crate::domain::config::parse(text).unwrap();
        sync_domain::rules_plan_for(&cfg, harness::by_id("codex").unwrap(), "codex").unwrap()
    }

    #[test]
    fn a_rules_file_is_written_whole_and_rewritten_whole() {
        let dir = temp_project("rules");
        let spec = harness::by_id("codex").unwrap().sync.as_ref().unwrap();
        let path = dir.join(".codex/rules/oneharness.rules");
        let plan = codex_rules("allowed_tools = [\"Bash(ls:*)\"]");
        // Nothing configured and no file: nothing to write.
        assert!(apply_rules(&dir, spec, &codex_rules(""), false, false)
            .unwrap()
            .is_none());
        let checked = apply_rules(&dir, spec, &plan, false, true)
            .unwrap()
            .unwrap();
        assert_eq!(checked.status, FileStatus::Created);
        assert!(!path.exists(), "a check writes nothing");
        let created = apply_rules(&dir, spec, &plan, false, false)
            .unwrap()
            .unwrap();
        assert_eq!(created.status, FileStatus::Created);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), plan.text);
        assert!(created.added.is_empty(), "rule diffs are an --exact report");
        let same = apply_rules(&dir, spec, &plan, false, false)
            .unwrap()
            .unwrap();
        assert_eq!(same.status, FileStatus::Unchanged);

        // A hand edit is replaced in either mode; --exact names what went.
        let hand = format!(
            "{}prefix_rule(pattern=[\"curl\"], decision=\"prompt\")\n",
            plan.text
        );
        std::fs::write(&path, &hand).unwrap();
        let exact = apply_rules(&dir, spec, &plan, true, true).unwrap().unwrap();
        assert_eq!(exact.status, FileStatus::Updated);
        assert_eq!(exact.removed.len(), 1);
        assert!(exact.removed[0].rule.contains("curl"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            hand,
            "check wrote nothing"
        );
        let fixed = apply_rules(&dir, spec, &plan, false, false)
            .unwrap()
            .unwrap();
        assert_eq!(fixed.status, FileStatus::Updated);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), plan.text);

        // The rules removed from the source: the owned file is emptied, not left.
        let emptied = apply_rules(&dir, spec, &codex_rules(""), true, false)
            .unwrap()
            .unwrap();
        assert_eq!(emptied.status, FileStatus::Updated);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            sync_domain::RULES_FILE_HEADER
        );
        assert!(emptied.removed[0].rule.contains("[\"ls\"]"));
    }

    /// The prose that restates this module's contract — the README's matrix
    /// row, translation table, `--exact` and live-proof sections, the engine's AGENTS.md,
    /// and the parity note — is held to the code here, so a rename or a
    /// changed translation cannot leave the documents describing the old one.
    #[test]
    fn the_documents_restating_the_sync_contract_match_the_code() {
        use crate::domain::sync::RuleList;
        let readme = include_str!("../../../../README.md").replace("\r\n", "\n");
        let agents = include_str!("../../AGENTS.md");
        let parity = include_str!("../../../../docs/sdk-parity.md");
        let e2e = include_str!("../../../../scripts/e2e-lib.sh");
        let lane = include_str!("../../../../scripts/e2e-codex.sh");

        // The matrix row names the registry's Codex target.
        let codex = harness::by_id("codex").unwrap().sync.as_ref().unwrap();
        let row = readme
            .lines()
            .find(|line| line.starts_with("| `codex` | OpenAI Codex CLI |"))
            .expect("README.md has the codex matrix row");
        assert!(row.contains(&format!("`{}`", codex.file)), "{row}");

        // Each translation-table row says what the translator does.
        for (list, rule) in [
            (RuleList::AllowedTools, "Bash(git status:*)"),
            (RuleList::AllowedTools, "Bash(git status *)"),
            (RuleList::DeniedTools, "Bash(rm -rf:*)"),
        ] {
            let text = match list {
                RuleList::AllowedTools => format!("allowed_tools = [{rule:?}]"),
                RuleList::DeniedTools => format!("denied_tools = [{rule:?}]"),
            };
            let rendered = codex_rules(&text).text;
            let statement = rendered.lines().last().unwrap();
            assert!(readme.contains(&format!("`{rule}`")), "{rule}");
            assert!(readme.contains(&format!("`{statement}`")), "{statement}");
        }
        for rule in [
            "Bash(just check)",
            "Bash(git -C * log*)",
            "Bash(git status*)",
            "Read",
        ] {
            assert!(sync_domain::exec_policy_pattern(rule).is_err(), "{rule}");
            assert!(readme.contains(&format!("`{rule}`")), "{rule}");
        }

        // The report fields every document names are the serialized ones.
        let mut result = SyncResult::new("codex", None, SyncStatus::Skipped);
        result.unmapped_rules = vec![UnmappedRule::new(RuleList::AllowedTools, "Read", "r")];
        result.added_rules = vec![RuleChange::new(Some(RuleList::AllowedTools), "a")];
        result.removed_rules = vec![RuleChange::new(None, "b")];
        let mut report = SyncReport::new(Vec::new(), true, vec![result]);
        report.exact = true;
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["exact"], true);
        for field in ["unmapped_rules", "added_rules", "removed_rules"] {
            assert!(value["results"][0].get(field).is_some(), "{field}");
            for (name, document) in [
                ("README.md", readme.as_str()),
                ("docs/sdk-parity.md", parity),
            ] {
                assert!(document.contains(&format!("`{field}`")), "{name}: {field}");
            }
        }
        assert!(agents.contains("`unmapped_rules`"));
        assert_eq!(
            value["results"][0]["unmapped_rules"][0]["list"],
            "allowed_tools"
        );
        assert!(readme.contains("`unmapped_rules: [{list, rule, reason}]`"));
        assert!(readme.contains("`added_rules` / `removed_rules` (`[{list, rule}]`)"));

        // The Rust spellings the documents give compile, and are the manifest's.
        let _: fn(&SyncRequest, SyncMode) -> Result<SyncReport, OneharnessError> = sync_with;
        let _ = (SyncMode::Exact, SyncSpec::format);
        let manifest = crate::domain::capability::CAPABILITIES
            .iter()
            .find(|capability| capability.method == "sync")
            .unwrap();
        assert_eq!(manifest.rust, "oneharness_core::io::sync::sync_with");
        for document in [readme.as_str(), parity] {
            assert!(document.contains("sync_with(&request"), "sync_with");
            assert!(document.contains("SyncMode::Exact"), "SyncMode::Exact");
        }
        assert!(agents.contains("`SyncMode::Exact`") && agents.contains("`SyncSpec::format`"));

        // The live proof the README describes is the one the lane runs.
        assert!(lane.contains("\noh_codex_rules_enforce\n"));
        assert!(e2e.contains("\noh_codex_rules_enforce() {"));
        assert!(e2e.contains(r#"allowed_tools = ["Bash(%s:*)"]"#));
        assert!(e2e.contains(r#"denied_tools = ["Bash(mkdir %s:*)"]"#));
        // The allow half's delete is spelled per shell: Windows Codex runs
        // PowerShell, whose `rm` alias rejects `-f`.
        assert!(e2e.contains("\n_oh_codex_render_forced_delete() {"));
        assert!(e2e.contains(r#"del="$(_oh_codex_render_forced_delete "$file")""#));
        assert!(e2e.contains("printf 'Remove-Item -Force %s' \"$1\""));
        assert!(e2e.contains("printf 'rm -f %s' \"$1\""));
        assert!(lane.contains("\noh_codex_rules_match\n"));
        assert!(e2e.contains("\noh_codex_rules_match() {"));
        assert!(e2e.contains("allow Remove-Item -Force $file\n"));
        assert!(
            readme.contains("`oh_codex_rules_enforce`")
                && readme.contains("`oh_codex_rules_match`")
        );
        assert!(
            readme.contains("`Bash(mkdir <dir>:*)`")
                && readme.contains("`rm -f <file>`")
                && readme.contains("`Remove-Item -Force <file>`")
        );
    }

    #[test]
    fn unparseable_existing_file_is_a_loud_error_and_untouched() {
        let dir = temp_project("jsonc");
        let path = dir.join(".alt.json");
        std::fs::write(&path, "{ // a comment\n  \"a\": 1 }").unwrap();
        let err = apply(&dir, &SPEC, &json!({"b": 2}), false).unwrap_err();
        assert!(err.to_string().contains("not valid JSON"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{ // a comment\n  \"a\": 1 }",
            "file must be left untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
