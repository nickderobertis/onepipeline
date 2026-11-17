//! Locating and reading config files. This is an I/O boundary: it reads the
//! environment, the platform config directory, and the filesystem. Parsing,
//! validation, and layering stay pure in `src/domain/config.rs`.

use std::path::{Path, PathBuf};

use crate::domain::config::{self, ExtendsPath, FileConfig};
use crate::errors::OneharnessError;

/// Project-level file names, checked in this order in each directory.
const PROJECT_FILE_NAMES: &[&str] = &["oneharness.toml", ".oneharness.toml"];

/// Set to `1`/`true` to ignore all config files (same as `--no-config`); lets
/// wrappers and hermetic test suites pin the binary's behavior regardless of
/// what is configured on the machine.
const NO_CONFIG_ENV: &str = "ONEHARNESS_NO_CONFIG";

/// Points the user-level config at an explicit file (which must then exist),
/// instead of the platform default `<config dir>/oneharness/config.toml`.
const USER_CONFIG_ENV: &str = "ONEHARNESS_CONFIG";

/// The most files one `extends` chain may span, the declaring file included.
/// Far past any real hierarchy; it exists so a pathological chain is refused
/// with the chain named rather than by exhausting memory or file handles.
const MAX_EXTENDS_CHAIN: usize = 32;

/// The fully layered configuration plus the files it actually came from.
#[derive(Debug, Default)]
pub struct LoadedConfig {
    /// Every layer merged, each later one winning per field.
    pub config: FileConfig,
    /// Paths loaded, in layering order (the explicit files in the order given,
    /// or user first and project last under discovery; each file's `extends`
    /// parents immediately before it, deepest ancestor first; `environment`
    /// last when any `ONEHARNESS_*` override is set).
    /// Surfaced in the run report so a consumer can see which files shaped a
    /// run.
    pub files: Vec<String>,
}

/// Load the effective config for an invocation: [`load_layers`], folded with
/// the domain's per-field merge.
pub fn load(
    explicit: &[PathBuf],
    no_config: bool,
    project_start: &Path,
) -> Result<LoadedConfig, OneharnessError> {
    let mut loaded = LoadedConfig::default();
    for (path, layer) in load_layers(explicit, no_config, project_start)? {
        loaded.config = config::merge(loaded.config, layer);
        loaded.files.push(path);
    }
    Ok(loaded)
}

/// Locate and parse the config layers for an invocation, in layering order
/// (the explicit files in the order given, else user first and project last). `oneharness config` consumes the layers
/// directly to attribute each value to its file; `run`/`detect` use [`load`].
///
/// Every file loaded here — explicit, user or project — has its `extends`
/// chain followed: each parent becomes its own layer immediately below the
/// file declaring it, named by its own path, so the chain folds through the
/// same [`config::merge`] as every other pair of layers and `oneharness config`
/// attributes an inherited value to the file it was written in. A relative
/// `extends` resolves against the declaring file's directory; an absolute one
/// is used as written. A parent that cannot be read, a cycle, or a chain past
/// its depth bound is an error: a file that names its parent has asserted it
/// exists.
///
/// - `no_config` (or `ONEHARNESS_NO_CONFIG=1`) loads nothing — neither files
///   nor the `ONEHARNESS_*` environment overrides — so a hermetic run sees only
///   CLI flags and built-in defaults.
/// - A non-empty `explicit` (each `--config <path>`, repeatable) loads exactly
///   those files — no discovery — in the order given, each file (its own
///   `extends` chain immediately beneath it) layered over the ones before it,
///   so a later file overrides an earlier one per field. A missing file is an
///   error, since the caller named it. A wrapper puts its defaults first and
///   its user's config after.
/// - Otherwise (empty `explicit`): the user-level file (`$ONEHARNESS_CONFIG`, else the platform
///   config dir) layered under the project-level file (`oneharness.toml` /
///   `.oneharness.toml`, walking up from `project_start`). A missing
///   discovered file is simply an absent layer, never an error.
///
/// The `ONEHARNESS_*` environment overrides ([`config::from_env`]) are appended
/// as a final layer in every non-`no_config` case, so they beat every config
/// file (an explicit `--config` included). CLI flags, applied by each command
/// after this, still beat them — giving CLI > env > files > defaults.
pub fn load_layers(
    explicit: &[PathBuf],
    no_config: bool,
    project_start: &Path,
) -> Result<Vec<(String, FileConfig)>, OneharnessError> {
    if no_config || env_flag(NO_CONFIG_ENV) {
        return Ok(Vec::new());
    }

    let mut layers = Vec::new();
    if !explicit.is_empty() {
        for path in explicit {
            layers.extend(with_parents(path.clone(), read_required(path)?)?);
        }
    } else {
        if let Some(path) = user_config_path()? {
            if let Some(user) = read_optional(&path)? {
                layers.extend(with_parents(path, user)?);
            }
        }
        if let Some(path) = find_project_file(project_start) {
            if let Some(project) = read_optional(&path)? {
                layers.extend(with_parents(path, project)?);
            }
        }
    }
    if let Some(env) = config::from_env(|name| std::env::var(name).ok())
        .map_err(OneharnessError::EnvConfigInvalid)?
    {
        layers.push((config::ENV_SOURCE.to_string(), env));
    }
    Ok(layers)
}

