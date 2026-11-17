//! Content-keyed attestations: a record that someone cleared a task's, a project's or a
//! document's authored content against a bar, which goes void the moment that content or the
//! bar it was cleared against moves.
//!
//! This module is the one declaration of the shapes — [`Attestation`], [`AttestationCheck`],
//! [`ProjectAttestationCheck`] and [`Attested`] — that the command line answers with and both
//! SDKs are generated from, and of the digest a record is keyed on. The verbs that record and
//! check one are the engine's ([`crate::Engine::attest`] and its siblings).
//!
//! # The record
//!
//! An item holds its attestations under the reserved metadata key
//! [`MetadataKey::ATTESTATIONS_KEY`] (`onetaskgraph.attestations`): a JSON object mapping an
//! attestation kind to the one record of that kind, so recording a kind again replaces it:
//!
//! ```json
//! {"design-approval": {"kind": "design-approval", "by": "<attester label>",
//!   "at": "<RFC 3339>", "content_digest": "sha256:<hex>", "bar": "<fingerprint>",
//!   "note": "<optional text>", "excludes": ["<caller key>", "…"]}}
//! ```
//!
//! `note` is left out when none was given, and `excludes` when it is empty.
//!
//! # The digest
//!
//! `content_digest` is computed by the store, never handed to it: `sha256:` and the lowercase
//! hex SHA-256 of the canonical JSON (keys sorted at every depth, no insignificant whitespace)
//! of the item's **authored content**:
//!
//! - `title` and `content` (the body, `null` when it has none);
//! - `answers_digest`: the `answers_digest` of the item's `onetaskgraph.template` entry — the
//!   hash of the answers it was rendered from — or `null` for an item rendered from none;
//! - `repositories`, sorted;
//! - `depends_on`: the item's own forward dependency edges, each `[<edge kind>, <far end's
//!   kind>, <far end>]` sorted, where the far end is named by its lineage root: the qualified
//!   id the far end records at `onetaskgraph.lineage`, else at `onetaskgraph.origin`, else its
//!   own, for an item copied from nowhere. A copy files the far end under another id and
//!   records where it came from and the root that came from, so every copy of one far end
//!   names one root without any copy between them being read — or being there at all — while
//!   two items sharing a title never do. A copy out of a source declared private into one that
//!   is not records neither, so as not to name that source: where any source is declared
//!   private, a root in a source that is not — a far end recording none, or the item a far
//!   end records, which is read once for the root it records in turn — is named instead by the
//!   root of the one item of a source declared private whose `onetaskgraph.copies` link names
//!   it: the item it was copied from.
//!   So an edge to `authoring:a` and its copy's edge to `plans:I_x`, where `authoring:a` links
//!   to `plans:I_x`, are both `[<kind>, "task", "lineage:authoring:a"]`. No link, two links,
//!   or a private source that cannot be asked leave the far end its own id, exactly as before.
//!   Every edge whose far end no copy made is spelled as it always was, so a record taken over
//!   one reads the same in every release since attestations landed. A document has none;
//! - `metadata`: every caller-owned metadata key and its value, but those the record
//!   `excludes`.
//!
//! Nothing else is in it: not the status, priority, labels, assets, location, id or version,
//! not any key of the reserved `onetaskgraph.` namespace — the attestations included — so an
//! item's attestation is not voided by recording another, and a copy, which carries every
//! member above and re-keys only what is left out, carries a record that stays true of its
//! copy.
//!
//! The attestation is deliberately policy-free: the store records and checks, and says
//! nothing about who may attest or which kinds anything requires.

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, SubsecRound as _, Utc};
use onetaskgraph_plugin_api::{DependencyKind, Designator, ItemKind, MetadataKey, Repository};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::GlobalId;
use crate::template::Sha256Digest;

/// What an attestation kind, an attester label or a bar fingerprint may be: a non-empty string
/// of at most this many characters with no line break, so a record reads back on one line.
pub const MAX_LABEL: usize = 200;

