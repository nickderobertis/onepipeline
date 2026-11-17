//! Layers, and the pure merge that turns a stack of them into one configuration.
//!
//! Every layer — a document, the environment, the command line — is reduced to the
//! same thing: a list of *leaf* settings, each a dotted path, a value, and the
//! [`Origin`] it came from. Precedence is then one rule applied once, rather than a
//! per-verb `if flag.is_some()` at every call site, and "which layer did this come
//! from" is an answer the merge already holds instead of one reconstructed later.
//!
//! Nothing here touches the filesystem or the environment: a layer arrives as text
//! or as pairs, and [`merge`] is a function of its arguments. Reading is
//! [`super::discovery`]'s job and nothing else's.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value};

use super::ConfigError;

/// Where one setting's value came from.
///
/// This is what makes precedence provable rather than asserted: `config show`
/// renders it per setting, so a user sees the same answer a test does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "layer", rename_all = "kebab-case")]
pub enum Origin {
    /// Nothing set it; this is the built-in value.
    Default,
    /// A configuration document, and which one.
    File {
        /// The document that set it.
        path: PathBuf,
    },
    /// The process environment, and which variable.
    Environment {
        /// The variable that set it.
        variable: String,
    },
    /// The command line, and which flag.
    Flag {
        /// The flag that set it, as a user typed it.
        flag: String,
    },
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("default"),
            Self::File { path } => write!(f, "file {}", path.display()),
            Self::Environment { variable } => write!(f, "environment {variable}"),
            Self::Flag { flag } => write!(f, "flag {flag}"),
        }
    }
}

/// A dotted path to one setting, such as `sources.work.config.root`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[serde(into = "String")]
// `serde(into)` is not one of the attributes schemars reads, so without this the
// generated schema would describe the segments this holds rather than the dotted
// string it serializes as — a bundle that misdescribes the output the binary emits.
#[schemars(with = "String")]
pub struct SettingPath(Vec<String>);

impl SettingPath {
    /// Build a path from segments, refusing an empty one.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Setting`] when there are no segments or one is empty —
    /// `--set .plugin=x` and `--set a..b=x` both address nothing.
    pub fn new(segments: Vec<String>, source: &str) -> Result<Self, ConfigError> {
        if segments.is_empty() || segments.iter().any(String::is_empty) {
            return Err(ConfigError::setting(
                source,
                "that is not a setting path; a path is one or more dot-separated names, \
                 none of them empty",
                "write it as a dotted path, for example `page_size` or \
                 `sources.work.config.root`.",
            ));
        }
        Ok(Self(segments))
    }

    /// Parse a dotted path such as `sources.work.config.root`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Setting`] when `dotted` has an empty segment.
    pub fn parse(dotted: &str) -> Result<Self, ConfigError> {
        Self::new(dotted.split('.').map(str::to_owned).collect(), dotted)
    }

    /// The segments, outermost first.
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.0
    }

    /// Whether `self` is `other`, or is an ancestor or descendant of it.
    ///
    /// Two settings that overlap this way cannot both survive a merge: one of them
    /// addresses a subtree the other addresses as a whole.
    fn overlaps(&self, other: &Self) -> bool {
        let shared = self.0.len().min(other.0.len());
        self.0[..shared] == other.0[..shared]
    }
}

impl fmt::Display for SettingPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join("."))
    }
}

impl From<SettingPath> for String {
    fn from(value: SettingPath) -> Self {
        value.to_string()
    }
}

/// One setting: what it is called, what it is set to, and where that came from.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Setting {
    /// The dotted path of the setting.
    pub key: SettingPath,
    /// Its value.
    pub value: Value,
    /// Where the value came from.
    pub origin: Origin,
}

/// One layer of configuration, flattened to its leaf settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layer {
    settings: Vec<Setting>,
}

impl Layer {
    /// A layer holding exactly `settings`, in the order given.
    #[must_use]
    pub fn new(settings: Vec<Setting>) -> Self {
        Self { settings }
    }

    /// The settings this layer carries.
    #[must_use]
    pub fn settings(&self) -> &[Setting] {
        &self.settings
    }

    /// Flatten one parsed document into a layer attributed to `path`.
    ///
    /// An empty document contributes nothing, which is what a file holding only
    /// comments should do.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Setting`] when the document's root is not a mapping.
    /// A document that is a list or a bare scalar sets no setting at all, and
    /// accepting it silently would make a whole misplaced file read as "unset".
    pub fn from_document(path: PathBuf, document: &Value) -> Result<Self, ConfigError> {
        let origin = Origin::File { path };
        let fields = match document {
            Value::Null => return Ok(Self::default()),
            Value::Object(fields) => fields,
            other => {
                return Err(ConfigError::setting(
                    "the document's root",
                    format!(
                        "a configuration document must be a mapping of settings, but {origin} \
                         holds {}",
                        kind_of(other)
                    ),
                    "write the document as `key: value` pairs — see `onetaskgraph config show \
                     --help` for the settings it may hold.",
                ));
            }
        };

        let mut settings = Vec::new();
        flatten(&mut Vec::new(), fields, &origin, &mut settings);
        Ok(Self { settings })
    }
}

