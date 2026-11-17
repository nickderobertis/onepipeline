//! Public projections: the neutral public views a project copy makes of a private
//! coordination project's public tasks, and the one map that ties each view to its canonical
//! task.
//!
//! **This module is the one definition of that map.** It lives on the private destination
//! project alone, at [`PublicProjections::KEY`], as
//! `{"schema_version": 1, "tasks": [{"task": "<source>:<id>", "projection": "<source>:<id>"}]}`:
//! `task` is the canonical task the private destination holds, `projection` its public view.
//! The copy that writes it, the re-copy that reads it back, the status mirror and both SDKs'
//! serialization tests all go through [`PublicProjections`], so no second spelling of the
//! shape exists to drift from this one.
//!
//! A projection is a **view**, never another executable task: it carries its canonical task's
//! screened title, content, repositories and status, its dependencies only on other
//! projections, its links only to its own headings and to other projections, and nothing else — no metadata, no origin, no link back, no name of the
//! private project. Everything it is made from is read from the canonical records on every
//! copy, so an edit made to a view is overwritten by the next copy and never reaches the
//! canonical graph.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use onetaskgraph_plugin_api::{
    AssetPayload, Classification, DependencyEdge, DependencyEndpoint, DependencyKind, ItemKind,
    ItemWrite, MetadataKey, NativeId, Priority, Project, Repository, SourceError, SourceName,
    Status, StatusCategory, Task, WriteTarget,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::super::assets;
use super::super::boundary::{Exposure, Outbound};
use super::super::fetch::unrepeated;
use super::super::{Engine, EngineError, Filters, Paging, ProjectRequest};
use super::{
    CopyOutcome, Creating, Item, Journal, Level, Planned, Undo, classification_of, create_refused,
    described, far_end,
};
use super::{PROJECT_PAGE, Prior, refused, remove};
use crate::GlobalId;
use crate::config::SourceVisibility;
use crate::resolve::ResolvedSource;

/// The private destination project's map from each canonical task to its public projection.
///
/// Every value of it holds to the shape: its `schema_version` is
/// [`PublicProjections::SCHEMA_VERSION`] and its entries are unique by both ids. Deserializing
/// one is held to both — [`PublicProjections::parse`] names what is wrong — and the entries
/// change only through [`PublicProjections::set`] and [`PublicProjections::retain`], which keep
/// them so. Written back with [`PublicProjections::to_value`].
// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] This type is the one source, and both SDKs' models are generated from its schema; what the schema cannot carry is uniqueness by a member, which JSON Schema has no keyword for. Nothing but the engine ever writes the map — the reserved key is refused for every caller's metadata write, the SDKs' included — so a generated model only reads maps this `TryFrom` already held to that rule, and a tampered map is refused here, as the journeys prove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, try_from = "Unchecked")]
pub struct PublicProjections {
    /// The version of this shape; [`PublicProjections::SCHEMA_VERSION`] is the only one.
    #[schemars(range(min = 1, max = 1))]
    schema_version: u32,
    /// One entry per canonical task that has a public projection, unique by both ids.
    tasks: Vec<PublicProjection>,
    /// One entry per public source the plan's views are filed in, unique by source: the
    /// allocation its public projection project is created under, and the id that source
    /// assigned it once it answered. Absent when empty, so a map recording none reads and
    /// writes exactly as one written before this member existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    projects: Vec<PublicProjectAllocation>,
}

/// One canonical task and its public projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicProjection {
    /// The canonical task, qualified: the one the private destination holds.
    pub task: GlobalId,
    /// Its public counterpart, qualified: a view in a source not declared private.
    pub projection: GlobalId,
}

/// The public projection project of one plan in one public source: the allocation recorded
/// privately before it was created, and the id the source assigned it, recorded before any
/// view is filed under it.
///
/// The allocation is a random nonce, derived from nothing private, that the project itself
/// carries at [`PublicProjectAllocation::KEY`]: a copy that never learned the id — its create's
/// answer lost, or the copy ended before it recorded one — finds the project it made by that
/// nonce alone, and never takes any other project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicProjectAllocation {
    /// The public source the project is in.
    pub source: SourceName,
    /// The nonce the project is created carrying, at [`PublicProjectAllocation::KEY`].
    pub allocation: Allocation,
    /// The id the source assigned the project, once it answered with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<NativeId>,
}

impl PublicProjectAllocation {
    /// The reserved metadata key a public projection project carries its allocation under.
    pub const KEY: &'static str = "onetaskgraph.allocation";
}

/// The language every [`Allocation`] is a word of: 128 bits as 32 lower-case hex digits.
pub const ALLOCATION_PATTERN: &str = "^[0-9a-f]{32}$";

/// The random nonce a public projection project is created carrying: 32 lower-case hex digits,
/// derived from nothing private, so it names nothing but the project created behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Allocation(String);

impl Allocation {
    /// Validate and wrap an allocation.
    ///
    /// # Errors
    ///
    /// A message naming `value` when it is not a word of [`ALLOCATION_PATTERN`].
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        // The language `ALLOCATION_PATTERN` states, hand-rolled; the unit test
        // `an_allocation_is_exactly_what_its_pattern_states` holds the two to one rule.
        if value.len() == 32
            && value
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        {
            Ok(Self(value))
        } else {
            Err(format!(
                "allocation {value:?} is not 32 lower-case hex digits ({ALLOCATION_PATTERN})"
            ))
        }
    }

    /// The allocation as it is written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A fresh allocation: 128 bits from the process's own randomly keyed hashers, the clock
    /// and the process id, digested — a nonce that names nothing it came from.
    #[must_use]
    pub fn fresh() -> Self {
        use std::hash::{BuildHasher, Hasher};
        let mut digest = Sha256::new();
        for _ in 0..2 {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u32(std::process::id());
            digest.update(hasher.finish().to_le_bytes());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        digest.update(now.to_le_bytes());
        Self(
            digest
                .finalize()
                .iter()
                .take(16)
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        )
    }
}

impl TryFrom<String> for Allocation {
    type Error = String;

    fn try_from(value: String) -> Result<Self, String> {
        Self::new(value)
    }
}

impl From<Allocation> for String {
    fn from(allocation: Allocation) -> Self {
        allocation.0
    }
}

impl JsonSchema for Allocation {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Allocation".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": ALLOCATION_PATTERN,
            "description": "The random nonce a public projection project is created carrying.",
        })
    }
}

/// The private destination project's map from each canonical task to its public projection,
/// as it was stored and before it is held to the shape — which is the schema a reader of it is
/// generated from, so it carries [`PublicProjections`]'s name and words.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "PublicProjections")]
struct Unchecked {
    /// The version of this shape; 1 is the only one.
    #[schemars(range(min = 1, max = 1))]
    schema_version: u32,
    /// One entry per canonical task that has a public projection, unique by both ids.
    tasks: Vec<PublicProjection>,
    /// One entry per public source the plan's views are filed in, unique by source: the
    /// allocation its public projection project is created under, and the id that source
    /// assigned it once it answered.
    #[serde(default)]
    projects: Vec<PublicProjectAllocation>,
}

impl TryFrom<Unchecked> for PublicProjections {
    type Error = String;

    fn try_from(map: Unchecked) -> Result<Self, String> {
        if map.schema_version != Self::SCHEMA_VERSION {
            return Err(format!(
                "{} is schema_version {}, and this engine reads only {}",
                Self::KEY,
                map.schema_version,
                Self::SCHEMA_VERSION
            ));
        }
        let mut tasks = BTreeSet::new();
        let mut projections = BTreeSet::new();
        for entry in &map.tasks {
            if !tasks.insert(entry.task.to_string()) {
                return Err(format!("{} names task {} twice", Self::KEY, entry.task));
            }
            if !projections.insert(entry.projection.to_string()) {
                return Err(format!(
                    "{} names projection {} twice",
                    Self::KEY,
                    entry.projection
                ));
            }
        }
        let mut sources = BTreeSet::new();
        for entry in &map.projects {
            if !sources.insert(entry.source.to_string()) {
                return Err(format!(
                    "{} names a project in {} twice",
                    Self::KEY,
                    entry.source
                ));
            }
            if entry
                .project
                .as_ref()
                .is_some_and(|project| project.0.trim().is_empty())
            {
                return Err(format!(
                    "{} names an empty project for {}",
                    Self::KEY,
                    entry.source
                ));
            }
        }
        Ok(Self {
            schema_version: map.schema_version,
            tasks: map.tasks,
            projects: map.projects,
        })
    }
}

impl Default for PublicProjections {
    fn default() -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            tasks: Vec::new(),
            projects: Vec::new(),
        }
    }
}

impl PublicProjections {
    /// The reserved metadata key the private destination project records the map under.
    pub const KEY: &'static str = "onetaskgraph.public_projections";

    /// The one version of the shape this engine reads and writes.
    pub const SCHEMA_VERSION: u32 = 1;

    /// The map `metadata` records, or an empty one when it records none.
    ///
    /// # Errors
    ///
    /// A message saying what is wrong with a recorded value: not this shape, another
    /// `schema_version`, or two entries naming one task or one projection.
    pub fn read(metadata: &BTreeMap<String, Value>) -> Result<Self, String> {
        match metadata.get(Self::KEY) {
            None => Ok(Self::default()),
            Some(value) => Self::parse(value),
        }
    }

    /// One recorded value, held to every rule of the shape.
    ///
    /// # Errors
    ///
    /// As [`read`](Self::read).
    pub fn parse(value: &Value) -> Result<Self, String> {
        serde_json::from_value(value.clone()).map_err(|error| {
            format!(
                "{} is not {{\"schema_version\": 1, \"tasks\": [{{\"task\": …, \"projection\": \
                 …}}]}} unique by both ids: {error}",
                Self::KEY
            )
        })
    }

