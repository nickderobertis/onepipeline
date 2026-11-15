//! Turning a configuration into live sources.
//!
//! Two steps, deliberately separable. [`validate_sources`] runs at load, for every
//! verb, and refuses a source whose `config:` block does not match the schema its
//! plugin declares — the plugin itself is already one this build has, because
//! [`SourceConfig::plugin`] is a [`PluginKind`](crate::PluginKind) and no other kind
//! can be represented. [`resolve`] then
//! builds the sources a command actually needs. The order is the point: a typo in a
//! per-source field is refused while the user is still looking at the file that
//! caused it, rather than surfacing as a confusing failure inside the first HTTP
//! call that source makes.

use std::fmt;
use std::sync::Arc;

use jsonschema::error::ValidationErrorKind;
use onetaskgraph_plugin_api::{
    SecretResolver, SharedClock, SourceError, SourceName, SourcePlugin, StatusMapping, TaskSource,
    system_clock,
};
use serde_json::Value;

use crate::PluginKind;
use crate::config::{Config, ConfigError, SourceConfig};
use crate::plan::SourceFailure;
use crate::subprocess::SubprocessPlugin;

/// One configured source, built and ready to answer.
///
/// Held behind its accessors, and constructible only by [`resolve`], because `kind` is a
/// claim about `source` rather than a value beside it: a caller that could write the two
/// independently could say `linear` over a source that reports `local-md`, and the plan a
/// query reports names the kind. Building it where the plugin builds the source is what
/// makes the pair an invariant instead of something every reader has to re-check.
pub struct ResolvedSource {
    name: SourceName,
    source: Box<dyn TaskSource>,
}

impl ResolvedSource {
    /// Adopt a source under `name`.
    ///
    /// The kind is not a second field a caller could set: it is read back off the source
    /// through [`TaskSource::kind`], so the pair cannot disagree and the plan a query
    /// reports names the kind the source itself claims. That also makes this the seam a
    /// source built outside the registry arrives through — the engine's own tests today,
    /// the subprocess-hosted plugins the protocol document describes later.
    #[must_use]
    pub fn adopt(name: SourceName, source: Box<dyn TaskSource>) -> Self {
        Self { name, source }
    }

    /// The name the configuration gave it, which qualifies every id it returns.
    #[must_use]
    pub fn name(&self) -> &SourceName {
        &self.name
    }

    /// The plugin kind that built it, as the source itself reports it.
    ///
    /// A `&str` rather than a [`PluginKind`](crate::PluginKind): a subprocess-hosted
    /// plugin reports a kind no compile-time enumeration can hold, which is also why
    /// [`SourcePlan::kind`](crate::SourcePlan::kind) is a `String`.
    #[must_use]
    pub fn kind(&self) -> &str {
        self.source.kind()
    }

    /// The source itself.
    #[must_use]
    pub fn source(&self) -> &dyn TaskSource {
        self.source.as_ref()
    }

    /// The same source, behind `wrap` — the store's local cache of it, which forwards
    /// everything it does not answer itself, so the kind it reports is still the plugin's.
    pub(crate) fn wrapped(
        self,
        wrap: impl FnOnce(Box<dyn TaskSource>) -> Box<dyn TaskSource>,
    ) -> Self {
        Self {
            name: self.name,
            source: wrap(self.source),
        }
    }
}

/// One configured source that could not be built at all.
///
/// A missing credential, a plugin whose implementation has not landed, a `config:` block
/// its own plugin refuses at build time: none of them is a reason to answer nothing for
/// the *other* sources, so this is carried beside the ones that built and reported as a
/// [`SourceFailure`] in every response.
#[derive(Debug, Clone, PartialEq)]
pub struct UnavailableSource {
    name: SourceName,
    /// The plugin kind that was asked to build it.
    ///
    /// The kind a plugin reports, which is an open vocabulary rather than an
    /// under-modelled one: a subprocess-hosted plugin reports a kind arriving over the
    /// wire from a binary this workspace never compiled, so no compile-time type can
    /// enumerate it, and a newtype over the same string would only move where an
    /// unrelated value is accepted. This is the same field, and the same reason, as
    /// `SourcePlan.kind` and `SourceListing.kind`, where the contract fixes it as a
    /// string outright.
    // llmlint: ignore[invalid_states_unrepresentable] the reason above, recorded a third
    // time because this is the third site the same field appears at: `kind` is what
    // `SourcePlan.kind` (plan.rs) and `SourceListing.kind` (engine/mod.rs) already carry
    // as approved contract text, and narrowing it here alone would only make this crate
    // disagree with the two documents it renders into.
    kind: &'static str,
    error: SourceError,
}