/// What a value is, for a message a user reads.
fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        // Nothing at all and a mapping are both answered before this is reached.
        _ => "a list",
    }
}

/// Walk `fields`, emitting one [`Setting`] per leaf.
///
/// A non-empty object is a branch; everything else — a scalar, a list, and an
/// *empty* object — is a leaf. Empty stays a leaf on purpose: `config: {}` is a
/// deliberate "this plugin takes no options", and flattening it to nothing would
/// silently turn it into "unset".
fn flatten(
    prefix: &mut Vec<String>,
    fields: &Map<String, Value>,
    origin: &Origin,
    out: &mut Vec<Setting>,
) {
    for (name, value) in fields {
        prefix.push(name.clone());
        match value {
            Value::Object(nested) if !nested.is_empty() => flatten(prefix, nested, origin, out),
            leaf => out.push(Setting {
                key: SettingPath(prefix.clone()),
                value: leaf.clone(),
                origin: origin.clone(),
            }),
        }
        prefix.pop();
    }
}

/// Apply `layers` lowest precedence first, returning every effective setting.
///
/// A setting from a later layer replaces one from an earlier layer at the same path,
/// *and* replaces any earlier setting above or below it in the tree: an environment
/// variable naming `sources.work.config.root` supersedes a document that set
/// `sources.work.config` whole, and vice versa. That leaves the result prefix-free,
/// which is what lets [`unflatten`] rebuild a document from it without conflict.
#[must_use]
pub fn merge(layers: &[Layer]) -> Merged {
    let mut merged: BTreeMap<SettingPath, Setting> = BTreeMap::new();
    for layer in layers {
        for setting in &layer.settings {
            merged.retain(|key, _| !key.overlaps(&setting.key));
            merged.insert(setting.key.clone(), setting.clone());
        }
    }
    Merged(merged)
}

/// The result of a [`merge`]: settings no one of which is an ancestor of another.
///
/// A newtype rather than a bare map, because that prefix-free property is the whole
/// reason [`unflatten`] can be infallible. Handed a map anybody could build, it would
/// have to decide what a leaf that is also a branch means — and the honest answers are
/// a panic or a silently dropped setting. Only `merge` constructs one of these, so
/// neither case can be reached.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Merged(BTreeMap<SettingPath, Setting>);

impl Merged {
    /// Replace the value of one setting already in this map, leaving everything else.
    ///
    /// Values alone: the keys are what make a [`Merged`] prefix-free and the origins are
    /// what `config show` reports, so a rewrite that could move either would take the two
    /// properties this type exists to hold. A key this map does not carry is not inserted
    /// — a rewrite addresses a setting somebody set, and inserting one would invent a
    /// setting with somebody else's origin on it.
    pub(super) fn set_value(&mut self, key: &SettingPath, value: Value) {
        if let Some(setting) = self.0.get_mut(key) {
            setting.value = value;
        }
    }
}

impl std::ops::Deref for Merged {
    type Target = BTreeMap<SettingPath, Setting>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Rebuild one document from merged settings.
///
/// Infallible because [`merge`] leaves no setting that is an ancestor of another, so
/// no leaf is ever asked to also be a branch — and [`Merged`] is the type that says so.
#[must_use]
pub fn unflatten(settings: &Merged) -> Value {
    let mut root = Map::new();
    for setting in settings.values() {
        let segments = setting.key.segments();
        let mut cursor = &mut root;
        for segment in &segments[..segments.len() - 1] {
            cursor = cursor
                .entry(segment.clone())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("`Merged` holds no setting that is an ancestor of another");
        }
        cursor.insert(segments[segments.len() - 1].clone(), setting.value.clone());
    }
    Value::Object(root)
}

/// Read one textual setting value the way the environment and `--set` both read it.
///
/// The two layers deliberately share this so a setting behaves the same whichever of
/// them supplies it. The rule is small and stated rather than inferred:
///
/// - a value containing a comma is a list, its parts read individually — this is the
///   contract's "a list is comma-separated";
/// - a part that reads as an integer or a decimal is a number, and `true`/`false` is
///   a boolean, so `page_size` set here is the same number the document would give;
/// - everything else is a string, verbatim.
///
/// Nothing here consults the schema of the setting being written, so a value that
/// reads as a number reaches a string field as a number and is refused by name. That
/// is the trade for one rule that holds at every path, including inside a plugin's
/// own `config` block, which the engine cannot type on its own.
#[must_use]
pub fn value_from_text(raw: &str) -> Value {
    if raw.contains(',') {
        Value::Array(raw.split(',').map(|part| scalar(part.trim())).collect())
    } else {
        scalar(raw)
    }
}

/// One scalar, typed as far as it reads.
fn scalar(raw: &str) -> Value {
    if let Ok(integer) = raw.parse::<i64>() {
        return Value::from(integer);
    }
    if let Ok(unsigned) = raw.parse::<u64>() {
        return Value::from(unsigned);
    }
    if let Ok(number) = raw.parse::<f64>()
        && let Some(value) = serde_json::Number::from_f64(number)
    {
        return Value::Number(value);
    }
    match raw {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        other => Value::String(other.to_owned()),
    }
}
