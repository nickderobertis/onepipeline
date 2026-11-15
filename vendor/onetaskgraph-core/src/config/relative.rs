//! Where a relative path in a configuration document is measured from.
//!
//! **The rule, stated once for a reader of either side:** a relative filesystem path a
//! *configuration document* supplies is resolved against the directory holding that
//! document. A relative path supplied through the environment layer or a command-line flag
//! is resolved against the **process working directory**, because there is no document to
//! rebase it on. `README.md`, under "Relative paths in a configuration document", is the
//! same rule for a user, and `docs/local-md.md` points a plugin's reader at it.
//!
//! Why it is here rather than in the plugin that reads the path: by the time a plugin is
//! built it holds a block of values and nothing about where they came from, and the
//! document a project's configuration was discovered in is *not* the working directory —
//! [`documents`](super::documents) walks upward from that directory to find it. The origin
//! is the one thing this layer already keeps per setting, so this is the only layer that
//! can answer the question at all.
//!
//! Which fields are paths is each plugin's to say and no part of this module's: every
//! plugin is asked, through [`SourcePlugin::document_relative_paths`](onetaskgraph_plugin_api::SourcePlugin::document_relative_paths), and one that names
//! none is simply never rebased.
//!
//! `subprocess` names none, and the rule does not stop there. Its `settings:` block
//! belongs to a plugin this build may never have compiled, so nothing inside it is
//! rebased here; instead [`supplying_document_dir`] finds the document that supplied the
//! block, and the handshake hands its directory to the child as `document_dir`
//! (`docs/plugin-protocol.md` §3), where the hosted plugin resolves the fields *it*
//! declares against it — through [`rebased`], the same arithmetic as here.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::PluginKind;
use crate::subprocess::DocumentDir;

use super::{ConfigError, Merged, Origin, SettingPath};

/// Rebase every relative path a configuration document supplied, in place.
///
/// The origins are left exactly as they were, so `onetaskgraph config show` still reports
/// which file supplied the setting — what changes is that the value it reports is the one
/// the run will really use. A setting from any other layer, a value that is not a string,
/// and a path that is already absolute are each left alone.
///
/// # Errors
///
/// A setting is refused, by name, when the directory holding its document is not valid
/// UTF-8, because a merged value is a JSON string and there is no such string to put
/// there. The alternative is the one thing this must not do: replacing the undecodable
/// bytes would hand the plugin a path built out of replacement characters, naming a
/// directory nobody has, and dropping the rebasing instead would silently send the run
/// back to the working directory this exists to stop it using. Neither degradation says
/// anything; [`ConfigError::Setting`] names the setting, the document and what to change.
pub fn resolve_document_relative_paths(settings: &mut Merged) -> Result<(), ConfigError> {
    let mut rewrites: Vec<(SettingPath, Value)> = Vec::new();
    for setting in settings.values() {
        let Origin::File { path: document } = &setting.origin else {
            continue;
        };
        // A source's cache directory is the store's own path rather than its plugin's, so it
        // is rebased whatever plugin the source names.
        if !is_cache_dir(&setting.key) {
            let Some((source, field)) = source_config_field(&setting.key) else {
                continue;
            };
            let Some(declared) = declared_paths_of(settings, source) else {
                continue;
            };
            if !declared.contains(&field.as_str()) {
                continue;
            }
        }
        let Some(raw) = setting.value.as_str() else {
            continue;
        };
        let Some(directory) = document.parent() else {
            continue;
        };
        let Some(rebased) = rebased(directory, raw) else {
            continue;
        };
        let rebased = rebased.into_os_string().into_string().map_err(|_| {
            ConfigError::setting(
                setting.key.to_string(),
                format!(
                    "this path is measured from {}, the directory holding the configuration \
                     document that set it, and that directory's name is not valid UTF-8, so \
                     the path it resolves to cannot be written down",
                    directory.display()
                ),
                "give this setting an absolute path, or move the configuration document \
                 under a directory whose name is valid UTF-8.",
            )
        })?;
        rewrites.push((setting.key.clone(), Value::String(rebased)));
    }
    for (key, value) in rewrites {
        settings.set_value(&key, value);
    }
    Ok(())
}

