//! Resolving and probing harness binaries. This is an I/O boundary: it reads the
//! environment, searches PATH, and may spawn `<bin> --version`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

use crate::domain::harness::HarnessSpec;
use crate::errors::OneharnessError;

/// Per-harness binary overrides, resolved from `--bin ID=PATH`, then the
/// `ONEHARNESS_BIN_<ID>` environment variable, then a config-file
/// `[harness.<id>] bin`, falling back to the spec default.
///
/// Every layer reads a variant-qualified id (`claude-code:hooked`) the same
/// way, and each layer is exhausted before the next is consulted: the id's own
/// spelling first, then its base harness's, so one base override covers every
/// member of that harness while an entry for the exact variant is the more
/// deliberate one and wins. So `--bin claude-code=/path` covers
/// `claude-code:hooked` (as does a `--mock-harness claude-code` swap), and it
/// still beats `ONEHARNESS_BIN_CLAUDE_CODE_HOOKED` and a config-file `bin` —
/// a flag is the most deliberate layer, whichever spelling it used (issue
/// #1329). The environment reads `ONEHARNESS_BIN_CLAUDE_CODE_HOOKED` then
/// `ONEHARNESS_BIN_CLAUDE_CODE` — see the private `bin_env_keys` derivation.
pub struct BinOverrides {
    map: HashMap<String, String>,
    /// Config-file bins: the lowest-precedence override layer, since an
    /// explicit flag or env var is more deliberate than a persisted default.
    config: HashMap<String, String>,
}

impl BinOverrides {
    /// Parse `--bin ID=PATH` values. Errors on a value missing its `=`.
    pub fn parse(values: &[String]) -> Result<Self, OneharnessError> {
        let mut map = HashMap::new();
        for value in values {
            let (id, path) = value
                .split_once('=')
                .ok_or_else(|| OneharnessError::BadBinOverride(value.clone()))?;
            if id.is_empty() || path.is_empty() {
                return Err(OneharnessError::BadBinOverride(value.clone()));
            }
            map.insert(id.to_string(), path.to_string());
        }
        Ok(Self {
            map,
            config: HashMap::new(),
        })
    }

    /// Attach the config-file bins (`[harness.<id>] bin = "..."`) as fallbacks.
    pub fn with_config_bins(mut self, bins: HashMap<String, String>) -> Self {
        self.config = bins;
        self
    }

    /// The binary to invoke for `id`: explicit override, then env var, then the
    /// config-file bin, then default. The explicit map is read for the exact
    /// id and then, for a variant, its base — the whole layer before the
    /// environment, so a base flag covers the variant and still outranks the
    /// variant's own env key.
    fn binary_for(&self, id: &str, default_bin: &str) -> String {
        for key in override_ids(id) {
            if let Some(path) = self.map.get(key) {
                return path.clone();
            }
        }
        for env_key in bin_env_keys(id) {
            if let Ok(value) = std::env::var(&env_key) {
                if !value.is_empty() {
                    return value;
                }
            }
        }
        if let Some(path) = self.config.get(id) {
            return path.clone();
        }
        default_bin.to_string()
    }
}

/// The ids an explicit `--bin` map may name the binary for `id` under, in the
/// order [`BinOverrides`] reads them: the id's own spelling, then — for a
/// variant-qualified id — its base harness's.
fn override_ids(id: &str) -> Vec<&str> {
    match id.split_once(':') {
        Some((base, _)) => vec![id, base],
        None => vec![id],
    }
}

/// The `ONEHARNESS_BIN_*` keys that may name the binary for `id`, in the order
/// [`BinOverrides`] reads them: the id's own spelling, then — for a
/// variant-qualified id — its base harness's. `-` and `:` both become `_`,
/// since neither can appear in a variable name.
fn bin_env_keys(id: &str) -> Vec<String> {
    let key = |name: &str| {
        format!(
            "ONEHARNESS_BIN_{}",
            name.to_uppercase().replace(['-', ':'], "_")
        )
    };
    match id.split_once(':') {
        Some((base, _)) => vec![key(id), key(base)],
        None => vec![key(id)],
    }
}

