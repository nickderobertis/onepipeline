// llmlint: ignore-file[code_lands_in_the_domain_that_owns_it] A part of the `template` module; why that module sits in this crate is stated once, at the head of its `mod.rs`.
//! A template file's front matter: split off the body, and read strictly by key.
//!
//! Read through serde-saphyr into [`Yaml`], a node whose mappings keep their keys in the
//! order written, rather than a derived struct, for two reasons a derive cannot serve: a
//! refusal has to name the key it is about (`variables.title.type`, not a serde position),
//! and a file's variables keep the order they were written in, which is the order a prompt
//! asks them in. Not `serde_json::Value`, whose object sorts its keys.

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::{
    ItemType, MemberDeclaration, TemplateError, TemplateVariable, VariableType, check_value,
    members, schema,
};

/// One node of front matter, as serde-saphyr reads it.
#[derive(Clone)]
enum Yaml {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Sequence(Vec<Yaml>),
    /// Every entry in the order the file wrote it.
    Mapping(Vec<(String, Yaml)>),
}

impl Yaml {
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    /// The node as a JSON value, the form a variable's default takes.
    fn into_json(self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(value),
            Self::Number(number) => Value::Number(number),
            Self::String(text) => Value::String(text),
            Self::Sequence(items) => Value::Array(items.into_iter().map(Self::into_json).collect()),
            Self::Mapping(entries) => Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, value.into_json()))
                    .collect::<Map<_, _>>(),
            ),
        }
    }
}

impl<'de> Deserialize<'de> for Yaml {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }
}

