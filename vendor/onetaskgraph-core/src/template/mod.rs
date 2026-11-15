// llmlint: ignore-file[code_lands_in_the_domain_that_owns_it] This module is where the task that introduced templates fixes their library API: at `onetaskgraph-core`'s crate root, beside the engine every surface shares, so a Rust consumer renders without the binary. A crate of its own would be a new published sibling — a change to the release surface `release-targets.toml` freezes — which is not this change's to make.
//! Task templates: a minijinja document with a declared set of variables, rendered from
//! answers.
//!
//! # The format
//!
//! A template file is UTF-8 minijinja source, optionally opened by YAML front matter — a
//! first line reading `---`, closed by the next line reading `---`:
//!
//! ```yaml
//! ---
//! onetaskgraph_template: 1          # required when front matter is present
//! description: <string>             # optional
//! variables:                        # optional; name -> declaration
//!   <name>:                         # ^[a-z][a-z0-9_]*$
//!     description: <string>         # required, non-empty: what a prompt shows
//!     type: string                  # string | text | integer | boolean | list | object
//!     items: string                 # list only: string | object; default string
//!     required: true                # default: true without `default`, false with one
//!     default: <value of `type`>    # optional; `required: true` beside one is refused
//!     schema: <JSON Schema>         # object and list only: draft 2020-12, inline
//! ---
//! ```
//!
//! Nothing here restricts how the body uses a variable: it is minijinja, whole.
//!
//! # The chain
//!
//! `extends`, `include` and `import` resolve names over a [`TemplateLoader`]'s search path —
//! the directories it was given, in order, then the `(name, source)` pairs registered on it —
//! and never the working directory. The **declared set** is the union of every chain file's
//! front matter. When two files declare one variable, the file nearer the rendered one (fewer
//! references away from it, then loaded first) gives its `description`, `default`,
//! `required` and `schema`; its `type` and `items` must be the other's, or the chain is refused naming both
//! files.
//!
//! # Rendering
//!
//! Strict: a name that is neither a declared variable nor set by the template fails the
//! render, naming the name and the file. No auto-escaping, trailing newlines kept,
//! `trim_blocks` and `lstrip_blocks` on. An optional variable given no answer and no default
//! renders as `none`.
//!
//! An `object` or `list` variable that declares a `schema` holds its answer — and its
//! `default` — to it, after holding it to its type: an answer the schema refuses is refused
//! as [`TemplateError::SchemaViolation`], naming the variable and the JSON pointer of the
//! first violation, before anything renders. A variable with no `schema` is held to its type
//! alone.
//!
//! # The digest
//!
//! `sha256:` and the lowercase hex SHA-256 over, for each chain file in first-load order, its
//! resolved name, a NUL, its full bytes — front matter included — and a NUL. First-load
//! order is the order rendering reads the files — for the chain's own digest, rendering with
//! every variable at its default — the rendered file first, and a file named by an expression
//! where the render first reads it like any other; the files no render reads, in a branch it
//! did not take, follow in the order the chain names them.
//!
//! No prompting happens here. A caller that prompts asks [`Template::unanswered`] what is
//! left, puts the answers it gathers over the ones it had with [`Answers::overlay`], and
//! renders; the command line is the one caller that does.

mod answers;
mod front_matter;
mod input;
mod members;
mod provenance;
mod scan;
mod schema;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

pub use answers::Answers;
use answers::Given;
pub use front_matter::{DECLARATION_KEYS, FRONT_MATTER_KEYS, is_variable_name};
pub use input::{LoaderDocument, TemplateInput};
pub use members::{MemberDeclaration, MemberTemplate, PreparedMember};
pub use provenance::{Sha256Digest, TemplateProvenance, answers_digest, body_digest};

/// What one variable holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum VariableType {
    /// One line of text.
    String,
    /// Text of any number of lines.
    Text,
    /// A whole number.
    Integer,
    /// `true` or `false`.
    Boolean,
    /// A list, of the variable's `items`.
    List,
    /// A mapping from string keys to values.
    Object,
}

impl VariableType {
    /// Every type, in the order front matter documents them.
    pub const ALL: [Self; 6] = [
        Self::String,
        Self::Text,
        Self::Integer,
        Self::Boolean,
        Self::List,
        Self::Object,
    ];

    /// The type a front matter `type:` spells, if it spells one.
    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    /// How front matter spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Text => "text",
            Self::Integer => "integer",
            Self::Boolean => "boolean",
            Self::List => "list",
            Self::Object => "object",
        }
    }
}

impl fmt::Display for VariableType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What each entry of a `list` variable holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ItemType {
    /// Strings.
    String,
    /// Mappings from string keys to values.
    Object,
}

impl ItemType {
    /// Every item type, in the order front matter documents them.
    pub const ALL: [Self; 2] = [Self::String, Self::Object];

    /// The item type a front matter `items:` spells, if it spells one.
    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|items| items.as_str() == text)
    }

    /// How front matter spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Object => "object",
        }
    }
}