    /// The map as the private destination project records it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("the projection map serializes")
    }

    /// Every entry, in the order they were recorded.
    #[must_use]
    pub fn tasks(&self) -> &[PublicProjection] {
        &self.tasks
    }

    /// The projection the map records for `task`.
    #[must_use]
    pub fn projection_of(&self, task: &GlobalId) -> Option<&GlobalId> {
        self.tasks
            .iter()
            .find(|entry| &entry.task == task)
            .map(|entry| &entry.projection)
    }

    /// Record `projection` as `task`'s, in place of any it recorded, answering whether the
    /// map changed. An entry another task held naming the same projection is dropped, so the
    /// map stays unique by both ids.
    pub fn set(&mut self, task: &GlobalId, projection: &GlobalId) -> bool {
        if self.projection_of(task) == Some(projection) {
            return false;
        }
        self.tasks
            .retain(|entry| &entry.task != task && &entry.projection != projection);
        self.tasks.push(PublicProjection {
            task: task.clone(),
            projection: projection.clone(),
        });
        true
    }

    /// The public projection project the map records in `source`.
    #[must_use]
    pub fn project_in(&self, source: &SourceName) -> Option<&PublicProjectAllocation> {
        self.projects.iter().find(|entry| &entry.source == source)
    }

    /// Record `entry` as its source's project, in place of any it recorded, answering whether
    /// the map changed.
    pub fn set_project(&mut self, entry: PublicProjectAllocation) -> bool {
        if self.project_in(&entry.source) == Some(&entry) {
            return false;
        }
        self.projects.retain(|held| held.source != entry.source);
        self.projects.push(entry);
        true
    }

    /// Every public projection project the map records, in the order they were recorded.
    #[must_use]
    pub fn projects(&self) -> &[PublicProjectAllocation] {
        &self.projects
    }

    /// Keep only the entries `keep` answers `true` for, answering whether any was dropped.
    pub fn retain(&mut self, keep: impl FnMut(&PublicProjection) -> bool) -> bool {
        let before = self.tasks.len();
        self.tasks.retain(keep);
        self.tasks.len() != before
    }
}

/// Which view a status mirror could not reach: the one the map names, or — when the map itself
/// could not be read — whichever the map on the private project would have named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirrorTarget {
    /// The view the map names.
    View(GlobalId),
    /// The map recorded on this private project, which could not be read.
    MapOn(GlobalId),
}

impl std::fmt::Display for MirrorTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::View(view) => write!(formatter, "{view}"),
            Self::MapOn(project) => write!(formatter, "recorded on {project}"),
        }
    }
}

/// The opaque native id a public projection of `of` is asked to be created under.
///
/// Stable — the same `of` always answers the same id, so a copy interrupted between creating a
/// view and recording it finds that view again by this id rather than creating a second — and
/// opaque: a digest that names nothing it was derived from. A destination that issues ids of
/// its own is free to ignore it, as it is any id a create is handed.
#[must_use]
pub fn opaque_id(prefix: &str, of: &GlobalId) -> NativeId {
    let mut digest = Sha256::new();
    digest.update(b"onetaskgraph public projection\0");
    digest.update(prefix.as_bytes());
    digest.update(b"\0");
    digest.update(of.to_string().as_bytes());
    let hex: String = digest
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    NativeId(format!("{prefix}-{hex}"))
}

/// The neutral title every public projection project carries.
pub const PUBLIC_PROJECT_TITLE: &str = "Public work";

/// What in `content` a public projection could not carry: every Markdown link or image whose
/// destination is neither a URL, a bare fragment nor one of `assets` as `./<name>`, and every
/// piece of raw HTML that could name a destination at all — a tag with any attribute but an
/// anchor's lone `id` or `name`, or a `style` or `script` element — since a view cannot tell
/// what such HTML points at. An anchor naming only itself is carried as written, so a fragment
/// link to it from the same view still resolves. Code spans and blocks hold neither, so nothing
/// in them is one.
#[must_use]
pub fn unresolved_references(content: &str, assets: &[String]) -> Vec<String> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut unresolved: Vec<String> = Vec::new();
    let mut note = |found: String| {
        if !unresolved.contains(&found) {
            unresolved.push(found);
        }
    };
    // Text too, each run of it whole — the parser hands it over in pieces split at `<` — since
    // what CommonMark reads as text, `<img/src=…>`, another renderer may read as a tag.
    let mut text = String::new();
    // A code block's text is code, shown as written, however it reads.
    let mut in_code = false;
    for event in Parser::new_ext(content, Options::empty()) {
        match &event {
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => in_code = false,
            Event::Text(_) if in_code => continue,
            Event::Text(piece) => {
                text.push_str(piece);
                continue;
            }
            _ => {}
        }
        if let Some(tag) = opaque_html(&std::mem::take(&mut text)) {
            note(tag);
        }
        match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                let destination = dest_url.trim();
                if !carried(destination, assets) {
                    note(destination.to_owned());
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                if let Some(tag) = opaque_html(&html) {
                    note(tag);
                }
            }
            _ => {}
        }
    }
    if let Some(tag) = opaque_html(&text) {
        note(tag);
    }
    unresolved
}

/// Whether a view can carry a link to `destination`: a URL of a scheme a reader follows over
/// the network or mail, a fragment of the view itself, or one of its own `assets`.
fn carried(destination: &str, assets: &[String]) -> bool {
    if destination.starts_with('#') {
        return true;
    }
    if let Some(name) = destination.strip_prefix("./") {
        return assets.iter().any(|asset| asset == name);
    }
    let Some((scheme, rest)) = destination.split_once(':') else {
        return false;
    };
    let named = scheme.len() > 1
        && scheme
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "+-.".contains(character));
    named
        && !scheme.eq_ignore_ascii_case("file")
        && (rest.starts_with("//") || scheme.eq_ignore_ascii_case("mailto"))
}

/// The first tag of raw HTML that could name a destination — one carrying anything past its
/// name but an anchor's lone `id` or `name`, or opening a `style` or `script` element — as
/// written; `None` when every tag is a bare name or such an anchor, neither of which names
/// anywhere.
fn opaque_html(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        let tag = &rest[open..];
        let body = tag[1..].trim_start_matches(['/', '!', '?']);
        // Only `<` straight before a name opens a tag: `a < b` is prose.
        if !body.starts_with(|character: char| character.is_ascii_alphabetic()) {
            rest = &tag[1..];
            continue;
        }
        let name_end = body
            .find(|character: char| !character.is_ascii_alphanumeric() && character != '-')
            .unwrap_or(body.len());
        let name = &body[..name_end];
        let attributes_at = tag.len() - body.len() + name_end;
        let Some((attributes, length)) = attributes(&tag[attributes_at..]) else {
            return Some(tag.to_owned());
        };
        let end = attributes_at + length;
        let anchor = name.eq_ignore_ascii_case("a")
            && !tag[1..].starts_with(['/', '!', '?'])
            && matches!(attributes.as_slice(), [(key, Some(value))]
                if (key.eq_ignore_ascii_case("id") || key.eq_ignore_ascii_case("name"))
                    && anchor_name(value));
        if !(attributes.is_empty() || anchor)
            || name.eq_ignore_ascii_case("style")
            || name.eq_ignore_ascii_case("script")
        {
            return Some(tag[..end].to_owned());
        }
        rest = &tag[end..];
    }
    None
}

/// One attribute of a tag, as written: its name and, when it is given one, its value.
type Attribute<'html> = (&'html str, Option<&'html str>);

/// The attributes of a tag whose name has been read, and how many bytes of `after` they and
/// the closing `>` span — read as HTML reads them, so a `>` inside a quoted value does not end
/// the tag. A tag with no `>` ends with the text; one whose quoted value never closes, or with
/// an `=` where a name belongs, is `None`, and is never carried.
fn attributes(after: &str) -> Option<(Vec<Attribute<'_>>, usize)> {
    let mut read = Vec::new();
    let mut at = 0;
    let bytes = after.as_bytes();
    let skip = |at: &mut usize| {
        while *at < bytes.len() && (bytes[*at].is_ascii_whitespace() || bytes[*at] == b'/') {
            *at += 1;
        }
    };
    loop {
        skip(&mut at);
        if at >= bytes.len() {
            return Some((read, at));
        }
        if bytes[at] == b'>' {
            return Some((read, at + 1));
        }
        let start = at;
        while at < bytes.len() && !b" \t\n\r\x0c/>=".contains(&bytes[at]) {
            at += 1;
        }
        let key = &after[start..at];
        if key.is_empty() {
            return None;
        }
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= bytes.len() || bytes[at] != b'=' {
            read.push((key, None));
            continue;
        }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let value = match bytes.get(at) {
            Some(quote @ (b'"' | b'\'')) => {
                let close = after[at + 1..].find(char::from(*quote))?;
                let value = &after[at + 1..at + 1 + close];
                at += close + 2;
                value
            }
            _ => {
                let start = at;
                while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'>' {
                    at += 1;
                }
                &after[start..at]
            }
        };
        read.push((key, Some(value)));
    }
}

/// Whether `value` is an anchor name an in-page link can reach and nothing can load: letters,
/// digits, `-`, `_` and `.` alone, so no entity, scheme or path hides in it.
fn anchor_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
}

/// One public task of a private plan a project copy projects, as its canonical record read.
pub(super) struct Projected {
    /// The canonical task, as the copy's source holds it.
    source: GlobalId,
    /// The source its repositories route it to, which is not declared private.
    to: SourceName,
    /// The task as its source read it, which the view is made from and nothing else is.
    task: Task,
    /// The image assets its content references, with their bytes.
    assets: Vec<AssetPayload>,
    /// The tasks of the same plan it depends on, by their source ids.
    depends_on: Vec<GlobalId>,
    /// What its content links to among the views landing beside it, when it links to any.
    links: Option<ViewLinks>,
}

/// One view's links to the other views of its destination, read before its content was reduced
/// to what the boundary check sees. Held in memory for the copy: the source's addresses it holds
/// are what identify each link, and never reach a rendering a destination is sent.
struct ViewLinks {
    /// The task's content before any link was reduced to its text.
    authored: String,
    /// The other views' tasks it names, each to be asked of what has landed.
    referents: Vec<super::sections::Linked>,
    /// What a rendering of it is screened of: every private address, and the source's address
    /// of each referent, for a link whose view has not landed.
    screened: Vec<String>,
    /// The fragments it would carry naming no heading, which only the boundary check vouches for.
    carried_fragments: Vec<String>,
}