/// `raw` measured from `directory`, or `None` when it is to be left exactly as it is.
///
/// An absolute path is left alone, and so is an empty one: a value that said nothing goes
/// on saying nothing, because rebasing `""` would turn a setting that fails today into a
/// silent "the document's own directory". One definition for both sides of the
/// `subprocess` seam, so a root cannot resolve one way in process and another behind it.
pub(crate) fn rebased(directory: &Path, raw: &str) -> Option<PathBuf> {
    if raw.is_empty() || Path::new(raw).is_absolute() {
        return None;
    }
    Some(directory.join(raw))
}

/// The absolute directory of the one document that supplied every setting of the
/// `subprocess` source `name`'s `settings:` block, or `None` when no single document did.
///
/// `None` when any of those settings came from the environment layer or a flag — a
/// relative path there keeps resolving against the process working directory, as it does
/// in process — and when two documents each supplied part of the block, because one
/// directory cannot answer for both and choosing one would measure the other's paths from
/// a place nobody wrote. Also `None` for a block nobody set, which holds no path.
///
/// # Errors
///
/// [`ConfigError::Setting`] naming the block when its document's directory is not valid
/// UTF-8, for the reason [`resolve_document_relative_paths`] refuses one: the handshake is
/// JSON, and dropping the directory instead would silently measure the child's paths from
/// its working directory.
pub(super) fn supplying_document_dir(
    settings: &Merged,
    name: &str,
) -> Result<Option<DocumentDir>, ConfigError> {
    let block = ["sources", name, "config", crate::subprocess::SETTINGS_FIELD];
    let mut supplying: Option<&Path> = None;
    for setting in settings.values() {
        if !setting
            .key
            .segments()
            .starts_with(&block.map(str::to_owned))
        {
            continue;
        }
        let Origin::File { path: document } = &setting.origin else {
            return Ok(None);
        };
        match supplying {
            Some(earlier) if earlier != document.as_path() => return Ok(None),
            _ => supplying = Some(document),
        }
    }
    let Some(directory) = supplying.and_then(Path::parent) else {
        return Ok(None);
    };
    // A document named without a directory is in the working directory, and the child is
    // told an absolute path because its working directory is not the engine's to promise.
    let directory = if directory.as_os_str().is_empty() {
        Path::new(".")
    } else {
        directory
    };
    // Only a working directory that cannot be read stops this, and then the directory is
    // passed on as it is so the check below refuses it by name rather than dropping it.
    let directory = std::path::absolute(directory).unwrap_or_else(|_| directory.to_path_buf());
    DocumentDir::new(&directory).map(Some).map_err(|problem| {
        ConfigError::setting(
            block.join("."),
            format!(
                "the paths in this block are measured from the directory holding the \
                 configuration document that set it, and {problem}"
            ),
            "give this block's paths absolute values, or move the configuration document \
             under a directory whose name is valid UTF-8.",
        )
    })
}

/// Whether `key` is one source's `cache.dir`.
fn is_cache_dir(key: &SettingPath) -> bool {
    matches!(key.segments(), [sources, _, cache, dir]
        if sources == "sources" && cache == "cache" && dir == "dir")
}

/// `("work", "root")` for `sources.work.config.root`, and nothing for any other key.
///
/// The field is the whole of the dotted path *inside* the block, so a plugin may name a
/// nested one.
fn source_config_field(key: &SettingPath) -> Option<(&str, String)> {
    match key.segments() {
        [sources, name, config, field @ ..]
            if sources == "sources" && config == "config" && !field.is_empty() =>
        {
            Some((name.as_str(), field.join(".")))
        }
        _ => None,
    }
}

/// What the plugin of the source called `name` declares as a document-relative path.
///
/// The plugin is read back out of the merge rather than out of a [`Config`](crate::Config),
/// because the rebasing happens before there is one: a `Config` holds the block a plugin
/// will be built from, and building it from an unresolved root is the defect this exists to
/// remove.
fn declared_paths_of(settings: &Merged, name: &str) -> Option<&'static [&'static str]> {
    let key = SettingPath::new(
        vec!["sources".to_owned(), name.to_owned(), "plugin".to_owned()],
        "sources",
    )
    .ok()?;
    let kind = PluginKind::parse(settings.get(&key)?.value.as_str()?)?;
    Some(kind.plugin().document_relative_paths())
}