/// One variable of a template's declared set, merged down its chain.
///
/// Built only by loading a template, so `items` is present exactly when `type` is `list`,
/// a `default` is a value of the variable's type, and `required` is never `true` beside one.
// llmlint: ignore-block[invalid_states_unrepresentable] Every field is private and the one constructor is `front_matter::declaration_of`, which refuses a declaration unless those three rules hold; the chain merge only clones what it built. The flat shape is the wire contract C1 fixes and `TemplateVariables` emits (`type`, `items`, `required`, `default` side by side), which a nested enum would change for both SDKs.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TemplateVariable {
    /// The variable's name, as the template body uses it.
    name: String,
    /// What the variable is for; what a prompt shows.
    description: String,
    /// What the variable holds.
    #[serde(rename = "type")]
    kind: VariableType,
    /// What each entry holds, for a `list` variable and no other.
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<ItemType>,
    /// Whether rendering without an answer is refused. An optional variable with no answer
    /// and no default renders as `none`.
    required: bool,
    /// The value used when no answer is given.
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<Value>,
    /// The JSON Schema (draft 2020-12) every answer must satisfy, for an `object` or `list`
    /// variable that declares one; absent for every other.
    #[serde(skip_serializing_if = "Option::is_none")]
    schema: Option<Value>,
    /// The chain file whose declaration this is: the one nearest the rendered template.
    declared_in: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl TemplateVariable {
    /// The variable's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What the variable is for.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// What the variable holds.
    #[must_use]
    pub fn kind(&self) -> VariableType {
        self.kind
    }

    /// What each entry holds, for a `list` variable.
    #[must_use]
    pub fn items(&self) -> Option<ItemType> {
        self.items
    }

    /// Whether rendering without an answer is refused.
    #[must_use]
    pub fn required(&self) -> bool {
        self.required
    }

    /// The value used when no answer is given.
    #[must_use]
    pub fn default(&self) -> Option<&Value> {
        self.default.as_ref()
    }

    /// The JSON Schema every answer must satisfy, when the variable declares one.
    #[must_use]
    pub fn schema(&self) -> Option<&Value> {
        self.schema.as_ref()
    }

    /// Hold `value` — already a value of the variable's type — to its declared schema.
    ///
    /// # Errors
    ///
    /// [`TemplateError::SchemaViolation`] naming the variable and the JSON pointer of the first
    /// violation. A variable with no schema accepts every value of its type.
    pub fn conform(&self, value: &Value) -> Result<(), TemplateError> {
        let Some(declared) = &self.schema else {
            return Ok(());
        };
        schema::conform(declared, value).map_err(|violation| TemplateError::SchemaViolation {
            name: self.name.clone(),
            pointer: violation.pointer,
            problem: violation.problem,
        })
    }

    /// The chain file whose declaration this is.
    #[must_use]
    pub fn declared_in(&self) -> &str {
        &self.declared_in
    }

    /// Whether a value of this variable is written over several lines: `text`, `list` and
    /// `object`.
    #[must_use]
    pub fn multi_line(&self) -> bool {
        matches!(
            self.kind,
            VariableType::Text | VariableType::List | VariableType::Object
        )
    }

    /// Read text as this variable's value: literally for `string` and `text`, as YAML for
    /// every other type — the rule `--var` and a prompt both follow.
    ///
    /// # Errors
    ///
    /// Why the text is not a value of this variable's type.
    pub fn parse(&self, text: &str) -> Result<Value, String> {
        let value = match self.kind {
            VariableType::String | VariableType::Text => Value::String(text.to_owned()),
            _ => crate::yaml::one_value(text)
                .map_err(|error| format!("it is not one YAML value: {error}"))?
                .unwrap_or(Value::Null),
        };
        self.check(&value)?;
        Ok(value)
    }

    /// Whether `value` is a value of this variable's type.
    ///
    /// # Errors
    ///
    /// Why it is not.
    pub fn check(&self, value: &Value) -> Result<(), String> {
        check_value(self.kind, self.items, value)
    }
}

/// Whether `value` is a value of `kind` (with `items`, for a list).
fn check_value(kind: VariableType, items: Option<ItemType>, value: &Value) -> Result<(), String> {
    let fits = match kind {
        VariableType::String => {
            if let Value::String(text) = value
                && text.contains(['\n', '\r'])
            {
                return Err("a `string` is one line; declare the variable `text` for more".into());
            }
            value.is_string()
        }
        VariableType::Text => value.is_string(),
        VariableType::Integer => value.is_i64() || value.is_u64(),
        VariableType::Boolean => value.is_boolean(),
        VariableType::Object => value.is_object(),
        VariableType::List => {
            let Value::Array(entries) = value else {
                return Err(format!("expected a list, got {}", describe(value)));
            };
            let items = items.unwrap_or(ItemType::String);
            if let Some((index, entry)) =
                entries.iter().enumerate().find(|(_, entry)| match items {
                    ItemType::String => !entry.is_string(),
                    ItemType::Object => !entry.is_object(),
                })
            {
                return Err(format!(
                    "entry {index} of the list is {}, and its items are {}s",
                    describe(entry),
                    items.as_str()
                ));
            }
            true
        }
    };
    if fits {
        Ok(())
    } else {
        Err(format!(
            "expected {}, got {}",
            article(kind),
            describe(value)
        ))
    }
}

/// A type with its article, for a message.
fn article(kind: VariableType) -> String {
    match kind {
        VariableType::Integer | VariableType::Object => format!("an {kind}"),
        _ => format!("a {kind}"),
    }
}

/// What kind of JSON value `value` is, for a message.
fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => format!("the boolean {value}"),
        Value::Number(number) => format!("the number {number}"),
        Value::String(text) => format!("the string {text:?}"),
        Value::Array(_) => "a list".to_owned(),
        Value::Object(_) => "a mapping".to_owned(),
    }
}

/// Which field of a declaration two chain files disagree about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainField {
    /// The variable's `type`.
    Type,
    /// A `list` variable's `items`.
    Items,
}

impl fmt::Display for ChainField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Type => "type",
            Self::Items => "items",
        })
    }
}