impl Projected {
    /// The tasks whose views this one's content links to.
    fn linked(&self) -> impl Iterator<Item = &GlobalId> {
        self.links
            .iter()
            .flat_map(|links| &links.referents)
            .map(|referent| &referent.id)
    }

    /// Every task this view is better landed after: what it depends on, then what it links to.
    fn landed_after(&self) -> impl Iterator<Item = &GlobalId> {
        self.depends_on.iter().chain(self.linked())
    }
}

/// One task's record as every view of its plan may link to it.
struct Sibling {
    id: GlobalId,
    to: SourceName,
    location: String,
    content: Option<String>,
    stored: Option<Vec<onetaskgraph_plugin_api::StoredHeading>>,
}

impl Projected {
    /// What the copy writes as this task's public view: `id` the id it is asked to be created
    /// under, filed under the public project `project`. Its title, content, repositories and
    /// status, and nothing else of the canonical record — no labels, priority, metadata,
    /// deliveries, origin or classification beyond public.
    fn view(&self, id: NativeId, project: Option<NativeId>) -> Task {
        Task {
            id,
            key: None,
            title: self.task.title.clone(),
            content: self.task.content.clone(),
            status: self.task.status.clone(),
            priority: Priority::None,
            labels: Vec::new(),
            project,
            url: None,
            location: None,
            created_at: None,
            updated_at: None,
            metadata: BTreeMap::new(),
            repositories: self.task.repositories.clone(),
            delivers: Vec::new(),
            delivered_by: Vec::new(),
            version: None,
            classification: Classification::Public,
            assignees: None,
        }
    }

    /// How a refusal names the view: by its canonical task, which only the caller reads.
    fn item(&self) -> String {
        format!("the public projection of {}", self.source)
    }
}

/// The neutral project every view of one plan in one public source is filed under, naming the
/// views' own public repositories — a board files a view only under a project issue of its
/// repository's owner — and nothing else.
fn public_project(id: NativeId, views: &[&Projected]) -> Project {
    let mut repositories: Vec<Repository> = Vec::new();
    for view in views {
        for repository in &view.task.repositories {
            if !repositories.contains(repository) {
                repositories.push(repository.clone());
            }
        }
    }
    Project {
        id,
        title: PUBLIC_PROJECT_TITLE.to_owned(),
        content: None,
        status: Status {
            category: StatusCategory::Todo,
            name: "todo".to_owned(),
        },
        labels: Vec::new(),
        url: None,
        location: None,
        created_at: None,
        updated_at: None,
        metadata: BTreeMap::new(),
        repositories,
        version: None,
        classification: Classification::Public,
    }
}

/// Whether `project` reads as a public projection project: public, titled
/// [`PUBLIC_PROJECT_TITLE`], with no prose of its own.
fn is_neutral(project: &Project) -> bool {
    project.title == PUBLIC_PROJECT_TITLE
        && project.classification == Classification::Public
        && project.content.as_deref().is_none_or(str::is_empty)
}

/// `id` as a create or a write of `method` answered it, held to naming something: an empty id
/// can be neither journaled nor recorded in the map, which refuses one when it is read back.
///
/// # Errors
///
/// [`SourceError::Malformed`] naming `method`, for an id that is empty or only whitespace.
fn answered_id(id: NativeId, method: &str) -> Result<NativeId, SourceError> {
    if id.0.trim().is_empty() {
        return Err(SourceError::Malformed {
            message: format!("{method} answered an empty id"),
        });
    }
    Ok(id)
}

/// The `(name, sha256)` of every asset in `assets`, in name order.
fn digests(assets: &[AssetPayload]) -> Vec<(String, String)> {
    let mut digests: Vec<(String, String)> = assets
        .iter()
        .map(|asset| (asset.name.as_str().to_owned(), asset.sha256.clone()))
        .collect();
    digests.sort_unstable();
    digests
}

/// Whether one private plan's copy makes public views, and of which of its tasks.
///
/// Only an active plan holds tasks: a copy that makes no views has nowhere to note one.
#[derive(Default)]
pub(super) enum Projecting {
    /// The plan makes no view: it is not private, its home is not declared private, or it
    /// neither has a public task to show nor a map recorded before to bring in step.
    #[default]
    Inactive,
    /// The plan's public tasks to make views of — none, when the map recorded before still has
    /// to forget a task that has no view any more.
    Active(Vec<Projected>),
}

impl Projecting {
    /// A plan whose public tasks this copy makes views of, none noted yet.
    pub(super) fn active() -> Self {
        Self::Active(Vec::new())
    }

    /// Whether this copy has views to land, or a recorded map to bring in step.
    pub(super) fn lands(&self) -> bool {
        matches!(self, Self::Active(_))
    }

    /// A view without an address cannot expose a private record's location.
    pub(super) fn could_address(&self) -> bool {
        self.tasks().iter().any(|task| {
            task.task
                .content
                .as_deref()
                .is_some_and(super::privacy::could_address)
        })
    }

    /// This plan, active only when it has a view to land or `project`'s destination already
    /// records a map to bring in step — so a private plan with neither reads nothing more.
    pub(super) fn settled(self, project: &Planned) -> Self {
        let recorded = project
            .held
            .as_ref()
            .is_some_and(|held| described(&held.item).1.contains_key(PublicProjections::KEY));
        match self {
            Self::Active(tasks) if !tasks.is_empty() || recorded => Self::Active(tasks),
            Self::Active(_) | Self::Inactive => Self::Inactive,
        }
    }

    /// Restore authored same-record links before the public boundary examines a view, and
    /// leave private cross-record links as text alone. A link to another view's task is text in
    /// what the boundary examines too, and is noted so its landing can point it at that view,
    /// whose address is public and unknown until it has landed.
    pub(super) fn public_bodies(
        &mut self,
        engine: &Engine,
        locations: &[String],
    ) -> Result<(), EngineError> {
        if let Self::Active(tasks) = self {
            let siblings = siblings(engine, tasks)?;
            for task in tasks {
                let source = engine.readable(&task.source.source)?;
                if let Some(content) = task.task.content.as_deref() {
                    let own = super::located(task.task.location.as_ref());
                    let stored = source.source().stored_headings(
                        onetaskgraph_plugin_api::MetadataRecord::Task,
                        &task.source.native,
                    );
                    let authored = crate::section_links::SectionForm::for_kind(source.kind())
                        .map_or_else(
                            || content.to_owned(),
                            |form| {
                                form.read_same_record(content, own.as_deref(), stored.as_deref())
                            },
                        );
                    // A destination that would keep a reference-style link only as text has it
                    // read inline, so it is reduced to its text as any inline link is.
                    let authored = if crate::section_links::SectionForm::for_kind(
                        engine.writable(&task.to)?.kind(),
                    )
                    .is_some_and(crate::section_links::SectionForm::inlines_reference_links)
                    {
                        crate::section_links::inline_reference_links(&authored)
                    } else {
                        authored
                    };
                    task.links = view_links(
                        engine,
                        task,
                        &authored,
                        own.as_deref(),
                        stored,
                        &siblings,
                        locations,
                    )?;
                    let screened = task
                        .links
                        .as_ref()
                        .map_or(locations, |links| links.screened.as_slice());
                    task.task.content = Some(super::privacy::without_private_links(
                        &authored,
                        screened,
                        if source.kind() == "local-md" {
                            own.as_deref()
                        } else {
                            None
                        },
                    ));
                }
            }
        }
        Ok(())
    }

    /// Every task this plan makes a view of.
    fn tasks(&self) -> &[Projected] {
        match self {
            Self::Active(tasks) => tasks,
            Self::Inactive => &[],
        }
    }

    /// Note `task`, landing canonically in the private home, as projected into `to`.
    pub(super) fn add(&mut self, task: &Planned, to: SourceName) {
        let (Self::Active(tasks), Item::Task(read)) = (self, &task.item) else {
            return;
        };
        tasks.push(Projected {
            source: task.source.clone(),
            to,
            task: (**read).clone(),
            assets: task.assets.clone(),
            depends_on: task
                .edges
                .iter()
                .filter(|edge| {
                    edge.kind == DependencyKind::Blocks && edge.to.kind == ItemKind::Task
                })
                .filter_map(|edge| far_end(&edge.to, &task.source.source))
                .collect(),
            links: None,
        });
    }

    /// The public sources the views land in, each once, in the order the plan reaches them.
    fn destinations(&self) -> Vec<SourceName> {
        let mut destinations: Vec<SourceName> = Vec::new();
        for task in self.tasks() {
            if !destinations.contains(&task.to) {
                destinations.push(task.to.clone());
            }
        }
        destinations
    }

    /// The views landing in `to`, every one after the views it depends on where they can be.
    fn ordered(&self, to: &SourceName) -> Vec<&Projected> {
        self.ordered_by(to, |task| task.depends_on.iter().collect())
    }

    /// [`Self::ordered`], and every view after the views it links to where they can be too, so
    /// a link costs a second write of its view only in a cycle.
    fn landing_order(&self, to: &SourceName) -> Vec<&Projected> {
        self.ordered_by(to, |task| task.landed_after().collect())
    }

    /// The views landing in `to`, every one after the views `after` names where they can be.
    fn ordered_by<'a>(
        &'a self,
        to: &SourceName,
        after: fn(&'a Projected) -> Vec<&'a GlobalId>,
    ) -> Vec<&'a Projected> {
        let here: Vec<&Projected> = self.tasks().iter().filter(|task| &task.to == to).collect();
        let mut ordered: Vec<&Projected> = Vec::new();
        let mut visiting: Vec<&GlobalId> = Vec::new();
        fn visit<'a>(
            task: &'a Projected,
            here: &[&'a Projected],
            after: fn(&'a Projected) -> Vec<&'a GlobalId>,
            ordered: &mut Vec<&'a Projected>,
            visiting: &mut Vec<&'a GlobalId>,
        ) {
            if ordered.iter().any(|done| done.source == task.source)
                || visiting.contains(&&task.source)
            {
                return;
            }
            visiting.push(&task.source);
            for far in after(task) {
                if let Some(first) = here.iter().find(|other| &other.source == far) {
                    visit(first, here, after, ordered, visiting);
                }
            }
            visiting.pop();
            ordered.push(task);
        }
        for task in &here {
            visit(task, &here, after, &mut ordered, &mut visiting);
        }
        ordered
    }
}