/// Why an attestation could not be composed from what a caller gave.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field} {problem}")]
pub struct AttestationInputError {
    /// Which input: `--kind`, `--by`, `--bar`, `--exclude` or `--note`.
    pub field: &'static str,
    /// What is wrong with it.
    pub problem: String,
}

/// One kind of attestation, such as `plan-review` or `design-approval`: one to 200 letters,
/// digits, `.`, `_` or `-`.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct AttestationKind(String);

impl AttestationKind {
    /// One kind, once it is established it is one.
    ///
    /// # Errors
    ///
    /// [`AttestationInputError`] naming `--kind` for an empty kind, one longer than
    /// [`MAX_LABEL`], or one holding anything but letters, digits, `.`, `_` and `-`.
    pub fn new(kind: impl Into<String>) -> Result<Self, AttestationInputError> {
        let kind = kind.into();
        let problem = if kind.is_empty() {
            Some("is empty; name the kind, such as `design-approval`".to_owned())
        } else if kind.chars().count() > MAX_LABEL {
            Some(format!("is longer than {MAX_LABEL} characters"))
        } else if !kind
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
        {
            Some(format!(
                "{kind:?} holds a character that is not a letter, a digit, `.`, `_` or `-`"
            ))
        } else {
            None
        };
        match problem {
            Some(problem) => Err(AttestationInputError {
                field: "--kind",
                problem,
            }),
            None => Ok(Self(kind)),
        }
    }

    /// The kind as it is spelled.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AttestationKind {
    type Error = AttestationInputError;

    fn try_from(kind: String) -> Result<Self, Self::Error> {
        Self::new(kind)
    }
}

impl From<AttestationKind> for String {
    fn from(kind: AttestationKind) -> Self {
        kind.0
    }
}

impl std::fmt::Display for AttestationKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Who attested, or the fingerprint of a bar: one line of one to 200 characters, not blank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct AttestationLabel(String);

impl AttestationLabel {
    /// One label, once it is established it is one; `field` is the flag it came from, which a
    /// refusal names.
    ///
    /// # Errors
    ///
    /// [`AttestationInputError`] naming `field` for a blank label, one longer than
    /// [`MAX_LABEL`], or one holding a line break.
    pub fn new(
        field: &'static str,
        value: impl Into<String>,
    ) -> Result<Self, AttestationInputError> {
        let value = value.into();
        let problem = if value.trim().is_empty() {
            Some("is empty".to_owned())
        } else if value.chars().count() > MAX_LABEL {
            Some(format!("is longer than {MAX_LABEL} characters"))
        } else if value.contains(['\n', '\r']) {
            Some("holds a line break".to_owned())
        } else {
            None
        };
        match problem {
            Some(problem) => Err(AttestationInputError { field, problem }),
            None => Ok(Self(value)),
        }
    }

    /// The label as it is spelled.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AttestationLabel {
    type Error = AttestationInputError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new("label", value)
    }
}

impl From<AttestationLabel> for String {
    fn from(label: AttestationLabel) -> Self {
        label.0
    }
}

impl std::fmt::Display for AttestationLabel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// What an attester adds: text of any length and any number of lines, not blank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct AttestationNote(String);

impl AttestationNote {
    /// One note, once it is established it is not blank.
    ///
    /// # Errors
    ///
    /// [`AttestationInputError`] naming `--note` for a blank one.
    pub fn new(value: impl Into<String>) -> Result<Self, AttestationInputError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(AttestationInputError {
                field: "--note",
                problem: "is empty".to_owned(),
            });
        }
        Ok(Self(value))
    }

    /// The note as it is written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AttestationNote {
    type Error = AttestationInputError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AttestationNote> for String {
    fn from(note: AttestationNote) -> Self {
        note.0
    }
}