/// Why a template could not be loaded, answered or rendered.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum TemplateError {
    /// A name or path the chain needs resolved to nothing.
    #[error(
        "template {name:?} was not found{}\nnext: {}",
        referenced_from.as_ref().map(|from| format!(" (named by {from})")).unwrap_or_default(),
        if searched.is_empty() {
            "give the directory holding it as a search path, or register it by name.".to_owned()
        } else {
            format!("it was looked for in {}; add the directory holding it as a search path.", searched.join(", "))
        }
    )]
    NotFound {
        /// The name or path.
        name: String,
        /// The chain file that named it, when one did.
        referenced_from: Option<String>,
        /// The search path directories it was looked for in.
        searched: Vec<String>,
    },
    /// A template that is there and could not be read — a permission, or a directory where a
    /// file was named.
    #[error(
        "template {name} could not be read: {message}\n\
         next: name a readable file, or give it the permissions this process reads with."
    )]
    Unreadable {
        /// The path it was read at.
        name: String,
        /// Why the read failed.
        message: String,
    },
    /// A search path directory that is missing or is not a directory, refused before any
    /// template is looked for, so a mistyped one is not silently searched as empty.
    #[error(
        "search path {directory} cannot be searched: {message}\n\
         next: give a directory that exists, or leave it out of the search path."
    )]
    SearchPath {
        /// The directory as it was given.
        directory: String,
        /// Why it cannot be searched.
        message: String,
    },
    /// A chain file that is not a template this format reads.
    #[error(
        "template {file}: {}{message}\nnext: {}",
        key.as_ref().map(|key| format!("front matter key `{key}` ")).unwrap_or_default(),
        "correct that file; a template's front matter is described in the README under \"Task templates\"."
    )]
    Malformed {
        /// The chain file.
        file: String,
        /// The front matter key the refusal is about, when it is about one.
        key: Option<String>,
        /// What is wrong.
        message: String,
    },
    /// Two chain files declaring one variable with different `type`s or `items`.
    #[error(
        "template variable {variable:?} is declared with {field} {nearer_value} in {nearer} and \
         with {field} {farther_value} in {farther}; a redeclaration may change `description`, \
         `default` and `required`, never `type` or `items`\n\
         next: declare it with the same {field} in both files, or rename one of them."
    )]
    ChainConflict {
        /// The variable.
        variable: String,
        /// Which field differs.
        field: ChainField,
        /// The file nearer the rendered template.
        nearer: String,
        /// What that file declares.
        nearer_value: &'static str,
        /// The file farther from it.
        farther: String,
        /// What that file declares.
        farther_value: &'static str,
    },
    /// An answers document that is not a mapping of answers.
    #[error(
        "the answers document is refused: {message}\n\
         next: write the answers as a YAML mapping from variable name to value."
    )]
    MalformedAnswers {
        /// What is wrong with it.
        message: String,
    },
    /// Answers naming no declared variable.
    #[error(
        "{} no variable the template declares: {}\n\
         next: remove {}, or declare {} in the template's front matter — `onetaskgraph template \
         variables` lists what it declares.",
        if names.len() == 1 { "an answer names" } else { "answers name" },
        names.join(", "),
        if names.len() == 1 { "that answer" } else { "those answers" },
        if names.len() == 1 { "it" } else { "them" }
    )]
    UnknownAnswer {
        /// Every undeclared name answered, in name order.
        names: Vec<String>,
    },
    /// An answer that is not a value of its variable's type.
    #[error(
        "the answer to {name:?} is not {expected}: {problem}\n\
         next: give {name} a value of type {kind}."
    )]
    MistypedAnswer {
        /// The variable.
        name: String,
        /// Its type.
        kind: VariableType,
        /// Its type with an article, for the message.
        expected: String,
        /// Why the answer is not one.
        problem: String,
    },
    /// An answer of its variable's type that the variable's declared `schema` refuses.
    #[error(
        "the answer to {name:?} does not satisfy its schema at JSON pointer {pointer:?}: \
         {problem}\n\
         next: give {name} a value its template's `schema` accepts — `onetaskgraph template \
         variables --json` prints the schema."
    )]
    SchemaViolation {
        /// The variable.
        name: String,
        /// The JSON pointer (RFC 6901) of the first failing value inside the answer: `""` for
        /// the answer as a whole.
        pointer: String,
        /// What the schema says is wrong there.
        problem: String,
    },
    /// Required variables no answer and no default covers.
    #[error(
        "{} unanswered: {}\n\
         next: answer {} with --var NAME=VALUE or in an answers file (--answers FILE), or run \
         interactively to be asked.",
        if names.len() == 1 { "a required variable is" } else { "required variables are" },
        names.join(", "),
        if names.len() == 1 { "it" } else { "each" }
    )]
    MissingRequired {
        /// Every one, in declaration order.
        names: Vec<String>,
    },
    /// The render itself failed: an undefined name, or an error a filter or tag raised.
    #[error(
        "template {}{}: {message}\n\
         next: {}",
        file.as_deref().unwrap_or("?"),
        line.map(|line| format!(" line {line}")).unwrap_or_default(),
        match name {
            Some(name) => format!(
                "declare {name} in the front matter of a file in the chain, or set it in the \
                 template before it is used."
            ),
            None => "correct the template at that line.".to_owned(),
        }
    )]
    Render {
        /// The chain file the render failed in.
        file: Option<String>,
        /// The file's own line, front matter counted.
        line: Option<usize>,
        /// The undefined name, when that is what failed.
        name: Option<String>,
        /// What failed.
        message: String,
    },
    /// One member of a project template's plan graph refused: its strings did not render, its
    /// template could not be found or loaded, or its answers were refused.
    #[error("member {member:?} of the project template: {error}")]
    Member {
        /// The member's id, rendered — or `members[<index>]` when its id is what failed.
        member: String,
        /// Why it was refused.
        error: Box<TemplateError>,
    },
    /// A template loader document that is not one this product reads.
    #[error(
        "the template loader document is refused: {message}\n\
         next: supply a JSON object with a non-empty string `reference`, a non-empty string \
         `entry`, an optional `search_path` of absolute directories, optional `templates` of \
         {{\"name\", \"source\"}} string pairs and an optional string `digest`."
    )]
    MalformedLoader {
        /// What is wrong with it.
        message: String,
    },
    /// A loader document stating a digest its chain does not compute.
    #[error(
        "the template loader document states the digest {stated}, and the chain it names \
         computes {computed}\n\
         next: state the digest of the chain the document names — `onetaskgraph template \
         variables --template-loader` reports it — or leave `digest` out."
    )]
    LoaderDigest {
        /// The digest the document states.
        stated: String,
        /// The digest its chain computes.
        computed: String,
    },
}

impl TemplateError {
    /// This product's kebab-case name for the failure.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "template-not-found",
            Self::Unreadable { .. } => "template-unreadable",
            Self::SearchPath { .. } => "template-search-path",
            Self::Malformed { .. } => "template-malformed",
            Self::ChainConflict { .. } => "template-chain-conflict",
            Self::MalformedAnswers { .. } => "template-answers-malformed",
            Self::UnknownAnswer { .. } => "template-unknown-answer",
            Self::MistypedAnswer { .. } => "template-mistyped-answer",
            Self::SchemaViolation { .. } => "template-schema-violation",
            Self::MissingRequired { .. } => "template-missing-required",
            Self::Render { .. } => "template-render",
            Self::MalformedLoader { .. } => "template-loader-malformed",
            Self::Member { error, .. } => error.kind(),
            Self::LoaderDigest { .. } => "template-loader-digest",
        }
    }

    /// Whether this refuses the answers rather than the template: an answers document that
    /// is not one, an answer to no declared variable or of the wrong type, and required
    /// variables left unanswered. The command line exits `2` for these.
    #[must_use]
    pub fn refuses_answers(&self) -> bool {
        if let Self::Member { error, .. } = self {
            return error.refuses_answers();
        }
        matches!(
            self,
            Self::MalformedAnswers { .. }
                | Self::UnknownAnswer { .. }
                | Self::MistypedAnswer { .. }
                | Self::SchemaViolation { .. }
                | Self::MissingRequired { .. }
        )
    }

    fn malformed(file: &str, key: Option<&str>, message: impl Into<String>) -> Self {
        Self::Malformed {
            file: file.to_owned(),
            key: key.map(str::to_owned),
            message: message.into(),
        }
    }
}

