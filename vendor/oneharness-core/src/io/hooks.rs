//! Installing a normalized hook into a harness's config — the I/O half of
//! cross-harness hook management. Works at either [`Scope`]: a project directory
//! or the harness's user-global location (`$HOME`/`$XDG_CONFIG_HOME`), with the
//! same write strategy and only the anchor moving. The shape rendering is pure
//! (`src/domain/hooks.rs`); this layer resolves the target file(s), seeds a new
//! file's scaffolding, deep-merges (or writes a JS shim), and writes atomically.
//!
//! Every write is non-destructive and idempotent, the same contract `oneharness
//! sync` already honours for permission rules: unrelated keys are preserved,
//! hook lists union, an unparseable target is refused and left intact, and the
//! write goes through a temp file + rename. The four [`HookBinding`] variants
//! cover all eight harnesses — three deep-merge JSON, OpenCode writes a shim.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::domain::harness::{HarnessSpec, HookBase, HookBinding};
use crate::domain::hooks::{render, HookSpec};
use crate::domain::sync::deep_merge;
use crate::errors::OneharnessError;
use crate::io::sync::{write_atomically, FileStatus};

/// The plugin/file identity used when a [`HookSpec`] names none.
const DEFAULT_PLUGIN_NAME: &str = "oneharness";