/// One attestation: who cleared the item's authored content, when, which content and against
/// which bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attestation {
    /// The kind of attestation this is; the key it is held under.
    pub kind: AttestationKind,
    /// Who attested, as the caller labels them.
    pub by: AttestationLabel,
    /// When it was recorded, in RFC 3339 to the second.
    pub at: DateTime<Utc>,
    /// The digest of the item's authored content when it was recorded, computed by the store
    /// (see the module's documentation).
    pub content_digest: Sha256Digest,
    /// The fingerprint of the bar the content was cleared against, as the caller gave it.
    pub bar: AttestationLabel,
    /// What the attester added, when they added anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<AttestationNote>,
    /// The caller-owned metadata keys left out of `content_digest`, sorted; empty when none
    /// were.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(!skip_serializing_if)]
    pub excludes: Vec<MetadataKey>,
}

/// What a check of one kind of attestation found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AttestationState {
    /// A record of the kind is there, the item's authored content is the content it cleared,
    /// and the bar asked about, when one was, is the bar it cleared it against.
    Valid,
    /// A record of the kind is there and no longer holds: `reason` says why.
    Void,
    /// The item holds no record of the kind.
    Absent,
}

/// Why a record of an attestation no longer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum VoidReason {
    /// The item's authored content is not the content the record cleared.
    ContentMoved,
    /// The bar asked about is not the bar the record cleared the content against.
    BarMoved,
}

/// What `attestation check` answers: one kind on one item.
///
/// Every member is always written, `reason` and `record` as `null` when they have nothing to
/// say.
// llmlint: ignore-block[invalid_states_unrepresentable] The task fixes this answer's JSON literally as `{"kind", "state", "reason": null|…, "record": {…}|null}`, four flat members every one always written, and both SDKs and the orchestrators gating on it read that shape; an enum carrying each state's fields would serialise differently. The one constructor, `check` below, only ever builds the three consistent combinations, and `a_check_says_content_before_bar` pins each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttestationCheck {
    /// The kind asked about.
    pub kind: AttestationKind,
    /// What the check found.
    pub state: AttestationState,
    /// Why the record is void: `null` unless `state` is `void`.
    pub reason: Option<VoidReason>,
    /// The record of the kind the item holds, or `null` when it holds none.
    pub record: Option<Attestation>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl Attestation {
    /// The record as the item's metadata holds it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let text = |text: &str| Value::String(text.to_owned());
        let mut record = Map::new();
        record.insert("kind".to_owned(), text(self.kind.as_str()));
        record.insert("by".to_owned(), text(self.by.as_str()));
        record.insert("at".to_owned(), Value::String(stamp(self.at)));
        record.insert(
            "content_digest".to_owned(),
            text(self.content_digest.as_str()),
        );
        record.insert("bar".to_owned(), text(self.bar.as_str()));
        if let Some(note) = &self.note {
            record.insert("note".to_owned(), text(note.as_str()));
        }
        if !self.excludes.is_empty() {
            record.insert(
                "excludes".to_owned(),
                Value::Array(self.excludes.iter().map(|key| text(key.as_str())).collect()),
            );
        }
        Value::Object(record)
    }
}

impl AttestationCheck {
    /// Whether the record holds.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.state == AttestationState::Valid
    }
}

/// One task's entry in a project's check: the task's qualified id beside what its check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskAttestationCheck {
    /// The task, by qualified id.
    pub id: GlobalId,
    /// What its check found.
    #[serde(flatten)]
    pub check: AttestationCheck,
}

/// What `project attestation check --tasks` answers: one entry per task filed under the
/// project, in the order the source lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectAttestationCheck {
    /// The project, by qualified id.
    pub project: GlobalId,
    /// The kind asked about.
    pub kind: AttestationKind,
    /// One entry per task.
    pub tasks: Vec<TaskAttestationCheck>,
}

impl ProjectAttestationCheck {
    /// Whether every task's record holds — a project with no task included.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.tasks.iter().all(|task| task.check.is_valid())
    }
}