/// Where a template and the templates it names are found.
///
/// Directories are searched first, in the order given, then the registered pairs — which
/// is how a Rust consumer supplies templates embedded in its own binary. The working
/// directory is never searched unless it is given.
#[derive(Debug, Clone, Default)]
pub struct TemplateLoader {
    directories: Vec<PathBuf>,
    registered: Vec<(String, String)>,
}

impl TemplateLoader {
    /// A loader with an empty search path.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `directory` to the end of the search path's directories.
    #[must_use]
    pub fn with_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.directories.push(directory.into());
        self
    }

    /// Register a template's source under `name`, searched after every directory.
    #[must_use]
    pub fn with_template(mut self, name: impl Into<String>, source: impl Into<String>) -> Self {
        self.registered.push((name.into(), source.into()));
        self
    }

    /// Load the template at `path` and every template its chain names.
    ///
    /// The file is known to its chain — and in its digest — by its file name.
    ///
    /// # Errors
    ///
    /// [`TemplateError::NotFound`] for a file that cannot be read or a chain name nothing
    /// resolves; [`TemplateError::Malformed`] for a chain file that is not UTF-8, whose front
    /// matter is refused, or whose body does not parse; [`TemplateError::ChainConflict`] for
    /// a variable two chain files type differently; [`TemplateError::SearchPath`] for a
    /// search path directory that is missing or is not a directory.
    pub fn load_path(&self, path: &Path) -> Result<Template, TemplateError> {
        self.searchable()?;
        let shown = path.display().to_string();
        let bytes = std::fs::read(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                TemplateError::NotFound {
                    name: shown.clone(),
                    referenced_from: None,
                    searched: Vec::new(),
                }
            } else {
                TemplateError::Unreadable {
                    name: shown.clone(),
                    message: error.to_string(),
                }
            }
        })?;
        let source = String::from_utf8(bytes)
            .map_err(|_| TemplateError::malformed(&shown, None, "it is not UTF-8 text"))?;
        let name = path
            .file_name()
            .map_or_else(|| shown.clone(), |name| name.to_string_lossy().into_owned());
        self.load_chain(name, source)
    }

    /// Load the template the search path resolves `name` to, and every template its chain
    /// names.
    ///
    /// # Errors
    ///
    /// As [`TemplateLoader::load_path`], and [`TemplateError::NotFound`] when the search
    /// path resolves `name` to nothing.
    pub fn load_name(&self, name: &str) -> Result<Template, TemplateError> {
        self.searchable()?;
        let source = self.find(name)?.ok_or_else(|| self.not_found(name, None))?;
        self.load_chain(name.to_owned(), source)
    }

    /// The source the search path resolves `name` to, or `None`.
    ///
    /// A name spelled to climb out of the search path (`..`), or an absolute one, is looked
    /// for among the registered pairs alone, so a template cannot name its way to a file
    /// outside the directories it was given. A symbolic link inside one of those directories
    /// is followed like any other file there: the check is on how the name is spelled, and
    /// whoever put the link in a search directory chose what it reaches.
    fn find(&self, name: &str) -> Result<Option<String>, TemplateError> {
        let path = Path::new(name);
        let spelled_within = !name.is_empty()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_)));
        if spelled_within {
            for directory in &self.directories {
                let candidate = directory.join(path);
                if candidate.is_file() {
                    let bytes =
                        std::fs::read(&candidate).map_err(|error| TemplateError::Unreadable {
                            name: candidate.display().to_string(),
                            message: error.to_string(),
                        })?;
                    return String::from_utf8(bytes)
                        .map(Some)
                        .map_err(|_| TemplateError::malformed(name, None, "it is not UTF-8 text"));
                }
            }
        }
        Ok(self
            .registered
            .iter()
            .find(|(registered, _)| registered == name)
            .map(|(_, source)| source.clone()))
    }

    fn not_found(&self, name: &str, referenced_from: Option<&str>) -> TemplateError {
        TemplateError::NotFound {
            name: name.to_owned(),
            referenced_from: referenced_from.map(str::to_owned),
            searched: self
                .directories
                .iter()
                .map(|directory| directory.display().to_string())
                .collect(),
        }
    }

    /// Read the whole chain from its root, depth first, put it in first-load order and merge
    /// its declarations.
    ///
    /// First-load order is the order a render with every variable at its default reads the
    /// files. A file that render cannot read, found by an expression, is not this chain's
    /// yet: the answers that name it reach it through [`Template::expand`], which refuses it.
    fn load_chain(&self, root: String, source: String) -> Result<Template, TemplateError> {
        let resolutions = Resolutions::new();
        let files = self.build(root.clone(), source, &resolutions)?;
        members_at_root(&root, &files)?;
        let variables = merge(&files)?;
        let Discovered { read, .. } = discover(&root, &files, &variables, self, &Answers::new());
        let files = in_read_order(files, &read);
        let variables = merge(&files)?;
        Ok(Template::assemble(
            root,
            files,
            variables,
            resolutions,
            self.clone(),
        ))
    }

    /// Refuse a search path directory that is missing or is not a directory.
    fn searchable(&self) -> Result<(), TemplateError> {
        for directory in &self.directories {
            let refused = |message: String| TemplateError::SearchPath {
                directory: directory.display().to_string(),
                message,
            };
            match std::fs::metadata(directory) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => return Err(refused("it is not a directory".to_owned())),
                Err(error) => return Err(refused(error.to_string())),
            }
        }
        Ok(())
    }

    /// Read the chain from `name`, whose source is `source`: that file, then every file it
    /// names, depth first in the order its tags name them, skipping any already read. A tag
    /// naming its template by a literal names what it spells; one naming it by an expression
    /// names what `resolutions` records it evaluating to, in the order it was reached. That
    /// is only the order the chain is found in: its callers put it in first-load order before
    /// a digest or a nearness tie is taken over it.
    fn build(
        &self,
        name: String,
        source: String,
        resolutions: &Resolutions,
    ) -> Result<Vec<ChainFile>, TemplateError> {
        let mut files = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut pending = vec![(name, source)];
        // A stack, pushed in reverse, so the first name a file spells is the next one read.
        while let Some((name, source)) = pending.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let split = front_matter::split(&name, &source)?;
            compile_check(&name, &split.body, split.offset_lines)?;
            let mut names = Vec::new();
            let mut children = Vec::new();
            for named in scan::named(&split.body) {
                let references = match named {
                    scan::Named::Literal(reference) => vec![reference],
                    // What the render named there resolves or fails in the render itself.
                    scan::Named::Expression { ordinal, .. } => resolutions
                        .get(&NamingTag {
                            file: name.clone(),
                            ordinal,
                        })
                        .into_iter()
                        .flatten()
                        .map(|candidates| scan::Reference {
                            candidates: candidates.names().to_vec(),
                            optional: true,
                        })
                        .collect(),
                };
                for reference in references {
                    // The first candidate that resolves is the one the render loads, so it is
                    // the one that belongs to the chain; the rest are not read.
                    let mut resolved = false;
                    for candidate in &reference.candidates {
                        if seen.contains(candidate) || *candidate == name {
                            names.push(candidate.clone());
                            resolved = true;
                            break;
                        }
                        if let Some(source) = self.find(candidate)? {
                            names.push(candidate.clone());
                            children.push((candidate.clone(), source));
                            resolved = true;
                            break;
                        }
                    }
                    if !resolved && !reference.optional {
                        return Err(self.not_found(&reference.candidates.join(" or "), Some(&name)));
                    }
                }
            }
            pending.extend(children.into_iter().rev());
            files.push(ChainFile {
                name,
                source,
                body: split.body,
                offset_lines: split.offset_lines,
                declarations: split.declarations,
                members: split.members,
                names,
            });
        }
        Ok(files)
    }
}

