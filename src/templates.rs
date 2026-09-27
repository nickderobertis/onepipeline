//! **Task templates**: which named templates a host registers, which file each name
//! resolves to now, and whether a task is still the rendering of one (contract C4, C6a,
//! C7 and C8).
//!
//! A task's shape is a template its host owns rather than a rule this crate restates:
//! `onetaskgraph` renders, stores and regenerates an item from a template, and this module
//! does the three things only the composition layer can. It **registers** names — a host's
//! own, beside the one built-in this crate ships, [`BUILT_IN`] — and **layers** each of them
//! the one way, so a target repository can override any registered template without this
//! crate knowing a host's document kinds. It **states** the resolved template as the loader
//! document `onetaskgraph` reads (its contract C3b), so the file is resolved here and
//! rendered there. And it **checks**: a role-`task` template has to extend the base and
//! declare its criteria, a rendering has to list one, and — where a launch asks for it — a
//! node has to still be the rendering its provenance records.
//!
//! Nothing here writes to a store or renders into one, and nothing in `onetaskgraph` calls
//! this crate: the dependency runs one way. The built-in is the only template this crate
//! embeds, and every other name a host uses is registered by that host's own root.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use onetaskgraph_core::template::{body_digest, ItemType, LoaderDocument, TemplateError};
use onetaskgraph_core::{Template, TemplateProvenance, VariableType};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

/// The one template name this crate ships: the dispatched task.
pub const BUILT_IN: &str = "plan-task";

/// The name the built-in base is registered under on every loader this crate states, and
/// the template every role-`task` template extends.
pub const BASE: &str = "onepipeline/plan-task.md.j2";

/// The built-in base's source, embedded in this crate.
pub const BASE_SOURCE: &str = include_str!("templates/onepipeline/plan-task.md.j2");

/// The variable every role-`task` template declares as a list of strings.
pub const CRITERIA_VARIABLE: &str = "acceptance_criteria";

/// The registration file, directly under the host root.
pub const REGISTRATION_FILE: &str = "templates.yaml";

/// The version key a registration file declares, and the one version this build reads.
pub const REGISTRATION_KEY: &str = "onepipeline_templates";

/// The registration schema version this build reads.
pub const REGISTRATION_VERSION: u32 = 1;

/// Where a repository overrides a registered name, relative to its checkout.
pub const REPOSITORY_DIR: &str = ".onepipeline/templates";

/// What a template file for a name is called: `<name>` and this.
pub const EXTENSION: &str = ".md.j2";

/// The pattern a registered name matches. [`is_template_name`] is its one reading, and
/// `tests/contract.rs` holds the two to each other and to the contract's block.
pub const NAME_PATTERN: &str = "^[a-z][a-z0-9-]*$";

/// The remedy every C7 refusal names, with `<name>` and `<qualified id>` filled in — and
/// `task render` read as `document render` for a role-`document` item.
pub const REMEDY: &str = "onepipeline template resolve <name> --json | onetaskgraph task render \
                          <qualified id> --template-loader -";

/// What a loader document this crate states records as its `reference`: this and the name.
pub const REFERENCE_PREFIX: &str = "onepipeline:";

/// The flag naming the host root.
pub const ROOT_FLAG: &str = "--template-root";

/// The environment variable naming the host root, below the flag.
pub const ROOT_ENVIRONMENT: &str = "ONEPIPELINE_TEMPLATE_ROOT";

/// The launch-config key naming the host root, below the environment.
pub const ROOT_KEY: &str = "template_root";

/// The flag turning the rendered-only check on or off.
pub const REQUIRE_RENDERED_FLAG: &str = "--require-rendered";

/// The environment variable turning the rendered-only check on or off, below the flag.
pub const REQUIRE_RENDERED_ENVIRONMENT: &str = "ONEPIPELINE_REQUIRE_RENDERED";

/// The launch-config key turning the rendered-only check on or off, below the environment.
pub const REQUIRE_RENDERED_KEY: &str = "require_rendered";

/// The launch-config schema version [`ROOT_KEY`] and [`REQUIRE_RENDERED_KEY`] arrived at.
pub const CONFIG_SCHEMA_VERSION: u32 = 12;

/// C6a's rule for a role-`task` template that does not extend the base.
pub const RULE_NOT_EXTENDED: &str = "does not extend onepipeline/plan-task.md.j2";

/// C6a's rule for a role-`task` template whose criteria are not a list of strings.
pub const RULE_CRITERIA_TYPE: &str = "acceptance_criteria not a list of strings";

/// C4's refusal for a name no registration declares, `<name>` filled in.
pub const RULE_NOT_REGISTERED: &str = "template <name> is not registered";

/// C4's refusal for a registered name no layer supplies, `<name>` filled in.
pub const RULE_NO_LAYER: &str = "no template for <name>";

/// Whether the rendered-only check is on where no rung says.
pub const REQUIRE_RENDERED_DEFAULT: bool = false;

/// C7's refusal for a node carrying no provenance.
pub const RULE_NO_PROVENANCE: &str = "not rendered: no provenance";

