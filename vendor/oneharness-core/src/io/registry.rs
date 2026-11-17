//! The registry description behind `oneharness list`, as a library call.
//!
//! [`list`] **returns** the [`ListReport`] instead of printing it, exactly as
//! [`crate::io::run::run`] returns its report — so a Rust consumer asking which
//! harnesses exist, what each supports, and what argv oneharness would build
//! never has to spawn the CLI and parse its stdout. The binary's `list` verb is
//! a shell over this.
//!
//! Most of the report is pure registry data; the `variants` array is the one
//! part that reads configuration, which is why this lives in `io` rather than
//! `domain`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;

use crate::domain::config::FileConfig;
use crate::domain::control::ControlShape;
use crate::domain::harness::{self, BuildCtx, SyncSpec};
use crate::domain::mode::{ModeHeadless, PermissionMode};
use crate::domain::report::{OutputFormat, SCHEMA_VERSION};
use crate::errors::OneharnessError;
use crate::io::config as config_io;

/// What a [`list`] call reads before describing the registry.
///
/// Every field mirrors one thing the CLI resolves from its own flags and
/// process state, so an embedder can state them rather than inherit them: the
/// CLI passes its `--config`/`--no-config` and the current directory, and a
/// library caller supplies whatever project the description should reflect.
#[derive(Debug, Clone, Default)]
pub struct ListRequest {
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery — the repeatable `--config` flag. Each later file
    /// (its own `extends` chain beneath it) overrides the earlier ones; empty
    /// means discovery. See [`crate::io::config::load_layers`].
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Vec<PathBuf>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. Takes
    /// precedence over `config`: set together, no file is loaded.
    pub no_config: bool,
    /// Where project-config discovery starts. `None` means the process's
    /// current directory, which is what the CLI uses.
    pub cwd: Option<PathBuf>,
}

/// One supported approval mode for a harness, with its headless behavior. A
/// [`PermissionMode`] absent from a harness's array is unsupported for it (a
/// `--mode` request would be refused).
#[derive(Debug, Clone, JsonSchema, Serialize)]
pub struct ModeInfo {
    pub mode: PermissionMode,
    /// `"clean"` (never blocks headless) or `"hangs"` (would block on an
    /// approval prompt; refused without `--permit-prompts`).
    pub headless: ModeHeadless,
}