/// A tag naming a template by an expression: the chain file it is written in, and which of
/// that file's such tags it is, counting from zero.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NamingTag {
    file: String,
    ordinal: usize,
}

/// What one evaluation of a naming expression gave: a name, or a list of them of which the
/// first that resolves is loaded. Never empty.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidates(Vec<String>);

impl Candidates {
    /// The names `value` gives, or `None` when it gives none — it is neither a string nor a
    /// sequence holding one.
    fn of(value: &minijinja::Value) -> Option<Self> {
        let names: Vec<String> = match value.as_str() {
            Some(name) => vec![name.to_owned()],
            None if value.kind() == minijinja::value::ValueKind::Seq => value
                .try_iter()
                .into_iter()
                .flatten()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect(),
            None => Vec::new(),
        };
        (!names.is_empty()).then_some(Self(names))
    }

    fn names(&self) -> &[String] {
        &self.0
    }
}

/// What each naming tag was found to name in one render: every distinct value it gave, in the
/// order the render first reached it.
type Resolutions = BTreeMap<NamingTag, Vec<Candidates>>;

/// The body of each of `files` as [`discover`] renders it: with every expression that names a
/// template wrapped in [`scan::HOOK`], so the render reports what it named.
fn hooked_bodies(files: &[ChainFile]) -> HashMap<String, String> {
    files
        .iter()
        .enumerate()
        .map(|(index, file)| (file.name.clone(), scan::hooked(&file.body, index)))
        .collect()
}

/// Give `environment` the function [`hooked_bodies`] calls, and answer where what it
/// records collects.
fn record_names(
    environment: &mut minijinja::Environment<'static>,
    files: &[ChainFile],
) -> Arc<Mutex<Resolutions>> {
    let names: Vec<String> = files.iter().map(|file| file.name.clone()).collect();
    let recorded: Arc<Mutex<Resolutions>> = Arc::default();
    let sink = Arc::clone(&recorded);
    environment.add_function(
        scan::HOOK,
        move |file: usize, ordinal: usize, value: minijinja::Value| {
            if let Some(name) = names.get(file)
                && let Some(candidates) = Candidates::of(&value)
            {
                let mut recorded = sink.lock().unwrap_or_else(PoisonError::into_inner);
                let found = recorded
                    .entry(NamingTag {
                        file: name.clone(),
                        ordinal,
                    })
                    .or_default();
                if !found.contains(&candidates) {
                    found.push(candidates);
                }
            }
            value
        },
    );
    recorded
}

/// Refuse a body that does not parse, before anything renders it.
fn compile_check(name: &str, body: &str, offset_lines: usize) -> Result<(), TemplateError> {
    let environment = environment();
    environment
        .template_from_named_str(name, body)
        .map(|_| ())
        .map_err(|error| TemplateError::Malformed {
            file: name.to_owned(),
            key: None,
            message: format!(
                "its body does not parse{}: {}",
                error
                    .line()
                    .map(|line| format!(" at line {}", line + offset_lines))
                    .unwrap_or_default(),
                error.detail().unwrap_or("a syntax error")
            ),
        })
}

/// Refuse `members` declared by any file of the chain but the rendered one, `root`: a plan
/// graph is the project template's own, never one a file it names brings with it.
fn members_at_root(root: &str, files: &[ChainFile]) -> Result<(), TemplateError> {
    match files.iter().skip(1).find(|file| !file.members.is_empty()) {
        Some(named) => Err(TemplateError::malformed(
            &named.name,
            Some("members"),
            format!(
                "is declared in a template {root} names; only the rendered project template \
                 declares members"
            ),
        )),
        None => Ok(()),
    }
}

/// One file of a loaded chain.
#[derive(Debug, Clone)]
struct ChainFile {
    name: String,
    source: String,
    body: String,
    offset_lines: usize,
    declarations: Vec<TemplateVariable>,
    /// The plan-graph members its front matter declares: only the rendered file may.
    members: Vec<MemberDeclaration>,
    /// The chain files its tags name, in the order they name them: what nearness is measured
    /// over.
    names: Vec<String>,
}