/// C7's refusal for a node rendered from a template this crate did not state, followed by
/// `: <template>`.
pub const RULE_FOREIGN: &str = "not rendered by onepipeline";

/// C7's refusal for a node whose template has changed since it was rendered.
pub const RULE_TEMPLATE_CHANGED: &str = "template changed since rendering";

/// C7's refusal for a node whose content is not the content it was rendered with.
pub const RULE_BODY_CHANGED: &str = "body differs from its rendering";

/// What a registered template is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// A task onepipeline dispatches: it extends the base, and every rendering lists its
    /// acceptance criteria.
    Task,
    /// Any other document a host renders: held to nothing past compiling.
    Document,
}

impl Role {
    /// The role as `templates.yaml` spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Document => "document",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where a name resolved: the first of these that supplies it wins, and they are never
/// merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    /// The file `--template` named.
    Explicit,
    /// `<repo>/.onepipeline/templates/<name>.md.j2`.
    Repository,
    /// `<host root>/<name>.md.j2`.
    Host,
    /// The embedded base, which only [`BUILT_IN`] resolves to.
    BuiltIn,
}

impl Layer {
    /// The layer as the verbs print it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Repository => "repository",
            Self::Host => "host",
            Self::BuiltIn => "built-in",
        }
    }
}

impl std::fmt::Display for Layer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What the `template` verbs resolve against.
///
/// Everything the binary reads from its environment arrives here already resolved — the
/// host root out of its flag or variable, and the working directory — so a caller that is
/// not the binary is not answered out of the binary's environment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TemplateOptions {
    /// `--repo DIR`: the checkout whose repository layer is searched.
    pub repo: Option<PathBuf>,
    /// `--repository ORIGIN`, repeatable: exactly one names the checkout `onevcs` resolves
    /// it to when `repo` names none.
    pub repositories: Vec<String>,
    /// `--template FILE`: the explicit layer for the one name a verb acts on.
    pub template: Option<PathBuf>,
    /// The host root, already resolved and read.
    pub template_root: Option<PathBuf>,
    /// The working directory: the repository layer's checkout when nothing else names one,
    /// and what a relative `repo` or `template` is resolved against.
    pub working_dir: PathBuf,
}

/// One registered name, as `template list` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedTemplate {
    /// The registered name.
    pub name: String,
    /// What it is for.
    pub role: Role,
    /// What the registration says it is.
    pub description: String,
    /// The layer it resolves at now, or null where no layer supplies it.
    pub layer: Option<Layer>,
    /// The file it resolves to now, or null for the built-in and for a name no layer
    /// supplies.
    // llmlint: ignore[invalid_states_unrepresentable] `layer` and `path` side by side, `path` null for the built-in, is the wire shape C8 fixes for every consumer of `template list --json` and `template resolve --json` (`docs/contract.md`'s `listed_keys` and `resolved_keys`, reconciled by `tests/contract.rs`); the one producer of these values is `templates::locate`, which pairs a file with every layer but the built-in and none with it, and a nested enum would change the JSON the UI and ai-orchestrator nodes read.
    pub path: Option<PathBuf>,
}

/// Every registered name, as `template list` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TemplateList {
    /// The registration file read, or null where no host root was named or the root holds
    /// none.
    pub registration: Option<PathBuf>,
    /// [`BUILT_IN`] first, then every host-registered name in name order.
    pub templates: Vec<ListedTemplate>,
}

/// One `(name, source)` pair a loader document registers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedSource {
    /// The name a chain reaches it by.
    pub name: String,
    /// Its whole source.
    pub source: String,
}

/// A resolved template, stated as `onetaskgraph`'s loader document (its contract C3b) with
/// what resolved it beside: what `template resolve --json` prints.
///
/// `reference`, `entry`, `search_path`, `templates` and `digest` are the loader document,
/// so the output pipes straight into `onetaskgraph ... --template-loader -`; that product
/// ignores the other four keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedTemplate {
    /// `onepipeline:<name>`: what an item rendered from it records as its provenance.
    pub reference: String,
    /// The name loaded: the resolved file's own name, or [`BASE`] for the built-in.
    pub entry: String,
    /// The resolved file's own directory, absolute; empty for the built-in.
    pub search_path: Vec<PathBuf>,
    /// The embedded base, registered under [`BASE`].
    pub templates: Vec<NamedSource>,
    /// The chain's digest, as `onetaskgraph` computes it over this document.
    pub digest: String,
    /// The registered name.
    pub name: String,
    /// What it is for.
    pub role: Role,
    /// The layer it resolved at.
    pub layer: Layer,
    /// The file it resolved to, absolute, or null for the built-in.
    // llmlint: ignore[invalid_states_unrepresentable] `layer` and `path` side by side, `path` null for the built-in, is the wire shape C8 fixes for every consumer of `template list --json` and `template resolve --json` (`docs/contract.md`'s `listed_keys` and `resolved_keys`, reconciled by `tests/contract.rs`); the one producer of these values is `templates::locate`, which pairs a file with every layer but the built-in and none with it, and a nested enum would change the JSON the UI and ai-orchestrator nodes read.
    pub path: Option<PathBuf>,
}