/// What `attest` answers: the record it wrote, on the item it wrote it to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attested {
    /// The item, by qualified id.
    pub id: GlobalId,
    /// The record as the source holds it after the write.
    pub record: Attestation,
}

/// One far end of an edge, as the digest names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FarEnd {
    /// By its lineage root: the qualified id that roots its chain of copies, as it records it —
    /// its own, for an item copied from nowhere — so it reads the same from every copy of it.
    Lineage(GlobalId),
    /// By its spelling, for a far end no configured source can be asked about.
    Id(String),
    /// By the tracker its designator names and its id there, for an opaque far end: the same
    /// id in another tracker is another item.
    Opaque {
        /// The tracker the endpoint's designator names.
        designator: Designator,
        /// Its id in that tracker.
        id: String,
    },
}

/// One of an item's own forward dependency edges, as the digest reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoredEdge {
    /// What the edge means.
    pub kind: DependencyKind,
    /// What its far end is.
    pub far: ItemKind,
    /// Its far end.
    pub end: FarEnd,
}

impl AuthoredEdge {
    /// The edge as the digest spells it: `[<edge kind>, <far end's kind>, "lineage:…" | "id:…" |
    /// "opaque:<designator>:<id>"]` — a designator holds no `:`, so the last reads one way.
    fn spelled(&self) -> [String; 3] {
        let kind = match self.kind {
            DependencyKind::Blocks => "blocks",
            DependencyKind::Related => "related",
        };
        let far = match self.far {
            ItemKind::Task => "task",
            ItemKind::Project => "project",
        };
        let end = match &self.end {
            FarEnd::Lineage(root) => format!("lineage:{root}"),
            FarEnd::Id(id) => format!("id:{id}"),
            FarEnd::Opaque { designator, id } => format!("opaque:{designator}:{id}"),
        };
        [kind.to_owned(), far.to_owned(), end]
    }
}

/// The item's authored content, as the digest reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Authored<'a> {
    /// Its title.
    pub title: &'a str,
    /// Its body.
    pub content: Option<&'a str>,
    /// Its repositories.
    pub repositories: &'a [Repository],
    /// Its whole metadata, as a read answers it: the caller's keys are taken from it, and the
    /// `onetaskgraph.template` entry's `answers_digest`.
    pub metadata: &'a BTreeMap<String, Value>,
    /// Its own forward dependency edges.
    pub depends_on: Vec<AuthoredEdge>,
}

