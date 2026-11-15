//! Locators (Specification 1, *Locators*): the strings a diagnostic names a record by,
//! screened before anything prints them.

use std::borrow::Cow;
use std::fmt;

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The reason a record whose locator was redacted is unfixable.
pub const LOCATOR_UNFIXABLE_REASON: &str = "identifier carries a private term";

/// What a locator names.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum LocatorKind {
    /// A source name.
    Source,
    /// An item id.
    Item,
    /// A user-defined field name.
    Field,
    /// A metadata key.
    MetadataKey,
    /// A comment id.
    Comment,
    /// A file path.
    Path,
    /// A repository identity.
    Identity,
}

impl LocatorKind {
    /// The kind's kebab-case name, as it serializes.
    pub fn name(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Item => "item",
            Self::Field => "field",
            Self::MetadataKey => "metadata-key",
            Self::Comment => "comment",
            Self::Path => "path",
            Self::Identity => "identity",
        }
    }
}

/// A screened locator: printed verbatim where it passed, and otherwise only as its kind
/// and its ordinal within one answer or report.
///
/// It serializes as a plain JSON string, or as exactly
/// `{"redacted": true, "kind": "<kind>", "ordinal": <n>}`; it displays as the string,
/// or as `<redacted item 2>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Locator {
    /// A locator carrying no non-exempt hit, verbatim.
    Plain(String),
    /// A locator that carried a non-exempt hit, or any locator of an unavailable check.
    Redacted {
        /// What it named.
        kind: LocatorKind,
        /// Its position among the redacted locators of one answer or report, from 1.
        ordinal: u32,
    },
}

impl fmt::Display for Locator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Locator::Plain(value) => f.write_str(value),
            Locator::Redacted { kind, ordinal } => {
                write!(f, "<redacted {} {ordinal}>", kind.name())
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RedactedWire {
    redacted: bool,
    kind: LocatorKind,
    ordinal: u32,
}

impl Serialize for Locator {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Locator::Plain(value) => serializer.serialize_str(value),
            Locator::Redacted { kind, ordinal } => RedactedWire {
                redacted: true,
                kind: *kind,
                ordinal: *ordinal,
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Locator {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Plain(String),
            Redacted(RedactedWire),
        }
        match Wire::deserialize(deserializer)? {
            Wire::Plain(value) => Ok(Locator::Plain(value)),
            Wire::Redacted(RedactedWire {
                redacted: true,
                kind,
                ordinal,
            }) => Ok(Locator::Redacted { kind, ordinal }),
            Wire::Redacted(_) => Err(D::Error::custom(
                "a redacted locator must say `\"redacted\": true`",
            )),
        }
    }
}

impl JsonSchema for Locator {
    fn schema_name() -> Cow<'static, str> {
        "Locator".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let kind = generator.subschema_for::<LocatorKind>();
        json_schema!({
            "description": "A screened locator: the string verbatim, or its redacted form.",
            "oneOf": [
                { "type": "string" },
                {
                    "type": "object",
                    "properties": {
                        "redacted": { "const": true },
                        "kind": kind,
                        "ordinal": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["redacted", "kind", "ordinal"],
                    "additionalProperties": false
                }
            ]
        })
    }
}