impl UnavailableSource {
    /// The name the configuration gave it.
    #[must_use]
    pub fn name(&self) -> &SourceName {
        &self.name
    }

    /// The plugin kind that was asked to build it.
    #[must_use]
    pub fn kind(&self) -> &str {
        self.kind
    }

    /// Why it did not build.
    #[must_use]
    pub fn error(&self) -> &SourceError {
        &self.error
    }

    /// The same thing, as a response carries it.
    #[must_use]
    pub fn failure(&self) -> SourceFailure {
        SourceFailure {
            source: self.name.clone(),
            error: self.error.clone(),
        }
    }
}

impl fmt::Debug for ResolvedSource {
    /// Name and kind, the kind spelled the way a configuration spells it rather than
    /// the way Rust spells the variant. A live source has no meaningful `Debug` of its
    /// own, and one that did would be a rendering of a user's work — which nothing
    /// outside the plugin may hold.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedSource")
            .field("name", &self.name)
            .field("kind", &self.kind())
            .finish_non_exhaustive()
    }
}

/// Check every configured source without building any of them.
///
/// # Errors
///
/// Returns [`ConfigError::Setting`] naming `sources.<name>.config...` for a block that
/// does not match its plugin's declared schema.
pub fn validate_sources(config: &Config) -> Result<(), ConfigError> {
    for (name, source) in config.sources() {
        checked_plugin(name, source)?;
    }
    one_meaning_per_board(config)
}

/// Refuse two `github-projects` sources naming one board with different `sub_issues`.
///
/// The two meanings answer differently for the same issues — one reads an issue with sub-issues
/// as a project, the other as a task with children — and an edge written through either rewrites
/// what the other reads, so a `parent` source needs a board of its own.
// llmlint: ignore[code_lands_in_the_domain_that_owns_it] The refusal is about two configured sources at once, and a plugin is built from its own `config` block alone, so no GitHub Projects code can see the other source; moving it behind a plugin hook would change the `SourcePlugin` contract every plugin implements (AGENTS.md: keep the api crate still) to answer what one comparison of two blocks the engine already holds answers here, read through the plugin's own `remote_identity`.
#[cfg(feature = "github-projects")]
fn one_meaning_per_board(config: &Config) -> Result<(), ConfigError> {
    use onetaskgraph_github_projects::SubIssues;
    let mut boards: std::collections::BTreeMap<String, (SourceName, SubIssues)> =
        std::collections::BTreeMap::new();
    for (name, source) in config.sources() {
        if source.plugin().as_str() != "github-projects" {
            continue;
        }
        let Some(board) = source.plugin().plugin().remote_identity(source.config()) else {
            continue;
        };
        // The plugin's own setting, read as the plugin reads it — its default where unset — so
        // no second spelling of its meanings lives here. A spelling it refuses was refused
        // when the source was checked, before this.
        let meaning: SubIssues = source
            .config()
            .get("sub_issues")
            .map_or_else(
                || Ok(SubIssues::default()),
                |set| serde_json::from_value(set.clone()),
            )
            .unwrap_or_default();
        match boards.get(&board) {
            Some((other, held)) if *held != meaning => {
                let spelled = |meaning: SubIssues| {
                    serde_json::to_value(meaning)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_default()
                };
                let (held, meaning) = (spelled(*held), spelled(meaning));
                return Err(ConfigError::setting(
                    format!("sources.{name}.config.sub_issues"),
                    format!(
                        "sources {other} and {name} name one GitHub board, {other} with \
                         `sub_issues: {held}` and {name} with `sub_issues: {meaning}`; the two \
                         meanings read the same issues differently, and an edge written through \
                         either rewrites what the other reads"
                    ),
                    format!(
                        "give the `sub_issues: parent` source a board of its own, or give {other} \
                         and {name} one `sub_issues`"
                    ),
                ));
            }
            Some(_) => {}
            None => {
                boards.insert(board, (name.clone(), meaning));
            }
        }
    }
    Ok(())
}