/// Everything the registry declares about one harness.
#[derive(Debug, Clone, JsonSchema, Serialize)]
pub struct HarnessInfo {
    pub id: &'static str,
    pub display: &'static str,
    pub default_bin: &'static str,
    pub install_hint: &'static str,
    pub output_format: OutputFormat,
    /// Whether `run --resume <session>` is supported for this harness.
    pub supports_resume: bool,
    /// Whether `run --session <name>` is supported — a uniform, caller-owned
    /// handle oneharness maps to the harness's native session id. `true` only for
    /// harnesses that expose a session id headlessly; `false` means `--session` is
    /// a loud usage error (there is no id to bind a name to).
    pub session_capable: bool,
    /// Whether `run --resume <session> --fork` is supported — branching a new
    /// session from the resumed one. `false` means it resumes linearly only.
    pub supports_fork: bool,
    /// How `run --control` + `oneharness interrupt` abort an in-flight turn on
    /// this harness (the mechanism id), or `null` when it has no out-of-band
    /// turn control at all — in which case `--control` is a loud usage error for
    /// it, never a socket that reports success while the turn keeps running.
    pub control: Option<ControlShape>,
    /// Whether a forked run reuses the parent session's prompt-cache prefix, so a
    /// fork-based `--batch-strategy min-tokens` run actually reduces tokens. `true`
    /// only for Claude Code today; `false` (incl. OpenCode, whose fork re-sends the
    /// prefix cold) means `min-tokens` only orders the calls (no saving).
    pub fork_reuses_cache: bool,
    /// The approval modes (`--mode`) this harness can express, each with its
    /// headless behavior. Modes not listed are unsupported for the harness.
    pub modes: Vec<ModeInfo>,
    /// Whether `run --schema` is delivered through a native structured-output
    /// flag for this harness (Claude Code's `--json-schema`). `false` means the
    /// portable prompt-based path is used — structured output works either way;
    /// oneharness always validates and retries.
    pub supports_native_schema: bool,
    /// Whether `run --reasoning <effort>` can be delivered on the argv for this
    /// harness (Claude Code's `--effort`, Codex's `model_reasoning_effort`).
    /// `false` means it has no headless reasoning flag — a reasoning request is
    /// then a loud usage error (effort is provider/model config there).
    pub supports_reasoning: bool,
    /// The project-scoped config file `oneharness sync` writes for this
    /// harness; `null` when it has none (sync settings are then rejected).
    pub sync_file: Option<&'static str>,
    /// Whether the unified allow/deny rule lists and hooks table can be synced
    /// into that file (see the README support matrix).
    pub supports_allowed_tools: bool,
    pub supports_denied_tools: bool,
    pub supports_hooks: bool,
    /// Whether `oneharness mock <id>` can express a pre-tool *deny* for this
    /// harness (the same protocol `oneharness gate` speaks).
    pub supports_mock_deny: bool,
    /// The input-rewrite verdict shape `oneharness mock <id>` speaks for this
    /// harness (`claude-nested`, `crush-flat`, `opencode-shim`); `null` when
    /// the harness has no verified rewrite — a rewrite rule for it is then a
    /// loud usage error (see the README mock support matrix).
    pub mock_rewrite: Option<&'static str>,
    /// Whether a large user prompt can be delivered off the argv (piped to the
    /// harness's stdin) so it never trips the OS argument ceiling (`E2BIG`).
    pub supports_prompt_stdin: bool,
    /// Whether a large system prompt can be delivered off the argv via a file
    /// flag (Claude Code's `--append-system-prompt-file`). `false` does not mean a
    /// large system is unhandled — for a harness whose system rides the prompt it
    /// travels on stdin with the prompt; see the README large-prompt matrix.
    pub supports_system_file: bool,
    /// The argv oneharness would build, with placeholders, so the adapter's
    /// shape is visible without running anything.
    pub example_command: Vec<String>,
    pub variants: Vec<VariantInfo>,
}

/// One configured identity of a harness — a `[harness.<id>.variant.<name>]`
/// table, resolved the way a run would resolve it.
#[derive(Debug, Clone, JsonSchema, Serialize)]
pub struct VariantInfo {
    // llmlint: ignore[invalid_states_unrepresentable] The JSON/SDK field is a string sourced exclusively from validated VariantName map keys, not arbitrary caller input.
    pub name: String,
    // llmlint: ignore[invalid_states_unrepresentable] This selector is constructed only from a registry base id plus that validated VariantName, and integration assertions pin the composition.
    pub harness_id: String,
    pub model: Option<String>,
    pub bin: Option<String>,
    pub args: Vec<String>,
    // llmlint: ignore[invalid_states_unrepresentable] This read-only JSON/SDK projection is populated only from a FileConfig that has passed environment-name validation; strings preserve the public list contract without duplicating the input boundary's validated representation.
    pub env_keys: Vec<String>,
    // llmlint: ignore[invalid_states_unrepresentable] This is the portable display value of an already-validated config path, not a path used for I/O; retaining a string preserves the established JSON/SDK contract.
    pub env_file: Option<String>,
    // llmlint: ignore[invalid_states_unrepresentable] Both sides are serialized environment names copied only after FileConfig validation; introducing output-only newtypes would duplicate the validated input boundary and change generated SDK types.
    pub env_from: BTreeMap<String, String>,
    // llmlint: ignore[invalid_states_unrepresentable] These serialized names come only from the validated config and are never accepted back as process inputs through this DTO; strings are the intentional stable output shape.
    pub unset_env: Vec<String>,
}