/// What `template check` is asked about beyond the template itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateCheck {
    /// The resolved template alone (C6a).
    Template,
    /// The template, and this text as one of its renderings (C6b, for role `task`).
    Rendering(String),
    /// The template, and the stored item with this qualified id (C6b for role `task`,
    /// and C7's three checks).
    // llmlint: ignore[invalid_states_unrepresentable] the id as the caller spelled it, parsed into the store's own qualified-id type by `template_check` before anything is read and refused there by name when it is not `<source>:<native>`, exactly as `start` and `plan check` take a project id; a validated id type here would be a public item the contract never promised.
    Item(String),
}

/// What `template check` checked beside the template itself: one of the three, as the
/// flags that ask for the other two exclude each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Checked {
    /// The template alone.
    Template,
    /// A rendering too.
    Rendering,
    /// The stored item with this qualified id too.
    // llmlint: ignore[invalid_states_unrepresentable] the id as `template_check` printed it after parsing it into the store's own qualified-id type — this value is only ever built from that parsed id — and a public qualified-id type here would be a public item the contract never promised, exactly as `TemplateCheck::Item` and every run and project id the post-launch verbs answer with are strings.
    Item(String),
}

/// What `template check` accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TemplateChecked {
    /// The registered name.
    pub name: String,
    /// What it is for.
    pub role: Role,
    /// The layer it resolved at.
    pub layer: Layer,
    /// The file it resolved to, or null for the built-in.
    // llmlint: ignore[invalid_states_unrepresentable] `layer` and `path` side by side, `path` null for the built-in, is the wire shape C8 fixes for every consumer of `template list --json` and `template resolve --json` (`docs/contract.md`'s `listed_keys` and `resolved_keys`, reconciled by `tests/contract.rs`); the one producer of these values is `templates::locate`, which pairs a file with every layer but the built-in and none with it, and a nested enum would change the JSON the UI and ai-orchestrator nodes read.
    pub path: Option<PathBuf>,
    /// The chain's digest.
    pub digest: String,
    /// What was checked beside the template.
    pub checked: Checked,
}

/// One name as `templates.yaml` declares it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Declared {
    role: Role,
    description: String,
}

/// `templates.yaml`, whole. Every key is required: an unknown one is refused by serde.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationFile {
    onepipeline_templates: u32,
    templates: BTreeMap<String, Declared>,
}

/// One registered name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Registered {
    pub name: String,
    pub role: Role,
    pub description: String,
}

/// Every name a host root registers, beside the built-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Registry {
    /// Where the names came from.
    from: Registration,
    /// [`BUILT_IN`] first, then the host's names in name order.
    names: Vec<Registered>,
}

/// Where a [`Registry`]'s names came from: one of three, so a registration file is never
/// read from a root that was not named.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Registration {
    /// No host root was named.
    NoRoot,
    /// A root was named and holds no registration file.
    NoFile(PathBuf),
    /// This registration file was read.
    Read(PathBuf),
}

/// [`NAME_PATTERN`], read by hand: this crate links no regular-expression engine, and
/// `tests/contract.rs` probes every class boundary of the pattern against it.
fn is_template_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl Registry {
    /// The names `root` registers — only [`BUILT_IN`] where there is no root, or the root
    /// holds no registration file.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] naming the root for one that cannot be read — so a root that is
    /// not there is never read as one holding no registration — and naming the file for a
    /// registration that does not parse, carries an unknown key, declares a version this
    /// build does not read, a bad name, a blank description, or [`BUILT_IN`] again; and for
    /// a file that is there and cannot be read.
    pub(crate) fn load(root: Option<&Path>) -> Result<Self> {
        let built_in = Registered {
            name: BUILT_IN.to_owned(),
            role: Role::Task,
            description: "The task onepipeline dispatches, ending in its acceptance criteria."
                .to_owned(),
        };
        let Some(root) = root else {
            return Ok(Self {
                from: Registration::NoRoot,
                names: vec![built_in],
            });
        };
        std::fs::read_dir(root).map_err(|error| {
            Error::Invalid(format!(
                "template root {} cannot be read: {error}",
                root.display()
            ))
        })?;
        let file = root.join(REGISTRATION_FILE);
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    from: Registration::NoFile(file),
                    names: vec![built_in],
                });
            }
            Err(error) => {
                return Err(Error::Invalid(format!(
                    "{}: the template registration cannot be read: {error}",
                    file.display()
                )))
            }
        };
        let refused = |why: String| Error::Invalid(format!("{}: {why}", file.display()));
        let parsed: RegistrationFile =
            serde_norway::from_str(&text).map_err(|error| refused(error.to_string()))?;
        if parsed.onepipeline_templates != REGISTRATION_VERSION {
            return Err(refused(format!(
                "`{REGISTRATION_KEY}: {}` is not a registration version this build reads; it \
                 reads {REGISTRATION_VERSION}",
                parsed.onepipeline_templates
            )));
        }
        let mut names = vec![built_in];
        for (name, declared) in parsed.templates {
            if name == BUILT_IN {
                return Err(refused(format!(
                    "`{BUILT_IN}` is pre-registered with the built-in and cannot be declared \
                     again; override it with a `{BUILT_IN}{EXTENSION}` file in the host root \
                     or a repository instead"
                )));
            }
            if !is_template_name(&name) {
                return Err(refused(format!(
                    "`{name}` is not a template name: a name matches {NAME_PATTERN}"
                )));
            }
            if declared.description.trim().is_empty() {
                return Err(refused(format!(
                    "template `{name}` has a blank `description`; say what it is for"
                )));
            }
            names.push(Registered {
                name,
                role: declared.role,
                description: declared.description,
            });
        }
        Ok(Self {
            from: Registration::Read(file),
            names,
        })
    }

    /// The registration file read, where one was.
    pub(crate) fn file(&self) -> Option<PathBuf> {
        match &self.from {
            Registration::Read(file) => Some(file.clone()),
            Registration::NoRoot | Registration::NoFile(_) => None,
        }
    }

    /// Every registered name, [`BUILT_IN`] first.
    pub(crate) fn names(&self) -> &[Registered] {
        &self.names
    }

    /// The registration of `name`.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] — `template <name> is not registered` — naming the registration
    /// file read, or saying none was.
    pub(crate) fn get(&self, name: &str) -> Result<&Registered> {
        self.names
            .iter()
            .find(|registered| registered.name == name)
            .ok_or_else(|| {
                let whence = match &self.from {
                    Registration::Read(file) => format!("registration read: {}", file.display()),
                    Registration::NoFile(file) => format!(
                        "no registration file at {}, so only {BUILT_IN} is registered",
                        file.display()
                    ),
                    Registration::NoRoot => format!(
                        "no template root was named ({ROOT_FLAG}, {ROOT_ENVIRONMENT} or the \
                         launch config's `{ROOT_KEY}`), so only {BUILT_IN} is registered"
                    ),
                };
                Error::Refused(format!(
                    "{} ({whence})",
                    RULE_NOT_REGISTERED.replace("<name>", name)
                ))
            })
    }
}

