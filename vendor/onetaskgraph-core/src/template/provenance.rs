// llmlint: ignore-file[code_lands_in_the_domain_that_owns_it] A part of the `template` module; why that module sits in this crate is stated once, at the head of its `mod.rs`.
//! Where an item rendered from a template came from: the reserved `onetaskgraph.template`
//! metadata entry (contract C3), and the two hashes it is checked by.
//!
//! The entry is four strings and nothing else — three fixed-size hashes and the caller's own
//! reference to the template — so it costs an item a few hundred bytes wherever it lands, a
//! GitHub issue body's metadata slot included, and never the answers themselves. Those live
//! only where a source keeps them beside the item, in the authoring file (contract C3a).
//!
//! **What it cannot prove.** A check that reads only these hashes trusts them: provenance a
//! person writes by hand, hashes and all, passes it. What they do prove, from the entry alone,
//! is a hand edit of the content (`body_digest` differs from the content's own hash) and a
//! changed template (`digest` differs from the chain's).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use onetaskgraph_plugin_api::MetadataKey;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::{RenderedTemplate, TemplateError};

/// `sha256:` and 64 lowercase hex digits: the one form every hash a provenance entry records
/// takes, and nothing else — built by hashing, or read and refused when it is not one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest(String);

/// Written by hand rather than derived, so the schema states the form a digest must take and
/// both SDKs' models refuse any other — a derive would describe the `String` it is read from.
impl JsonSchema for Sha256Digest {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Sha256Digest".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "`sha256:` and 64 lowercase hex digits.",
            "type": "string",
            "pattern": "^sha256:[0-9a-f]{64}$",
        })
    }
}

impl Sha256Digest {
    /// The digest of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(sha256(bytes))
    }

    /// The digest `text` spells.
    ///
    /// # Errors
    ///
    /// Why `text` is not `sha256:` and 64 lowercase hex digits.
    pub fn parse(text: impl Into<String>) -> Result<Self, String> {
        let text = text.into();
        let hex = text.strip_prefix("sha256:").unwrap_or_default();
        if hex.len() == 64
            && hex
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            Ok(Self(text))
        } else {
            Err(format!(
                "{text:?} is not a digest: `sha256:` and 64 lowercase hex digits"
            ))
        }
    }

    /// The digest as it is spelled.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Sha256Digest {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(text)
    }
}

impl From<Sha256Digest> for String {
    fn from(digest: Sha256Digest) -> Self {
        digest.0
    }
}

impl std::fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a task, a project or a document created or regenerated from a template records under
/// [`TemplateProvenance::KEY`].
///
/// An item created from a plain body records none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateProvenance {
    /// The template it was rendered from: the absolute path of a template file, or a loader
    /// document's `reference` verbatim. Recorded whole, whatever its length.
    pub template: String,
    /// The chain digest the content was rendered with.
    pub digest: Sha256Digest,
    /// The SHA-256 of the item's content exactly as written.
    pub body_digest: Sha256Digest,
    /// The SHA-256 of the resolved answers — defaults applied — as canonical JSON: keys sorted,
    /// no insignificant whitespace, UTF-8.
    pub answers_digest: Sha256Digest,
}

impl TemplateProvenance {
    /// The reserved metadata key this entry lives under: `onetaskgraph.template`.
    pub const KEY: &'static str = MetadataKey::TEMPLATE_KEY;

    /// The provenance of `rendered`, rendered from the template `template` names.
    ///
    /// # Errors
    ///
    /// [`TemplateError::Malformed`] when the rendering's `digest` is not one — a
    /// [`RenderedTemplate`] a caller assembled by hand rather than a render answered.
    pub fn of(
        template: impl Into<String>,
        rendered: &RenderedTemplate,
    ) -> Result<Self, TemplateError> {
        let template = template.into();
        let digest = Sha256Digest::parse(rendered.digest.clone()).map_err(|message| {
            TemplateError::Malformed {
                file: template.clone(),
                key: Some("digest".to_owned()),
                message,
            }
        })?;
        Ok(Self {
            template,
            digest,
            body_digest: Sha256Digest::of(rendered.body.as_bytes()),
            answers_digest: Sha256Digest(answers_digest(&rendered.answers)),
        })
    }