/// Without the GitHub Projects plugin no source can name a board, so there is nothing to refuse.
#[cfg(not(feature = "github-projects"))]
fn one_meaning_per_board(_config: &Config) -> Result<(), ConfigError> {
    Ok(())
}

/// Build every configured source, in name order.
///
/// The order is the map's, so two runs over one configuration produce the same
/// sources in the same sequence — which is what makes a multi-source result stable
/// enough to page through.
///
/// # Errors
///
/// Returns what [`validate_sources`] returns, and [`ConfigError::Setting`] naming
/// `sources.<name>` when the plugin itself refuses to build the source.
pub fn resolve(
    config: &Config,
    secrets: &dyn SecretResolver,
) -> Result<Vec<ResolvedSource>, ConfigError> {
    validate_sources(config)?;
    let (built, unavailable) = resolve_available(config, secrets);
    match unavailable.first() {
        None => Ok(built),
        Some(failed) => Err(ConfigError::setting(
            format!("sources.{}", failed.name()),
            failed.error().to_string(),
            format!(
                "correct that source's configuration, or remove it — `onetaskgraph \
                 config show` reports every setting under `sources.{}` and the layer it \
                 came from.",
                failed.name()
            ),
        )),
    }
}

/// Build every configured source, keeping the ones that refused beside the ones that
/// built.
///
/// This is what the engine resolves through, and the difference from [`resolve`] is the
/// whole point: one source with an expired token must not stop the other two from
/// answering. A refusal here is reported per source, exactly as a source that fails
/// mid-query is.
///
/// The `config:` blocks are not re-checked, because a [`Config`] cannot exist holding one
/// its own plugin would refuse — [`Config::from_document`](crate::Config::from_document)
/// checks every block against its plugin's declared schema on the way in.
#[must_use]
pub fn resolve_available(
    config: &Config,
    secrets: &dyn SecretResolver,
) -> (Vec<ResolvedSource>, Vec<UnavailableSource>) {
    resolve_available_with_clock(config, secrets, &system_clock())
}

/// [`resolve_available`], building every in-process source through
/// [`SourcePlugin::build_with_clock`] with `clock`, the clock it paces and backs off on.
///
/// A `subprocess` source is not handed it: the program behind the pipe keeps its own time.
#[must_use]
pub fn resolve_available_with_clock(
    config: &Config,
    secrets: &dyn SecretResolver,
    clock: &SharedClock,
) -> (Vec<ResolvedSource>, Vec<UnavailableSource>) {
    let mut built = Vec::new();
    let mut unavailable = Vec::new();
    for (name, source) in config.sources() {
        let plugin = source.plugin().plugin();
        // The one plugin that passes an origin on: the trait hands a plugin values and no
        // origins, so a `subprocess` source is built with the directory of the document
        // its settings came from, which its child measures declared paths from.
        let outcome = if source.plugin() == PluginKind::Subprocess {
            SubprocessPlugin.build_from_document(
                name,
                source.config(),
                secrets,
                source.document_dir(),
            )
        } else {
            // llmlint: ignore[changed_behavior_has_e2e] No plugin of this build waits on the
            // clock it is handed yet — routing the hosted plugins' pacing through it is the
            // work this seam exists for — so nothing a source does can show which clock it was
            // built with. What is observable is proven: the binary settles one clock and
            // attaches to a test's coordinator with it (`crates/onetaskgraph-e2e/tests/e2e/clock.rs`), and every
            // built-in plugin builds through this call in every journey.
            plugin.build_with_clock(name, source.config(), secrets, Arc::clone(clock))
        };
        match outcome {
            Ok(source) => built.push(ResolvedSource::adopt(name.clone(), source)),
            Err(error) => unavailable.push(UnavailableSource {
                name: name.clone(),
                kind: plugin.kind(),
                error,
            }),
        }
    }
    (built, unavailable)
}