/// What every view of one plan in one public source lands against: the project they are
/// filed under, the map as recorded so far, the views already landed by source task id, and
/// where each view a link can be pointed at is.
struct Landing<'a> {
    home: &'a GlobalId,
    project: &'a NativeId,
    map: &'a PublicProjections,
    views: &'a HashMap<GlobalId, NativeId>,
    located: &'a HashMap<GlobalId, (NativeId, String)>,
}

/// One view as it landed: its id, and where its destination says it is when it says.
struct LandedView {
    id: NativeId,
    location: Option<String>,
}

/// Every projected task a view could link to: each has a location to be recognised by, and no
/// other task reports the same one — a link that could name either is pointed at neither.
fn siblings(engine: &Engine, tasks: &[Projected]) -> Result<Vec<Sibling>, EngineError> {
    let mut siblings = Vec::new();
    for task in tasks {
        let Some(location) = super::located(task.task.location.as_ref()) else {
            continue;
        };
        if tasks
            .iter()
            .filter(|other| {
                super::located(other.task.location.as_ref()).as_ref() == Some(&location)
            })
            .count()
            > 1
        {
            continue;
        }
        siblings.push(Sibling {
            id: task.source.clone(),
            to: task.to.clone(),
            location,
            content: task.task.content.clone(),
            stored: engine
                .readable(&task.source.source)?
                .source()
                .stored_headings(
                    onetaskgraph_plugin_api::MetadataRecord::Task,
                    &task.source.native,
                ),
        });
    }
    Ok(siblings)
}

/// What `task`, whose content reads as `authored`, links to among the views landing beside it —
/// `None` when it names none, or when its destination has no form a link to one is written in
/// here. A folder of Markdown and a GitHub board are the kinds a public body's links to its
/// counterparts are written for; every other keeps its links as text, as a plain copy's do.
fn view_links(
    engine: &Engine,
    task: &Projected,
    authored: &str,
    own: Option<&str>,
    stored: Option<Vec<onetaskgraph_plugin_api::StoredHeading>>,
    siblings: &[Sibling],
    locations: &[String],
) -> Result<Option<ViewLinks>, EngineError> {
    use super::sections::{BodyLinks, Counterpart, Linked};
    use crate::section_links::SectionForm;
    let to = SectionForm::for_kind(engine.writable(&task.to)?.kind());
    if !matches!(to, Some(SectionForm::LocalMd | SectionForm::GithubProjects)) {
        return Ok(None);
    }
    let from = SectionForm::for_kind(engine.readable(&task.source.source)?.kind());
    let referents: Vec<Linked> = siblings
        .iter()
        .filter(|sibling| {
            sibling.id != task.source
                && sibling.to == task.to
                && BodyLinks::names(
                    from,
                    authored,
                    own,
                    &sibling.location,
                    // A view is a public body: its rendering reads a path written alone.
                    super::sections::Reading::BarePathsToo,
                )
        })
        .map(|sibling| {
            let mut linked = Linked::new(
                Level::Task,
                sibling.id.clone(),
                sibling.location.clone(),
                from,
                sibling.content.as_deref(),
                sibling.stored.clone(),
            );
            linked.counterpart = Counterpart::InCopy;
            linked
        })
        .collect();
    if referents.is_empty() {
        return Ok(None);
    }
    let mut screened = locations.to_vec();
    screened.extend(
        referents
            .iter()
            .flat_map(|referent| referent.spellings().iter().cloned()),
    );
    let carried_fragments = BodyLinks::new(
        authored.to_owned(),
        from,
        to,
        Level::Task,
        own.map(str::to_owned),
        stored,
        referents.clone(),
    )
    .carried_fragments();
    Ok(Some(ViewLinks {
        authored: authored.to_owned(),
        referents,
        screened,
        carried_fragments,
    }))
}

impl Engine {
    /// Whether a routed project copy whose home is `home` makes public views of its public
    /// tasks: while the boundary is active, for a plan classified private whose home lands in
    /// a source declared private. Every task of such a plan then lands canonically in the home.
    pub(super) fn makes_projections(&self, home: &SourceName, class: Classification) -> bool {
        self.boundary_active()
            && class == Classification::Private
            && self.declared(home) == SourceVisibility::Private
    }

    /// Where `task`, landing canonically in `home`, is projected: the source the copy's
    /// routing places it at as public, when that is not the home and not declared private.
    pub(super) fn projection_target(
        &self,
        destination: &SourceName,
        home: &SourceName,
        task: &Planned,
    ) -> Option<SourceName> {
        let Item::Task(read) = &task.item else {
            return None;
        };
        // A canonical task its destination already records private — an earlier copy's, an
        // earlier release's by inheritance — stays private, and has no view.
        let recorded = task
            .held
            .as_ref()
            .map_or(Classification::Public, |held| classification_of(&held.item));
        if task.classification.strictest(recorded) != Classification::Public
            || read.repositories.is_empty()
        {
            return None;
        }
        let placed =
            self.routes
                .place_classified(destination, Classification::Public, &read.repositories);
        (&placed.destination != home
            && self.declared(&placed.destination) != SourceVisibility::Private)
            .then_some(placed.destination)
    }

    /// Hold every view `projecting` would write, and the neutral project each is filed under, to
    /// the public boundary before anything of the copy is written: a local link its content
    /// could not carry, a status its destination has no name for, and the caller's check of
    /// everything the view and its project put in front of a reader.
    pub(super) async fn preflight_projections(
        &self,
        projecting: &Projecting,
    ) -> Result<(), EngineError> {
        for to in projecting.destinations() {
            let destination = self.writable(&to)?;
            let project = public_project(NativeId(String::new()), &projecting.ordered(&to));
            let classification = self
                .classify(Classification::Public, &project.repositories)
                .await;
            self.preflight(
                destination,
                &Outbound {
                    item: "the public projection project".to_owned(),
                    classification,
                    target: WriteTarget::New {
                        repositories: &project.repositories,
                        project: None,
                    },
                    exposure: &Exposure::of_project(&project),
                },
            )
            .await
            .and_then(|rewrite| {
                unrewritten(destination, "the public projection project", &rewrite)
            })?;
            destination
                .source()
                .check_status_write(ItemKind::Project, project.status.category, None)
                .await
                .map_err(|error| refused(destination, error))?;
        }
        for task in projecting.tasks() {
            let names: Vec<String> = task
                .assets
                .iter()
                .map(|asset| asset.name.as_str().to_owned())
                .collect();
            if let Some(reference) =
                unresolved_references(task.task.content.as_deref().unwrap_or_default(), &names)
                    .into_iter()
                    .next()
            {
                return Err(EngineError::ProjectionReference {
                    task: task.source.clone(),
                    reference,
                });
            }
            let destination = self.writable(&task.to)?;
            let view = task.view(NativeId(String::new()), None);
            let classification = self
                .classify(Classification::Public, &view.repositories)
                .await;
            self.preflight(
                destination,
                &Outbound {
                    item: task.item(),
                    classification,
                    target: WriteTarget::New {
                        repositories: &view.repositories,
                        project: None,
                    },
                    exposure: &Exposure::of_task(&view).assets(&task.assets).and(
                        Exposure::metadata(
                            task.links
                                .iter()
                                .flat_map(|links| links.carried_fragments.iter().cloned()),
                        ),
                    ),
                },
            )
            .await
            .and_then(|rewrite| unrewritten(destination, &task.item(), &rewrite))?;
            destination
                .source()
                .check_status_write(ItemKind::Task, view.status.category, None)
                .await
                .map_err(|error| refused(destination, error))?;
        }
        Ok(())
    }