/// Where one resolution looks: the explicit file, the repository checkout, the host root.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Search<'a> {
    pub explicit: Option<&'a Path>,
    pub repo: Option<&'a Path>,
    pub root: Option<&'a Path>,
}

/// A name resolved, and its chain loaded.
pub(crate) struct Resolution {
    pub stated: ResolvedTemplate,
    pub template: Template,
}

impl Resolution {
    /// How a refusal about this resolution names where it came from.
    pub(crate) fn whence(&self) -> String {
        whence(self.stated.layer, self.stated.path.as_deref())
    }
}

fn whence(layer: Layer, path: Option<&Path>) -> String {
    match path {
        Some(path) => format!("{layer} layer, {}", path.display()),
        None => format!("{layer} layer"),
    }
}

/// An absolute spelling of `path`, relative ones taken against `base`.
fn absolute(path: &Path, base: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    std::path::absolute(&joined).unwrap_or(joined)
}

/// Resolve `name` through C4's layers, first found wins: explicit, repository, host, and
/// — for [`BUILT_IN`] only — the embedded base; then load its chain over the resolved file's
/// own directory and the embedded base, exactly as the stated loader document does.
///
/// # Errors
///
/// [`Error::Refused`] for a name that is not registered, a registered name no layer
/// supplies — naming every path searched — and a chain that does not load, naming the
/// layer and the file; [`Error::Invalid`] for an explicit file that is not one.
pub(crate) fn resolve(registry: &Registry, name: &str, search: Search<'_>) -> Result<Resolution> {
    let registered = registry.get(name)?;
    let (layer, path) = locate(name, search)?;
    let reference = format!("{REFERENCE_PREFIX}{name}");
    let base = NamedSource {
        name: BASE.to_owned(),
        source: BASE_SOURCE.to_owned(),
    };
    let (entry, search_path) = match &path {
        Some(path) => {
            let entry = path
                .file_name()
                .map(|file| file.to_string_lossy().into_owned())
                .unwrap_or_default();
            let directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
            (entry, vec![directory])
        }
        None => (BASE.to_owned(), Vec::new()),
    };
    let unloadable = |error: String| {
        Error::Refused(format!(
            "template {name} ({}) does not load: {error}",
            whence(layer, path.as_deref())
        ))
    };
    let mut document = LoaderDocument::new(reference.clone(), entry.clone())
        .map_err(|error| unloadable(error.to_string()))?;
    for directory in &search_path {
        document = document
            .with_directory(directory)
            .map_err(|error| unloadable(error.to_string()))?;
    }
    let document = document
        .with_template(&base.name, &base.source)
        .map_err(|error| unloadable(error.to_string()))?;
    let template = document.load().map_err(|error| match &error {
        TemplateError::ChainConflict { variable, .. }
            if variable == CRITERIA_VARIABLE && registered.role == Role::Task =>
        {
            Error::Refused(format!(
                "template {name} ({}): {RULE_CRITERIA_TYPE}: {error}",
                whence(layer, path.as_deref())
            ))
        }
        _ => unloadable(error.to_string()),
    })?;
    Ok(Resolution {
        stated: ResolvedTemplate {
            reference,
            entry,
            search_path,
            templates: vec![base],
            digest: template.digest().to_owned(),
            name: name.to_owned(),
            role: registered.role,
            layer,
            path,
        },
        template,
    })
}

