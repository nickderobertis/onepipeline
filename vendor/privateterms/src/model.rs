//! The declaration (Specification 1, *The declaration*): its members, and the one
//! validation every route into a [`Declaration`] goes through — [`parse`](crate::parse)
//! in either encoding and `serde` deserialization alike.

use std::borrow::Cow;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::de::Error as _;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::error::{DeclarationError, DeclarationErrorKind as Kind, Member};
use crate::normalize::{normalize, term_key};
use crate::LevelKind;

/// One level's declaration of private terms.
///
/// A value of this type has passed validation for *some* level: deserializing one
/// (through [`parse`](crate::parse) or `serde`) refuses every invalid form the
/// specification lists. Whether `exceptions` is allowed depends on the level, which
/// [`parse`](crate::parse) is told and `serde` is not; [`resolve`](crate::resolve)
/// checks it again for every level it is given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct Declaration {
    /// The model version, 1 or 2.
    pub schema_version: u32,
    /// Terms added, each matched as a whole word, not case-sensitive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terms: Vec<String>,
    /// Narrowings of the repository's own rules (a repository's own declaration only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<Exception>,
    /// From version 2: how a term is replaced, or that it is rejected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replacements: Vec<Replacement>,
    /// From version 2: public phrases exempting a hit wholly inside a standing occurrence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phrases: Vec<String>,
}

/// One exception: a term, and how the rules carrying it are narrowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Exception {
    /// The term it names, compared normalized and folded.
    pub term: String,
    /// What it does.
    pub action: ExceptionAction,
}

/// What an exception does to the rules carrying its term.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ExceptionAction {
    /// Remove the rule.
    Drop,
    /// Match it only as a whole word.
    WholeWord,
    /// Match it only spelled exactly as the exception spells it.
    CaseSensitive,
    /// Leave it found only through the repository's qualified `owner/name`.
    OwnerNameOnly,
}

/// One replacement entry: replace a term with a string, or reject it.
///
/// Serializes as `{"term": …, "with": …}` or `{"term": …, "reject": true}`; any other
/// shape is refused.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Replacement {
    /// Rewrite the term's hits to `with`.
    Replace {
        /// The term.
        term: String,
        /// What its hits are rewritten to.
        with: String,
    },
    /// Refuse a text carrying the term.
    Reject {
        /// The term.
        term: String,
    },
}

impl Replacement {
    /// The term this entry names.
    pub fn term(&self) -> &str {
        match self {
            Replacement::Replace { term, .. } | Replacement::Reject { term } => term,
        }
    }
}

impl Serialize for Replacement {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        match self {
            Replacement::Replace { term, with } => {
                map.serialize_entry("term", term)?;
                map.serialize_entry("with", with)?;
            }
            Replacement::Reject { term } => {
                map.serialize_entry("term", term)?;
                map.serialize_entry("reject", &true)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Replacement {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        replacement(&value, 0).map_err(D::Error::custom)
    }
}

impl JsonSchema for Replacement {
    fn schema_name() -> Cow<'static, str> {
        "Replacement".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "Replace a term with a string, or reject it: exactly one.",
            "oneOf": [
                {
                    "type": "object",
                    "properties": { "term": { "type": "string" }, "with": { "type": "string" } },
                    "required": ["term", "with"],
                    "additionalProperties": false
                },
                {
                    "type": "object",
                    "properties": { "term": { "type": "string" }, "reject": { "const": true } },
                    "required": ["term", "reject"],
                    "additionalProperties": false
                }
            ]
        })
    }
}

impl<'de> Deserialize<'de> for Declaration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        declaration(&value).map_err(D::Error::custom)
    }
}

/// The members a declaration may carry, and the model version each needs.
const MEMBERS: [(&str, Member, u32); 5] = [
    ("schema_version", Member::SchemaVersion, 1),
    ("terms", Member::Terms, 1),
    ("exceptions", Member::Exceptions, 1),
    ("replacements", Member::Replacements, 2),
    ("phrases", Member::Phrases, 2),
];

/// A model value, validated into a declaration for any level.
pub(crate) fn declaration(value: &Value) -> Result<Declaration, DeclarationError> {
    let Value::Object(object) = value else {
        return Err(DeclarationError::new(Kind::Malformed));
    };
    if object
        .keys()
        .any(|key| !MEMBERS.iter().any(|(name, _, _)| name == key))
    {
        return Err(DeclarationError::new(Kind::UnknownMember));
    }
    let version = match object.get("schema_version") {
        Some(Value::Number(number)) => match number.as_u64() {
            Some(v @ (1 | 2)) => v as u32,
            _ => {
                return Err(DeclarationError::in_member(
                    Kind::UnsupportedVersion,
                    Member::SchemaVersion,
                ))
            }
        },
        _ => {
            return Err(DeclarationError::in_member(
                Kind::UnsupportedVersion,
                Member::SchemaVersion,
            ))
        }
    };
    for (name, member, since) in MEMBERS {
        if since > version && object.contains_key(name) {
            return Err(DeclarationError::in_member(
                Kind::MemberRequiresVersion2,
                member,
            ));
        }
    }
    let terms = strings(object, Member::Terms, Kind::EmptyTerm)?;
    distinct(terms.iter().map(String::as_str), Member::Terms)?;
    let exceptions = list(object, Member::Exceptions)?
        .iter()
        .enumerate()
        .map(|(index, entry)| exception(entry, index))
        .collect::<Result<Vec<_>, _>>()?;
    distinct(
        exceptions.iter().map(|e| e.term.as_str()),
        Member::Exceptions,
    )?;
    let replacements = list(object, Member::Replacements)?
        .iter()
        .enumerate()
        .map(|(index, entry)| replacement(entry, index))
        .collect::<Result<Vec<_>, _>>()?;
    distinct(
        replacements.iter().map(Replacement::term),
        Member::Replacements,
    )?;
    // Phrases that are one once normalized and folded are kept once, the first as written.
    let mut phrases: Vec<String> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for phrase in strings(object, Member::Phrases, Kind::EmptyPhrase)? {
        let key = term_key(&phrase);
        if !keys.contains(&key) {
            keys.push(key);
            phrases.push(phrase);
        }
    }
    Ok(Declaration {
        schema_version: version,
        terms,
        exceptions,
        replacements,
        phrases,
    })
}