/// The result of locating a harness binary on the current system.
pub struct Resolved {
    /// The binary string oneharness will invoke (name or path).
    pub bin: String,
    /// The absolute path it resolved to, if found.
    pub path: Option<PathBuf>,
    /// Whether the binary was found and is executable.
    pub available: bool,
}

/// Resolve the binary for a harness without running it.
pub fn resolve(spec: &HarnessSpec, overrides: &BinOverrides) -> Resolved {
    resolve_named(spec, spec.id, overrides)
}

/// Resolve a composed harness id, allowing a variant-specific config/CLI bin
/// while retaining the base adapter's default.
// llmlint: ignore[invalid_states_unrepresentable] Callers pass only config-validated selectors paired with their registry-resolved base spec; keeping the serialized id here lets BinOverrides preserve exact per-variant lookup, covered by detect integration tests for valid and unknown variants.
pub fn resolve_named(spec: &HarnessSpec, id: &str, overrides: &BinOverrides) -> Resolved {
    let bin = overrides.binary_for(id, spec.default_bin);
    match which::which(&bin) {
        Ok(path) => Resolved {
            bin,
            path: Some(path),
            available: true,
        },
        Err(_) => Resolved {
            bin,
            path: None,
            available: false,
        },
    }
}

/// Best-effort `<bin> --version`, returning the first non-empty output line.
/// Never fails loudly: a probe that errors or times out simply yields `None`.
pub fn probe_version(bin: &str) -> Option<String> {
    let mut child = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let status = child.wait_timeout(Duration::from_secs(5)).ok()?;
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let output = child.wait_with_output().ok()?;
    first_line(&output.stdout).or_else(|| first_line(&output.stderr))
}