/// # Errors
///
/// As [`resolve`], for everything but a chain that does not load — which is why
/// `template list` asks this: a broken template still reports the layer it sits at.
pub(crate) fn locate(name: &str, search: Search<'_>) -> Result<(Layer, Option<PathBuf>)> {
    if let Some(explicit) = search.explicit {
        if !explicit.is_file() {
            return Err(Error::Invalid(format!(
                "--template {} is not a readable template file",
                explicit.display()
            )));
        }
        return Ok((Layer::Explicit, Some(explicit.to_path_buf())));
    }
    let file = format!("{name}{EXTENSION}");
    let mut searched = Vec::new();
    for (layer, directory) in [
        (
            Layer::Repository,
            search.repo.map(|repo| repo.join(REPOSITORY_DIR)),
        ),
        (Layer::Host, search.root.map(Path::to_path_buf)),
    ] {
        let Some(directory) = directory else {
            continue;
        };
        let candidate = directory.join(&file);
        if candidate.is_file() {
            return Ok((layer, Some(candidate)));
        }
        searched.push(candidate.display().to_string());
    }
    if name == BUILT_IN {
        return Ok((Layer::BuiltIn, None));
    }
    let searched = if searched.is_empty() {
        "no layer was searched: name a repository or a template root".to_owned()
    } else {
        format!("searched {}", searched.join(", "))
    };
    Err(Error::Refused(format!(
        "{}: {searched}",
        RULE_NO_LAYER.replace("<name>", name)
    )))
}

/// C6a over one resolved template, without rendering it: for role `task`, the chain
/// extends [`BASE`] and declares [`CRITERIA_VARIABLE`] as a list of strings. Role
/// `document` requires nothing past loading, which [`resolve`] has already done.
///
/// # Errors
///
/// The rule the template breaks, spelled as the contract spells it.
pub(crate) fn validate(resolution: &Resolution) -> std::result::Result<(), &'static str> {
    if resolution.stated.role == Role::Document {
        return Ok(());
    }
    if let Some(path) = &resolution.stated.path {
        if !extends_base(path) {
            return Err(RULE_NOT_EXTENDED);
        }
    }
    let criteria = resolution
        .template
        .variables()
        .iter()
        .find(|variable| variable.name() == CRITERIA_VARIABLE);
    match criteria {
        Some(variable)
            if variable.kind() == VariableType::List
                && variable.items() == Some(ItemType::String) =>
        {
            Ok(())
        }
        _ => Err(RULE_CRITERIA_TYPE),
    }
}

/// The name a template's `{% extends %}` tag spells literally, if it has one.
///
/// An `extends` naming its parent by an expression is not one this can follow, and a
/// template that relies on one is refused as not extending the base: the check is on the
/// file as written, never on a render. A tag inside a `{# comment #}` or a
/// `{% raw %}` block is text, not a tag, and is passed over.
fn extends_literal(source: &str) -> Option<String> {
    let mut rest = source;
    loop {
        let tag_at = rest.find("{%");
        let comment_at = rest.find("{#");
        let open = match (tag_at, comment_at) {
            (Some(tag), Some(comment)) if comment < tag => {
                let after = &rest[comment + 2..];
                rest = &after[after.find("#}")? + 2..];
                continue;
            }
            (Some(tag), _) => tag,
            (None, _) => return None,
        };
        let after = &rest[open + 2..];
        let close = after.find("%}")?;
        let tag = after[..close]
            .trim_start_matches(['-', '+'])
            .trim_end_matches(['-', '+'])
            .trim();
        if tag == "raw" {
            let body = &after[close + 2..];
            let mut end = body;
            loop {
                let at = end.find("{%")?;
                let inner = &end[at + 2..];
                let shut = inner.find("%}")?;
                let named = inner[..shut]
                    .trim_start_matches(['-', '+'])
                    .trim_end_matches(['-', '+'])
                    .trim();
                end = &inner[shut + 2..];
                if named == "endraw" {
                    break;
                }
            }
            rest = end;
            continue;
        }
        if let Some(named) = tag.strip_prefix("extends") {
            let named = named.trim();
            let quote = named.chars().next()?;
            if quote == '"' || quote == '\'' {
                let inner = &named[1..];
                return inner.find(quote).map(|end| inner[..end].to_owned());
            }
            return None;
        }
        rest = &after[close + 2..];
    }
}

/// Whether the file at `path` reaches [`BASE`] by `extends`, following each parent it
/// names in its own directory — the search path a stated loader document gives it.
fn extends_base(path: &Path) -> bool {
    let directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut seen = BTreeSet::new();
    let mut at = path.to_path_buf();
    loop {
        if !seen.insert(at.clone()) {
            return false;
        }
        let Ok(source) = std::fs::read_to_string(&at) else {
            return false;
        };
        let Some(parent) = extends_literal(&source) else {
            return false;
        };
        let within = Path::new(&parent)
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
        let local = directory.join(&parent);
        // The directory is searched before the base, so a file of that name there is what
        // the chain loads.
        if within && local.is_file() {
            at = local;
            continue;
        }
        return parent == BASE;
    }
}