/// One loaded file and every `extends` ancestor it names, deepest ancestor
/// first, each paired with the path it was read from.
fn with_parents(
    path: PathBuf,
    config: FileConfig,
) -> Result<Vec<(String, FileConfig)>, OneharnessError> {
    // Identity for cycle detection is the canonical path, so `./a.toml` and a
    // symlink to it are one file; the display name stays the path as resolved.
    let mut seen = vec![canonical_or_original(&path)];
    let mut chain = vec![(path, config)];
    loop {
        let (declaring, current) = chain.last().expect("chain starts non-empty");
        let Some(extends) = current.extends.as_ref().map(ExtendsPath::as_str) else {
            break;
        };
        let parent = declaring
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(extends);
        let names = |chain: &[(PathBuf, FileConfig)]| {
            chain
                .iter()
                .map(|(p, _)| p.display().to_string())
                .chain(std::iter::once(parent.display().to_string()))
                .collect::<Vec<_>>()
                .join(" -> ")
        };
        if seen.contains(&canonical_or_original(&parent)) {
            return Err(OneharnessError::ConfigInvalid {
                path: declaring.display().to_string(),
                message: format!(
                    "`extends = \"{extends}\"` closes a cycle: {}",
                    names(&chain)
                ),
            });
        }
        if chain.len() >= MAX_EXTENDS_CHAIN {
            return Err(OneharnessError::ConfigInvalid {
                path: chain[0].0.display().to_string(),
                message: format!(
                    "`extends` chain is longer than {MAX_EXTENDS_CHAIN} files: {}",
                    names(&chain)
                ),
            });
        }
        let text =
            std::fs::read_to_string(&parent).map_err(|e| OneharnessError::ConfigInvalid {
                path: declaring.display().to_string(),
                message: format!(
                    "`extends = \"{extends}\"` names `{}`, which could not be read: {e}",
                    parent.display()
                ),
            })?;
        let config = parse_at(&parent, &text)?;
        seen.push(canonical_or_original(&parent));
        chain.push((parent, config));
    }
    Ok(chain
        .into_iter()
        .rev()
        .map(|(p, c)| (p.display().to_string(), c))
        .collect())
}

/// A path's canonical form, or the path itself where it cannot be resolved
/// (a missing parent is then reported by the read that follows).
fn canonical_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Truthy env flag: set and not `""`/`0`/`false`.
fn env_flag(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => !matches!(v.as_str(), "" | "0" | "false"),
        Err(_) => false,
    }
}

/// The user-level config path: `$ONEHARNESS_CONFIG` if set (the file must
/// exist — an explicitly named config that is missing is a configuration
/// error, not a silent no-op), else the platform config directory.
fn user_config_path() -> Result<Option<PathBuf>, OneharnessError> {
    if let Ok(value) = std::env::var(USER_CONFIG_ENV) {
        if !value.is_empty() {
            let path = PathBuf::from(&value);
            if !path.is_file() {
                return Err(OneharnessError::ConfigInvalid {
                    path: value,
                    message: format!("{USER_CONFIG_ENV} points at a file that does not exist"),
                });
            }
            return Ok(Some(path));
        }
    }
    Ok(platform_config_dir().map(|d| d.join("oneharness").join("config.toml")))
}

/// The per-user configuration directory, resolved like `gh` and friends:
/// `%APPDATA%` on Windows; `$XDG_CONFIG_HOME` (else `~/.config`) everywhere
/// else, macOS included — a dotfile-style path suits a developer CLI better
/// than `Library/Application Support`.
fn platform_config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("APPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg));
    }
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|home| PathBuf::from(home).join(".config"))
}

/// Walk up from `start` looking for a project config; the first (deepest)
/// match wins, so a nested project shadows its parent.
fn find_project_file(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        for name in PROJECT_FILE_NAMES {
            let candidate = d.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        dir = d.parent();
    }
    None
}

