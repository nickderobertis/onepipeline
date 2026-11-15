//! Scaffolding a starter `oneharness.toml`, as a library call.
//!
//! Unlike the other verbs, `init` *creates* configuration rather than reading
//! it, so it takes no discovery at all. The starter string is pure
//! ([`starter_config`]) so a test can round-trip it through
//! [`crate::domain::config::parse`]; the write/overwrite shell is the only I/O.

use std::path::{Path, PathBuf};

use crate::errors::OneharnessError;

/// The default scaffold location, matching the CLI's own `[PATH]` default.
pub const DEFAULT_PATH: &str = "oneharness.toml";

/// The starter `oneharness.toml` contents. A commented fallback-mode chain
/// (codex → claude-code), the recommended shape: the same committed file works
/// wherever one of the harnesses happens to be authenticated. Every key here is
/// a real `FileConfig`/`HarnessConfig` field (`run_mode`, `harnesses`,
/// `[harness.<id>].model`), so it parses via [`crate::domain::config::parse`].
#[must_use]
pub fn starter_config() -> &'static str {
    "\
# Starter oneharness config. Keep it at your project root as `oneharness.toml`;
# it sets the default harness/model selection for `oneharness run` in this repo.

# extends: inherit a shared parent config and state only what differs here. A
# relative path resolves against this file's directory.
# extends = \"../shared/oneharness.toml\"

# run_mode: how the selected harnesses are run.
#   \"fallback\" (default) tries them in priority order and stops at the first
#   that can actually run, falling through only harnesses that cannot run at all
#   (not installed, unspawnable, or an auth/quota rejection) — so one committed
#   file works wherever a given harness happens to be authenticated.
#   \"parallel\" (opt-in) runs them all at once and reports each.
run_mode = \"fallback\"

# harnesses: the selection, in priority order (the first is preferred under
# fallback). See `oneharness list` for every supported id.
harnesses = [\"codex\", \"claude-code\"]

# [harness.<id>]: per-harness overrides. Model names differ per provider, so
# each harness names its own.
[harness.codex]
model = \"gpt-5.5\"

[harness.claude-code]
model = \"claude-opus-4-8\"

# Variants are opt-in named presets selected as <id>:<name>. Keep credential
# values outside this file; env_from maps a uniquely named parent variable into
# the canonical variable for only this child, while unset_env masks ambient auth.
# [harness.claude-code.variant.work]
# env_from = { ANTHROPIC_API_KEY = \"ANTHROPIC_API_KEY_WORK\" }
# [harness.claude-code.variant.subscription]
# unset_env = [\"ANTHROPIC_API_KEY\", \"CLAUDE_CODE_OAUTH_TOKEN\"]
# env = { CLAUDE_CONFIG_DIR = \"/absolute/path/to/an/isolated/login\" }

# Inspect the effective, fully layered config (and where each value came from)
# with `oneharness config`.
"
}

/// What an [`init`] call writes, as plain data.
#[derive(Debug, Clone, Default)]
pub struct InitRequest {
    /// Where to write the scaffold. `None` takes [`DEFAULT_PATH`], relative to
    /// the process's working directory — the CLI's own default.
    pub path: Option<PathBuf>,
    /// Overwrite an existing file. Without it, an existing file is a loud error
    /// and nothing is written.
    pub force: bool,
}

/// What an [`init`] call wrote.
///
/// This verb's stdout is a human confirmation line rather than the JSON
/// contract the query verbs emit — a scaffold is run interactively, and the
/// deliverable is the file. The path is the whole of it, returned as data so a
/// programmatic caller does not have to re-derive or re-parse it.
#[derive(Debug, Clone, PartialEq, Eq, schemars::JsonSchema, serde::Serialize)]
pub struct InitReport {
    // llmlint: ignore[invalid_states_unrepresentable] The written location is reported as its portable display string, which is what a JSON/SDK consumer reads back; the typed path it came from is the request's, not this output projection's.
    pub path: String,
}

/// Write the starter config and report where it landed.
///
/// # Errors
///
/// [`OneharnessError::InitFileExists`] when the target exists and `force` is
/// unset (nothing is written), or [`OneharnessError::InitWrite`] when the write
/// itself fails.
pub fn init(request: &InitRequest) -> Result<InitReport, OneharnessError> {
    let path: &Path = request.path.as_deref().unwrap_or(Path::new(DEFAULT_PATH));
    // Safe by default: never clobber an existing file without `force`. A read
    // error from `try_exists` (e.g. a permission fault) is treated as "present"
    // so we refuse rather than risk an overwrite.
    if !request.force && path.try_exists().unwrap_or(true) {
        return Err(OneharnessError::InitFileExists {
            path: path.display().to_string(),
        });
    }

    std::fs::write(path, starter_config()).map_err(|source| OneharnessError::InitWrite {
        path: path.display().to_string(),
        source,
    })?;

    Ok(InitReport {
        path: path.display().to_string(),
    })
}