/// C6b over one rendering of a role-`task` template, through the one rule every dispatched
/// task is held to.
///
/// # Errors
///
/// The rule's own words.
pub(crate) fn check_rendering(text: &str) -> std::result::Result<(), &'static str> {
    crate::plan::check_criteria(text).map_err(crate::plan::CriteriaRule::as_str)
}

/// One stored item C7 is asked about.
pub(crate) struct Stored<'a> {
    /// Its qualified id, which the remedy names.
    pub qualified: &'a crate::taskgraph::QualifiedId,
    /// Its metadata, where its provenance is.
    pub metadata: &'a BTreeMap<String, Value>,
    /// Its content exactly as stored.
    pub content: &'a str,
}

/// C7's three checks over one stored item, in order: its provenance names a template this
/// crate stated for a registered name of `role` — `expected` where the caller names one —
/// that name's chain resolved now has the recorded digest, and the content's SHA-256 is the
/// recorded body digest.
///
/// It reads no answers and renders nothing, so it answers the same over a copy of the item
/// on another board. It trusts the recorded provenance.
///
/// # Errors
///
/// The refusal, followed by the remedy that clears it.
pub(crate) fn check_rendered(
    registry: &Registry,
    search: Search<'_>,
    role: Role,
    expected: Option<&str>,
    item: &Stored<'_>,
) -> std::result::Result<(), String> {
    let remedy = |name: &str, answers: bool| {
        let command = REMEDY
            .replace("<name>", name)
            .replace("<qualified id>", item.qualified.as_str());
        let command = match role {
            Role::Task => command,
            Role::Document => command.replace(" task render ", " document render "),
        };
        format!(
            "; re-render it: {command}{}",
            if answers {
                " --answers FILE (FILE holding every required answer)"
            } else {
                ""
            }
        )
    };
    let fallback = expected.unwrap_or(BUILT_IN);
    let provenance = match TemplateProvenance::read(item.metadata) {
        Ok(Some(provenance)) => provenance,
        Ok(None) => return Err(format!("{RULE_NO_PROVENANCE}{}", remedy(fallback, true))),
        Err(why) => {
            return Err(format!(
                "{RULE_NO_PROVENANCE} this build can read ({why}){}",
                remedy(fallback, true)
            ))
        }
    };
    let name = provenance
        .template
        .strip_prefix(REFERENCE_PREFIX)
        .filter(|name| expected.is_none_or(|expected| expected == *name))
        .filter(|name| {
            registry
                .names()
                .iter()
                .any(|registered| registered.name == *name && registered.role == role)
        });
    let Some(name) = name else {
        return Err(format!(
            "{RULE_FOREIGN}: {}{}",
            provenance.template,
            remedy(fallback, true)
        ));
    };
    let resolution = resolve(registry, name, search).map_err(|error| match error {
        Error::Refused(why) | Error::Invalid(why) => why,
        other => other.to_string(),
    })?;
    if resolution.stated.digest != provenance.digest.as_str() {
        return Err(format!(
            "{RULE_TEMPLATE_CHANGED}: rendered with {}, and {name} resolves now to {} ({}){}",
            provenance.digest,
            resolution.stated.digest,
            resolution.whence(),
            remedy(name, false)
        ));
    }
    if body_digest(item.content) != provenance.body_digest.as_str() {
        return Err(format!("{RULE_BODY_CHANGED}{}", remedy(name, false)));
    }
    Ok(())
}

/// The host root out of its three rungs — the flag, then [`ROOT_ENVIRONMENT`], then the
/// launch config's [`ROOT_KEY`] — absolute and checked readable.
///
/// The flag and the variable are relative to the working directory, and the key to the
/// launch directory. A rung that is there and blank is refused by its own name, as is a
/// root that cannot be read.
///
/// # Errors
///
/// [`Error::Invalid`] naming the rung, for a blank one, a variable this build cannot read
/// as text, and a root that cannot be read.
pub(crate) fn resolve_root(
    flag: Option<&Path>,
    configured: Option<&str>,
    working_dir: &Path,
    launch_dir: &Path,
) -> Result<Option<PathBuf>> {
    let (named, base, whence) = match flag {
        Some(flag) => (flag.to_path_buf(), working_dir, ROOT_FLAG.to_owned()),
        None => match std::env::var(ROOT_ENVIRONMENT) {
            Ok(variable) => (
                PathBuf::from(variable),
                working_dir,
                ROOT_ENVIRONMENT.to_owned(),
            ),
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(Error::Invalid(format!(
                    "{ROOT_ENVIRONMENT} holds something this build cannot read as text — set \
                     it to a directory, or unset it"
                )))
            }
            Err(std::env::VarError::NotPresent) => match configured {
                Some(configured) => (
                    PathBuf::from(configured),
                    launch_dir,
                    format!("the launch config's `{ROOT_KEY}`"),
                ),
                None => return Ok(None),
            },
        },
    };
    if named.as_os_str().to_string_lossy().trim().is_empty() {
        return Err(Error::Invalid(format!(
            "{whence} is blank: name the template root directory, or leave it out"
        )));
    }
    let root = absolute(&named, base);
    std::fs::read_dir(&root).map_err(|error| {
        Error::Invalid(format!(
            "template root {} (from {whence}) cannot be read: {error}",
            root.display()
        ))
    })?;
    Ok(Some(root))
}