/// Read and parse a discovered file; `Ok(None)` when it does not exist.
fn read_optional(path: &Path) -> Result<Option<FileConfig>, OneharnessError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_at(path, &text).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(OneharnessError::ConfigRead {
            path: path.display().to_string(),
            source,
        }),
    }
}

/// Read and parse an explicitly named file; missing is an error.
fn read_required(path: &Path) -> Result<FileConfig, OneharnessError> {
    let text = std::fs::read_to_string(path).map_err(|source| OneharnessError::ConfigRead {
        path: path.display().to_string(),
        source,
    })?;
    parse_at(path, &text)
}

fn parse_at(path: &Path, text: &str) -> Result<FileConfig, OneharnessError> {
    config::parse(text).map_err(|message| OneharnessError::ConfigInvalid {
        path: path.display().to_string(),
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::scratch::ScratchDir;

    fn temp_dir(tag: &str) -> ScratchDir {
        ScratchDir::new(&format!("cfg-{tag}")).unwrap()
    }

    #[test]
    fn project_file_is_found_walking_up() {
        let root = temp_dir("walk");
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("oneharness.toml"), "model = \"outer\"").unwrap();
        let found = find_project_file(&nested).unwrap();
        assert_eq!(found, root.join("oneharness.toml"));

        // A deeper file shadows the outer one, and the dotted name is honored.
        std::fs::write(nested.join(".oneharness.toml"), "model = \"inner\"").unwrap();
        let found = find_project_file(&nested).unwrap();
        assert_eq!(found, nested.join(".oneharness.toml"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_discovered_file_is_an_absent_layer() {
        let dir = temp_dir("missing");
        assert!(read_optional(&dir.join("oneharness.toml"))
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_file_carries_its_path_in_the_error() {
        let dir = temp_dir("invalid");
        let path = dir.join("oneharness.toml");
        std::fs::write(&path, "not = valid = toml").unwrap();
        let err = read_optional(&path).unwrap_err();
        assert!(err.to_string().contains("oneharness.toml"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Write each `(relative path, text)` under `dir`, creating its parents.
    fn plant(dir: &Path, files: &[(&str, &str)]) {
        for (name, text) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    fn invalid(err: OneharnessError) -> (String, String) {
        match err {
            OneharnessError::ConfigInvalid { path, message } => (path, message),
            other => panic!("expected ConfigInvalid, got {other:?}"),
        }
    }

    #[test]
    fn a_three_file_chain_folds_deepest_ancestor_first_from_each_files_own_dir() {
        let dir = temp_dir("chain");
        plant(
            &dir,
            &[
                (
                    "shared/root.toml",
                    "timeout = 10\nmodel = \"root\"\nsystem = \"root\"\n[env]\nA = \"root\"",
                ),
                (
                    "shared/mid/mid.toml",
                    "extends = \"../root.toml\"\nmodel = \"mid\"\n[env]\nB = \"mid\"",
                ),
                (
                    "roles/child.toml",
                    "extends = \"../shared/mid/mid.toml\"\nsystem = \"child\"",
                ),
            ],
        );
        let child = dir.join("roles/child.toml");
        let loaded = load(&[child.clone()], false, &dir).unwrap();
        // Each parent resolves against its declaring file's own directory, joined
        // the way the platform joins it (`roles\../shared/mid/mid.toml` on Windows).
        let mid = dir.join("roles").join("../shared/mid/mid.toml");
        let root = mid.parent().unwrap().join("../root.toml");
        assert_eq!(
            loaded.files[..3],
            [
                root.display().to_string(),
                mid.display().to_string(),
                child.display().to_string()
            ]
        );
        assert_eq!(loaded.config.timeout, Some(10));
        assert_eq!(loaded.config.model.as_deref(), Some("mid"));
        assert_eq!(loaded.config.system.as_deref(), Some("child"));
        assert_eq!(loaded.config.env["A"], "root");
        assert_eq!(loaded.config.env["B"], "mid");
        assert_eq!(loaded.config.extends, None);
    }

    #[test]
    fn an_absolute_extends_is_used_as_written() {
        let dir = temp_dir("absolute");
        plant(&dir, &[("elsewhere/base.toml", "model = \"base\"")]);
        let base = dir.join("elsewhere/base.toml");
        let child = dir.join("child.toml");
        std::fs::write(
            &child,
            format!("extends = {:?}", base.display().to_string()),
        )
        .unwrap();
        let loaded = load(&[child.clone()], false, &dir).unwrap();
        assert_eq!(loaded.config.model.as_deref(), Some("base"));
        assert_eq!(loaded.files[0], base.display().to_string());
    }

    #[test]
    fn a_cycle_is_refused_naming_the_file_closing_it_and_the_chain() {
        let dir = temp_dir("cycle");
        plant(
            &dir,
            &[
                ("a.toml", "extends = \"b.toml\""),
                ("b.toml", "extends = \"./a.toml\""),
            ],
        );
        let a = dir.join("a.toml");
        let (path, message) = invalid(load(&[a.clone()], false, &dir).unwrap_err());
        assert_eq!(path, dir.join("b.toml").display().to_string());
        let chain = format!(
            "{} -> {} -> {}",
            a.display(),
            dir.join("b.toml").display(),
            dir.join("./a.toml").display()
        );
        assert!(message.contains("closes a cycle"), "{message}");
        assert!(message.contains(&chain), "{message}");

        // A file naming itself is the shortest cycle.
        plant(&dir, &[("self.toml", "extends = \"self.toml\"")]);
        let (_, message) = invalid(load(&[dir.join("self.toml")], false, &dir).unwrap_err());
        assert!(message.contains("closes a cycle"), "{message}");
    }

    #[test]
    fn a_chain_past_the_depth_bound_is_refused_with_the_chain_named() {
        let dir = temp_dir("deep");
        for i in 0..=MAX_EXTENDS_CHAIN {
            std::fs::write(
                dir.join(format!("{i}.toml")),
                format!("extends = \"{}.toml\"", i + 1),
            )
            .unwrap();
        }
        let first = dir.join("0.toml");
        let (path, message) = invalid(load(&[first.clone()], false, &dir).unwrap_err());
        assert_eq!(path, first.display().to_string());
        assert!(
            message.contains(&format!("longer than {MAX_EXTENDS_CHAIN} files")),
            "{message}"
        );
        assert!(
            message.contains(&format!("{}", first.display())),
            "{message}"
        );
        assert!(
            message.contains(&format!(
                "{}",
                dir.join(format!("{MAX_EXTENDS_CHAIN}.toml")).display()
            )),
            "{message}"
        );

        // Ending the chain at exactly the bound's length is legal.
        std::fs::write(
            dir.join(format!("{}.toml", MAX_EXTENDS_CHAIN - 1)),
            "model = \"last\"",
        )
        .unwrap();
        let loaded = load(&[first.clone()], false, &dir).unwrap();
        assert_eq!(loaded.config.model.as_deref(), Some("last"));
    }

    #[test]
    fn a_missing_parent_is_refused_naming_the_declaring_file_and_resolved_path() {
        let dir = temp_dir("missing-parent");
        plant(&dir, &[("sub/child.toml", "extends = \"../gone.toml\"")]);
        let child = dir.join("sub/child.toml");
        // The parent resolves against the declaring file's own directory, joined
        // the way the platform joins it (`sub\../gone.toml` on Windows).
        let parent = dir.join("sub").join("../gone.toml").display().to_string();
        let (path, message) = invalid(load(&[child.clone()], false, &dir).unwrap_err());
        assert_eq!(path, child.display().to_string());
        assert!(message.contains(&parent), "{message}");
        assert!(message.contains("could not be read"), "{message}");

        // A parent that exists but does not parse names the parent itself.
        plant(&dir, &[("gone.toml", "modle = 1")]);
        let (path, _) = invalid(load(&[child.clone()], false, &dir).unwrap_err());
        assert_eq!(path, parent);
    }

    #[test]
    fn a_discovered_project_file_resolves_its_own_chain() {
        let dir = temp_dir("project-chain");
        plant(
            &dir,
            &[
                (
                    "oneharness.toml",
                    "extends = \"conf/base.toml\"\nmodel = \"proj\"",
                ),
                ("conf/base.toml", "timeout = 7"),
            ],
        );
        let nested = dir.join("x");
        std::fs::create_dir_all(&nested).unwrap();
        let layers = load_layers(&[], false, &nested).unwrap();
        let names: Vec<&str> = layers.iter().map(|(p, _)| p.as_str()).collect();
        let base = dir.join("conf/base.toml").display().to_string();
        let project = dir.join("oneharness.toml").display().to_string();
        let at = names
            .iter()
            .position(|n| *n == base)
            .expect("parent layered");
        assert_eq!(names[at + 1], project);
    }

    #[test]
    fn the_readme_states_the_chain_bound_the_loader_enforces() {
        let readme = include_str!("../../../../README.md");
        assert!(
            readme.contains(&format!("a chain longer than {MAX_EXTENDS_CHAIN} files")),
            "README's `extends` section must state the bound `MAX_EXTENDS_CHAIN` enforces"
        );
    }
}