/// The plugin this source names, with its `config:` block already checked.
fn checked_plugin(
    name: &SourceName,
    source: &SourceConfig,
) -> Result<Box<dyn SourcePlugin>, ConfigError> {
    let plugin = source.plugin().plugin();
    check_block(name, source.config(), plugin.as_ref())?;
    Ok(plugin)
}

/// Check one source's `config:` block against the schema its plugin declares.
fn check_block(
    name: &SourceName,
    block: &Value,
    plugin: &dyn SourcePlugin,
) -> Result<(), ConfigError> {
    // A plugin's own schema is this build's, not a user's, so a schema that will not
    // compile is a defect in this binary rather than something a user did. It is still
    // reported rather than panicked on: a user whose one broken source is a plugin they
    // do not use can drop that source and carry on, which a panic would not let them do.
    // `every_registered_plugin_declares_a_schema_that_compiles_and_accepts_a_valid_block`
    // is what keeps it from reaching anybody in the first place.
    let schema = plugin.config_schema();
    let validator = jsonschema::validator_for(schema.as_value()).map_err(|error| {
        ConfigError::setting(
            format!("sources.{name}.plugin"),
            format!(
                "the `{}` plugin declares a configuration schema this build cannot \
                 compile: {error}",
                plugin.kind()
            ),
            "that is a defect in this binary rather than in your configuration — please \
             report it, naming the plugin above. Removing that source lets the rest of \
             this configuration run in the meantime.",
        )
    })?;

    let Some(problem) = validator.iter_errors(block).next() else {
        return Ok(());
    };

    // A plugin whose source is not written yet declares a schema with no properties at
    // all, which forbids every field — and a validator has nothing to say about that
    // beyond "false schema does not allow 7", which names neither the field nor the
    // reason. Both are worth saying plainly.
    if schema.as_value().get("properties").is_none()
        && let Some(fields) = block.as_object()
        && let Some(first) = fields.keys().next()
    {
        return Err(ConfigError::setting(
            format!("sources.{name}.config.{first}"),
            format!(
                "the `{}` plugin declares no configuration fields, so its `config:` block \
                 must be empty or absent; this one sets {}",
                plugin.kind(),
                fields.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
            format!(
                "remove those fields — `onetaskgraph schema` prints what this plugin \
                 accepts under `plugin_config.{}`.",
                plugin.kind()
            ),
        ));
    }

    // A `status_mapping` is one grammar every plugin that names its statuses shares, and a
    // validator can only say a value matched none of its forms — `{}` is "not valid under any
    // of the schemas listed in the 'anyOf' keyword". The grammar's own reading says which part
    // is wrong and what to write instead, so a refusal inside one is put in its words.
    let pointer = problem.instance_path().to_string();
    if (pointer == "/status_mapping" || pointer.starts_with("/status_mapping/"))
        && let Some(mapping) = block.get("status_mapping")
        && let Err(error) = serde_json::from_value::<StatusMapping>(mapping.clone())
    {
        return Err(ConfigError::setting(
            format!("sources.{name}.config.status_mapping"),
            error.to_string(),
            "write each category as a name, as null, or as an object naming a `task`, a \
             `project` or both — `onetaskgraph schema` prints the grammar under \
             `roots.StatusMapping`.",
        ));
    }

    // A validator reports an unexpected field against the *object* that holds it, so
    // the path alone would name the block and leave the user to find the field inside
    // the message. The field is the whole of what they have to go and fix, so it is
    // lifted into the key.
    let pointer = pointer.replace('/', ".");
    let unexpected = match problem.kind() {
        ValidationErrorKind::AdditionalProperties { unexpected } => unexpected.first(),
        _ => None,
    };
    let key = match unexpected {
        Some(field) => format!("sources.{name}.config{pointer}.{field}"),
        None => format!("sources.{name}.config{pointer}"),
    };
    Err(ConfigError::setting(
        key,
        problem.to_string(),
        format!(
            "check that field against the `{}` plugin's schema — `onetaskgraph schema` \
             prints it under `plugin_config.{}`.",
            plugin.kind(),
            plugin.kind()
        ),
    ))
}