fn first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// What a [`detect`] sweep probes, as plain data.
///
/// Every field is one thing the CLI resolves from its own flags, so an embedder
/// states them rather than inheriting them from a process it does not own.
#[derive(Debug, Clone, Default)]
pub struct DetectRequest {
    /// Probe every supported harness. Also the default when neither `harness`
    /// nor `exclude` names anything.
    pub all: bool,
    /// Harness id(s) to probe, `<id>` or `<id>:<variant>`.
    pub harness: Vec<String>,
    /// Harness id(s) to drop from a sweep. This NARROWS a selection rather than
    /// requesting one: naming only `exclude` selects nothing and is the same
    /// loud usage error the CLI raises, so pair it with `all` (or with the
    /// configured `harnesses`).
    pub exclude: Vec<String>,
    /// `--bin ID=PATH` overrides, in the CLI's own spelling.
    pub bin: Vec<String>,
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

/// One probed harness identity.
// llmlint: ignore[invalid_states_unrepresentable] `available` beside optional `path`/`version` is the published `detect` wire contract, generated into both language SDKs and depended on by their validators; folding it into a sum type is a breaking output-shape change, not a refactor, and this move from `src/commands/detect.rs` deliberately preserved the shape byte for byte. `path` is legitimately absent for an available harness the resolver took from an explicit `--bin` override, and `version` for one whose `--version` probe fails, so the states are reachable rather than merely representable.
#[derive(Debug, Clone, schemars::JsonSchema, serde::Serialize)]
pub struct DetectInfo {
    // llmlint: ignore[invalid_states_unrepresentable] This JSON boundary mirrors the CLI selector string; selection and variant lookup validate it before construction, with integration coverage for valid and invalid composed ids.
    pub id: String,
    pub bin: String,
    pub available: bool,
    pub path: Option<String>,
    pub version: Option<String>,
}

/// The `oneharness detect` output contract.
#[derive(Debug, Clone, schemars::JsonSchema, serde::Serialize)]
pub struct DetectReport {
    pub schema_version: &'static str,
    pub detected: Vec<DetectInfo>,
}

impl DetectReport {
    /// Whether any probed harness was not installed — what the CLI's
    /// `--require-available` turns into a non-zero exit.
    #[must_use]
    pub fn any_missing(&self) -> bool {
        self.detected.iter().any(|info| !info.available)
    }
}

/// Probe the selected harnesses' binaries and return the report.
///
/// Configured binaries apply, so this reports the same binary a run from the
/// same directory would invoke.
///
/// # Errors
///
/// Returns a usage error for an unknown harness id, an unknown `<id>:<variant>`
/// selector, a malformed `--bin` value, or a configuration layer that cannot be
/// loaded — the same loud failures the CLI verb raises.
pub fn detect(request: &DetectRequest) -> Result<DetectReport, OneharnessError> {
    // Detection defaults to every harness; an explicit selection narrows it.
    let all = request.all || (request.harness.is_empty() && request.exclude.is_empty());
    let specs = crate::domain::select::select_specs(all, &request.harness, &request.exclude)?;
    let selected_ids = crate::domain::select::dedupe_exact_ids(&request.harness);
    let fallback;
    let start: &std::path::Path = match request.cwd.as_deref() {
        Some(dir) => dir,
        None => {
            fallback = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            &fallback
        }
    };
    let loaded = crate::io::config::load(&request.config, request.no_config, start)?;
    for id in request.harness.iter().chain(&request.exclude) {
        if let Some((base, variant)) = id.split_once(':') {
            if loaded.config.variant_for(id).is_none() {
                return Err(OneharnessError::UnknownHarnessVariant {
                    id: id.clone(),
                    base: base.to_string(),
                    variant: variant.to_string(),
                });
            }
        }
    }
    let mut config_bins: HashMap<String, String> = loaded
        .config
        .harness
        .iter()
        .filter_map(|(id, h)| h.bin.clone().map(|bin| (id.clone(), bin)))
        .collect();
    for (base, harness) in &loaded.config.harness {
        for name in harness.variant.keys() {
            let id = format!("{base}:{name}");
            if let Some(bin) = loaded.config.bin_for(&id) {
                config_bins.insert(id, bin.to_string());
            }
        }
    }
    let overrides = BinOverrides::parse(&request.bin)?.with_config_bins(config_bins);

    let detected = specs
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let id = selected_ids
                .get(index)
                .map_or(spec.id.to_string(), Clone::clone);
            let resolved = resolve_named(spec, &id, &overrides);
            let version = if resolved.available {
                probe_version(&resolved.bin)
            } else {
                None
            };
            DetectInfo {
                id,
                bin: resolved.bin,
                available: resolved.available,
                path: resolved.path.map(|p| p.display().to_string()),
                version,
            }
        })
        .collect();

    Ok(DetectReport {
        schema_version: crate::domain::report::SCHEMA_VERSION,
        detected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, PoisonError};

    // Serializes the env-mutating tests: the process environment is global, so
    // concurrent set/remove from two tests in the same binary would race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Takes [`ENV_LOCK`], recovering it from a poisoning: the guarded data is
    /// `()`, so a sibling test that failed while holding it left nothing torn,
    /// and its failure must not cascade into every later env test.
    fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn override_takes_precedence_over_default() {
        let ov = BinOverrides::parse(&["claude-code=/opt/claude".to_string()]).unwrap();
        assert_eq!(ov.binary_for("claude-code", "claude"), "/opt/claude");
    }

    #[test]
    fn default_used_when_no_override() {
        // A miss reads `ONEHARNESS_BIN_CODEX`, which a sibling test sets.
        let _guard = env_lock();
        let ov = BinOverrides::parse(&[]).unwrap();
        assert_eq!(ov.binary_for("codex", "codex"), "codex");
    }

    #[test]
    fn config_bin_is_used_but_loses_to_an_explicit_flag() {
        let _guard = env_lock();
        let bins = HashMap::from([("codex".to_string(), "/cfg/codex".to_string())]);
        let ov = BinOverrides::parse(&[]).unwrap().with_config_bins(bins);
        assert_eq!(ov.binary_for("codex", "codex"), "/cfg/codex");

        let bins = HashMap::from([("codex".to_string(), "/cfg/codex".to_string())]);
        let ov = BinOverrides::parse(&["codex=/flag/codex".to_string()])
            .unwrap()
            .with_config_bins(bins);
        assert_eq!(ov.binary_for("codex", "codex"), "/flag/codex");
    }

    #[test]
    fn a_base_flag_covers_a_variant_and_an_exact_variant_flag_wins() {
        // Issue #1329: the map is read for the exact id, then the base. A
        // miss reads the environment, so the lock keeps a sibling test's key
        // out of the answer.
        let _guard = env_lock();
        let ov = BinOverrides::parse(&["claude-code=/flag/claude".to_string()]).unwrap();
        assert_eq!(ov.binary_for("claude-code:work", "claude"), "/flag/claude");
        assert_eq!(ov.binary_for("claude-code", "claude"), "/flag/claude");
        assert_eq!(ov.binary_for("codex:work", "codex"), "codex");

        let ov = BinOverrides::parse(&[
            "claude-code=/flag/claude".to_string(),
            "claude-code:work=/flag/work-claude".to_string(),
        ])
        .unwrap();
        assert_eq!(
            ov.binary_for("claude-code:work", "claude"),
            "/flag/work-claude"
        );
        assert_eq!(ov.binary_for("claude-code:other", "claude"), "/flag/claude");
        assert_eq!(ov.binary_for("claude-code", "claude"), "/flag/claude");
        assert_eq!(
            override_ids("claude-code:work"),
            ["claude-code:work", "claude-code"]
        );
        assert_eq!(override_ids("codex"), ["codex"]);
    }

    #[test]
    fn a_base_flag_outranks_the_variant_s_own_env_key_and_config_bin() {
        // The map is one layer, exhausted before the environment is read: a
        // flag is the most deliberate override whichever spelling it used.
        let _guard = env_lock();
        let variant_key = "ONEHARNESS_BIN_CLAUDE_CODE_WORK";
        let prev = std::env::var(variant_key).ok();
        std::env::set_var(variant_key, "/env/work-claude");
        let bins = HashMap::from([(
            "claude-code:work".to_string(),
            "/cfg/work-claude".to_string(),
        )]);
        let ov = BinOverrides::parse(&["claude-code=/flag/claude".to_string()])
            .unwrap()
            .with_config_bins(bins);
        assert_eq!(ov.binary_for("claude-code:work", "claude"), "/flag/claude");
        match prev {
            Some(v) => std::env::set_var(variant_key, v),
            None => std::env::remove_var(variant_key),
        }
    }

    #[test]
    fn malformed_override_is_rejected() {
        assert!(BinOverrides::parse(&["no-equals".to_string()]).is_err());
        assert!(BinOverrides::parse(&["=/path".to_string()]).is_err());
        assert!(BinOverrides::parse(&["id=".to_string()]).is_err());
    }

    #[test]
    fn first_line_skips_blanks() {
        assert_eq!(
            first_line(b"\n  \nv1.2.3\nextra"),
            Some("v1.2.3".to_string())
        );
        assert_eq!(first_line(b"   "), None);
    }

    #[test]
    fn env_var_override_is_used_above_config_and_default() {
        // With no `--bin` flag, the per-harness `ONEHARNESS_BIN_<ID>` env var
        // (id upper-cased, `-`→`_`) selects the binary, ahead of any config-file
        // bin and the spec default. A unique key keeps this independent of the
        // ambient environment.
        let _guard = env_lock();
        let key = "ONEHARNESS_BIN_CLAUDE_CODE";
        let prev = std::env::var(key).ok();

        std::env::set_var(key, "/env/claude");
        let bins = HashMap::from([("claude-code".to_string(), "/cfg/claude".to_string())]);
        let ov = BinOverrides::parse(&[]).unwrap().with_config_bins(bins);
        assert_eq!(ov.binary_for("claude-code", "claude"), "/env/claude");

        // An empty env value is ignored — the next layer (config, then default)
        // wins instead.
        std::env::set_var(key, "");
        assert_eq!(ov.binary_for("claude-code", "claude"), "/cfg/claude");

        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn a_variant_selection_reads_its_own_env_key_then_the_base_harness_s() {
        assert_eq!(
            bin_env_keys("claude-code:hooked"),
            [
                "ONEHARNESS_BIN_CLAUDE_CODE_HOOKED",
                "ONEHARNESS_BIN_CLAUDE_CODE"
            ]
        );
        assert_eq!(bin_env_keys("codex"), ["ONEHARNESS_BIN_CODEX"]);
    }

    /// The README's `--bin` bullet is where a caller learns which variable
    /// names the binary for a variant, and the names it spells are derived here
    /// from the same function the runtime reads, in the order the runtime reads
    /// them — so the documented keys cannot drift from the ones honoured.
    #[test]
    fn readme_names_the_env_keys_a_variant_selection_reads() {
        let readme = include_str!("../../../../README.md").replace("\r\n", "\n");
        let bullet_start = readme
            .find("- `--bin <id>=<path>`")
            .expect("README.md documents --bin");
        let bullet = &readme[bullet_start..];
        let bullet = &bullet[..bullet.find("\n- ").unwrap_or(bullet.len())];
        let keys = bin_env_keys("claude-code:work");
        assert_eq!(
            keys.len(),
            2,
            "a variant reads its own key, then the base's"
        );
        let own = bullet
            .find(&format!("`{}`", keys[0]))
            .unwrap_or_else(|| panic!("README.md must name `{}` for `claude-code:work`", keys[0]));
        let base = bullet
            .find(&format!("`{}`", keys[1]))
            .unwrap_or_else(|| panic!("README.md must name `{}` for `claude-code:work`", keys[1]));
        assert!(
            own < base,
            "README.md must name the variant's own key before the base's, as the runtime reads them"
        );
    }

    #[test]
    fn base_env_var_covers_a_variant_qualified_selection() {
        // Issue #1308: the key was derived from the whole composed id and kept
        // its `:`, which no environment variable can carry, so
        // `ONEHARNESS_BIN_CLAUDE_CODE` covered `claude-code` and silently not
        // `claude-code:hooked` — the config-file `bin` already fell back from
        // the variant to its base, and the env layer now does the same.
        let _guard = env_lock();
        let base_key = "ONEHARNESS_BIN_CLAUDE_CODE";
        let variant_key = "ONEHARNESS_BIN_CLAUDE_CODE_HOOKED";
        let prev = (
            std::env::var(base_key).ok(),
            std::env::var(variant_key).ok(),
        );
        std::env::remove_var(variant_key);

        std::env::set_var(base_key, "/env/claude");
        // Above the config layer for the variant too, as for a base id: the
        // config map already carries the variant's own fallback-resolved bin.
        let bins = HashMap::from([("claude-code:hooked".to_string(), "/cfg/claude".to_string())]);
        let ov = BinOverrides::parse(&[]).unwrap().with_config_bins(bins);
        assert_eq!(ov.binary_for("claude-code:hooked", "claude"), "/env/claude");

        // A variant-specific spelling is the more deliberate one, so it wins
        // over the base key when both are set...
        std::env::set_var(variant_key, "/env/hooked-claude");
        assert_eq!(
            ov.binary_for("claude-code:hooked", "claude"),
            "/env/hooked-claude"
        );
        // ...and never reaches a sibling variant or the base itself.
        assert_eq!(ov.binary_for("claude-code:other", "claude"), "/env/claude");
        assert_eq!(ov.binary_for("claude-code", "claude"), "/env/claude");

        std::env::remove_var(variant_key);
        match prev.0 {
            Some(v) => std::env::set_var(base_key, v),
            None => std::env::remove_var(base_key),
        }
        if let Some(v) = prev.1 {
            std::env::set_var(variant_key, v);
        }
    }

    #[test]
    fn explicit_flag_beats_env_var() {
        // A `--bin` flag takes precedence over the env var for the same id.
        let _guard = env_lock();
        let key = "ONEHARNESS_BIN_CODEX";
        let prev = std::env::var(key).ok();
        std::env::set_var(key, "/env/codex");

        let ov = BinOverrides::parse(&["codex=/flag/codex".to_string()]).unwrap();
        assert_eq!(ov.binary_for("codex", "codex"), "/flag/codex");

        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