    /// The provenance `metadata` records, `None` when it records none.
    ///
    /// # Errors
    ///
    /// Why the entry under [`Self::KEY`] is not one this product writes, when it is there and
    /// is not: not the four strings, or a digest that is not `sha256:` and 64 lowercase hex
    /// digits.
    pub fn read(metadata: &BTreeMap<String, Value>) -> Result<Option<Self>, String> {
        let Some(value) = metadata.get(Self::KEY) else {
            return Ok(None);
        };
        serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| {
                format!(
                    "its `{}` entry is not a template reference and three digests: {error}",
                    Self::KEY
                )
            })
    }

    /// The entry as the JSON value a metadata map holds.
    #[must_use]
    pub fn to_value(&self) -> Value {
        // Four strings always serialise.
        serde_json::to_value(self).expect("a provenance entry renders as JSON")
    }
}

/// `sha256:` and the lowercase hex SHA-256 of `content`'s UTF-8 bytes: an item's
/// [`TemplateProvenance::body_digest`].
#[must_use]
pub fn body_digest(content: &str) -> String {
    sha256(content.as_bytes())
}

/// `sha256:` and the lowercase hex SHA-256 of `answers` as canonical JSON — keys sorted at
/// every depth, no insignificant whitespace, UTF-8: an item's
/// [`TemplateProvenance::answers_digest`].
#[must_use]
pub fn answers_digest(answers: &BTreeMap<String, Value>) -> String {
    let mut canonical = String::new();
    write_canonical(
        &Value::Object(answers.clone().into_iter().collect()),
        &mut canonical,
    );
    sha256(canonical.as_bytes())
}

/// `value` as canonical JSON, appended to `out`.
///
/// Keys are sorted here rather than trusted to the map's own order, which a dependency
/// enabling `serde_json`'s `preserve_order` would turn into insertion order.
fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(entries) => {
            let mut sorted: Vec<(&String, &Value)> = entries.iter().collect();
            sorted.sort_by(|left, right| left.0.cmp(right.0));
            out.push('{');
            for (index, (key, entry)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(entry, out);
            }
            out.push('}');
        }
        Value::Array(entries) => {
            out.push('[');
            for (index, entry) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(entry, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

fn sha256(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(71);
    hex.push_str("sha256:");
    for byte in Sha256::digest(bytes) {
        // Writing to a `String` never fails.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_answers_digest_is_over_keys_sorted_at_every_depth_and_no_whitespace() {
        let answers: BTreeMap<String, Value> =
            serde_json::from_value(json!({"b": {"z": 1, "a": [true, null, "x y"]}, "a": 2.5}))
                .unwrap();
        assert_eq!(
            answers_digest(&answers),
            sha256(br#"{"a":2.5,"b":{"a":[true,null,"x y"],"z":1}}"#)
        );
        assert_eq!(
            body_digest(""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn a_provenance_entry_reads_back_and_a_foreign_one_is_refused_by_name() {
        let entry = TemplateProvenance {
            template: "/t.md".to_owned(),
            digest: Sha256Digest::parse(format!("sha256:{}", "1".repeat(64))).unwrap(),
            body_digest: Sha256Digest::of(b"body"),
            answers_digest: Sha256Digest::parse(answers_digest(&BTreeMap::new())).unwrap(),
        };
        let metadata = BTreeMap::from([(TemplateProvenance::KEY.to_owned(), entry.to_value())]);
        assert_eq!(TemplateProvenance::read(&metadata), Ok(Some(entry.clone())));
        assert_eq!(TemplateProvenance::read(&BTreeMap::new()), Ok(None));
        let mut extra = entry.to_value();
        extra["signed_by"] = json!("someone");
        let extra = BTreeMap::from([(TemplateProvenance::KEY.to_owned(), extra)]);
        assert!(
            TemplateProvenance::read(&extra)
                .unwrap_err()
                .contains("signed_by")
        );
        let foreign = BTreeMap::from([(TemplateProvenance::KEY.to_owned(), json!("hand"))]);
        assert!(
            TemplateProvenance::read(&foreign)
                .unwrap_err()
                .contains("onetaskgraph.template")
        );
        let mut short = entry.to_value();
        short["body_digest"] = json!("sha256:abc");
        let short = BTreeMap::from([(TemplateProvenance::KEY.to_owned(), short)]);
        assert!(
            TemplateProvenance::read(&short)
                .unwrap_err()
                .contains("\"sha256:abc\" is not a digest")
        );
    }
}