/// Collects a mapping's entries in the order they arrive.
struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Yaml;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a YAML value")
    }

    fn visit_unit<E>(self) -> Result<Yaml, E> {
        Ok(Yaml::Null)
    }

    fn visit_none<E>(self) -> Result<Yaml, E> {
        Ok(Yaml::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Yaml, D::Error> {
        Yaml::deserialize(deserializer)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Yaml, E> {
        Ok(Yaml::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Yaml, E> {
        Ok(Yaml::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Yaml, E> {
        Ok(Yaml::Number(value.into()))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Yaml, E> {
        Number::from_f64(value)
            .map(Yaml::Number)
            .ok_or_else(|| E::custom(format!("{value} is not a number JSON can carry")))
    }

    fn visit_str<E>(self, value: &str) -> Result<Yaml, E> {
        Ok(Yaml::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Yaml, E> {
        Ok(Yaml::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Yaml, A::Error> {
        let mut read = Vec::new();
        while let Some(item) = items.next_element()? {
            read.push(item);
        }
        Ok(Yaml::Sequence(read))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<Yaml, A::Error> {
        let mut read = Vec::new();
        // llmlint: ignore[boundary_inputs_validated] A key is what serde-saphyr reads it as, by the plan's fixed YAML contract: it refuses a null, a sequence and a repeated key itself and reads a plain `1` or `true` as a string, and front matter adds no key-kind check of its own; `a_variables_key_is_read_as_yaml_reads_it` pins each case, and a name still has to match the variable-name pattern.
        while let Some(entry) = entries.next_entry()? {
            read.push(entry);
        }
        Ok(Yaml::Mapping(read))
    }
}

/// The front matter's own version key, and the one value it may take.
const VERSION_KEY: &str = "onetaskgraph_template";

/// The only front matter version this build reads.
const VERSION: u64 = 1;

/// Every key front matter may carry, and nothing else: any other is refused by name.
pub const FRONT_MATTER_KEYS: [&str; 4] = [VERSION_KEY, "description", "variables", "members"];

/// Every key one variable's declaration may carry, and nothing else: any other is refused by
/// name.
pub const DECLARATION_KEYS: [&str; 6] = [
    "description",
    "type",
    "items",
    "required",
    "default",
    "schema",
];

/// `keys` as a message lists them: `` `a`, `b` and `c` ``.
fn listed(keys: &[&str]) -> String {
    let quoted: Vec<String> = keys.iter().map(|key| format!("`{key}`")).collect();
    match quoted.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => quoted.concat(),
    }
}

/// A file with its front matter taken off.
pub(super) struct Split {
    /// The declarations the front matter made, in the order it made them, each
    /// `declared_in` this file until the chain is merged.
    pub declarations: Vec<TemplateVariable>,
    /// The plan-graph members the front matter declares, in the order it declares them.
    pub members: Vec<MemberDeclaration>,
    /// What the renderer compiles: everything after the closing `---` line.
    pub body: String,
    /// How many lines the front matter took, so a body line number can be reported as the
    /// file's own.
    pub offset_lines: usize,
}

/// Split `source` into its declarations and its body.
///
/// A file whose first line is not `---` has no front matter and declares nothing. One whose
/// first line is `---` must close it with a later line reading `---`.
pub(super) fn split(file: &str, source: &str) -> Result<Split, TemplateError> {
    let mut lines = source.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Ok(unfronted(source));
    };
    if trim_line_end(first) != "---" {
        return Ok(unfronted(source));
    }

    let mut consumed = first.len();
    let mut offset_lines = 1;
    let mut matter = String::new();
    let mut closed = false;
    for line in lines {
        consumed += line.len();
        offset_lines += 1;
        if trim_line_end(line) == "---" {
            closed = true;
            break;
        }
        matter.push_str(line);
    }
    if !closed {
        return Err(TemplateError::malformed(
            file,
            None,
            "its first line opens front matter with `---`, and no later line reading `---` \
             closes it",
        ));
    }

    let parsed: Yaml = crate::yaml::from_str(&matter).map_err(|error| {
        TemplateError::malformed(file, None, format!("its front matter is not YAML: {error}"))
    })?;
    let (declarations, members) = read(file, &parsed)?;
    Ok(Split {
        declarations,
        members,
        body: source[consumed..].to_owned(),
        offset_lines,
    })
}

/// A file with no front matter: its whole source is the body.
fn unfronted(source: &str) -> Split {
    Split {
        declarations: Vec::new(),
        members: Vec::new(),
        body: source.to_owned(),
        offset_lines: 0,
    }
}

/// A line without its line ending, so a file written with CRLF reads the same.
fn trim_line_end(line: &str) -> &str {
    line.trim_end_matches('\n').trim_end_matches('\r')
}

/// The declarations and members front matter makes.
type Declared = (Vec<TemplateVariable>, Vec<MemberDeclaration>);

/// Read the front matter mapping, refusing any key C1 does not define.
fn read(file: &str, matter: &Yaml) -> Result<Declared, TemplateError> {
    let Yaml::Mapping(mapping) = matter else {
        return Err(TemplateError::malformed(
            file,
            None,
            "its front matter is not a mapping; it opens with `onetaskgraph_template: 1`",
        ));
    };

    let mut version = None;
    let mut variables = None;
    let mut declared_members = Vec::new();
    for (key, value) in mapping {
        // The list alone decides which keys are accepted; the arms below give each its meaning,
        // and `the_published_keys_are_exactly_the_ones_read` proves every listed key has one.
        let listed_key = FRONT_MATTER_KEYS.contains(&key.as_str());
        match key.as_str() {
            _ if !listed_key => {
                return Err(TemplateError::malformed(
                    file,
                    Some(key),
                    format!(
                        "is not a front matter key; the keys are {}",
                        listed(&FRONT_MATTER_KEYS)
                    ),
                ));
            }
            VERSION_KEY => version = Some(value),
            "description" => {
                if !matches!(value, Yaml::String(_)) {
                    return Err(TemplateError::malformed(
                        file,
                        Some("description"),
                        "is not a string",
                    ));
                }
            }
            "variables" => variables = Some(value),
            "members" => declared_members = members::read(file, value.clone().into_json())?,
            other => {
                return Err(TemplateError::malformed(
                    file,
                    Some(other),
                    "is listed as a front matter key and has no reading",
                ));
            }
        }
    }

    match version {
        None => {
            return Err(TemplateError::malformed(
                file,
                Some(VERSION_KEY),
                "is missing; front matter declares `onetaskgraph_template: 1`",
            ));
        }
        Some(Yaml::Number(number)) if number.as_u64() == Some(VERSION) => {}
        Some(_) => {
            return Err(TemplateError::malformed(
                file,
                Some(VERSION_KEY),
                "is not 1, the only template format version this build reads",
            ));
        }
    }

    let Some(variables) = variables else {
        return Ok((Vec::new(), declared_members));
    };
    let variables = match variables {
        Yaml::Null => return Ok((Vec::new(), declared_members)),
        Yaml::Mapping(variables) => variables,
        _ => {
            return Err(TemplateError::malformed(
                file,
                Some("variables"),
                "is not a mapping from variable name to declaration",
            ));
        }
    };

    let declarations = variables
        .iter()
        .map(|(name, declaration)| declaration_of(file, name.clone(), declaration))
        .collect::<Result<_, _>>()?;
    Ok((declarations, declared_members))
}

/// Whether `name` is one a variable can be declared with: `^[a-z][a-z0-9_]*$`.
#[must_use]
pub fn is_variable_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|rest| rest.is_ascii_lowercase() || rest.is_ascii_digit() || rest == '_')
}

/// One variable's declaration, refused by key.
fn declaration_of(
    file: &str,
    name: String,
    value: &Yaml,
) -> Result<TemplateVariable, TemplateError> {
    let base = format!("variables.{name}");
    if !is_variable_name(&name) {
        return Err(TemplateError::malformed(
            file,
            Some(&base),
            "is not a variable name; a name matches ^[a-z][a-z0-9_]*$",
        ));
    }
    let Yaml::Mapping(fields) = value else {
        return Err(TemplateError::malformed(
            file,
            Some(&base),
            "is not a mapping; a declaration carries at least a `description`",
        ));
    };

    let mut description = None;
    let mut kind = VariableType::String;
    let mut items = None;
    let mut required = None;
    let mut default = None;
    let mut declared_schema = None;
    for (key, field) in fields {
        let path = format!("{base}.{key}");
        // As in `read`: the list decides which keys are accepted, the arms what each means.
        let listed_key = DECLARATION_KEYS.contains(&key.as_str());
        match key.as_str() {
            _ if !listed_key => {
                return Err(TemplateError::malformed(
                    file,
                    Some(&path),
                    format!(
                        "is not a declaration key; the keys are {}",
                        listed(&DECLARATION_KEYS)
                    ),
                ));
            }
            "description" => match field {
                Yaml::String(text) if !text.trim().is_empty() => description = Some(text.clone()),
                _ => {
                    return Err(TemplateError::malformed(
                        file,
                        Some(&path),
                        "is not a non-empty string; it is what a prompt shows",
                    ));
                }
            },
            "type" => {
                kind = match field.as_str().and_then(VariableType::parse) {
                    Some(kind) => kind,
                    None => {
                        return Err(TemplateError::malformed(
                            file,
                            Some(&path),
                            "is not one of string, text, integer, boolean, list, object",
                        ));
                    }
                };
            }
            "items" => {
                items = match field.as_str().and_then(ItemType::parse) {
                    Some(items) => Some(items),
                    None => {
                        return Err(TemplateError::malformed(
                            file,
                            Some(&path),
                            "is not one of string, object",
                        ));
                    }
                };
            }
            "required" => match field {
                Yaml::Bool(value) => required = Some(*value),
                _ => {
                    return Err(TemplateError::malformed(
                        file,
                        Some(&path),
                        "is not a boolean",
                    ));
                }
            },
            "default" => default = Some(field.clone().into_json()),
            "schema" => match field {
                Yaml::Mapping(_) | Yaml::Bool(_) => {
                    declared_schema = Some(field.clone().into_json());
                }
                _ => {
                    return Err(TemplateError::malformed(
                        file,
                        Some(&path),
                        "is not a JSON Schema: a schema is a mapping (or `true` or `false`)",
                    ));
                }
            },
            _ => {
                return Err(TemplateError::malformed(
                    file,
                    Some(&path),
                    "is listed as a declaration key and has no reading",
                ));
            }
        }
    }

    let Some(description) = description else {
        return Err(TemplateError::malformed(
            file,
            Some(&format!("{base}.description")),
            "is missing; every variable says what it is for",
        ));
    };
    if items.is_some() && kind != VariableType::List {
        return Err(TemplateError::malformed(
            file,
            Some(&format!("{base}.items")),
            "is given for a variable whose type is not `list`",
        ));
    }
    let items = (kind == VariableType::List).then(|| items.unwrap_or(ItemType::String));
    if required == Some(true) && default.is_some() {
        return Err(TemplateError::malformed(
            file,
            Some(&format!("{base}.required")),
            "is `true` beside a `default`; a variable with a default is optional",
        ));
    }
    if let Some(default) = &default
        && let Err(problem) = check_value(kind, items, default)
    {
        return Err(TemplateError::malformed(
            file,
            Some(&format!("{base}.default")),
            format!("is not a value of the variable's type: {problem}"),
        ));
    }

    if let Some(declared) = &declared_schema {
        if !matches!(kind, VariableType::Object | VariableType::List) {
            return Err(TemplateError::malformed(
                file,
                Some(&format!("{base}.schema")),
                "is given for a variable whose type is neither `object` nor `list`",
            ));
        }
        if let Err(problem) = schema::compile(declared) {
            return Err(TemplateError::malformed(
                file,
                Some(&format!("{base}.schema")),
                format!("is not a JSON Schema (draft 2020-12) this build compiles: {problem}"),
            ));
        }
        if let Some(default) = &default
            && let Err(violation) = schema::conform(declared, default)
        {
            return Err(TemplateError::malformed(
                file,
                Some(&format!("{base}.default")),
                format!(
                    "does not satisfy the variable's schema at JSON pointer {:?}: {}",
                    violation.pointer, violation.problem
                ),
            ));
        }
    }

    Ok(TemplateVariable {
        required: required.unwrap_or(default.is_none()),
        schema: declared_schema,
        name,
        description,
        kind,
        items,
        default,
        declared_in: file.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published key lists are the parser's: every listed key is read, and a key beside
    /// them is refused naming the whole list.
    #[test]
    fn the_published_keys_are_exactly_the_ones_read() {
        let every_key = "---\n\
onetaskgraph_template: 1\n\
description: all of it\n\
members: []\n\
variables:\n  \
  steps:\n    \
    description: d\n    \
    type: list\n    \
    items: string\n    \
    required: false\n    \
    default: [a]\n    \
    schema: {type: array}\n\
---\n";
        let read = split("t.md", every_key).expect("every published key is read");
        assert_eq!(read.declarations.len(), 1);
        assert_eq!(
            FRONT_MATTER_KEYS.len(),
            every_key
                .lines()
                .filter(|line| line.contains(':') && !line.starts_with(' '))
                .count()
        );
        assert_eq!(
            DECLARATION_KEYS.len(),
            every_key
                .lines()
                .filter(|line| line.starts_with("    "))
                .count()
        );

        let refused = split("t.md", "---\nonetaskgraph_template: 1\nother: x\n---\n")
            .err()
            .expect("an unlisted key");
        assert!(
            refused.to_string().contains(&listed(&FRONT_MATTER_KEYS)),
            "{refused}"
        );
        let refused = split(
            "t.md",
            "---\nonetaskgraph_template: 1\nvariables:\n  x: {description: d, other: y}\n---\n",
        )
        .err()
        .expect("an unlisted declaration key");
        assert!(
            refused.to_string().contains(&listed(&DECLARATION_KEYS)),
            "{refused}"
        );
    }
}