/// Whether the rendered-only check is on, out of its three rungs — the flag, then
/// [`REQUIRE_RENDERED_ENVIRONMENT`], then the launch config's [`REQUIRE_RENDERED_KEY`] —
/// and off where none says.
///
/// # Errors
///
/// [`Error::Invalid`] for a variable that is not `true` or `false`.
pub(crate) fn resolve_require_rendered(
    flag: Option<bool>,
    configured: Option<bool>,
) -> Result<bool> {
    if let Some(flag) = flag {
        return Ok(flag);
    }
    match std::env::var(REQUIRE_RENDERED_ENVIRONMENT) {
        Ok(variable) => match variable.trim() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(Error::Invalid(format!(
                "{REQUIRE_RENDERED_ENVIRONMENT} is {other:?}; set it to true or false, or unset it"
            ))),
        },
        Err(std::env::VarError::NotUnicode(_)) => Err(Error::Invalid(format!(
            "{REQUIRE_RENDERED_ENVIRONMENT} holds something this build cannot read as text; \
             set it to true or false, or unset it"
        ))),
        Err(std::env::VarError::NotPresent) => Ok(configured.unwrap_or(REQUIRE_RENDERED_DEFAULT)),
    }
}

/// The checkout the repository layer searches for the `template` verbs: `--repo`, else
/// the checkout `onevcs` resolves the single `--repository` origin to, else the working
/// directory.
///
/// # Errors
///
/// [`Error::Invalid`] for an origin `onevcs` cannot resolve to a checkout.
pub(crate) fn verb_checkout(options: &TemplateOptions) -> Result<PathBuf> {
    if let Some(repo) = &options.repo {
        let checkout = absolute(repo, &options.working_dir);
        // A checkout that is not there would search nothing at the repository layer and
        // resolve the name one layer down, as though the repository overrode nothing.
        std::fs::read_dir(&checkout).map_err(|error| {
            Error::Invalid(format!(
                "--repo {} cannot be read as a checkout: {error}",
                checkout.display()
            ))
        })?;
        return Ok(checkout);
    }
    if let [origin] = options.repositories.as_slice() {
        return crate::destination::resolve(origin)
            .map(|destination| destination.resolved.publication_checkout)
            .map_err(|why| {
                Error::Invalid(format!(
                    "--repository {origin}: its checkout could not be resolved: {why}"
                ))
            });
    }
    // For the reason `--repo` is read above: a working directory that is not there would
    // search nothing at the repository layer.
    std::fs::read_dir(&options.working_dir).map_err(|error| {
        Error::Invalid(format!(
            "the working directory {} cannot be read as the checkout: {error}",
            options.working_dir.display()
        ))
    })?;
    Ok(options.working_dir.clone())
}

pub(crate) fn verb_explicit(options: &TemplateOptions) -> Option<PathBuf> {
    options
        .template
        .as_deref()
        .map(|template| absolute(template, &options.working_dir))
}

/// What one launch holds its plan's templates to: the host root it read, and whether a
/// node has to be the rendering its provenance records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launch {
    pub root: Option<PathBuf>,
    pub require_rendered: bool,
}