    /// Land every view of `projecting`, once the canonical copy whose `outcomes` these are has
    /// landed in its private home, and record each in the home's map before anything else is
    /// written — so a copy run again after an interruption finds every view it made.
    pub(super) async fn land_projections(
        &self,
        projecting: &Projecting,
        outcomes: &[CopyOutcome],
        journal: &mut Journal,
    ) -> Result<(), EngineError> {
        let Some(home) = outcomes.first().and_then(CopyOutcome::destination).cloned() else {
            return Ok(());
        };
        let canonical: HashMap<GlobalId, GlobalId> = outcomes
            .iter()
            .filter_map(|outcome| {
                outcome
                    .destination()
                    .map(|landed| (outcome.source.clone(), landed.clone()))
            })
            .collect();
        let at = self.writable(&home.source)?;
        let Some(held) = self.prior(at, Level::Project, &home.native).await? else {
            return Ok(());
        };
        let Item::Project(held) = &held.item else {
            return Ok(());
        };
        let mut map = self.recorded_projections(&home, &held.metadata)?;
        // A canonical task of this plan with no view this time — recorded private since, or no
        // longer routed anywhere public — is no longer named; its old view is left as a person
        // finds it, never deleted.
        let projected: Vec<&GlobalId> = projecting
            .tasks()
            .iter()
            .filter_map(|task| canonical.get(&task.source))
            .collect();
        if map.retain(|entry| {
            !canonical.values().any(|landed| landed == &entry.task)
                || projected.contains(&&entry.task)
        }) {
            self.record_projections(at, &home, &map, journal).await?;
        }
        for to in projecting.destinations() {
            let destination = self.writable(&to)?;
            let project = self
                .land_public_project(
                    destination,
                    &home,
                    &projecting.ordered(&to),
                    &mut map,
                    journal,
                )
                .await?;
            let ordered = projecting.landing_order(&to);
            let mut views: HashMap<GlobalId, NativeId> = HashMap::new();
            let mut located: HashMap<GlobalId, (NativeId, String)> = HashMap::new();
            // The tasks under the public project no map entry names, listed once when a view is
            // looked for by what it says.
            let mut unrecorded: Option<Vec<Task>> = None;
            let mut incomplete = Vec::new();
            for task in &ordered {
                let Some(canonical_task) = canonical.get(&task.source) else {
                    continue;
                };
                // A view linking to one landing after it — a cycle — is pointed at it where an
                // earlier copy's map says it already is, so a repeat copy has nothing to write.
                for far in task.linked() {
                    if located.contains_key(far) {
                        continue;
                    }
                    if let Some(found) = self
                        .recorded_view(
                            destination,
                            canonical.get(far).and_then(|at| map.projection_of(at)),
                            &project,
                        )
                        .await?
                    {
                        located.insert(far.clone(), found);
                    }
                }
                let complete = task.depends_on.iter().all(|far| {
                    !ordered.iter().any(|other| &other.source == far) || views.contains_key(far)
                }) && task.linked().all(|far| located.contains_key(far));
                let landing = Landing {
                    home: &home,
                    project: &project,
                    map: &map,
                    views: &views,
                    located: &located,
                };
                let landed = self
                    .land_view(
                        destination,
                        task,
                        canonical_task,
                        &landing,
                        &mut unrecorded,
                        journal,
                    )
                    .await?;
                views.insert(task.source.clone(), landed.id.clone());
                match landed.location {
                    Some(location) => {
                        located.insert(task.source.clone(), (landed.id.clone(), location));
                    }
                    None => {
                        located.remove(&task.source);
                    }
                }
                if map.set(canonical_task, &GlobalId::new(to.clone(), landed.id)) {
                    self.record_projections(at, &home, &map, journal).await?;
                }
                if !complete {
                    incomplete.push(*task);
                }
            }
            // A view that depends on or links to one landed after it — a cycle — is written
            // again now that every view it names is there.
            for task in incomplete {
                let canonical_task = &canonical[&task.source];
                let landing = Landing {
                    home: &home,
                    project: &project,
                    map: &map,
                    views: &views,
                    located: &located,
                };
                self.land_view(
                    destination,
                    task,
                    canonical_task,
                    &landing,
                    &mut unrecorded,
                    journal,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// The view `recorded` names, and where its destination says it is, when it is one of this
    /// destination's filed under `project` — what [`Engine::land_view`] would take it for.
    async fn recorded_view(
        &self,
        destination: &ResolvedSource,
        recorded: Option<&GlobalId>,
        project: &NativeId,
    ) -> Result<Option<(NativeId, String)>, EngineError> {
        let Some(recorded) = recorded.filter(|at| &at.source == destination.name()) else {
            return Ok(None);
        };
        let Some(prior) = self
            .prior(destination, Level::Task, &recorded.native)
            .await?
        else {
            return Ok(None);
        };
        let Item::Task(view) = &prior.item else {
            return Ok(None);
        };
        if view.project.as_ref() != Some(project) {
            return Ok(None);
        }
        Ok(super::located(view.location.as_ref()).map(|location| (view.id.clone(), location)))
    }

    /// The map `metadata` of the private home `home` records, held to [`Engine::held_to_home`].
    fn recorded_projections(
        &self,
        home: &GlobalId,
        metadata: &BTreeMap<String, Value>,
    ) -> Result<PublicProjections, EngineError> {
        PublicProjections::read(metadata)
            .and_then(|map| self.held_to_home(home, map))
            .map_err(|why| EngineError::ProjectionsMalformed {
                project: home.clone(),
                why,
            })
    }

    /// `map` as the private home `home` may record it, or why not: every task of the source
    /// holding the home, and every projection somewhere not declared private.
    fn held_to_home(
        &self,
        home: &GlobalId,
        map: PublicProjections,
    ) -> Result<PublicProjections, String> {
        for entry in map.tasks() {
            if entry.task.source != home.source {
                return Err(format!(
                    "task {} is not of {}, the source holding the project and its canonical \
                     tasks",
                    entry.task, home.source
                ));
            }
            if self.declared(&entry.projection.source) == SourceVisibility::Private {
                return Err(format!(
                    "projection {} is in a source declared private, and a projection is public",
                    entry.projection
                ));
            }
        }
        for entry in map.projects() {
            if self.declared(&entry.source) == SourceVisibility::Private {
                return Err(format!(
                    "the public projection project in {} is in a source declared private, and \
                     a projection is public",
                    entry.source
                ));
            }
        }
        Ok(map)
    }

    /// The public project the views of one plan in `destination` are filed under: the one `map`
    /// records there — by the id the source assigned it, or, when the copy never learned that
    /// id, by the allocation it was created carrying — else one created under this plan's opaque
    /// id. No other project is ever taken, whatever it says: not the one a view sits under, which
    /// a person can move it into, and not one already at that id, which is refused, never adopted
    /// for its id.
    ///
    /// A project is created only behind its allocation: recorded privately on the home before
    /// the create is sent, carried by the project, and journaled, so a copy that cannot finish
    /// takes it back even when the create's answer was lost, and a copy that was ended finds it
    /// again. The id the source assigns is recorded before any view is filed under it.
    async fn land_public_project(
        &self,
        destination: &ResolvedSource,
        home: &GlobalId,
        ordered: &[&Projected],
        map: &mut PublicProjections,
        journal: &mut Journal,
    ) -> Result<NativeId, EngineError> {
        let at = self.writable(&home.source)?;
        let not_one = |id: &NativeId| EngineError::ProjectionsMalformed {
            project: home.clone(),
            why: format!(
                "{}:{id} is recorded as the public projection project, and is not one",
                destination.name()
            ),
        };
        let recorded = map.project_in(destination.name()).cloned();
        if let Some(PublicProjectAllocation {
            project: Some(id), ..
        }) = &recorded
            && let Some(held) = destination
                .source()
                .get_project(id)
                .await
                .map_err(|error| refused(destination, error))?
        {
            return if is_neutral(&held) {
                Ok(id.clone())
            } else {
                Err(not_one(id))
            };
        }
        if let Some(entry) = &recorded {
            let found = self.allocated(destination, &entry.allocation).await?;
            match found.as_slice() {
                [] => {}
                [id] => {
                    let held = destination
                        .source()
                        .get_project(id)
                        .await
                        .map_err(|error| refused(destination, error))?;
                    if !held.as_ref().is_some_and(is_neutral) {
                        return Err(not_one(id));
                    }
                    self.record_project(at, home, map, entry, id, journal)
                        .await?;
                    return Ok(id.clone());
                }
                [first, ..] => {
                    return Err(EngineError::ProjectionsMalformed {
                        project: home.clone(),
                        why: format!(
                            "{} holds {} projects carrying the allocation recorded for its public \
                             projection project, {}:{first} among them, and only one may",
                            destination.name(),
                            found.len(),
                            destination.name()
                        ),
                    });
                }
            }
        }
        // The id a create asks for is no claim on what already sits at it: a project there that no
        // record of this copy names and that carries none of its allocation is somebody else's,
        // however neutral it reads, and a destination honouring the id would write over it.
        let requested = opaque_id("pw", home);
        if destination
            .source()
            .get_project(&requested)
            .await
            .map_err(|error| refused(destination, error))?
            .is_some()
        {
            return Err(EngineError::ProjectionsMalformed {
                project: home.clone(),
                why: format!(
                    "{}:{requested} holds the id its public projection project is made under, and \
                     nothing this copy recorded names it; move or remove that project, then copy \
                     again",
                    destination.name()
                ),
            });
        }
        // Nothing is there: create it, behind an allocation recorded before it is asked for.
        let entry = recorded.unwrap_or_else(|| PublicProjectAllocation {
            source: destination.name().clone(),
            allocation: Allocation::fresh(),
            project: None,
        });
        let entry = PublicProjectAllocation {
            project: None,
            ..entry
        };
        let mut project = public_project(requested, ordered);
        project.metadata.insert(
            PublicProjectAllocation::KEY.to_owned(),
            Value::String(entry.allocation.as_str().to_owned()),
        );
        let item = "the public projection project";
        self.at_write(
            destination,
            item,
            &WriteTarget::New {
                repositories: &project.repositories,
                project: None,
            },
        )
        .await?;
        destination
            .source()
            .check_status_write(ItemKind::Project, project.status.category, None)
            .await
            .map_err(|error| refused(destination, error))?;
        if map.set_project(entry.clone()) {
            self.record_projections(at, home, map, journal).await?;
        }
        journal.record(Undo::Allocated {
            at: destination.name().clone(),
            allocation: entry.allocation.clone(),
            home: home.clone(),
        });
        // A create's answer the source mangled is no uncertainty here: the allocation names what
        // it stored, so taking the copy back takes the project with it.
        let landed = destination
            .source()
            .write_project(&ItemWrite {
                target: None,
                item: project,
                depends_on: Vec::new(),
            })
            .await
            .and_then(|landed| answered_id(landed, "write_project"))
            .map_err(|error| refused(destination, error))?;
        journal.record(Undo::Created {
            at: destination.name().clone(),
            kind: Level::Project,
            id: landed.clone(),
        });
        self.record_project(at, home, map, &entry, &landed, journal)
            .await?;
        Ok(landed)
    }

    /// Every project `destination` holds carrying `allocation` at
    /// [`PublicProjectAllocation::KEY`], exactly — the public projection project created behind
    /// it, and never any other.
    ///
    /// Read through the engine's own project listing of every project the source holds, paged by
    /// its own token as a project's members are, and narrowed here by the allocation alone —
    /// never by title, which a person can change.
    ///
    /// # Errors
    ///
    /// The listing's own, and a source handing back the page it was asked for as its next.
    pub(super) async fn allocated(
        &self,
        destination: &ResolvedSource,
        allocation: &Allocation,
    ) -> Result<Vec<NativeId>, EngineError> {
        let mut request = ProjectRequest {
            sources: vec![destination.name().clone()],
            // No narrowing but the allocation: a project a person renamed still carries it, and
            // is found — and refused, being no public projection project — rather than missed and
            // made again beside.
            filters: Filters::default(),
            paging: Paging {
                limit: PROJECT_PAGE,
                token: None,
            },
        };
        let carried = Value::String(allocation.as_str().to_owned());
        let mut found = Vec::new();
        loop {
            let asked = request.paging.token.clone();
            let response = self.projects(&request).await?;
            if let Some(failure) = response.errors.first() {
                return Err(EngineError::SourceRefused {
                    name: failure.source.to_string(),
                    error: failure.error.clone(),
                });
            }
            unrepeated(
                response.next.as_ref(),
                asked.as_ref(),
                "the projects of a source were being read for a public projection project's \
                 allocation",
            )
            .map_err(|error| refused(destination, error))?;
            found.extend(
                response
                    .items
                    .into_iter()
                    .filter(|project| {
                        project.item.metadata.get(PublicProjectAllocation::KEY) == Some(&carried)
                    })
                    .map(|project| project.item.id),
            );
            match response.next {
                Some(token) => request.paging.token = Some(token),
                None => return Ok(found),
            }
        }
    }

    /// Remove every project `destination` holds carrying `allocation`, answering what is still
    /// there and why when it cannot: nothing named, when the listing that finds them failed.
    pub(super) async fn take_back_allocated(
        &self,
        destination: &ResolvedSource,
        allocation: &Allocation,
    ) -> Result<(), (Vec<NativeId>, SourceError)> {
        let found = self
            .allocated(destination, allocation)
            .await
            .map_err(|problem| {
                (
                    Vec::new(),
                    SourceError::Refused {
                        message: problem.to_string(),
                    },
                )
            })?;
        for (at, id) in found.iter().enumerate() {
            if let Err(problem) = remove(destination, Level::Project, id).await {
                return Err((found[at..].to_vec(), problem));
            }
        }
        Ok(())
    }

    /// Record on the private home `home` that `entry`'s project is `id`.
    async fn record_project(
        &self,
        at: &ResolvedSource,
        home: &GlobalId,
        map: &mut PublicProjections,
        entry: &PublicProjectAllocation,
        id: &NativeId,
        journal: &mut Journal,
    ) -> Result<(), EngineError> {
        if map.set_project(PublicProjectAllocation {
            project: Some(id.clone()),
            ..entry.clone()
        }) {
            self.record_projections(at, home, map, journal).await?;
        }
        Ok(())
    }

    /// Why a view `projection` filed under `project`, of `destination`, may not be written —
    /// `None` when that project is a public projection project — so a map naming somebody else's
    /// task never redirects a write into a project that is not one.
    ///
    /// # Errors
    ///
    /// The project's read failing, as that source's failure.
    async fn not_neutral(
        &self,
        destination: &ResolvedSource,
        projection: &GlobalId,
        project: &NativeId,
    ) -> Result<Option<String>, EngineError> {
        let held = destination
            .source()
            .get_project(project)
            .await
            .map_err(|error| refused(destination, error))?;
        Ok((!held.as_ref().is_some_and(is_neutral)).then(|| {
            format!(
                "projection {projection} is filed under {}:{project}, which is not a public \
                 projection project",
                destination.name()
            )
        }))
    }

    /// Land one view: over the one the map names, or the one its opaque id names, when either
    /// is there — written only when it differs from what the canonical task now says — and
    /// created otherwise. Answers the id it is held under, and where its destination says it is.
    ///
    /// A view this copy made and never learned the id of — its create's answer lost, at a
    /// destination that issues ids of its own and so ignores the opaque one — is found by what
    /// it says instead: the one task under the public project no map entry names that has the
    /// view's title and repositories. Two such tasks are ambiguous, and neither is taken.
    async fn land_view(
        &self,
        destination: &ResolvedSource,
        task: &Projected,
        canonical: &GlobalId,
        landing: &Landing<'_>,
        unrecorded: &mut Option<Vec<Task>>,
        journal: &mut Journal,
    ) -> Result<LandedView, EngineError> {
        let opaque = opaque_id("pt", canonical);
        // What the view now holds, read once: it is what is compared, and what the journal puts
        // back if the copy cannot finish.
        let mut held: Option<(Task, Prior)> = None;
        for candidate in landing
            .map
            .projection_of(canonical)
            .filter(|projection| &projection.source == destination.name())
            .map(|projection| projection.native.clone())
            .into_iter()
            .chain(std::iter::once(opaque.clone()))
        {
            if let Some(prior) = self.prior(destination, Level::Task, &candidate).await?
                && let Item::Task(view) = &prior.item
                && view.project.as_ref() == Some(landing.project)
            {
                held = Some(((**view).clone(), prior));
                break;
            }
        }
        if held.is_none() {
            let listed = match unrecorded {
                Some(listed) => listed,
                None => unrecorded.insert(
                    self.project_member_tasks(
                        &GlobalId::new(destination.name().clone(), landing.project.clone()),
                        "for a public projection",
                    )
                    .await?
                    .into_iter()
                    .map(|task| task.item)
                    .filter(|view| {
                        !landing.map.tasks().iter().any(|entry| {
                            &entry.projection.source == destination.name()
                                && entry.projection.native == view.id
                        }) && !landing.views.values().any(|landed| landed == &view.id)
                    })
                    .collect(),
                ),
            };
            let mut matching = listed.iter().filter(|view| {
                view.title == task.task.title && view.repositories == task.task.repositories
            });
            if let (Some(found), None) = (matching.next(), matching.next()) {
                let found = found.id.clone();
                listed.retain(|view| view.id != found);
                if let Some(prior) = self.prior(destination, Level::Task, &found).await?
                    && let Item::Task(view) = &prior.item
                {
                    held = Some(((**view).clone(), prior));
                }
            }
        }
        if let Some((view, _)) = &held
            && view.classification == Classification::Private
        {
            return Err(EngineError::ProjectionsMalformed {
                project: landing.home.clone(),
                why: format!(
                    "view {}:{} is recorded private, and a view is public; nothing loosens a \
                     private record",
                    destination.name(),
                    view.id
                ),
            });
        }
        let id = held.as_ref().map_or(opaque, |(view, _)| view.id.clone());
        let mut view = task.view(id.clone(), Some(landing.project.clone()));
        // A view's links to its own headings and to the views landing beside it, in the
        // destination's section form: any other link to a record is its text alone by now, or
        // was refused by the preflight.
        // llmlint: ignore-block[changed_behavior_has_e2e] A public view's section links are held by `a_views_link_to_its_own_heading_lands_in_the_boards_section_form`, `a_new_linear_views_link_to_its_own_heading_names_the_id_its_create_was_answered_with` and the two rollback journeys beside them in `crates/onetaskgraph-e2e/tests/e2e/boundary/projections.rs`, and its links to other views by `a_views_link_to_another_views_heading_lands_in_the_boards_section_form` and the journeys after it there; the projection fixture plans are authored in a folder of Markdown, and a plan held in a tracker reaches a view through the same rendering, whose tracker-form reading the section-link matrix journeys in `section_links.rs` hold for every source kind.
        let mut view_links = None;
        if let Some(content) = view.content.clone() {
            let from = crate::section_links::SectionForm::for_kind(
                self.readable(&task.source.source)?.kind(),
            );
            let to = crate::section_links::SectionForm::for_kind(destination.kind());
            let links = |content: String, referents| {
                Ok::<_, EngineError>(super::sections::BodyLinks::new(
                    content,
                    from,
                    to,
                    Level::Task,
                    // Where the canonical task is, so a link to its own heading spelled with its
                    // own address — as a view copied out of a tracker holds one — is read as one.
                    // A local path in a view is refused before this, by the preflight.
                    super::located(task.task.location.as_ref()),
                    self.readable(&task.source.source)?
                        .source()
                        .stored_headings(
                            onetaskgraph_plugin_api::MetadataRecord::Task,
                            &task.source.native,
                        ),
                    referents,
                ))
            };
            let reduced = links(content, Vec::new())?;
            let linking = task
                .links
                .as_ref()
                .map(|held| {
                    links(held.authored.clone(), held.referents.clone())
                        .map(|links| links.screen_public(Some(&held.screened)).always_relative())
                })
                .transpose()?;
            let own = held.as_ref().and_then(|(held, _)| {
                held.location
                    .as_ref()
                    .and_then(|location| super::located(Some(location)))
                    .map(|location| (held.id.clone(), location))
            });
            // A folder of Markdown's link is relative to the view's own file, which a new view
            // has none of yet: its first write holds the link's text, as a kept placeholder
            // would have to, and the write finishing it below holds the link.
            let addressable =
                own.is_some() || to != Some(crate::section_links::SectionForm::LocalMd);
            let knows = ViewKnows {
                destination,
                own,
                located: landing.located,
            };
            if let Some(linking) = linking.as_ref().filter(|_| addressable) {
                view.content = Some(linking.render(&knows, super::sections::Pass::Final).content);
            } else if reduced.matters() {
                view.content = Some(reduced.render(&knows, super::sections::Pass::Final).content);
            }
            view_links = linking.or_else(|| reduced.matters().then_some(reduced));
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]
        let edges: Vec<DependencyEdge> = task
            .depends_on
            .iter()
            .filter_map(|far| landing.views.get(far))
            .map(|far| DependencyEdge {
                from: DependencyEndpoint::from_native(id.clone(), ItemKind::Task),
                to: DependencyEndpoint::from_native(far.clone(), ItemKind::Task),
                kind: DependencyKind::Blocks,
            })
            .collect();
        let target = held.as_ref().map(|(view, _)| view.id.clone());
        if let Some((held, prior)) = &held {
            let mut were: Vec<&str> = prior.edges.iter().map(|edge| edge.to.id()).collect();
            let mut are: Vec<&str> = edges.iter().map(|edge| edge.to.id()).collect();
            were.sort_unstable();
            are.sort_unstable();
            // Every member a view is written with — what a person may have edited in public
            // included — so the next copy puts back whatever differs. A destination's own
            // reserved keys are its bookkeeping, never a field of the view.
            let own_keys = held
                .metadata
                .keys()
                .any(|key| !key.starts_with(&format!("{}.", MetadataKey::RESERVED_NAMESPACE)));
            if held.title == view.title
                && held.content == view.content
                && held.status.category == view.status.category
                && held.repositories == view.repositories
                && held.labels.is_empty()
                && held.priority == Priority::None
                && held.delivers.is_empty()
                && held.delivered_by.is_empty()
                && !own_keys
                && were == are
                && digests(prior.assets.as_deref().unwrap_or_default()) == digests(&task.assets)
            {
                return Ok(LandedView {
                    id: held.id.clone(),
                    location: super::located(held.location.as_ref()),
                });
            }
        }
        let item = task.item();
        let write_target = match &target {
            Some(id) => WriteTarget::Existing(id),
            None => WriteTarget::New {
                repositories: &view.repositories,
                project: Some(landing.project),
            },
        };
        self.at_write(destination, &item, &write_target).await?;
        destination
            .source()
            .check_status_write(ItemKind::Task, view.status.category, target.as_ref())
            .await
            .map_err(|error| refused(destination, error))?;
        let recorded = held.as_ref().and_then(|(_, prior)| prior.recorded.clone());
        let held_location = held
            .as_ref()
            .and_then(|(held, _)| super::located(held.location.as_ref()));
        // A view carries assets when its task references any, and when the view it overwrites
        // holds any: what lands then holds exactly the task's, none included.
        let carrying = !task.assets.is_empty()
            || held
                .as_ref()
                .is_some_and(|(_, prior)| prior.assets.is_some());
        if let Some((held, mut prior)) = held {
            if !task.assets.is_empty() && prior.assets.is_none() {
                // Putting the view back then has to take away the assets this write adds.
                prior.assets = Some(Vec::new());
            }
            journal.record(Undo::Updated {
                at: destination.name().clone(),
                id: held.id,
                prior,
            });
        }
        let write = ItemWrite {
            target: target.clone(),
            item: view,
            depends_on: edges,
        };
        // What an asset write said it stored, from which the assets it uploaded are read.
        let mut stored = None;
        let landed = if !carrying {
            destination.source().write_task(&write).await
        } else {
            destination
                .source()
                .write_task_with_assets(
                    &write,
                    None,
                    &assets::write_of(task.assets.clone(), recorded),
                )
                .await
                .map(|written| {
                    stored = written.content;
                    written.id
                })
        }
        .and_then(|landed| answered_id(landed, "write_task"))
        .map_err(|error| match target {
            None => create_refused(destination, Creating::Task, error),
            Some(_) => refused(destination, error),
        })?;
        if target.is_none() {
            journal.record(Undo::Created {
                at: destination.name().clone(),
                kind: Level::Task,
                id: landed.clone(),
            });
            // A view's links to its own headings name its own address and, on Linear, the ids
            // its create was answered with, so a new view is written once more with them: an
            // update of what this copy created, which the create's undo takes back with it.
            if let Some(links) = &view_links {
                let own = super::located(
                    destination
                        .source()
                        .written_location(onetaskgraph_plugin_api::MetadataRecord::Task, &landed)
                        .as_ref(),
                )
                .map(|location| (landed.clone(), location));
                let knows = ViewKnows {
                    destination,
                    own,
                    located: landing.located,
                };
                let finished = links.render(&knows, super::sections::Pass::Final).content;
                if write.item.content.as_deref() != Some(finished.as_str()) {
                    let mut item = write.item.clone();
                    item.content = Some(finished);
                    let again = ItemWrite {
                        target: Some(landed.clone()),
                        item,
                        depends_on: write.depends_on.clone(),
                    };
                    let finished = if carrying {
                        // Its assets go with it, each the create uploaded reused rather than sent
                        // again.
                        let uploaded = super::super::stored::served(
                            write.item.content.as_deref().unwrap_or_default(),
                            stored.as_deref().unwrap_or_default(),
                            &task.assets,
                        );
                        destination
                            .source()
                            .write_task_with_assets(
                                &again,
                                None,
                                &assets::write_of(task.assets.clone(), uploaded),
                            )
                            .await
                            .map(|written| written.id)
                    } else {
                        destination.source().write_task(&again).await
                    }
                    .and_then(|answered| answered_id(answered, "write_task"))
                    .map_err(|error| refused(destination, error))?;
                    // An update of the view it names, and an answer naming another is one this copy
                    // cannot say what was written to.
                    if finished != landed {
                        return Err(refused(
                            destination,
                            SourceError::Malformed {
                                message: format!(
                                    "the write finishing view {landed} answered with the item {finished}"
                                ),
                            },
                        ));
                    }
                }
            }
        }
        let location = super::located(
            destination
                .source()
                .written_location(onetaskgraph_plugin_api::MetadataRecord::Task, &landed)
                .as_ref(),
        )
        .or(held_location);
        Ok(LandedView {
            id: landed,
            location,
        })
    }

    /// Write `map` onto the private home `home` through its own project write, unless it
    /// records exactly that already; the home as it stood is what the journal restores.
    async fn record_projections(
        &self,
        at: &ResolvedSource,
        home: &GlobalId,
        map: &PublicProjections,
        journal: &mut Journal,
    ) -> Result<(), EngineError> {
        let Some(prior) = self.prior(at, Level::Project, &home.native).await? else {
            return Ok(());
        };
        let Item::Project(mut project) = prior.item.clone() else {
            return Ok(());
        };
        let value = map.to_value();
        if project.metadata.get(PublicProjections::KEY) == Some(&value) {
            return Ok(());
        }
        project
            .metadata
            .insert(PublicProjections::KEY.to_owned(), value);
        at.source()
            .check_status_write(
                ItemKind::Project,
                project.status.category,
                Some(&home.native),
            )
            .await
            .map_err(|error| refused(at, error))?;
        self.at_write(at, &home.to_string(), &WriteTarget::Existing(&home.native))
            .await?;
        // A follow-up of this home is sent after this write, and carries the map too.
        journal.carry_into_follow_up(
            &home.source,
            &home.native,
            described(&prior.item).1,
            &project.metadata,
        );
        let edges = prior.edges.clone();
        journal.record(Undo::Updated {
            at: home.source.clone(),
            id: home.native.clone(),
            prior,
        });
        at.source()
            .write_project(&ItemWrite {
                target: Some(home.native.clone()),
                item: *project,
                depends_on: edges,
            })
            .await
            .map_err(|error| refused(at, error))?;
        Ok(())
    }

    /// Mirror a canonical task's new status category to its public projection, through the
    /// projection's own screened status write, answering the projection when there is one.
    ///
    /// Only the category crosses: nothing of why it moved, who moved it or what ran. A task
    /// has a projection only in a source declared private while the boundary is active, in a
    /// store where some source routes — a projection is made only by a routed copy — filed
    /// under a project whose [`PublicProjections`] names it; so nothing is read for any other
    /// task, and a store with no route spends what it did before.
    pub(in crate::engine) async fn mirror_status(
        &self,
        task: &GlobalId,
        held: &Task,
        category: StatusCategory,
    ) -> Result<Option<GlobalId>, EngineError> {
        if !self.boundary_active()
            || !self.routes.any()
            || self.declared(&task.source) != SourceVisibility::Private
        {
            return Ok(None);
        }
        // A task private by its record or any of its repositories has no view to hand anything
        // to, whatever a map says.
        if self.classify(held.classification, &held.repositories).await == Classification::Private {
            return Ok(None);
        }
        let Some(project) = held.project.as_ref() else {
            return Ok(None);
        };
        // The canonical status has landed by now, so anything that keeps it from reaching the
        // view is reported as a mirror that did not happen, never as a write that did not.
        let not_mirrored =
            |projection: MirrorTarget, why: String| EngineError::ProjectionNotMirrored {
                task: task.clone(),
                projection: Box::new(projection),
                why,
            };
        let home = GlobalId::new(task.source.clone(), project.clone());
        let recorded_on = MirrorTarget::MapOn(home.clone());
        let source = self.readable(&task.source)?;
        let Some(record) = source
            .source()
            .get_project(project)
            .await
            .map_err(|error| {
                not_mirrored(recorded_on.clone(), refused(source, error).to_string())
            })?
        else {
            return Ok(None);
        };
        let map = PublicProjections::read(&record.metadata)
            .and_then(|map| self.held_to_home(&home, map))
            .map_err(|why| not_mirrored(recorded_on.clone(), why))?;
        let Some(projection) = map.projection_of(task).cloned() else {
            return Ok(None);
        };
        // The view has to be one: a task filed under a public projection project.
        let named = projection.to_string();
        let view = MirrorTarget::View(projection.clone());
        let public = self
            .writable(&projection.source)
            .map_err(|error| not_mirrored(view.clone(), error.to_string()))?;
        let filed = public
            .source()
            .get_task(&projection.native)
            .await
            .map_err(|error| not_mirrored(view.clone(), refused(public, error).to_string()))?
            .ok_or_else(|| not_mirrored(view.clone(), format!("no task {named} is there")))?
            .project
            .ok_or_else(|| {
                not_mirrored(view.clone(), format!("{named} is filed under no project"))
            })?;
        // And under the one this plan recorded: a view a person moved into another project, however
        // neutral that one reads, is not written there.
        if map
            .project_in(&projection.source)
            .and_then(|entry| entry.project.as_ref())
            != Some(&filed)
        {
            return Err(not_mirrored(
                view,
                format!(
                    "{named} is filed under {}:{filed}, which is not the public projection project \
                     this plan recorded",
                    projection.source
                ),
            ));
        }
        if let Some(why) = self
            .not_neutral(public, &projection, &filed)
            .await
            .map_err(|error| not_mirrored(view.clone(), error.to_string()))?
        {
            return Err(not_mirrored(view, why));
        }
        Box::pin(self.set_task_status(&projection, category))
            .await
            .map_err(|error| not_mirrored(view.clone(), error.to_string()))?;
        Ok(Some(projection))
    }
}

/// What a public view's rendering knows: the view's own counterpart, when it is updated, the
/// other views located so far, and the heading ids the destination's read of it answered.
struct ViewKnows<'a> {
    /// The public destination.
    destination: &'a ResolvedSource,
    /// The view already there, and where its destination says it is.
    own: Option<(NativeId, String)>,
    /// Where each view a link can be pointed at is, by its source task id.
    located: &'a HashMap<GlobalId, (NativeId, String)>,
}

impl super::sections::Knows for ViewKnows<'_> {
    fn landed(&self, referent: &super::sections::Linked) -> Option<(NativeId, String)> {
        self.located.get(&referent.id).cloned()
    }

    fn stored(
        &self,
        level: Level,
        id: &NativeId,
    ) -> Option<Vec<onetaskgraph_plugin_api::StoredHeading>> {
        self.destination
            .source()
            .stored_headings(level.record(), id)
    }

    fn own(&self) -> Option<(NativeId, String)> {
        self.own.clone()
    }
}

/// A view is neutral by construction and is never rewritten: one a sanitisation would have to
/// rewrite names something private, and is refused as the check refuses it, before anything of
/// the copy is written.
fn unrewritten(
    destination: &crate::resolve::ResolvedSource,
    item: &str,
    rewrite: &super::super::boundary::Rewrite,
) -> Result<(), EngineError> {
    if rewrite.is_identity() {
        return Ok(());
    }
    Err(EngineError::BoundaryRefused {
        item: item.to_owned(),
        destination: destination.name().to_string(),
        reason: "it carries a term of a private repository, and a public view is never \
                 rewritten"
            .to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(text: &str) -> GlobalId {
        text.parse().expect("a qualified id")
    }

    #[test]
    fn a_map_round_trips_and_absent_reads_as_empty() {
        let mut map = PublicProjections::default();
        assert!(map.set(&id("vault:a"), &id("site:pt-1")));
        assert!(!map.set(&id("vault:a"), &id("site:pt-1")));
        let value = map.to_value();
        assert_eq!(
            value,
            json!({"schema_version": 1, "tasks": [{"task": "vault:a", "projection": "site:pt-1"}]})
        );
        let mut metadata = BTreeMap::new();
        assert_eq!(
            PublicProjections::read(&metadata),
            Ok(PublicProjections::default())
        );
        metadata.insert(PublicProjections::KEY.to_owned(), value);
        assert_eq!(PublicProjections::read(&metadata), Ok(map));
    }

    #[test]
    fn a_map_records_each_public_project_by_its_allocation_and_omits_none() {
        let mut map = PublicProjections::default();
        assert!(map.set(&id("vault:a"), &id("site:pt-1")));
        let before = map.to_value();
        assert!(before.get("projects").is_none(), "{before}");
        let site = SourceName::new("site").expect("a source name");
        let pending = PublicProjectAllocation {
            source: site.clone(),
            allocation: Allocation::fresh(),
            project: None,
        };
        assert!(map.set_project(pending.clone()));
        assert!(!map.set_project(pending.clone()));
        let landed = PublicProjectAllocation {
            project: Some(NativeId("ISSUE-7".to_owned())),
            ..pending.clone()
        };
        assert!(map.set_project(landed.clone()));
        assert_eq!(map.projects(), std::slice::from_ref(&landed));
        assert_eq!(map.project_in(&site), Some(&landed));
        let value = map.to_value();
        assert_eq!(
            value["projects"],
            json!([{"source": "site", "allocation": pending.allocation.as_str(), "project": "ISSUE-7"}])
        );
        assert_eq!(PublicProjections::parse(&value), Ok(map));
        let written_before = json!({"schema_version": 1, "tasks": []});
        assert_eq!(
            PublicProjections::parse(&written_before).map(|map| map.to_value()),
            Ok(written_before)
        );
    }

    #[test]
    fn an_answered_id_that_names_nothing_is_refused_naming_the_method() {
        assert_eq!(
            answered_id(NativeId("ISSUE-7".to_owned()), "write_task"),
            Ok(NativeId("ISSUE-7".to_owned()))
        );
        for empty in ["", "  "] {
            let refused = answered_id(NativeId(empty.to_owned()), "write_project");
            assert!(
                matches!(&refused, Err(SourceError::Malformed { message })
                    if message == "write_project answered an empty id"),
                "{empty:?}: {refused:?}"
            );
        }
    }

    #[test]
    fn a_fresh_allocation_is_new_every_time() {
        let first = Allocation::fresh();
        assert_ne!(first, Allocation::fresh());
        assert_eq!(Allocation::new(first.as_str()), Ok(first));
    }

    #[test]
    fn an_allocation_is_exactly_what_its_pattern_states() {
        let pattern = ALLOCATION_PATTERN
            .strip_prefix("^[0-9a-f]{")
            .and_then(|rest| rest.strip_suffix("}$"))
            .and_then(|count| count.parse::<usize>().ok())
            .expect("the pattern is a counted class of lower-case hex digits");
        let held = |value: &str| {
            value.len() == pattern
                && value
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        };
        for value in [
            "0123456789abcdef0123456789abcdef",
            "0123456789ABCDEF0123456789abcdef",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789abcdeg0123456789abcdef",
            "",
            " ",
        ] {
            assert_eq!(Allocation::new(value).is_ok(), held(value), "{value:?}");
        }
    }

    #[test]
    fn a_malformed_map_is_refused_naming_what_is_wrong() {
        for (value, said) in [
            (
                json!({"schema_version": 2, "tasks": []}),
                "schema_version 2",
            ),
            (json!({"schema_version": 1}), "missing field"),
            (
                json!({"schema_version": 1, "tasks": [], "extra": 1}),
                "unknown field",
            ),
            (
                json!({"schema_version": 1, "tasks": [{"task": "vault:a", "projection": "site"}]}),
                "not a qualified id",
            ),
            (
                json!({"schema_version": 1, "tasks": [
                    {"task": "vault:a", "projection": "site:1"},
                    {"task": "vault:a", "projection": "site:2"}]}),
                "names task vault:a twice",
            ),
            (
                json!({"schema_version": 1, "tasks": [
                    {"task": "vault:a", "projection": "site:1"},
                    {"task": "vault:b", "projection": "site:1"}]}),
                "names projection site:1 twice",
            ),
            (
                json!({"schema_version": 1, "tasks": [], "projects": [
                    {"source": "site", "allocation": "0123456789abcdef0123456789abcdef"},
                    {"source": "site", "allocation": "fedcba9876543210fedcba9876543210", "project": "p"}]}),
                "names a project in site twice",
            ),
            (
                json!({"schema_version": 1, "tasks": [], "projects": [
                    {"source": "site", "allocation": " "}]}),
                "is not 32 lower-case hex digits",
            ),
            (
                json!({"schema_version": 1, "tasks": [], "projects": [
                    {"source": "site", "allocation": "0123456789abcdef0123456789abcdef", "project": ""}]}),
                "empty project for site",
            ),
            (
                json!({"schema_version": 1, "tasks": [], "projects": [
                    {"source": "site", "allocation": "0123456789abcdef0123456789abcdef", "via": "x"}]}),
                "unknown field",
            ),
        ] {
            let error = PublicProjections::parse(&value).expect_err("refused");
            assert!(error.contains(said), "{value}: {error}");
        }
    }

    #[test]
    fn an_opaque_id_is_stable_and_names_nothing_it_came_from() {
        let first = opaque_id("pt", &id("vault:secret-launch"));
        assert_eq!(first, opaque_id("pt", &id("vault:secret-launch")));
        assert_ne!(first, opaque_id("pt", &id("vault:other")));
        assert!(!first.as_str().contains("secret"), "{first:?}");
        assert!(first.as_str().starts_with("pt-"));
    }

    #[test]
    fn an_anchor_naming_only_itself_is_carried() {
        for content in [
            "<a id=\"library-copy-seconds\"></a>\n\n| Budget | Limit |\n|---|---|\n| copy | 30 |\n\n\
             See [the budget](#user-content-library-copy-seconds).\n",
            "Inline <a name='step_2.b'></a> anchor and <A ID=top> another.",
            "<a id=\"x\" />",
        ] {
            assert!(
                unresolved_references(content, &[]).is_empty(),
                "{content}: {:?}",
                unresolved_references(content, &[])
            );
        }
    }

    #[test]
    fn only_urls_fragments_held_assets_and_bare_html_are_carried() {
        let content = "See [docs](https://example.org/x), [here](#part), \
                       ![shot](./shot.png), [mail](mailto:a@b.c), \
                       [local](../plans/secret.md), [abs](/home/me/x.md), \
                       [file](file:///etc/x) and `[code](./not-a-link.md)` and \
                       `<a href=\"../code.md\">`.\n\n```\n<img src=\"../block.png\">\n```\n\n\
                       <details>\n<summary>More</summary>\n\nText<br/>\n\n</details>\n";
        assert_eq!(
            unresolved_references(content, &["shot.png".to_owned()]),
            vec!["../plans/secret.md", "/home/me/x.md", "file:///etc/x"]
        );
        assert_eq!(
            unresolved_references("![other](./other.png)", &[]),
            vec!["./other.png"]
        );
        for html in [
            "<a href=\"https://example.org/y\">y</a>",
            "<a title=\">\" href=\"../private.md\">x</a>",
            "<img/src=\"../private.png\">",
            "<img srcset=\"a.png 1x, ../b.png 2x\">",
            "<div style=\"background: u\\72l('../c.png')\">c</div>",
            "<style>body { background: url(../d.png) }</style>",
            "<a ping=\"../e\">e</a>",
            "<a id=\"f\" href=\"../f.md\">f</a>",
            "<a href=\"../g.md\" id=\"g\">g</a>",
            "<a name=\"h\" onclick=\"fetch('../h')\">h</a>",
            "<a id=\"i\" style=\"background: url(../i.png)\"></a>",
            "<a id=\"j\" ping=\"../j\"></a>",
            "<a id='k' srcset='../k.png 2x'></a>",
            "<a id=\"l\" src=\"../l.png\"></a>",
            "<a id=\"m\" OnMouseOver=\"x()\"></a>",
            "<a id=\">\" href=\"../n.md\">n</a>",
            "<a id=\"../o.md\"></a>",
            "<a id=\"p&#x2f;q\"></a>",
            "<a id></a>",
            "<a id=\"r\" id=\"s\"></a>",
            "<a id=\"unclosed></a>",
            "<img id=\"t\">",
            "<form action=\"../u\"></form>",
            "<button formaction=\"../v\">v</button>",
            "<script id=\"w\"></script>",
            "<style></style>",
        ] {
            // Prose comparing two things is no tag.
            assert!(unresolved_references("Keep a < b and c > d.", &[]).is_empty());
            assert!(
                !unresolved_references(&format!("Text {html} text."), &[]).is_empty(),
                "{html}"
            );
        }
    }
}