/// Merge every chain file's declarations into the declared set.
///
/// Nearness is the fewest references from the rendered file, ties going to the file earlier
/// in `files`; the order is theirs, each variable where it was first declared.
fn merge(files: &[ChainFile]) -> Result<Vec<TemplateVariable>, TemplateError> {
    let index_of: HashMap<&str, usize> = files
        .iter()
        .enumerate()
        .map(|(index, file)| (file.name.as_str(), index))
        .collect();
    let mut depth = vec![usize::MAX; files.len()];
    if !files.is_empty() {
        depth[0] = 0;
    }
    let mut queue = VecDeque::from([0usize]);
    while let Some(at) = queue.pop_front() {
        for child in &files[at].names {
            if let Some(&child) = index_of.get(child.as_str())
                && depth[child] == usize::MAX
            {
                depth[child] = depth[at] + 1;
                queue.push_back(child);
            }
        }
    }
    let mut nearness: Vec<usize> = (0..files.len()).collect();
    nearness.sort_by_key(|&index| (depth[index], index));

    let mut merged: BTreeMap<&str, TemplateVariable> = BTreeMap::new();
    for &index in &nearness {
        let file = &files[index];
        for declaration in &file.declarations {
            match merged.get(declaration.name.as_str()) {
                None => {
                    merged.insert(&declaration.name, declaration.clone());
                }
                Some(nearer) => {
                    if nearer.kind != declaration.kind {
                        return Err(conflict(
                            nearer,
                            ChainField::Type,
                            nearer.kind.as_str(),
                            &file.name,
                            declaration.kind.as_str(),
                            &declaration.name,
                        ));
                    }
                    if nearer.items != declaration.items {
                        let spell =
                            |items: Option<ItemType>| items.map_or("none", ItemType::as_str);
                        return Err(conflict(
                            nearer,
                            ChainField::Items,
                            spell(nearer.items),
                            &file.name,
                            spell(declaration.items),
                            &declaration.name,
                        ));
                    }
                }
            }
        }
    }

    let mut ordered = Vec::with_capacity(merged.len());
    for file in files {
        for declaration in &file.declarations {
            if let Some(variable) = merged.remove(declaration.name.as_str()) {
                ordered.push(variable);
            }
        }
    }
    Ok(ordered)
}

fn conflict(
    nearer: &TemplateVariable,
    field: ChainField,
    nearer_value: &'static str,
    farther: &str,
    farther_value: &'static str,
    variable: &str,
) -> TemplateError {
    TemplateError::ChainConflict {
        variable: variable.to_owned(),
        field,
        nearer: nearer.declared_in.clone(),
        nearer_value,
        farther: farther.to_owned(),
        farther_value,
    }
}

/// The digest over `files`, each a resolved name and its full source, in first-load order.
fn digest<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut hasher = Sha256::new();
    for (name, source) in files {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(source.as_bytes());
        hasher.update([0]);
    }
    let hash = hasher.finalize();
    let mut rendered = String::with_capacity(7 + hash.len() * 2);
    rendered.push_str("sha256:");
    for byte in hash {
        rendered.push_str(&format!("{byte:02x}"));
    }
    rendered
}

/// The renderer every template is compiled and rendered in.
fn environment() -> minijinja::Environment<'static> {
    let mut environment = minijinja::Environment::new();
    environment.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    environment.set_auto_escape_callback(|_| minijinja::AutoEscape::None);
    environment.set_keep_trailing_newline(true);
    environment.set_trim_blocks(true);
    environment.set_lstrip_blocks(true);
    environment
}

/// A loaded template: its chain, its declared set and its digest.
#[derive(Debug, Clone)]
pub struct Template {
    name: String,
    files: Vec<ChainFile>,
    variables: Vec<TemplateVariable>,
    /// What the tags naming a template by an expression were found to name.
    resolutions: Resolutions,
    digest: String,
    loader: TemplateLoader,
}

/// What `onetaskgraph template variables` answers with: a template's declared set.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct TemplateVariables {
    /// The rendered template's resolved name.
    pub template: String,
    /// The chain's digest: `sha256:` and 64 lowercase hex digits, over every file it reads
    /// in first-load order — each template an expression names counted as it names one when
    /// every variable takes its default.
    pub digest: String,
    /// Every declared variable, in declaration order along the chain.
    pub variables: Vec<TemplateVariable>,
}

/// A rendered template: what `onetaskgraph template render` answers with.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RenderedTemplate {
    /// The rendered text.
    pub body: String,
    /// The digest of every file of the chain these answers render, in first-load order:
    /// `sha256:` and 64 lowercase hex digits. That is the order the render first read each
    /// file, a template an expression named included, then any file of the chain it did not
    /// read.
    pub digest: String,
    /// Every declared variable and the value it rendered with — an answer, a default, or
    /// `null` for an optional variable given neither.
    pub answers: BTreeMap<String, Value>,
}

impl Template {
    fn assemble(
        name: String,
        files: Vec<ChainFile>,
        variables: Vec<TemplateVariable>,
        resolutions: Resolutions,
        loader: TemplateLoader,
    ) -> Self {
        let digest = digest(
            files
                .iter()
                .map(|file| (file.name.as_str(), file.source.as_str())),
        );
        Self {
            name,
            files,
            variables,
            resolutions,
            digest,
            loader,
        }
    }

    /// This template with every template an expression names for `answers` added to its
    /// chain — each with the files its own literals name — and their front matter merged into
    /// the declared set under the same rules as the rest of the chain.
    ///
    /// Which file an expression names can depend on the answers, so it is found by rendering:
    /// leniently, from the answers that fit their declarations and the defaults of the rest,
    /// every tag naming a template by an expression reporting what it named. A file found that
    /// way may declare a variable, or a nearer default, that decides what an expression
    /// names, so it repeats until a render names what the one before it did; a file only an
    /// earlier render named is not part of the chain. Each file so found joins the chain at
    /// the tag that names it, exactly as a file a literal names would: its nearness is its own
    /// distance from the rendered template. The expanded chain is in first-load order — the
    /// order that last render first read each file, then any it did not read. A template
    /// named by no expression expands to its own files in that order.
    ///
    /// [`Template::unanswered`], [`Template::resolve`] and [`Template::render`] each expand
    /// first, so a caller need not.
    ///
    /// # Errors
    ///
    /// [`TemplateError::Malformed`], [`TemplateError::Unreadable`] and
    /// [`TemplateError::NotFound`] for a file found this way, or one its literals name, as for
    /// any chain file; [`TemplateError::ChainConflict`] for a declaration of it whose `type`
    /// or `items` another chain file's contradicts; and [`TemplateError::Malformed`] for the
    /// rendered template when the files its expressions name give defaults that name one
    /// another in turn, so that what they name never settles.
    pub fn expand(&self, answers: &Answers) -> Result<Self, TemplateError> {
        let mut expanded = self.clone();
        let mut named = vec![expanded.resolutions.clone()];
        loop {
            let Discovered {
                resolutions,
                read,
                failure,
            } = discover(
                &expanded.name,
                &expanded.files,
                &expanded.variables,
                &expanded.loader,
                answers,
            );
            if let Some(error) = failure {
                return Err(error);
            }
            if resolutions == expanded.resolutions {
                let files = in_read_order(expanded.files, &read);
                let variables = merge(&files)?;
                return Ok(Self::assemble(
                    self.name.clone(),
                    files,
                    variables,
                    resolutions,
                    self.loader.clone(),
                ));
            }
            if named.contains(&resolutions) {
                return Err(TemplateError::malformed(
                    &self.name,
                    None,
                    "what its expressions name never settles: each template they name gives a \
                     default naming another, round a cycle; answer the variable that decides it",
                ));
            }
            named.push(resolutions.clone());
            let files = expanded.rebuild(&resolutions)?;
            members_at_root(&self.name, &files)?;
            let variables = merge(&files)?;
            expanded = Self::assemble(
                self.name.clone(),
                files,
                variables,
                resolutions,
                self.loader.clone(),
            );
        }
    }