/// Which scope a hook install targets: a project directory, or the user-global
/// location resolved from the environment.
pub enum Scope<'a> {
    /// Install under a project directory (the harness's project-relative paths).
    Project(&'a Path),
    /// Install at the harness's user-global location, anchored under these dirs.
    Global(&'a GlobalDirs),
}

/// The user-level base directories a [`Scope::Global`] install resolves against.
/// Injected so installs stay hermetic and testable; populate from the process
/// with [`GlobalDirs::from_env`].
#[derive(Debug, Clone, Default)]
pub struct GlobalDirs {
    /// `$HOME`.
    pub home: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME`.
    pub config_home: Option<PathBuf>,
}

impl GlobalDirs {
    /// Read `HOME` and `XDG_CONFIG_HOME` from the process environment (and, on
    /// Windows, `USERPROFILE`).
    pub fn from_env() -> Self {
        Self::from_vars(
            std::env::var_os("HOME"),
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("USERPROFILE"),
        )
    }

    /// Pure resolution of the directories from raw env values, so the
    /// platform-specific home selection stays testable. On Windows the user home
    /// is `%USERPROFILE%` — what Node-based CLIs (`os.homedir()`) and most tools
    /// anchor `~` to — whereas `$HOME` is often unset or an MSYS path (e.g. Git
    /// Bash's `/c/Users/...`), which a native harness can't resolve. Preferring
    /// `USERPROFILE` there keeps a `--global` hook install landing where the
    /// harness actually reads it. Unix is unchanged: `HOME` only.
    fn from_vars(
        home: Option<std::ffi::OsString>,
        config_home: Option<std::ffi::OsString>,
        userprofile: Option<std::ffi::OsString>,
    ) -> Self {
        let home = if cfg!(windows) {
            userprofile.or(home)
        } else {
            let _ = userprofile;
            home
        };
        Self {
            home: home.map(PathBuf::from),
            config_home: config_home.map(PathBuf::from),
        }
    }

    /// Resolve a [`HookBase`] to a concrete directory, applying the
    /// `$XDG_CONFIG_HOME` → `$HOME/.config` fallback for `ConfigHome`.
    fn resolve(&self, base: HookBase) -> Option<PathBuf> {
        match base {
            HookBase::Home => self.home.clone(),
            HookBase::ConfigHome => self
                .config_home
                .clone()
                .or_else(|| self.home.as_ref().map(|h| h.join(".config"))),
        }
    }
}

/// One file `install` created, updated, or found already current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookWrite {
    pub path: PathBuf,
    pub status: FileStatus,
}

/// Install `hook` into `spec`'s config at `scope`, returning a write per file
/// touched (a Goose install touches two). `check` plans without writing. The
/// strategy is identical across scopes — only the anchor moves — so a project
/// and a global install of the same hook produce the same file shapes. A harness
/// with no hook mapping (or no global location, for [`Scope::Global`]) is a loud
/// [`OneharnessError`], never a silent no-op.
pub fn install(
    scope: Scope,
    spec: &HarnessSpec,
    hook: &HookSpec,
    check: bool,
) -> Result<Vec<HookWrite>, OneharnessError> {
    let Some(binding) = &spec.hooks else {
        return Err(OneharnessError::HookUnsupported { id: spec.id.into() });
    };
    let name = hook.plugin_name.as_deref().unwrap_or(DEFAULT_PLUGIN_NAME);
    let anchor = anchor_for(&scope, spec, binding, name)?;

    match binding {
        HookBinding::SameFile { shape, path } => {
            let fragment = wrap(path, render(hook, *shape));
            Ok(vec![merge_json(&anchor, &fragment, None, check)?])
        }
        HookBinding::File {
            shape, path, seed, ..
        } => {
            let fragment = wrap(path, render(hook, *shape));
            Ok(vec![merge_json(&anchor, &fragment, *seed, check)?])
        }
        HookBinding::GoosePlugin {
            shape,
            manifest,
            path,
            ..
        } => {
            // `anchor` is the plugin directory; the manifest and hooks file sit
            // beneath it. A caller-supplied description overrides the manifest's
            // default text (set on the parsed value to avoid JSON-escaping the
            // raw template).
            let mut manifest_json = parse_seed(&manifest.replace("{name}", name));
            if let (Some(desc), Value::Object(map)) = (&hook.description, &mut manifest_json) {
                map.insert("description".into(), Value::String(desc.clone()));
            }
            let manifest_write =
                merge_json(&anchor.join("plugin.json"), &manifest_json, None, check)?;
            let fragment = wrap(path, render(hook, *shape));
            let hooks_write = merge_json(
                &anchor.join("hooks").join("hooks.json"),
                &fragment,
                None,
                check,
            )?;
            Ok(vec![manifest_write, hooks_write])
        }
        HookBinding::JsPlugin { template, .. } => {
            let content = render_shim(template, name, &hook.command);
            Ok(vec![write_text(&anchor, &content, check)?])
        }
    }
}

/// A byte-level snapshot of config files about to be touched by an ephemeral
/// hook install (`run --mock-rules`), so the workspace can be put back exactly
/// as it was after the run — including in an "original" workspace whose
/// existing config the install merged into. A file absent at snapshot time is
/// restored by deletion, along with any directories the install had to create
/// for it (removed only while empty, so nothing else is ever swept up).
#[derive(Debug, Default)]
pub struct HookSnapshot {
    /// Each path with its pre-install bytes (`None` = did not exist).
    entries: Vec<(PathBuf, Option<Vec<u8>>)>,
    /// Ancestor directories that did not exist at snapshot time, deepest last.
    created_dirs: Vec<PathBuf>,
}

impl HookSnapshot {
    /// Record the current state of `paths` (typically the planned writes a
    /// check-mode [`install`] returned) before the real install runs.
    pub fn capture(paths: &[PathBuf]) -> Self {
        let mut snapshot = HookSnapshot::default();
        for path in paths {
            snapshot
                .entries
                .push((path.clone(), std::fs::read(path).ok()));
            // Note which ancestors the install will have to create, so restore
            // can remove them again (nearest-missing first here; restore walks
            // them deepest-first).
            let mut missing = Vec::new();
            let mut cursor = path.parent();
            while let Some(dir) = cursor {
                if dir.as_os_str().is_empty() || dir.exists() {
                    break;
                }
                missing.push(dir.to_path_buf());
                cursor = dir.parent();
            }
            for dir in missing.into_iter().rev() {
                if !snapshot.created_dirs.contains(&dir) {
                    snapshot.created_dirs.push(dir);
                }
            }
        }
        snapshot
    }

    /// Fold another snapshot's entries into this one (per-harness installs are
    /// captured separately so an earlier install's writes are never re-captured
    /// as if they were pre-existing state).
    pub fn extend(&mut self, other: HookSnapshot) {
        self.entries.extend(other.entries);
        for dir in other.created_dirs {
            if !self.created_dirs.contains(&dir) {
                self.created_dirs.push(dir);
            }
        }
    }

    /// Put every captured path back: rewrite prior bytes, delete files that did
    /// not exist, and prune the directories the install created (best-effort,
    /// only while empty). Returns the failures (path + error) instead of
    /// aborting, so a caller can warn per file — a restore must never take the
    /// run's results down with it.
    pub fn restore(&self) -> Vec<(PathBuf, std::io::Error)> {
        let mut failures = Vec::new();
        for (path, prior) in &self.entries {
            let result = match prior {
                Some(bytes) => std::fs::write(path, bytes),
                None => match std::fs::remove_file(path) {
                    // Already absent — nothing to undo.
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    other => other,
                },
            };
            if let Err(err) = result {
                failures.push((path.clone(), err));
            }
        }
        // Deepest-first so nested created dirs empty out before their parents;
        // `remove_dir` refuses non-empty dirs, which is exactly the safety we
        // want (never remove anything the user put there meanwhile).
        for dir in self.created_dirs.iter().rev() {
            let _ = std::fs::remove_dir(dir);
        }
        failures
    }
}

/// The path a binding anchors at for the given scope: a file for the JSON-merge
/// and JS-shim strategies, the plugin directory for Goose. The project paths come
/// from the [`HookBinding`]; the global ones from the harness's `global_hook`.
fn anchor_for(
    scope: &Scope,
    spec: &HarnessSpec,
    binding: &HookBinding,
    name: &str,
) -> Result<PathBuf, OneharnessError> {
    match scope {
        Scope::Project(dir) => Ok(project_anchor(dir, spec, binding, name)),
        Scope::Global(dirs) => {
            let global = spec
                .global_hook
                .ok_or_else(|| OneharnessError::HookGlobalUnsupported { id: spec.id.into() })?;
            let base =
                dirs.resolve(global.base)
                    .ok_or_else(|| OneharnessError::HookGlobalDirMissing {
                        id: spec.id.into(),
                        var: match global.base {
                            HookBase::Home => "HOME",
                            HookBase::ConfigHome => "XDG_CONFIG_HOME or HOME",
                        },
                    })?;
            Ok(base.join(global.anchor.replace("{name}", name)))
        }
    }
}

/// Project-scope anchor for a binding under `project_dir`.
fn project_anchor(
    project_dir: &Path,
    spec: &HarnessSpec,
    binding: &HookBinding,
    name: &str,
) -> PathBuf {
    match binding {
        HookBinding::SameFile { .. } => {
            // Hooks share the permissions file, so honour the same alt_files
            // precedence `sync` does (e.g. crush's `.crush.json`).
            let sync = spec
                .sync
                .as_ref()
                .expect("SameFile hooks require a sync config file");
            sync.alt_files
                .iter()
                .map(|f| project_dir.join(f))
                .find(|p| p.is_file())
                .unwrap_or_else(|| project_dir.join(sync.file))
        }
        HookBinding::File { file, .. } => project_dir.join(file.replace("{name}", name)),
        HookBinding::GoosePlugin { plugins_dir, .. } => project_dir.join(plugins_dir).join(name),
        HookBinding::JsPlugin { plugin_dir, .. } => {
            project_dir.join(plugin_dir).join(format!("{name}.js"))
        }
    }
}

/// Nest `value` under `path` (e.g. `["hooks"]` -> `{"hooks": value}`), the
/// object the harness reads at its top level.
fn wrap(path: &[&str], value: Value) -> Value {
    let mut node = value;
    for key in path.iter().rev() {
        let mut map = Map::new();
        map.insert((*key).to_string(), node);
        node = Value::Object(map);
    }
    node
}

/// Render the OpenCode shim: substitute the JS-safe export identifier, the
/// command as a JSON argv array, and the display name.
fn render_shim(template: &str, name: &str, command: &str) -> String {
    let argv: Vec<&str> = command.split_whitespace().collect();
    let argv_json = serde_json::to_string(&argv).expect("argv of strings serializes");
    template
        .replace("{export}", &js_identifier(name))
        .replace("{argv}", &argv_json)
        .replace("{name}", name)
}

/// A safe JS identifier from a plugin name: non-alphanumeric runs become `_`,
/// and a leading digit is prefixed, so `my-tool` -> `my_tool`.
fn js_identifier(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out.chars().next().is_none_or(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// Parse a registry seed/manifest, which is test-pinned valid JSON.
fn parse_seed(text: &str) -> Value {
    serde_json::from_str(text).expect("registry seed/manifest is valid JSON (test-pinned)")
}

/// Deep-merge `fragment` into `target`'s JSON. A missing file is created from
/// `seed` (or an empty object) then merged; an existing one is merged in place.
/// An unparseable target is refused and left untouched.
fn merge_json(
    target: &Path,
    fragment: &Value,
    seed: Option<&str>,
    check: bool,
) -> Result<HookWrite, OneharnessError> {
    let existing: Option<Value> = match std::fs::read_to_string(target) {
        Ok(text) => Some(serde_json::from_str(&text).map_err(|e| {
            OneharnessError::HarnessConfigUnmergeable {
                path: target.display().to_string(),
                message: format!("not valid JSON ({e}); fix or remove it and re-run"),
            }
        })?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(OneharnessError::HarnessConfigRead {
                path: target.display().to_string(),
                source,
            })
        }
    };

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
        None => {
            let base = seed.map(parse_seed).unwrap_or(Value::Object(Map::new()));
            (deep_merge(&base, fragment), FileStatus::Created)
        }
    };

    if !check && status != FileStatus::Unchanged {
        write_atomically(target, &merged)?;
    }
    Ok(HookWrite {
        path: target.to_path_buf(),
        status,
    })
}

/// Write a text file (the JS shim) idempotently and atomically: unchanged when
/// the bytes already match, else created/updated via a temp file + rename.
fn write_text(target: &Path, content: &str, check: bool) -> Result<HookWrite, OneharnessError> {
    let existing = match std::fs::read_to_string(target) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(OneharnessError::HarnessConfigRead {
                path: target.display().to_string(),
                source,
            })
        }
    };
    let status = match &existing {
        Some(text) if text == content => FileStatus::Unchanged,
        Some(_) => FileStatus::Updated,
        None => FileStatus::Created,
    };

    if !check && status != FileStatus::Unchanged {
        let write_err = |source: std::io::Error| OneharnessError::HarnessConfigWrite {
            path: target.display().to_string(),
            source,
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(write_err)?;
        }
        let tmp = target.with_extension("oneharness.tmp");
        std::fs::write(&tmp, content).map_err(write_err)?;
        std::fs::rename(&tmp, target).map_err(write_err)?;
    }
    Ok(HookWrite {
        path: target.to_path_buf(),
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::harness;
    use crate::io::scratch::ScratchDir;
    use serde_json::json;

    #[test]
    fn global_home_prefers_userprofile_on_windows_only() {
        // A `--global` hook must land where the (often Node-based) harness reads
        // `~`: %USERPROFILE% on Windows, $HOME elsewhere.
        let dirs = GlobalDirs::from_vars(Some("/home/u".into()), None, Some(r"C:\Users\u".into()));
        if cfg!(windows) {
            assert_eq!(dirs.home, Some(PathBuf::from(r"C:\Users\u")));
        } else {
            assert_eq!(dirs.home, Some(PathBuf::from("/home/u")));
        }
    }

    fn temp_project(tag: &str) -> ScratchDir {
        ScratchDir::new(&format!("hooks-{tag}-{:?}", std::thread::current().id())).unwrap()
    }

    fn install_one(dir: &Path, id: &str, hook: &HookSpec) -> Vec<HookWrite> {
        install(
            Scope::Project(dir),
            harness::by_id(id).unwrap(),
            hook,
            false,
        )
        .expect("install should succeed")
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Claude shares its permissions file: the hook lands under `hooks` in
    /// `.claude/settings.json` without disturbing existing keys.
    #[test]
    fn same_file_merges_into_claude_settings_without_clobbering() {
        let dir = temp_project("claude");
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(
            dir.join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Read"]}}"#,
        )
        .unwrap();
        let hook = HookSpec {
            command: "guard hook claude-code".into(),
            matcher: Some("Bash".into()),
            timeout: None,
            plugin_name: None,
            description: None,
        };
        let writes = install_one(&dir, "claude-code", &hook);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].status, FileStatus::Updated);
        assert_eq!(
            read_json(&dir.join(".claude/settings.json")),
            json!({
                "permissions": { "allow": ["Read"] },
                "hooks": {
                    "PreToolUse": [
                        { "matcher": "Bash", "hooks": [{ "type": "command", "command": "guard hook claude-code" }] }
                    ]
                }
            }),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Codex hooks live in a dedicated file that is created on first install.
    #[test]
    fn file_strategy_creates_codex_hooks_file() {
        let dir = temp_project("codex");
        let writes = install_one(&dir, "codex", &HookSpec::command("guard hook codex"));
        assert_eq!(writes[0].status, FileStatus::Created);
        assert_eq!(writes[0].path, dir.join(".codex/hooks.json"));
        assert_eq!(
            read_json(&dir.join(".codex/hooks.json")),
            json!({
                "hooks": { "PreToolUse": [{ "hooks": [{ "type": "command", "command": "guard hook codex" }] }] }
            }),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cursor's dedicated file is seeded with its required `version` and fans
    /// the command across the three `before*` events plus `preToolUse` (the
    /// rewrite-capable event `oneharness mock` rides).
    #[test]
    fn file_strategy_seeds_cursor_version() {
        let dir = temp_project("cursor");
        install_one(&dir, "cursor", &HookSpec::command("guard hook cursor"));
        assert_eq!(
            read_json(&dir.join(".cursor/hooks.json")),
            json!({
                "version": 1,
                "hooks": {
                    "beforeShellExecution": [{ "command": "guard hook cursor" }],
                    "beforeReadFile": [{ "command": "guard hook cursor" }],
                    "beforeMCPExecution": [{ "command": "guard hook cursor" }],
                    "preToolUse": [{ "command": "guard hook cursor" }],
                }
            }),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Copilot's per-owner file name comes from the plugin identity.
    #[test]
    fn file_strategy_names_copilot_file_after_plugin() {
        let dir = temp_project("copilot");
        let hook = HookSpec {
            plugin_name: Some("guard".into()),
            ..HookSpec::command("guard hook copilot")
        };
        let writes = install_one(&dir, "copilot", &hook);
        assert_eq!(writes[0].path, dir.join(".github/hooks/guard.json"));
        assert_eq!(
            read_json(&dir.join(".github/hooks/guard.json")),
            json!({
                "version": 1,
                "hooks": {
                    "preToolUse": [
                        { "type": "command", "bash": "guard hook copilot", "powershell": "guard hook copilot" }
                    ]
                }
            }),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Goose installs two files: a one-time manifest and the hooks json, both
    /// under a plugin dir named for the plugin identity.
    #[test]
    fn goose_plugin_writes_manifest_and_hooks() {
        let dir = temp_project("goose");
        let hook = HookSpec {
            command: "guard hook goose".into(),
            matcher: Some("^(shell|read)$".into()),
            timeout: Some(10),
            plugin_name: None,
            description: None,
        };
        let writes = install_one(&dir, "goose", &hook);
        assert_eq!(writes.len(), 2);
        let plugin = dir.join(".agents/plugins/oneharness");
        assert_eq!(
            read_json(&plugin.join("plugin.json")),
            json!({
                "name": "oneharness",
                "version": "0.1.0",
                "description": "Pre-tool hook installed by oneharness.",
            }),
        );
        assert_eq!(
            read_json(&plugin.join("hooks/hooks.json")),
            json!({
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "^(shell|read)$",
                            "hooks": [{ "type": "command", "command": "guard hook goose", "timeout": 10 }]
                        }
                    ]
                }
            }),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A caller-supplied description brands the Goose manifest, overriding the
    /// default text — so a consumer (e.g. allowlister) keeps its own plugin
    /// description instead of inheriting oneharness's.
    #[test]
    fn goose_manifest_honors_a_caller_description() {
        let dir = temp_project("goose-desc");
        let hook = HookSpec {
            description: Some("Gate AI-agent shell commands through allowlister.".into()),
            ..HookSpec::command("allowlister hook goose")
        };
        install_one(&dir, "goose", &hook);
        let manifest = read_json(&dir.join(".agents/plugins/oneharness/plugin.json"));
        assert_eq!(
            manifest["description"],
            "Gate AI-agent shell commands through allowlister."
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// OpenCode gets a JS shim that spawns the command as an argv array under
    /// its plugin name; the command is wired in, never hardcoded.
    #[test]
    fn js_plugin_writes_shim_with_command_argv() {
        let dir = temp_project("opencode");
        let hook = HookSpec {
            plugin_name: Some("guard".into()),
            ..HookSpec::command("guard hook opencode")
        };
        let writes = install_one(&dir, "opencode", &hook);
        assert_eq!(writes[0].path, dir.join(".opencode/plugin/guard.js"));
        let shim = std::fs::read_to_string(writes[0].path.clone()).unwrap();
        assert!(
            shim.contains(r#"Bun.spawn(["guard","hook","opencode"]"#),
            "command must be wired into the shim as an argv array:\n{shim}"
        );
        assert!(
            shim.contains("export const guard ="),
            "export uses the plugin identity:\n{shim}"
        );
        // The shim forwards OpenCode's camelCase `input.sessionID`, normalized to
        // the snake_case `session_id` every adapter feeds the engine.
        assert!(
            shim.contains("const session_id = (input && input.sessionID) || undefined;"),
            "session id must be read from input.sessionID:\n{shim}"
        );
        assert!(
            shim.contains("JSON.stringify({ tool_name, tool_input: args, cwd, session_id })"),
            "session_id must be on the stdin payload:\n{shim}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A snapshot taken before an ephemeral install restores an existing file
    /// byte-identically, deletes a file that did not exist, and prunes the
    /// directories the install created — but only while they are empty.
    #[test]
    fn snapshot_restores_existing_files_and_removes_created_ones() {
        let dir = temp_project("snapshot");
        let existing = dir.join("crush.json");
        std::fs::write(&existing, r#"{"keep":"me"}"#).unwrap();
        let created = dir.join(".codex").join("hooks.json");

        let snapshot = HookSnapshot::capture(&[existing.clone(), created.clone()]);
        // Simulate the installs: mutate the existing file, create the new one.
        std::fs::write(&existing, r#"{"keep":"me","hooks":{}}"#).unwrap();
        std::fs::create_dir_all(created.parent().unwrap()).unwrap();
        std::fs::write(&created, r#"{"hooks":{}}"#).unwrap();

        let failures = snapshot.restore();
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(
            std::fs::read_to_string(&existing).unwrap(),
            r#"{"keep":"me"}"#
        );
        assert!(!created.exists(), "created file must be deleted");
        assert!(
            !dir.join(".codex").exists(),
            "the directory the install created must be pruned"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A created directory that gained OTHER content meanwhile is left alone —
    /// restore only ever prunes empty dirs, never sweeps up user files. And
    /// restoring an already-absent created file is a no-op, not a failure.
    #[test]
    fn snapshot_restore_never_removes_nonempty_dirs_and_tolerates_absence() {
        let dir = temp_project("snapshot-keep");
        let created = dir.join(".codex").join("hooks.json");
        let snapshot = HookSnapshot::capture(std::slice::from_ref(&created));
        std::fs::create_dir_all(created.parent().unwrap()).unwrap();
        // The install never happened (or the file vanished) — and the user put
        // something else into the created dir meanwhile.
        std::fs::write(dir.join(".codex").join("user.txt"), "mine").unwrap();

        let failures = snapshot.restore();
        assert!(failures.is_empty(), "{failures:?}");
        assert!(
            dir.join(".codex").join("user.txt").exists(),
            "restore must not touch user content"
        );
        // Snapshots merge without double-counting created dirs.
        let mut a = HookSnapshot::capture(std::slice::from_ref(&created));
        let b = HookSnapshot::capture(&[created]);
        a.extend(b);
        let _ = a.restore();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Re-installing the same hook is a no-op everywhere — across plain files,
    /// the seeded Cursor file, the Goose plugin pair, and the JS shim.
    #[test]
    fn reinstall_is_idempotent_across_strategies() {
        for id in [
            "claude-code",
            "codex",
            "cursor",
            "copilot",
            "goose",
            "opencode",
        ] {
            let dir = temp_project(&format!("idem-{id}"));
            let hook = HookSpec {
                command: format!("guard hook {id}"),
                matcher: Some("Bash".into()),
                timeout: Some(10),
                plugin_name: None,
                description: None,
            };
            install_one(&dir, id, &hook);
            let second = install(
                Scope::Project(dir.path()),
                harness::by_id(id).unwrap(),
                &hook,
                false,
            )
            .unwrap();
            assert!(
                second.iter().all(|w| w.status == FileStatus::Unchanged),
                "second install of `{id}` must be all-unchanged, got {second:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// `check` plans the writes without creating any file.
    #[test]
    fn check_mode_writes_nothing() {
        let dir = temp_project("check");
        let writes = install(
            Scope::Project(&dir),
            harness::by_id("codex").unwrap(),
            &HookSpec::command("x"),
            true,
        )
        .unwrap();
        assert_eq!(writes[0].status, FileStatus::Created);
        assert!(!dir.join(".codex/hooks.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An unparseable target is refused and left exactly as it was.
    #[test]
    fn unparseable_target_is_refused_and_untouched() {
        let dir = temp_project("bad");
        std::fs::create_dir_all(dir.join(".codex")).unwrap();
        let path = dir.join(".codex/hooks.json");
        std::fs::write(&path, "{ not json").unwrap();
        let err = install(
            Scope::Project(&dir),
            harness::by_id("codex").unwrap(),
            &HookSpec::command("x"),
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not valid JSON"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every harness in the registry can take a hook — the cross-harness
    /// promise — and each install produces at least one write.
    #[test]
    fn every_harness_supports_hook_install() {
        for spec in harness::all() {
            let dir = temp_project(&format!("all-{}", spec.id));
            let writes = install(
                Scope::Project(&dir),
                spec,
                &HookSpec::command("guard hook x"),
                false,
            )
            .unwrap_or_else(|e| panic!("{}: {e}", spec.id));
            assert!(!writes.is_empty(), "{}: no writes", spec.id);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A global install anchors under the injected HOME / XDG dirs at each
    /// harness's user-global location — which for several harnesses is a
    /// *different* relative path than the project one (Copilot's `.github/hooks`
    /// becomes `.copilot/hooks`; Crush and OpenCode move under the config dir).
    #[test]
    fn global_scope_anchors_at_each_harness_user_location() {
        let root = temp_project("global");
        let home = root.join("home");
        let xdg = root.join("xdg");
        let dirs = GlobalDirs {
            home: Some(home.clone()),
            config_home: Some(xdg.clone()),
        };
        let hook = HookSpec {
            plugin_name: Some("guard".into()),
            ..HookSpec::command("guard hook x")
        };
        let cases: &[(&str, PathBuf)] = &[
            ("claude-code", home.join(".claude/settings.json")),
            ("codex", home.join(".codex/hooks.json")),
            ("qwen", home.join(".qwen/settings.json")),
            ("cursor", home.join(".cursor/hooks.json")),
            ("copilot", home.join(".copilot/hooks/guard.json")),
            ("crush", xdg.join("crush/crush.json")),
            ("opencode", xdg.join("opencode/plugin/guard.js")),
        ];
        for (id, expected) in cases {
            let writes = install(
                Scope::Global(&dirs),
                harness::by_id(id).unwrap(),
                &hook,
                false,
            )
            .unwrap();
            assert_eq!(writes[0].path, *expected, "{id} global anchor");
            assert!(expected.is_file(), "{id}: file not written at {expected:?}");
        }
        // Goose installs its plugin pair under the global plugins dir.
        let goose = install(
            Scope::Global(&dirs),
            harness::by_id("goose").unwrap(),
            &hook,
            false,
        )
        .unwrap();
        let plugin = home.join(".agents/plugins/guard");
        assert_eq!(goose[0].path, plugin.join("plugin.json"));
        assert_eq!(goose[1].path, plugin.join("hooks/hooks.json"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The OpenCode global shim is byte-identical to the project one — same
    /// protocol, just a different location — so the gate command works either way.
    #[test]
    fn global_and_project_opencode_shims_match() {
        let root = temp_project("opencode-scope");
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let dirs = GlobalDirs {
            home: Some(root.join("home")),
            config_home: Some(root.join("xdg")),
        };
        let hook = HookSpec::command("allowlister hook opencode");
        let spec = harness::by_id("opencode").unwrap();
        let proj = install(Scope::Project(&project), spec, &hook, false).unwrap();
        let glob = install(Scope::Global(&dirs), spec, &hook, false).unwrap();
        let proj_js = std::fs::read_to_string(&proj[0].path).unwrap();
        let glob_js = std::fs::read_to_string(&glob[0].path).unwrap();
        assert_eq!(
            proj_js, glob_js,
            "global and project shims must be identical"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A `ConfigHome` harness falls back to `$HOME/.config` when XDG is unset.
    #[test]
    fn config_home_falls_back_to_dot_config() {
        let root = temp_project("xdg-fallback");
        let home = root.join("home");
        let dirs = GlobalDirs {
            home: Some(home.clone()),
            config_home: None,
        };
        let writes = install(
            Scope::Global(&dirs),
            harness::by_id("crush").unwrap(),
            &HookSpec::command("x"),
            false,
        )
        .unwrap();
        assert_eq!(writes[0].path, home.join(".config/crush/crush.json"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A global install with no resolvable base directory is a loud error, never
    /// a write to a guessed path.
    #[test]
    fn global_without_base_dir_is_a_loud_error() {
        let dirs = GlobalDirs::default(); // neither HOME nor XDG set
        let err = install(
            Scope::Global(&dirs),
            harness::by_id("claude-code").unwrap(),
            &HookSpec::command("x"),
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("HOME is not set"), "{err}");
    }
}