/// The digest of `authored`, leaving out the caller-owned metadata keys `excludes` names.
#[must_use]
pub fn content_digest(authored: &Authored<'_>, excludes: &[MetadataKey]) -> Sha256Digest {
    let metadata: Map<String, Value> = authored
        .metadata
        .iter()
        .filter(|(key, _)| MetadataKey::new(key.as_str()).is_ok())
        .filter(|(key, _)| {
            !excludes
                .iter()
                .any(|excluded| excluded.as_str() == key.as_str())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let answers = authored
        .metadata
        .get(MetadataKey::TEMPLATE_KEY)
        .and_then(|entry| entry.get("answers_digest"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut repositories: Vec<&str> = authored
        .repositories
        .iter()
        .map(Repository::as_str)
        .collect();
    repositories.sort_unstable();
    let mut edges: Vec<[String; 3]> = authored
        .depends_on
        .iter()
        .map(AuthoredEdge::spelled)
        .collect();
    edges.sort();
    edges.dedup();
    let document = json!({
        "title": authored.title,
        "content": authored.content,
        "answers_digest": answers,
        "repositories": repositories,
        "depends_on": edges,
        "metadata": metadata,
    });
    Sha256Digest::of(canonical(&document).as_bytes())
}

/// `value` as canonical JSON: keys sorted at every depth, no insignificant whitespace.
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(entries) => {
            let sorted: BTreeMap<&String, &Value> = entries.iter().collect();
            let inner: Vec<String> = sorted
                .into_iter()
                .map(|(key, entry)| format!("{}:{}", Value::String(key.clone()), canonical(entry)))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(entries) => {
            let inner: Vec<String> = entries.iter().map(canonical).collect();
            format!("[{}]", inner.join(","))
        }
        scalar => scalar.to_string(),
    }
}

/// Every entry of the attestations `metadata` holds, by kind, as it is held — readable or not,
/// so a write replacing one kind keeps every other exactly as it was — and none when it holds no
/// attestations entry. [`record_of`] is what reads one as a record.
#[must_use]
pub fn records(metadata: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    match metadata.get(MetadataKey::ATTESTATIONS_KEY) {
        Some(Value::Object(entries)) => entries
            .iter()
            .map(|(kind, record)| (kind.clone(), record.clone()))
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// The record of `kind` that `metadata` holds, when it holds one this product can read.
#[must_use]
pub fn record_of(
    metadata: &BTreeMap<String, Value>,
    kind: &AttestationKind,
) -> Option<Attestation> {
    records(metadata)
        .get(kind.as_str())
        .and_then(|record| serde_json::from_value::<Attestation>(record.clone()).ok())
        .filter(|record| record.kind == *kind)
}

/// Hold `record` to the item's current `digest` and, when one is asked about, to `bar`.
#[must_use]
pub fn check(
    kind: &AttestationKind,
    record: Option<Attestation>,
    digest: impl FnOnce(&[MetadataKey]) -> Sha256Digest,
    bar: Option<&AttestationLabel>,
) -> AttestationCheck {
    let Some(record) = record else {
        return AttestationCheck {
            kind: kind.clone(),
            state: AttestationState::Absent,
            reason: None,
            record: None,
        };
    };
    let reason = if digest(&record.excludes) != record.content_digest {
        Some(VoidReason::ContentMoved)
    } else if bar.is_some_and(|bar| *bar != record.bar) {
        Some(VoidReason::BarMoved)
    } else {
        None
    };
    AttestationCheck {
        kind: kind.clone(),
        state: if reason.is_some() {
            AttestationState::Void
        } else {
            AttestationState::Valid
        },
        reason,
        record: Some(record),
    }
}

/// The instant a record is recorded at: now, to the second.
#[must_use]
pub fn now() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(0)
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authored(metadata: &BTreeMap<String, Value>) -> Authored<'_> {
        Authored {
            title: "T",
            content: Some("body"),
            repositories: &[],
            metadata,
            depends_on: vec![AuthoredEdge {
                kind: DependencyKind::Blocks,
                far: ItemKind::Task,
                end: FarEnd::Lineage("src:1".parse().expect("a qualified id")),
            }],
        }
    }

    fn key(key: &str) -> MetadataKey {
        MetadataKey::new(key).expect("a caller key")
    }

    #[test]
    fn the_digest_reads_caller_keys_and_answers_and_never_reserved_ones() {
        let mut metadata = BTreeMap::new();
        metadata.insert("myapp.size".to_owned(), json!(3));
        let first = content_digest(&authored(&metadata), &[]);

        // A reserved key — the attestations themselves among them — never moves it.
        metadata.insert(MetadataKey::ATTESTATIONS_KEY.to_owned(), json!({"a": 1}));
        metadata.insert("onetaskgraph.origin".to_owned(), json!("x:1"));
        assert_eq!(content_digest(&authored(&metadata), &[]), first);

        // The answers digest of the provenance entry does, and nothing else of the entry.
        metadata.insert(
            MetadataKey::TEMPLATE_KEY.to_owned(),
            json!({"answers_digest": "sha256:a", "body_digest": "sha256:b"}),
        );
        let answered = content_digest(&authored(&metadata), &[]);
        assert_ne!(answered, first);
        metadata.insert(
            MetadataKey::TEMPLATE_KEY.to_owned(),
            json!({"answers_digest": "sha256:a", "body_digest": "sha256:c"}),
        );
        assert_eq!(content_digest(&authored(&metadata), &[]), answered);

        // A caller key moves it, unless it is excluded.
        metadata.insert("myapp.size".to_owned(), json!(4));
        assert_ne!(content_digest(&authored(&metadata), &[]), answered);
        let mut without = metadata.clone();
        without.remove("myapp.size");
        assert_eq!(
            content_digest(&authored(&metadata), &[key("myapp.size")]),
            content_digest(&authored(&without), &[])
        );

        // An edge to another far end moves it.
        let mut moved = authored(&metadata);
        moved.depends_on[0].end = FarEnd::Id("src:9".into());
        assert_ne!(
            content_digest(&moved, &[]),
            content_digest(&authored(&metadata), &[])
        );
    }

    fn record(kind: &AttestationKind) -> Attestation {
        Attestation {
            kind: kind.clone(),
            by: AttestationLabel::new("--by", "ada").expect("a label"),
            at: DateTime::UNIX_EPOCH,
            content_digest: Sha256Digest::of(b"x"),
            bar: AttestationLabel::new("--bar", "bar-1").expect("a label"),
            note: None,
            excludes: Vec::new(),
        }
    }

    #[test]
    fn a_check_says_content_before_bar() {
        let kind = AttestationKind::new("design-approval").expect("a kind");
        let held = || Sha256Digest::of(b"x");
        let bar = |text: &str| AttestationLabel::new("--bar", text).expect("a bar");
        let valid = check(&kind, Some(record(&kind)), |_| held(), Some(&bar("bar-1")));
        assert_eq!((valid.state, valid.reason), (AttestationState::Valid, None));
        let other = check(&kind, Some(record(&kind)), |_| held(), Some(&bar("bar-2")));
        assert_eq!(other.reason, Some(VoidReason::BarMoved));
        let moved = check(
            &kind,
            Some(record(&kind)),
            |_| Sha256Digest::of(b"y"),
            Some(&bar("bar-2")),
        );
        assert_eq!(moved.reason, Some(VoidReason::ContentMoved));
        let absent = check(&kind, None, |_| unreachable!(), None);
        assert_eq!(
            (absent.state, absent.record),
            (AttestationState::Absent, None)
        );
    }

    #[test]
    fn a_record_reads_back_as_it_serialises() {
        let kind = AttestationKind::new("plan-review").expect("a kind");
        let noted = Attestation {
            note: Some(AttestationNote::new("n").expect("a note")),
            excludes: vec![key("myapp.k")],
            ..record(&kind)
        };
        assert_eq!(
            noted.to_value(),
            serde_json::to_value(&noted).expect("JSON")
        );
        let bare = record(&kind);
        assert_eq!(bare.to_value(), serde_json::to_value(&bare).expect("JSON"));
        assert!(bare.to_value().get("note").is_none());
        assert_eq!(bare.to_value()["at"], "1970-01-01T00:00:00Z");
        let mut held = BTreeMap::new();
        held.insert(
            MetadataKey::ATTESTATIONS_KEY.to_owned(),
            json!({"plan-review": noted.to_value(), "other": {"kind": "other"}}),
        );
        assert_eq!(record_of(&held, &kind), Some(noted));
        let other = AttestationKind::new("other").expect("a kind");
        assert_eq!(
            record_of(&held, &other),
            None,
            "an unreadable record is passed over"
        );
    }

    #[test]
    fn a_kind_and_a_label_are_refused_by_their_flag() {
        assert!(AttestationKind::new("plan-review").is_ok());
        assert_eq!(AttestationKind::new("").unwrap_err().field, "--kind");
        assert!(AttestationKind::new("two words").is_err());
        assert_eq!(
            AttestationLabel::new("--by", "a\nb").unwrap_err().field,
            "--by"
        );
        assert_eq!(
            AttestationLabel::new("--bar", " ").unwrap_err().field,
            "--bar"
        );
        assert!(AttestationNote::new("a\nb").is_ok());
        assert_eq!(AttestationNote::new("").unwrap_err().field, "--note");
        assert_eq!(stamp(DateTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }
}
