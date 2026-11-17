//! The environment a config *variant* selects for one harness — the identity
//! seam.
//!
//! I/O by nature: it reads `env_file` off disk and resolves `env_from` through
//! the host's own environment. It lives in the engine rather than the CLI
//! because a run and a usage probe must point a child at the same subscription
//! by the same machinery, and a library caller of [`crate::io::run`] gets that
//! resolution for free.

use crate::domain::config::{names_absolute_path, valid_env_name, FileConfig};
use crate::errors::OneharnessError;

/// The environment a variant selects for one harness: `[env]`, then
/// `[harness.<id>.env]`, then the variant's `env_file`, `env`, and `env_from`,
/// last write winning — the layering the runner applies to a child.
///
/// Shared by `run` and `usage` on purpose: an identity is an environment, so a
/// usage probe must be pointed at a subscription by the same machinery that
/// points a run at one. A second selector here would be a second thing to keep
/// in step, and a usage report attributed to an identity `run` would not have
/// used is worse than no report.
// llmlint: ignore[invalid_states_unrepresentable] This spawn-boundary helper only receives selectors after config validation and select_specs resolution; introducing a second identity type here would duplicate VariantName while its real subprocess tests pin masking, sourcing, and isolation.
pub fn variant_environment(
    cfg: &FileConfig,
    composed: &str,
    project_start: &std::path::Path,
) -> Result<Vec<(String, String)>, OneharnessError> {
    let (base, _) = cfg.split_harness_id(composed);
    let mut env: Vec<(String, String)> = cfg
        .env
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if let Some(harness) = cfg.harness.get(base) {
        env.extend(
            harness
                .env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    let Some(variant) = cfg.variant_for(composed) else {
        return Ok(env);
    };
    if let Some(file) = &variant.env_file {
        let path = {
            let path = std::path::PathBuf::from(file);
            if path.is_absolute() {
                path
            } else {
                project_start.join(path)
            }
        };
        let metadata =
            std::fs::metadata(&path).map_err(|source| OneharnessError::VariantEnvFile {
                path: path.display().to_string(),
                source,
            })?;
        #[cfg(unix)]
        let private = {
            use std::os::unix::fs::PermissionsExt;
            metadata.is_file() && metadata.permissions().mode() & 0o077 == 0
        };
        #[cfg(not(unix))]
        let private = metadata.is_file();
        if !private {
            return Err(OneharnessError::VariantEnvFilePermissions {
                path: path.display().to_string(),
            });
        }
        let text =
            std::fs::read_to_string(&path).map_err(|source| OneharnessError::VariantEnvFile {
                path: path.display().to_string(),
                source,
            })?;
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(OneharnessError::VariantEnvFileLine {
                    path: path.display().to_string(),
                    line: index + 1,
                });
            };
            if !valid_env_name(key) {
                return Err(OneharnessError::VariantEnvFileLine {
                    path: path.display().to_string(),
                    line: index + 1,
                });
            }
            if value.contains('\0') {
                return Err(OneharnessError::VariantEnvFileLine {
                    path: path.display().to_string(),
                    line: index + 1,
                });
            }
            env.push((key.to_string(), value.to_string()));
        }
    }
    env.extend(
        variant
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    for (target, source) in &variant.env_from {
        let value =
            std::env::var(source).map_err(|_| OneharnessError::VariantEnvSourceMissing {
                name: source.clone(),
            })?;
        env.push((target.clone(), value));
    }
    Ok(env)
}

/// An identity a variant selects through `env_from` whose home directory is not
/// on disk — so nothing has been provisioned there yet.
pub struct UnprovisionedIdentity {
    /// The variable the child would have received (e.g. `CODEX_HOME`).
    // llmlint: ignore[invalid_states_unrepresentable] Environment names are TOML map keys/values that stay strings for backward-compatible config merging (see `VariantConfig::env_from`); `validate` checks every target before FileConfig is exposed, and this payload only quotes the already-validated name in a terminal diagnostic.
    pub target: String,
    /// The parent variable it was sourced from (e.g. `ORCHESTRATOR_CODEX_ALT_HOME`).
    // llmlint: ignore[invalid_states_unrepresentable] Same validated-at-parse-time env name as `target`, from the same TOML map; a competing env-name type here would duplicate that boundary without removing the string the config contract stores.
    pub source: String,
    /// The absolute path that does not exist.
    pub path: std::path::PathBuf,
}

/// The first `env_from` indirection of `composed`'s variant that names an
/// absolute path which is **not on disk**, if any.
///
/// A variant's `env_from` is the identity-selection seam: it points a child at
/// one account's home directory without the parent switching to it. A home
/// directory that does not exist holds no credentials, which is the same
/// "account not set up yet" state an *empty* one is in — and an empty one is
/// already an `auth` failure that a fallback chain routes around, because the
/// harness starts, gets rejected, and says so. An absent one is not: the CLI
/// refuses before it can report anything readable (Codex exits 2 with `CODEX_HOME
/// points to … but that path does not exist`), so the chain stops at a candidate
/// nobody has logged into. Reading it here makes the two states behave alike, and
/// makes an unauthenticated candidate free to leave in a committed chain without
/// pre-creating a directory for it.
///
/// Scoped to `env_from` on purpose. It is the only environment field whose value
/// comes from *outside* the config — a committed `env` path is the author's own
/// declaration, and treating its absence as a provisioning state would silently
/// route around a typo. Only [`names_absolute_path`] values are probed, so a
/// credential is never touched.
#[must_use]
pub fn variant_unprovisioned_identity(
    cfg: &FileConfig,
    composed: &str,
) -> Option<UnprovisionedIdentity> {
    let variant = cfg.variant_for(composed)?;
    variant.env_from.iter().find_map(|(target, source)| {
        let value = std::env::var(source).ok()?;
        names_absolute_path(&value).then_some(())?;
        let path = std::path::PathBuf::from(value);
        (!path.exists()).then(|| UnprovisionedIdentity {
            target: target.clone(),
            source: source.clone(),
            path,
        })
    })
}