/// The `oneharness list` output contract.
#[derive(Debug, Clone, JsonSchema, Serialize)]
pub struct ListReport {
    pub schema_version: &'static str,
    pub harnesses: Vec<HarnessInfo>,
}

/// Describe every supported harness, resolving configured variants from the
/// layered configuration the request selects.
///
/// # Errors
///
/// Returns the configuration layer's own error when a selected config file is
/// missing, unparseable, or carries an unknown field — the same loud failure a
/// run from that directory would get.
pub fn list(request: &ListRequest) -> Result<ListReport, OneharnessError> {
    let fallback;
    let start: &Path = match request.cwd.as_deref() {
        Some(dir) => dir,
        None => {
            fallback = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            &fallback
        }
    };
    let loaded = config_io::load(&request.config, request.no_config, start)?;
    let cfg = &loaded.config;
    let harnesses = harness::all()
        .iter()
        .map(|spec| describe(spec, cfg))
        .collect();
    Ok(ListReport {
        schema_version: SCHEMA_VERSION,
        harnesses,
    })
}

fn describe(spec: &'static harness::HarnessSpec, cfg: &FileConfig) -> HarnessInfo {
    let ctx = BuildCtx {
        bin: spec.default_bin,
        prompt: "<PROMPT>",
        model: None,
        system: None,
        resume: None,
        fork: false,
        mode: PermissionMode::Bypass,
        output_format: spec.output_format,
        schema: None,
        system_file: None,
        delivery: harness::PromptDelivery::Argv,
        plugins: cfg.plugins_for(spec.id),
        apps: cfg.apps_for(spec.id),
        analytics: cfg.analytics_for(spec.id),
        model_catalog: None,
    };
    let sync = spec.sync.as_ref();
    HarnessInfo {
        id: spec.id,
        display: spec.display,
        default_bin: spec.default_bin,
        install_hint: spec.install_hint,
        output_format: spec.output_format,
        supports_resume: spec.supports_resume,
        session_capable: spec.session_capable(),
        supports_fork: spec.supports_fork,
        control: spec.control,
        fork_reuses_cache: spec.fork_reuses_cache,
        modes: spec
            .modes
            .iter()
            .map(|m| ModeInfo {
                mode: m.mode,
                headless: m.headless,
            })
            .collect(),
        supports_native_schema: spec.native_schema.is_some(),
        supports_reasoning: spec.reasoning.is_some(),
        sync_file: sync.map(|s| s.file),
        supports_allowed_tools: sync.is_some_and(SyncSpec::carries_allowed_tools),
        supports_denied_tools: sync.is_some_and(SyncSpec::carries_denied_tools),
        supports_hooks: sync.is_some_and(|s| s.hooks_path.is_some()),
        supports_mock_deny: spec.gate_deny.is_some(),
        mock_rewrite: spec.mock_rewrite.map(|s| s.as_str()),
        supports_prompt_stdin: spec.large_input.prompt_stdin,
        supports_system_file: spec.large_input.system_file_flag.is_some(),
        example_command: (spec.build_argv)(&ctx),
        variants: cfg
            .harness
            .get(spec.id)
            .into_iter()
            .flat_map(|harness| &harness.variant)
            .map(|(name, variant)| {
                let id = format!("{}:{name}", spec.id);
                VariantInfo {
                    name: name.to_string(),
                    harness_id: id.clone(),
                    model: cfg.model_for(&id).map(str::to_string),
                    bin: cfg.bin_for(&id).map(str::to_string),
                    args: cfg.args_for(&id).to_vec(),
                    env_keys: variant.env.keys().cloned().collect(),
                    env_file: variant.env_file.clone(),
                    env_from: variant.env_from.clone(),
                    unset_env: variant.unset_env.clone(),
                }
            })
            .collect(),
    }
}