    fn rebuild(&self, resolutions: &Resolutions) -> Result<Vec<ChainFile>, TemplateError> {
        let root = self
            .files
            .first()
            .map_or_else(String::new, |file| file.source.clone());
        self.loader.build(self.name.clone(), root, resolutions)
    }

    /// The rendered template's resolved name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The chain's digest.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The declared set, in declaration order along the chain.
    #[must_use]
    pub fn variables(&self) -> &[TemplateVariable] {
        &self.variables
    }

    /// The resolved name of every file of the chain, in first-load order: the files
    /// [`Template::digest`] is taken over, in its order. For a loaded template these are the
    /// rendered one and each `extends`, `include` and `import` its literals reach; what
    /// [`Template::expand`] answers adds each template an expression names.
    pub fn chain(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(|file| file.name.as_str())
    }

    /// The plan-graph members the rendered template declares, as written: none for a template
    /// that declares none, which is every task and document template.
    #[must_use]
    pub fn members(&self) -> &[MemberDeclaration] {
        self.files
            .first()
            .map_or(&[], |file| file.members.as_slice())
    }

    /// Render every member from `answers` — this template's resolved answers, as a rendering
    /// of it answers them — and each member's body from its own template, looking for a
    /// relative member template in `base`; see [`members`](self::members).
    ///
    /// # Errors
    ///
    /// [`TemplateError::Malformed`] for a member graph that does not hold together, and
    /// [`TemplateError::Member`] naming the member for every refusal of one.
    pub fn prepare_members(
        &self,
        answers: &BTreeMap<String, Value>,
        base: &[PathBuf],
    ) -> Result<Vec<PreparedMember>, TemplateError> {
        members::prepare(&self.name, self.members(), answers, base)
    }

    /// The declared set as `template variables` reports it.
    #[must_use]
    pub fn describe(&self) -> TemplateVariables {
        TemplateVariables {
            template: self.name.clone(),
            digest: self.digest.clone(),
            variables: self.variables.clone(),
        }
    }

    /// The declared variables `answers` leaves unanswered, in declaration order — whether or
    /// not a default covers them — over the chain [`Template::expand`] reaches for `answers`.
    /// What a prompting caller asks for; answering one can name a further template, so a
    /// caller asks again until nothing new is left.
    ///
    /// # Errors
    ///
    /// [`TemplateError::UnknownAnswer`] and [`TemplateError::MistypedAnswer`], as
    /// [`Template::render`] refuses them.
    pub fn unanswered(&self, answers: &Answers) -> Result<Vec<TemplateVariable>, TemplateError> {
        let expanded = self.expand(answers)?;
        let typed = expanded.typed(answers)?;
        Ok(expanded
            .variables
            .into_iter()
            .filter(|variable| !typed.contains_key(&variable.name))
            .collect())
    }

    /// Every declared variable's value — over the chain [`Template::expand`] reaches for
    /// `answers` — its answer, else its default, else `null` for an optional one.
    ///
    /// # Errors
    ///
    /// [`TemplateError::UnknownAnswer`] naming every answer to no declared variable,
    /// [`TemplateError::MistypedAnswer`] for an answer not of its variable's type, and
    /// [`TemplateError::MissingRequired`] naming every required variable left unanswered.
    pub fn resolve(&self, answers: &Answers) -> Result<BTreeMap<String, Value>, TemplateError> {
        self.expand(answers)?.resolve_here(answers)
    }

    /// [`Template::resolve`] over this chain as it stands.
    fn resolve_here(&self, answers: &Answers) -> Result<BTreeMap<String, Value>, TemplateError> {
        let mut typed = self.typed(answers)?;
        let missing: Vec<String> = self
            .variables
            .iter()
            .filter(|variable| {
                variable.required
                    && variable.default.is_none()
                    && !typed.contains_key(&variable.name)
            })
            .map(|variable| variable.name.clone())
            .collect();
        if !missing.is_empty() {
            return Err(TemplateError::MissingRequired { names: missing });
        }
        for variable in &self.variables {
            typed
                .entry(variable.name.clone())
                .or_insert_with(|| variable.default.clone().unwrap_or(Value::Null));
        }
        Ok(typed)
    }

    /// Render the template from `answers`, over the chain [`Template::expand`] reaches for
    /// them: a template an expression names is part of the chain, its declarations part of
    /// the declared set the answers are held to, and its bytes part of the digest.
    ///
    /// # Errors
    ///
    /// Every refusal of [`Template::expand`] and [`Template::resolve`], and
    /// [`TemplateError::Render`] for a render that fails — an undefined name among them.
    pub fn render(&self, answers: &Answers) -> Result<RenderedTemplate, TemplateError> {
        self.expand(answers)?.render_here(answers)
    }

    /// [`Template::render`] over this chain as it stands.
    ///
    /// The render reads only the files of this chain. [`Template::expand`] found them by
    /// rendering from exactly the values this render is given, so a file it names that the
    /// chain does not hold is one that was not there to find, and is not found here either.
    fn render_here(&self, answers: &Answers) -> Result<RenderedTemplate, TemplateError> {
        let resolved = self.resolve_here(answers)?;

        let offsets: HashMap<String, usize> = self
            .files
            .iter()
            .map(|file| (file.name.clone(), file.offset_lines))
            .collect();
        let bodies: HashMap<String, String> = self
            .files
            .iter()
            .map(|file| (file.name.clone(), file.body.clone()))
            .collect();
        let mut environment = environment();
        environment.set_loader(move |name| Ok(bodies.get(name).cloned()));
        let rendered = environment
            .get_template(&self.name)
            .and_then(|template| template.render(&resolved))
            .map_err(|error| render_error(&error, &offsets))?;
        Ok(RenderedTemplate {
            body: rendered,
            digest: self.digest.clone(),
            answers: resolved,
        })
    }