/// C7 over a loaded plan, when the launch asked for it: every agent node — never a step,
/// which is held to C6b alone — is checked where it is stored, its repository layer
/// resolved against its lifecycle checkout, or `launch_dir` for a direct node.
///
/// # Errors
///
/// The first refusal, naming the node.
pub(crate) fn check_plan(
    launch: &Launch,
    read: &crate::taskgraph::Read,
    launch_dir: &Path,
) -> std::result::Result<(), crate::refusal::Refusal> {
    use crate::refusal::Refusal;
    let registry = Registry::load(launch.root.as_deref())
        .map_err(|error| Refusal::plain(error.to_string()))?;
    if !launch.require_rendered {
        return Ok(());
    }
    for node in &read.plan.tasks {
        if node.kind == crate::plan::NodeKind::Human {
            continue;
        }
        let checkout = match &node.repo {
            Some(repo) => crate::destination::resolve(repo)
                .map(|destination| destination.resolved.publication_checkout)
                .map_err(|why| {
                    Refusal::node(
                        &node.id,
                        format!(
                            "its template cannot be resolved: the checkout of {repo} could not \
                             be resolved: {why}"
                        ),
                    )
                    .field("task")
                })?,
            None => launch_dir.to_path_buf(),
        };
        let Some(stored) = read.stored.get(&node.id) else {
            continue;
        };
        let metadata: BTreeMap<String, Value> = read
            .metadata
            .get(&node.id)
            .map(|map| map.clone().into_iter().collect())
            .unwrap_or_default();
        check_rendered(
            &registry,
            Search {
                explicit: None,
                repo: Some(&checkout),
                root: launch.root.as_deref(),
            },
            Role::Task,
            None,
            &Stored {
                qualified: &stored.qualified,
                metadata: &metadata,
                content: &stored.content,
            },
        )
        .map_err(|why| Refusal::node(&node.id, why).field("task"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_lowercase_led_and_kebab() {
        for good in ["plan-task", "design-doc", "a", "x9-y"] {
            assert!(is_template_name(good), "{good} is a name");
        }
        for bad in [
            "",
            "Plan",
            "9lives",
            "-x",
            "under_score",
            "dot.ted",
            "sp ace",
        ] {
            assert!(!is_template_name(bad), "{bad:?} is not a name");
        }
    }

    #[test]
    fn an_extends_tag_is_read_however_it_is_spaced_and_quoted() {
        assert_eq!(
            extends_literal("---\na: 1\n---\n{% extends \"onepipeline/plan-task.md.j2\" %}"),
            Some(BASE.to_owned())
        );
        assert_eq!(
            extends_literal("{%- extends 'parent.md.j2' -%}\n"),
            Some("parent.md.j2".to_owned())
        );
        assert_eq!(
            extends_literal("{% block x %}{% endblock %}{%extends\"late.md.j2\"%}"),
            Some("late.md.j2".to_owned())
        );
        assert_eq!(extends_literal("{% extends parent %}"), None);
        assert_eq!(extends_literal("no tags at all {{ x }}"), None);
        assert_eq!(extends_literal("{% unclosed"), None);
        // A tag inside a comment or a raw block is text, and the real one after it is read.
        assert_eq!(
            extends_literal("{# {% extends \"commented.md.j2\" %} #}{% extends \"real.md.j2\" %}"),
            Some("real.md.j2".to_owned())
        );
        assert_eq!(
            extends_literal("{# {% extends \"onepipeline/plan-task.md.j2\" %} #}\nno tag"),
            None
        );
        assert_eq!(
            extends_literal(
                "{% raw %}{% extends \"raw.md.j2\" %}{% endraw %}{%- extends 'after.md.j2' %}"
            ),
            Some("after.md.j2".to_owned())
        );
        assert_eq!(extends_literal("{# unclosed comment"), None);
    }

    #[test]
    fn the_built_in_base_declares_its_criteria_and_renders_them_as_a_list() {
        let template = LoaderDocument::new("onepipeline:plan-task", BASE)
            .and_then(|document| document.with_template(BASE, BASE_SOURCE))
            .and_then(|document| document.load())
            .expect("the base loads");
        let [variable] = template.variables() else {
            panic!("the base declares exactly one variable");
        };
        assert_eq!(variable.name(), CRITERIA_VARIABLE);
        assert_eq!(variable.kind(), VariableType::List);
        assert_eq!(variable.items(), Some(ItemType::String));
        assert!(variable.required());
        assert_eq!(
            variable.description(),
            "What must be true when this task is done; each entry is one criterion."
        );
        let mut answers = onetaskgraph_core::Answers::new();
        answers.set(
            CRITERIA_VARIABLE,
            serde_json::json!(["it builds", "two\nlines"]),
        );
        let rendered = template.render(&answers).expect("the base renders");
        assert_eq!(
            rendered.body,
            "## Acceptance criteria\n\n- it builds\n- two\n  lines\n"
        );
        assert_eq!(check_rendering(&rendered.body), Ok(()));
    }

    #[test]
    fn a_root_that_is_not_there_is_refused_and_never_read_as_registering_nothing() {
        let missing = std::env::temp_dir().join(format!(
            "onepipeline-no-such-template-root-{}",
            std::process::id()
        ));
        let refused = Registry::load(Some(&missing)).expect_err("a missing root is refused");
        assert!(
            refused.to_string().contains(&format!(
                "template root {} cannot be read",
                missing.display()
            )),
            "{refused}"
        );
        let listed = crate::verbs::template_list(&TemplateOptions {
            template_root: Some(missing.clone()),
            working_dir: std::env::temp_dir(),
            ..TemplateOptions::default()
        });
        assert!(
            listed.is_err(),
            "the SDK listed names under a root that is not there"
        );
        let nowhere = crate::verbs::template_list(&TemplateOptions {
            working_dir: missing.clone(),
            ..TemplateOptions::default()
        })
        .expect_err("a working directory that is not there is not a checkout");
        assert!(
            nowhere
                .to_string()
                .contains("cannot be read as the checkout"),
            "{nowhere}"
        );
        let registry = Registry::load(None).expect("no root registers the built-in");
        assert_eq!(registry.names().len(), 1);
        assert_eq!(registry.file(), None);
    }

    #[test]
    fn a_require_rendered_flag_wins_over_everything_under_it() {
        assert_eq!(
            resolve_require_rendered(Some(true), Some(false)).ok(),
            Some(true)
        );
        assert_eq!(
            resolve_require_rendered(Some(false), Some(true)).ok(),
            Some(false)
        );
    }
}