/// Refuse a declaration that is not valid for a level of `kind`.
pub(crate) fn for_level(
    declaration: &Declaration,
    kind: LevelKind,
) -> Result<(), DeclarationError> {
    // A value built in code rather than read is held to the same rules.
    let value =
        serde_json::to_value(declaration).map_err(|_| DeclarationError::new(Kind::Malformed))?;
    self::declaration(&value)?;
    if kind != LevelKind::Repository && !declaration.exceptions.is_empty() {
        return Err(DeclarationError::in_member(
            Kind::ExceptionsOutsideRepository,
            Member::Exceptions,
        ));
    }
    Ok(())
}

fn list(object: &Map<String, Value>, member: Member) -> Result<&[Value], DeclarationError> {
    match object.get(member.name()) {
        None => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(DeclarationError::in_member(Kind::Malformed, member)),
    }
}

fn strings(
    object: &Map<String, Value>,
    member: Member,
    empty: Kind,
) -> Result<Vec<String>, DeclarationError> {
    list(object, member)?
        .iter()
        .enumerate()
        .map(|(index, item)| match item {
            Value::String(text) if normalize(text).is_empty() => {
                Err(DeclarationError::at(empty, member, index))
            }
            Value::String(text) => Ok(text.clone()),
            _ => Err(DeclarationError::at(Kind::Malformed, member, index)),
        })
        .collect()
}

fn distinct<'a>(
    terms: impl Iterator<Item = &'a str>,
    member: Member,
) -> Result<(), DeclarationError> {
    let mut keys: Vec<String> = Vec::new();
    for (index, term) in terms.enumerate() {
        let key = term_key(term);
        if keys.contains(&key) {
            return Err(DeclarationError::at(Kind::DuplicateEntry, member, index));
        }
        keys.push(key);
    }
    Ok(())
}

fn entry_object(
    value: &Value,
    member: Member,
    index: usize,
    allowed: &[&str],
) -> Result<Map<String, Value>, DeclarationError> {
    let Value::Object(object) = value else {
        return Err(DeclarationError::at(Kind::Malformed, member, index));
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(DeclarationError::at(Kind::UnknownMember, member, index));
    }
    Ok(object.clone())
}

fn term_of(
    object: &Map<String, Value>,
    member: Member,
    index: usize,
) -> Result<String, DeclarationError> {
    match object.get("term") {
        Some(Value::String(term)) if normalize(term).is_empty() => {
            Err(DeclarationError::at(Kind::EmptyTerm, member, index))
        }
        Some(Value::String(term)) => Ok(term.clone()),
        _ => Err(DeclarationError::at(Kind::Malformed, member, index)),
    }
}

fn exception(value: &Value, index: usize) -> Result<Exception, DeclarationError> {
    let member = Member::Exceptions;
    let object = entry_object(value, member, index, &["term", "action"])?;
    let term = term_of(&object, member, index)?;
    let action = object
        .get("action")
        .and_then(|action| ExceptionAction::deserialize(action).ok())
        .ok_or_else(|| DeclarationError::at(Kind::Malformed, member, index))?;
    Ok(Exception { term, action })
}

fn replacement(value: &Value, index: usize) -> Result<Replacement, DeclarationError> {
    let member = Member::Replacements;
    let object = entry_object(value, member, index, &["term", "with", "reject"])?;
    let term = term_of(&object, member, index)?;
    match (object.get("with"), object.get("reject")) {
        (Some(Value::String(with)), None) if normalize(with).is_empty() => {
            Err(DeclarationError::at(Kind::EmptyTerm, member, index))
        }
        (Some(Value::String(with)), None) => Ok(Replacement::Replace {
            term,
            with: with.clone(),
        }),
        (None, Some(Value::Bool(true))) => Ok(Replacement::Reject { term }),
        (Some(Value::String(_)) | None, Some(Value::Bool(_)) | None) => {
            Err(DeclarationError::at(Kind::ReplacementShape, member, index))
        }
        _ => Err(DeclarationError::at(Kind::Malformed, member, index)),
    }
}