    /// Check `answers` against the declared set and type each one.
    fn typed(&self, answers: &Answers) -> Result<BTreeMap<String, Value>, TemplateError> {
        let declared: HashMap<&str, &TemplateVariable> = self
            .variables
            .iter()
            .map(|variable| (variable.name.as_str(), variable))
            .collect();
        let unknown: Vec<String> = answers
            .names()
            .filter(|name| !declared.contains_key(name))
            .map(str::to_owned)
            .collect();
        if !unknown.is_empty() {
            return Err(TemplateError::UnknownAnswer { names: unknown });
        }

        let mut typed = BTreeMap::new();
        for variable in &self.variables {
            let value = match answers.given(&variable.name) {
                None => continue,
                Some(Given::Value(value)) => variable.check(value).map(|()| value.clone()),
                Some(Given::Text(text)) => variable.parse(text),
            };
            let value = value.map_err(|problem| TemplateError::MistypedAnswer {
                name: variable.name.clone(),
                kind: variable.kind,
                expected: article(variable.kind),
                problem,
            })?;
            variable.conform(&value)?;
            typed.insert(variable.name.clone(), value);
        }
        Ok(typed)
    }
}

/// What a lenient render of `files` reads: what every tag naming a template by an
/// expression named, in the order the render reached them, and the order it first read each
/// chain file.
///
/// The render is only a way to learn what an expression names: its output is dropped, and
/// so is any failure of the render itself — a strict render reports those. A file it asks
/// for that is there and cannot be read is that file's failure, and is answered as one for
/// the caller to raise.
/// Each variable takes the answer given for it when that is one of its type, else its
/// default, else `none` for an optional one — as the strict render has it; a required one
/// with neither is left undefined, which a lenient render reads as empty.
fn discover(
    root: &str,
    files: &[ChainFile],
    variables: &[TemplateVariable],
    loader: &TemplateLoader,
    answers: &Answers,
) -> Discovered {
    let mut context = BTreeMap::new();
    for variable in variables {
        let answered = match answers.given(&variable.name) {
            Some(Given::Value(value)) => variable.check(value).ok().map(|()| value.clone()),
            Some(Given::Text(text)) => variable.parse(text).ok(),
            None => None,
        };
        if let Some(value) = answered.or_else(|| variable.default.clone()) {
            context.insert(variable.name.clone(), value);
        } else if !variable.required {
            context.insert(variable.name.clone(), Value::Null);
        }
    }

    let bodies = hooked_bodies(files);
    let loader = loader.clone();
    let failed: Arc<Mutex<Option<TemplateError>>> = Arc::default();
    let failure = Arc::clone(&failed);
    let read: Arc<Mutex<Vec<String>>> = Arc::default();
    let reading = Arc::clone(&read);
    let mut environment = environment();
    environment.set_undefined_behavior(minijinja::UndefinedBehavior::Lenient);
    let recorded = record_names(&mut environment, files);
    environment.set_loader(move |name| {
        let mut reading = reading.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(body) = bodies.get(name) {
            if !reading.iter().any(|read| read == name) {
                reading.push(name.to_owned());
            }
            return Ok(Some(body.clone()));
        }
        let source = match loader.find(name) {
            Ok(Some(source)) => source,
            Ok(None) => return Ok(None),
            Err(error) => {
                failure
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_or_insert(error);
                return Ok(None);
            }
        };
        Ok(Some(
            front_matter::split(name, &source)
                .map(|split| split.body)
                .unwrap_or_default(),
        ))
    });
    let _ = environment
        .get_template(root)
        .and_then(|template| template.render(&context));
    let failure = failed.lock().unwrap_or_else(PoisonError::into_inner).take();
    let resolutions = recorded
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let read = read.lock().unwrap_or_else(PoisonError::into_inner).clone();
    Discovered {
        resolutions,
        read,
        failure,
    }
}

/// What a lenient render of a chain found: what its naming expressions named, which of the
/// chain's files it read, in the order it first read each, and the first file it asked for
/// that is there and could not be read.
struct Discovered {
    resolutions: Resolutions,
    read: Vec<String>,
    failure: Option<TemplateError>,
}

/// `files` in first-load order: the root, then every file a render of them read in the order
/// it first read it, then the files no render reaches — an untaken branch's — in the order
/// the chain names them.
fn in_read_order(files: Vec<ChainFile>, read: &[String]) -> Vec<ChainFile> {
    let mut files: Vec<(usize, ChainFile)> = files
        .into_iter()
        .enumerate()
        .map(|(index, file)| {
            let at = if index == 0 {
                0
            } else {
                read.iter()
                    .position(|name| *name == file.name)
                    .map_or(read.len() + index, |at| at + 1)
            };
            (at, file)
        })
        .collect();
    files.sort_by_key(|(at, _)| *at);
    files.into_iter().map(|(_, file)| file).collect()
}

/// A minijinja failure as the template failure it is, located in the file's own lines.
///
/// A failure inside an included or imported file reaches here wrapped in one per template
/// it passed through; the innermost is where it happened.
fn render_error(error: &minijinja::Error, offsets: &HashMap<String, usize>) -> TemplateError {
    let mut error = error;
    while let Some(inner) = std::error::Error::source(error)
        .and_then(|source| source.downcast_ref::<minijinja::Error>())
    {
        error = inner;
    }
    let file = error.name().map(str::to_owned);
    let offset = file
        .as_deref()
        .and_then(|file| offsets.get(file))
        .copied()
        .unwrap_or(0);
    let line = error.line().map(|line| line + offset);
    if error.kind() == minijinja::ErrorKind::UndefinedError {
        let name = error
            .template_source()
            .zip(error.range())
            .and_then(|(source, range)| source.get(range))
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        return TemplateError::Render {
            message: match &name {
                Some(name) => format!(
                    "{name} is undefined: it is neither a declared variable nor set by the template"
                ),
                None => "a value there is undefined".to_owned(),
            },
            file,
            line,
            name,
        };
    }
    TemplateError::Render {
        file,
        line,
        name: None,
        message: error
            .detail()
            .map_or_else(|| error.kind().to_string(), str::to_owned),
    }
}
