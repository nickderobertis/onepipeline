//! The shared event-filter grammar, and this library's two uses of it.
//!
//! One grammar across the stack — `{include, exclude}` over `source`, a `kind`
//! glob on the kebab-case wire string, and the reserved labels `run_id`, `node`,
//! `step`, `member`, `persona`. Like the [envelope](crate::event::Envelope)
//! beside it there is deliberately no shared util crate: each producer owns its
//! copy, and `tests/contract.rs` drives the grammar committed in
//! `docs/contract.md` through these types, so a copy that stops matching that
//! text fails its own gate rather than drifting quietly away from the other two.
//!
//! `onepipeline` uses it twice, and the two are not the same thing:
//!
//! - **Source filters** ([`Filters::agentgraph`], [`Filters::vcs`]) are passed
//!   through to the libraries that own those streams — `oneagentgraph`'s
//!   `--event-filter` and `onevcs`'s filtered `EventStream` — so a run stops
//!   paying to relay events nobody will read. They decide what enters the run's
//!   merged store, and they are declared once, at launch.
//! - **Read-time profiles** ([`Filters::profiles`]) shape what one reader is
//!   shown. They never touch the store, so two readers of the same run see the
//!   same events differently and neither loses any.

// llmlint: ignore-file[invalid_states_unrepresentable] these are the *wire* types of a
// grammar shared across three repositories with no shared crate, and the shape is the
// contract: `EventFilter` and `Matcher` are declared field for field as
// `oneagentgraph::event::EventFilter` and `onevcs::EventFilter` declare them, so one spec
// deserializes into the same value whichever producer read it. A newtype over a profile
// name, a kind glob, or a reserved label would make this copy structurally different from
// the other two — the one thing the shared grammar forbids — and would be public
// vocabulary `docs/contract.md` does not name. What is enforced instead is the thing that
// matters: every one of these values arrives by *deserialization*, from a command line, a
// file, or the launch record, and `from_document` refuses an unknown field, a
// field-less matcher, and an empty field at that boundary — so a state `validate` calls
// invalid is not reachable from outside this process.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::event::{Envelope, Labels, Source};

/// The profile `next` and `monitor` read through when a caller names none.
pub const DEFAULT_PROFILE: &str = "planner";

/// The profile that shows the detailed activity the default one leaves out.
pub const MONITOR_PROFILE: &str = "monitor";

/// The matcher fields the grammar has, for the refusal that names them.
const MATCHER_FIELDS: &str = "`source`, `kind`, `run_id`, `node`, `step`, `member`, `persona`";

/// The launch-config schema version this build **writes**.
///
/// **3** since a launch declares the command a node introduced by a live edit is
/// checked by: `node_validator` is a key versions 1 and 2 never had, so a
/// document carrying it is a different document and says so.
pub const LAUNCH_CONFIG_SCHEMA_VERSION: u32 = 3;

/// Every launch-config version this build **reads**, newest first.
///
/// The same rule the plan schema is read by, and for the same reason: a config
/// is a file an operator wrote at a version, and what each version added is
/// keyed to the version the document declares. An earlier config is a complete
/// document — a version-1 one says nothing about drafting and a version-2 one
/// says nothing about validating, which is what a launch naming neither means —
/// and naming a later key there is refused **by that field's name**, exactly as
/// a key no version ever had is.
pub const LAUNCH_CONFIG_SCHEMA_VERSIONS_READ: [u32; 3] = [LAUNCH_CONFIG_SCHEMA_VERSION, 2, 1];

/// Each key younger than the schema itself: the version it arrived at, and
/// whether a blank value is refused.
///
/// A table rather than a comparison against [`LAUNCH_CONFIG_SCHEMA_VERSION`]:
/// asked that way, every earlier key becomes refused the moment the schema
/// version moves again, and a version-2 config naming the drafting graph
/// version 2 introduced would start being turned down by the bump that added an
/// unrelated key.
///
/// The blank rule is **per key and not per schema**, for the same reason. It is
/// `node_validator`'s alone: that key arrives with this version, so no config
/// on disk carries a blank one, and refusing it costs nobody a launch that used
/// to work. `pr_author_graph` has shipped since version 2 and a document
/// already written may carry a blank one; whatever that meant then it goes on
/// meaning, because a build that started refusing it would break a config over
/// a key the operator did not change.
const KEYS_BY_VERSION: &[(&str, u32, BlankValue)] = &[
    ("pr_author_graph", 2, BlankValue::Kept),
    ("node_validator", 3, BlankValue::Refused),
];

/// What a key present and holding nothing means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlankValue {
    /// Refused by the key's own name: a decision half-written, which everything
    /// downstream would read as a launch that named one.
    Refused,
    /// Read as the document wrote it, whatever that key made of it before —
    /// the promise every config already on disk was written against.
    Kept,
}

/// A launch config: what a launch declares about its run, as one document.
///
/// The `filters:` block is long enough to be worth keeping in a file beside the
/// plan rather than pasted onto one line of argv, and it is the kind of thing a
/// team writes once and reuses across launches — so `start --launch-config FILE`
/// reads it, and the repeatable flags spell exactly the same block for a launch
/// that would rather say it inline.
///
/// A block rather than a bare `filters:` key at the document root, because what
/// a launch declares is a subject of its own: this is where a second launch-level
/// decision goes, rather than beside the filters that happen to be the first one.
///
/// Versioned and closed. It is **external input** — a file an operator wrote —
/// so an unknown key is refused by name rather than silently dropped, a key a
/// declared version never had is refused by *its* name, and a document declaring
/// a version this build does not read is refused by its number rather than read
/// as though it said something else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchConfig {
    /// Schema version; [`LAUNCH_CONFIG_SCHEMA_VERSION`] for anything this crate
    /// writes.
    pub schema_version: u32,
    /// What this launch says about its run's events.
    ///
    /// Omitted when empty, so a config that declares nothing about events
    /// round-trips as the file wrote it.
    #[serde(default, skip_serializing_if = "Filters::is_empty")]
    pub filters: Filters,
    /// The agent graph this launch drafts change request bodies with, if any.
    ///
    /// The second launch-level decision, and it is one a team writes down beside
    /// a plan for the same reason the filters are: which graph authors a change
    /// request is a property of how a team works rather than of one launch.
    /// `--pr-author-graph` spells the same thing for a launch that would rather
    /// say it inline, and overrides this when both are given.
    ///
    /// A key [`LAUNCH_CONFIG_SCHEMA_VERSION`] added, so a document below it may
    /// not carry one. Omitted when absent, so a config that names no graph
    /// round-trips as the file wrote it — and so what this crate writes for a
    /// launch that named none is a document a version-1 reader still accepts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_author_graph: Option<String>,
    /// The command this launch checks a live-edited node with, if any.
    ///
    /// The third launch-level decision, and it is written down beside a plan for
    /// the reason the first two are: which rules a node has to satisfy before it
    /// is dispatched is a property of how a team works rather than of one
    /// launch. `--node-validator` spells the same thing for a launch that would
    /// rather say it inline and overrides this, as does
    /// `ONEPIPELINE_NODE_VALIDATOR` between them.
    ///
    /// A key [`LAUNCH_CONFIG_SCHEMA_VERSION`] added, so a document below it may
    /// not carry one. Omitted when absent, so a config that names no validator
    /// round-trips as the file wrote it — and so what this crate writes for a
    /// launch that named none is a document an earlier reader still accepts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_validator: Option<String>,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            schema_version: LAUNCH_CONFIG_SCHEMA_VERSION,
            filters: Filters::default(),
            pr_author_graph: None,
            node_validator: None,
        }
    }
}

impl LaunchConfig {
    /// Read a launch config file: JSON, or the YAML the document is written in,
    /// of which JSON is a subset.
    ///
    /// Read the way [`Plan::load`](crate::plan::Plan::load) reads a plan, and
    /// refused at the same boundary: this is a file an operator wrote, and the
    /// only place it can be refused *before* a run exists is where it is read.
    ///
    /// # Errors
    ///
    /// [`Error::Ledger`] for a file that cannot be read, and [`Error::Invalid`]
    /// — naming the path — for a document this schema does not accept, a key its
    /// declared version never had, a version this build does not read, or a
    /// filter that could not be honoured.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Ledger {
            path: path.to_path_buf(),
            source,
        })?;
        let named = |why: String| Error::Invalid(format!("{}: {why}", path.display()));
        let config: Self =
            serde_norway::from_str(&text).map_err(|failure| named(failure.to_string()))?;
        if !LAUNCH_CONFIG_SCHEMA_VERSIONS_READ.contains(&config.schema_version) {
            let known = LAUNCH_CONFIG_SCHEMA_VERSIONS_READ
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(named(format!(
                "launch config schema_version {}, and this build reads {known} — set \
                 `schema_version: {LAUNCH_CONFIG_SCHEMA_VERSION}`",
                config.schema_version
            )));
        }
        // The field's own name, not the version's number: an operator who wrote a
        // drafting graph and had it dropped would find that out from a change
        // request nobody drafted a body for, and one who wrote a validator would
        // find it out from a node nothing checked.
        let carried: [(&str, Option<&String>); 2] = [
            ("pr_author_graph", config.pr_author_graph.as_ref()),
            ("node_validator", config.node_validator.as_ref()),
        ];
        for (key, value) in carried {
            let Some((arrived, blank)) = KEYS_BY_VERSION
                .iter()
                .find_map(|(named, at, blank)| (*named == key).then_some((*at, *blank)))
            else {
                continue;
            };
            if value.is_some() && config.schema_version < arrived {
                return Err(named(format!(
                    "`{key}` is a schema {arrived} key and this config declares schema_version \
                     {} — set `schema_version: {LAUNCH_CONFIG_SCHEMA_VERSION}`",
                    config.schema_version
                )));
            }
            // A key present and blank is a decision half-written: it reads as
            // "this launch names one" everywhere downstream and resolves to a
            // command nothing can start. Refused here, at the boundary, rather
            // than left to fail every edit later — the only thing about a
            // command this crate can check is that there is one.
            //
            // Only for a key that arrives with this version. An older one may
            // already carry a blank value in a file somebody wrote, and turning
            // that document down would break a launch over a key its author
            // never touched — see [`KEYS_BY_VERSION`].
            if blank == BlankValue::Refused && value.is_some_and(|value| value.trim().is_empty()) {
                return Err(named(format!(
                    "`{key}` is present and names nothing — give it a value, or leave the \
                     key out to declare that this launch has none"
                )));
            }
        }
        Ok(config)
    }
}

/// What one launch says about its run's events.
///
/// Empty is what every launch made before this block existed says, and goes on
/// meaning: nothing is filtered on the way into the store, and the shipped
/// profiles are what a reader reads through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filters {
    /// Forwarded to every `oneagentgraph` launch this run starts, restricting
    /// what that source relays into the merged store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agentgraph: Option<EventFilter>,
    /// Passed to every followed `onevcs` session's stream, restricting what that
    /// source relays into the merged store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs: Option<EventFilter>,
    /// Named read-time profiles, overriding the shipped ones by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, EventFilter>,
}

impl Filters {
    /// Whether this launch declared nothing at all, which is what a record
    /// written before the block existed carries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// The profile a reader named, or the reason there is none.
    ///
    /// A launch's own profile of that name wins over the shipped one, so both
    /// `planner` and `monitor` are overridable without being special-cased here:
    /// the launch's map is consulted first and the shipped defaults are the
    /// fallback.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] naming the profile asked for and listing the ones this
    /// run has, because a planner who mistyped a profile name would otherwise be
    /// silently served the default view of a run they meant to look at another
    /// way.
    pub fn profile(&self, name: &str) -> Result<EventFilter> {
        if let Some(filter) = self.profiles.get(name) {
            return Ok(filter.clone());
        }
        if let Some(filter) = shipped_profile(name) {
            return Ok(filter);
        }
        let mut names: Vec<&str> = self.profiles.keys().map(String::as_str).collect();
        for shipped in [DEFAULT_PROFILE, MONITOR_PROFILE] {
            if !names.contains(&shipped) {
                names.push(shipped);
            }
        }
        names.sort_unstable();
        Err(Error::Invalid(format!(
            "'{name}' is not a filter profile of this run; it has {}",
            names.join(", ")
        )))
    }
}

/// The shipped profile of that name, before any launch override.
///
/// `planner` is every pipeline-level event and nothing else — node dispatch,
/// settlement and failure, decisions, surfaces, edits, attestations, stop and
/// adopt — with the detailed `agentgraph` and `vcs` activity behind them left
/// out, because planner attention is the scarce resource. `monitor` is
/// unfiltered: the observer's whole job is to read the detail.
fn shipped_profile(name: &str) -> Option<EventFilter> {
    match name {
        DEFAULT_PROFILE => Some(EventFilter {
            include: vec![Matcher {
                source: Some(Source::Pipeline),
                ..Matcher::default()
            }],
            exclude: Vec::new(),
        }),
        MONITOR_PROFILE => Some(EventFilter::default()),
        _ => None,
    }
}

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] the duplication is
// the approved contract's own mechanism rather than a missing gate, and it cannot be
// closed from inside one of the three repositories: `oneagentgraph`, `onevcs`, and this
// crate are released independently, so a shared crate would make them co-version — the
// same decision, and the same reasoning, as the envelope in `src/event.rs` beside it. The
// source is the grammar committed in `docs/contract.md`, and the gate is
// `tests/contract.rs`, which extracts that document's own `filters:` fixture and drives it
// through the types below rather than restating it — so a copy that stops matching the
// text fails `just check`. Each sibling carries the same text and runs the same gate
// against it, and the cross-repository half is the contract owner reading one committed
// grammar. `docs/contract-divergences.md` entry 32 records the corners of that agreement
// this repository cannot enforce alone, as a proposal to the planner who owns it.

/// Which envelopes a consumer of a stream is shown.
///
/// [`EventFilter::default`] — no matcher on either list — admits everything, so
/// a run naming no filter streams exactly what it always did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct EventFilter {
    /// Matchers an envelope satisfies one of to pass. Absent or empty admits
    /// every envelope, so a filter that only rejects need name nothing here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<Matcher>,
    /// Matchers that reject. A match here rejects whatever
    /// [`include`](Self::include) said, so a broad include beside a narrow
    /// exclude is how "all of this except that" is written.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<Matcher>,
}

/// One matcher: every field it names must hold of an envelope, and a field it
/// does not name is not consulted.
///
/// Deliberately absent: `stream`, which identifies a producing process rather
/// than anything a consumer means by an event; the payload, whose fields differ
/// per kind; and `round`, which the approved matcher list does not name and
/// which nothing this library writes stamps any more.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Matcher {
    /// The producing library, by exact equality.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    /// A glob over the kind's kebab-case wire string, where `*` stands for any
    /// run of characters including none and every other character is itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The `run_id` label the envelope was stamped with, by exact equality.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The `node` label the envelope was stamped with, by exact equality.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// The `step` label the envelope was stamped with, by exact equality.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// The `member` label the envelope was stamped with, by exact equality.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// The `persona` label the envelope was stamped with, by exact equality.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

impl EventFilter {
    /// Read a filter from the text of a spec: JSON, or the YAML the grammar is
    /// written in, of which JSON is a subset.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] carrying the refusal, which names the matcher it is
    /// about — which list, and which position in it — because that is what an
    /// operator has to find in what they wrote. Both refusals are here: a
    /// document that is not a filter, and a filter that could not be honoured —
    /// see [`validate`](Self::validate).
    pub fn parse(spec: &str) -> Result<Self> {
        serde_norway::from_str(spec)
            .map_err(|failure| Error::Invalid(format!("the event filter is unusable: {failure}")))
    }

    /// The filter a spec names: a path to a file holding one, or the document
    /// itself inline as JSON.
    ///
    /// A spec whose first non-space character is `{` is the document — the shape
    /// a caller composing one line of argv writes — and anything else is a path
    /// to one, read as YAML so a filter kept beside a plan is written the way it
    /// would be written inside the launch record.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for a file that cannot be read, or for anything
    /// [`parse`](Self::parse) refuses.
    pub fn read(spec: &str) -> Result<Self> {
        if spec.trim_start().starts_with('{') {
            return Self::parse(spec);
        }
        let document = std::fs::read_to_string(Path::new(spec)).map_err(|failure| {
            Error::Invalid(format!("cannot read the event filter {spec}: {failure}"))
        })?;
        Self::parse(&document)
    }

    /// Whether an envelope reaches a consumer reading through this filter.
    ///
    /// `exclude` wins: a matcher there rejects whatever `include` admitted, and
    /// an empty `include` admits everything.
    #[must_use]
    pub fn matches(&self, envelope: &Envelope) -> bool {
        self.allows(envelope.source, &envelope.kind.0, &envelope.labels)
    }

    /// [`matches`](Self::matches), for a caller holding the three addressing
    /// values rather than a whole envelope.
    ///
    /// The kind arrives as its wire string rather than as a [`PipelineKind`], because
    /// the merged stream carries what a sibling library relayed as well as what
    /// this one produced: a filter typed on this crate's closed set would have to
    /// either silence every relayed event or refuse a spec for naming one.
    ///
    /// [`PipelineKind`]: crate::event::PipelineKind
    #[must_use]
    pub fn allows(&self, source: Source, kind: &str, labels: &Labels) -> bool {
        if self
            .exclude
            .iter()
            .any(|matcher| matcher.matches(source, kind, labels))
        {
            return false;
        }
        self.include.is_empty()
            || self
                .include
                .iter()
                .any(|matcher| matcher.matches(source, kind, labels))
    }

    /// Whether every matcher in this filter could match anything.
    ///
    /// A spec is external input — a `--filter` an operator typed, or the block a
    /// launch record carries — so this is its trust boundary, and a launch checks
    /// it before it starts rather than after a paid turn has been spent streaming
    /// the wrong thing.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] naming the offending matcher — which list it is in,
    /// where in that list, and what it says — for a matcher that names no field
    /// at all (it matches *every* envelope, so one in `exclude` silences the
    /// stream entirely), or one whose field is empty (nothing on the stream
    /// carries an empty kind or an empty label, so it matches nothing).
    pub fn validate(&self) -> Result<()> {
        for (list, matchers) in [("include", &self.include), ("exclude", &self.exclude)] {
            for (at, matcher) in matchers.iter().enumerate() {
                matcher.check().map_err(|why| {
                    Error::Invalid(format!(
                        "the event filter's {list} matcher {}: {why}",
                        at + 1
                    ))
                })?;
            }
        }
        Ok(())
    }
}

/// Routed through the same reading [`EventFilter::parse`] uses rather than
/// derived, so a filter embedded in a launch record is refused by the same rules
/// — and with the same message — as one typed on a command line.
impl<'de> Deserialize<'de> for EventFilter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let document = Value::deserialize(deserializer)?;
        from_document(&document).map_err(serde::de::Error::custom)
    }
}

/// The filter a document holds, or the reason it is not one.
fn from_document(document: &Value) -> std::result::Result<EventFilter, String> {
    let object = document.as_object().ok_or_else(|| {
        format!(
            "an event filter is a mapping of `include` and `exclude`, not {}",
            shape(document)
        )
    })?;
    if let Some(stray) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "include" | "exclude"))
    {
        return Err(format!(
            "an event filter names `include` and `exclude`; {stray:?} is neither"
        ));
    }
    let filter = EventFilter {
        include: matchers(object.get("include"), "include")?,
        exclude: matchers(object.get("exclude"), "exclude")?,
    };
    // Both refusals at the one boundary. A spec arrives from a command line, from
    // a file beside a plan, and — every time a later `next` or `monitor` opens a
    // run — from the launch record on disk, which is external input like any
    // other file this process re-reads. A filter checked only where an operator
    // typed it would be a launch record that could be edited into a matcher this
    // build says it will not honour, and then honoured.
    filter.validate().map_err(|refusal| refusal.to_string())?;
    Ok(filter)
}

/// The matchers one of the two lists holds, or the reason it is not a list of
/// them.
fn matchers(value: Option<&Value>, list: &str) -> std::result::Result<Vec<Matcher>, String> {
    // Absent is the documented "everything passes include" / "nothing is
    // excluded". Present-but-not-a-list is not: `include:` with nothing after it
    // means one of those two to whoever wrote it and the other to whoever reads
    // it, which is the guess this refuses to make.
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let entries = value.as_array().ok_or_else(|| {
        format!(
            "an event filter's `{list}` is a list of matchers, not {}",
            shape(value)
        )
    })?;
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| matcher(entry, list, index + 1))
        .collect()
}

/// One matcher of a list, or the reason it is not one.
fn matcher(value: &Value, list: &str, position: usize) -> std::result::Result<Matcher, String> {
    let named = format!("the event filter's {list} matcher {position}");
    let fields = value.as_object().ok_or_else(|| {
        format!(
            "{named} is a mapping of matcher fields, not {}",
            shape(value)
        )
    })?;
    let mut matcher = Matcher::default();
    for (field, value) in fields {
        match field.as_str() {
            // The families are named by serde's own refusal rather than restated
            // here: `Source`'s derive already spells every one it has, and a
            // second copy is a list that a family added to the enum leaves
            // behind.
            "source" => {
                matcher.source = Some(
                    serde_json::from_value(value.clone())
                        .map_err(|failure| format!("{named} names no source family: {failure}"))?,
                );
            }
            "kind" => matcher.kind = Some(text(value, &named, field)?),
            "run_id" => matcher.run_id = Some(text(value, &named, field)?),
            "node" => matcher.node = Some(text(value, &named, field)?),
            "step" => matcher.step = Some(text(value, &named, field)?),
            "member" => matcher.member = Some(text(value, &named, field)?),
            "persona" => matcher.persona = Some(text(value, &named, field)?),
            unknown => {
                return Err(format!(
                    "{named} names {unknown:?}, which is not a matcher field ({MATCHER_FIELDS})"
                ))
            }
        }
    }
    Ok(matcher)
}

/// One matcher field's value, which every field but `source` compares as a
/// string.
fn text(value: &Value, named: &str, field: &str) -> std::result::Result<String, String> {
    value.as_str().map(str::to_owned).ok_or_else(|| {
        format!(
            "{named} matches {field} against {}, which is not a string",
            shape(value)
        )
    })
}

/// What a value is, for a refusal that has to say what was there instead.
fn shape(value: &Value) -> &'static str {
    match value {
        Value::Null => "nothing",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "a mapping",
    }
}

impl Matcher {
    /// What this matcher asks of the reserved labels, in the order the grammar
    /// lists them.
    ///
    /// One list rather than two, because [`matches`](Self::matches) and
    /// [`check`](Self::check) must read exactly the same keys: a key added to the
    /// grammar and to only one of them is either unchecked or unmatched, and both
    /// are silent.
    fn labels_asked(&self) -> [(&'static str, Option<&str>); 5] {
        [
            ("run_id", self.run_id.as_deref()),
            ("node", self.node.as_deref()),
            ("step", self.step.as_deref()),
            ("member", self.member.as_deref()),
            ("persona", self.persona.as_deref()),
        ]
    }

    /// Whether every field this matcher names holds of the envelope.
    fn matches(&self, source: Source, kind: &str, labels: &Labels) -> bool {
        if self.source.is_some_and(|named| named != source) {
            return false;
        }
        if self
            .kind
            .as_deref()
            .is_some_and(|pattern| !glob(pattern, kind))
        {
            return false;
        }
        // `member` has no typed slot on this crate's `Labels` — the reserved keys
        // it declares are the ones a `DispatchRequest` carries — so it is read
        // out of the extras like any other stamp, which is where a relayed
        // sibling envelope puts it.
        let typed = [
            labels.run_id.as_deref(),
            labels.node.as_deref(),
            labels.step.as_deref(),
            None,
            labels.persona.as_deref(),
        ];
        // A label the envelope never stamped is `None`, which no asked-for value
        // equals — "a matcher naming a label the envelope did not stamp does not
        // match it".
        self.labels_asked()
            .iter()
            .zip(typed)
            .all(|((key, asked), typed)| match asked {
                None => true,
                Some(asked) => stamped(&labels.extra, key, typed) == Some(*asked),
            })
    }

    /// Whether this matcher could match anything; see [`EventFilter::validate`].
    fn check(&self) -> std::result::Result<(), String> {
        let mut named = usize::from(self.source.is_some());
        for (field, asked) in
            std::iter::once(("kind", self.kind.as_deref())).chain(self.labels_asked())
        {
            let Some(asked) = asked else { continue };
            named += 1;
            if asked.trim().is_empty() {
                return Err(format!(
                    "`{field}` is empty, and nothing on the stream carries an empty {field} — \
                     omit the field to leave it unasked"
                ));
            }
        }
        if named == 0 {
            return Err(format!(
                "a matcher naming no field matches every event — name at least one of \
                 {MATCHER_FIELDS}"
            ));
        }
        Ok(())
    }
}

/// What an envelope carries under one reserved label key.
///
/// The typed slot, or — where that is unset — the same key among the extras,
/// because a matcher asks about the key *as the envelope carries it*. [`Labels`]
/// flattens its extras beside the reserved fields, so a stamp a relaying sibling
/// wrote under a name this crate has no typed slot for still reaches the wire
/// under exactly the name the grammar names, and a filter that consulted only
/// the typed slot would refuse to see a label its own consumer can plainly read.
/// A non-string extra is not a label value and matches nothing.
fn stamped<'a>(
    extra: &'a Map<String, Value>,
    key: &str,
    typed: Option<&'a str>,
) -> Option<&'a str> {
    typed.or_else(|| extra.get(key).and_then(Value::as_str))
}

/// Whether `pattern` matches `text`, where `*` stands for any run of characters
/// including none and every other character is itself.
///
/// The whole dialect, stated rather than inherited: this is a cross-repo grammar
/// with no shared implementation, so a `?` or a `[a-z]` supported here and
/// nowhere else would be a spec that filters differently depending on which
/// producer read it. Kebab-case wire strings need neither.
fn glob(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    // Where to resume from if the run this `*` is currently standing for turns
    // out to be one character too short.
    let (mut star, mut resume) = (None, 0);
    while t < text.len() {
        if pattern.get(p) == Some(&'*') {
            star = Some(p);
            resume = t;
            p += 1;
        } else if pattern.get(p) == Some(&text[t]) {
            p += 1;
            t += 1;
        } else if let Some(at) = star {
            p = at + 1;
            resume += 1;
            t = resume;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|character| *character == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checked-in shape of a launch config, one file per version this build
    /// reads.
    ///
    /// Read rather than restated: this is the document an operator writes and a
    /// later build parses, and the only thing that stops a key being renamed, an
    /// omitted block becoming an explicit empty one, or the version moving
    /// without anyone deciding to move it. The earlier ones stay checked in for
    /// the half a single golden cannot pin — that a config written before the
    /// current version is still a document this build reads.
    const GOLDEN: &str = include_str!("../tests/golden/launch-config-v3.json");

    /// The same document as each earlier version wrote it: the block it had, and
    /// no key that version never had, newest first.
    const GOLDEN_EARLIER: [(u32, &str); 2] = [
        (2, include_str!("../tests/golden/launch-config-v2.json")),
        (1, include_str!("../tests/golden/launch-config-v1.json")),
    ];

    /// The filters both goldens carry.
    ///
    /// Both source filters and both shipped profile names, because each is a
    /// distinct shape on the wire — an `exclude`-only filter, an `include` of
    /// several matchers, an overridden profile, and the empty filter that means
    /// "unfiltered" — and a golden carrying one of them would pin a quarter of
    /// the document.
    fn pinned_filters() -> Filters {
        let kind = |glob: &str| Matcher {
            kind: Some(glob.to_string()),
            ..Matcher::default()
        };
        Filters {
            agentgraph: Some(EventFilter {
                include: Vec::new(),
                exclude: vec![kind("turn-activity")],
            }),
            vcs: Some(EventFilter {
                include: vec![kind("gate-*"), kind("session-closed")],
                exclude: Vec::new(),
            }),
            profiles: [
                (
                    DEFAULT_PROFILE.to_string(),
                    shipped_profile(DEFAULT_PROFILE).expect("planner ships"),
                ),
                (
                    MONITOR_PROFILE.to_string(),
                    shipped_profile(MONITOR_PROFILE).expect("monitor ships"),
                ),
            ]
            .into_iter()
            .collect(),
        }
    }

    /// The document [`GOLDEN`] pins, built through the types: the block, and the
    /// launch's other decision.
    fn golden() -> LaunchConfig {
        LaunchConfig {
            schema_version: LAUNCH_CONFIG_SCHEMA_VERSION,
            filters: pinned_filters(),
            pr_author_graph: Some("./graphs/pr-author.yaml".to_string()),
            node_validator: Some("./scripts/check-node.sh".to_string()),
        }
    }

    #[test]
    fn a_launch_config_is_the_shape_its_version_golden_pins() {
        let rendered = serde_json::to_string_pretty(&golden()).expect("it serialises");
        assert_eq!(
            rendered.trim(),
            GOLDEN.trim(),
            "the launch config changed shape. If that was deliberate, bump \
             LAUNCH_CONFIG_SCHEMA_VERSION and add tests/golden/launch-config-v<n>.json \
             in the same change"
        );
    }

    #[test]
    fn the_schema_version_and_the_golden_name_the_same_number() {
        let parsed: LaunchConfig = serde_json::from_str(GOLDEN).expect("the golden parses");
        assert_eq!(parsed.schema_version, LAUNCH_CONFIG_SCHEMA_VERSION);
        assert_eq!(parsed, golden(), "the golden is not the document it pins");
    }

    /// Every version before this one is still a document this build reads.
    ///
    /// A promise to every config already written beside a plan: each carries the
    /// same block, declares its own number, and says nothing about the keys its
    /// version did not have — which is what a launch naming no drafting graph
    /// and no validator means. Held against the checked-in files rather than
    /// strings built here, because those files are what an operator has on disk.
    #[test]
    fn every_earlier_version_still_reads_and_says_nothing_about_the_keys_it_never_had() {
        for (version, golden) in GOLDEN_EARLIER {
            let earlier: LaunchConfig =
                serde_json::from_str(golden).expect("the earlier golden parses");
            assert_eq!(
                earlier,
                LaunchConfig {
                    schema_version: version,
                    filters: pinned_filters(),
                    // Version 2 is the one that declared the drafting graph, and
                    // it names one; version 1 never had the key at all.
                    pr_author_graph: (version >= 2).then(|| "./graphs/pr-author.yaml".to_string()),
                    node_validator: None,
                }
            );
            assert!(
                LAUNCH_CONFIG_SCHEMA_VERSIONS_READ.contains(&earlier.schema_version),
                "the version the earlier golden declares is not one this build reads"
            );
            // And it is an *earlier* document, not this one wearing an older
            // number: what this build writes carries the current version.
            assert_ne!(earlier.schema_version, LAUNCH_CONFIG_SCHEMA_VERSION);
        }
    }

    /// The launch's other two decisions round-trip when they are there and are
    /// written as no key at all when they are not.
    ///
    /// The second half is what keeps each bump additive: a launch that named
    /// neither is written as a document carrying nothing about drafting or
    /// validating, which is what an earlier reader accepts and what an earlier
    /// file already says.
    #[test]
    fn the_launch_level_keys_round_trip_when_named_and_are_omitted_when_they_are_not() {
        let named = LaunchConfig {
            pr_author_graph: Some("./graphs/pr-author.yaml".to_string()),
            node_validator: Some("./scripts/check-node.sh".to_string()),
            ..LaunchConfig::default()
        };
        let rendered = serde_json::to_string(&named).expect("it serialises");
        assert_eq!(
            rendered,
            format!(
                r#"{{"schema_version":{LAUNCH_CONFIG_SCHEMA_VERSION},"pr_author_graph":"./graphs/pr-author.yaml","node_validator":"./scripts/check-node.sh"}}"#
            )
        );
        assert_eq!(
            serde_json::from_str::<LaunchConfig>(&rendered).expect("it re-parses"),
            named
        );

        let unnamed = LaunchConfig::default();
        let rendered = serde_json::to_string(&unnamed).expect("it serialises");
        for key in ["pr_author_graph", "node_validator"] {
            assert!(
                !rendered.contains(key),
                "a launch that named no {key} was written one: {rendered}"
            );
        }
        assert_eq!(
            serde_json::from_str::<LaunchConfig>(&rendered).expect("it re-parses"),
            unnamed
        );
    }

    /// A config that declares no events round-trips as the file wrote it.
    ///
    /// The backward-compatible half, checked at the wire rather than through the
    /// types: `Filters::default()` and an explicit `filters: {}` are the same
    /// value in Rust whatever the serializer does, but writing the empty block
    /// out would have every consumer branching on a key that is always present
    /// and usually meaningless — and would stop a document written before the
    /// block existed from being what this build writes back.
    #[test]
    fn a_launch_config_declaring_no_events_omits_the_block_and_round_trips() {
        let bare = LaunchConfig::default();
        let rendered = serde_json::to_string(&bare).expect("it serialises");
        assert_eq!(
            rendered,
            format!(r#"{{"schema_version":{LAUNCH_CONFIG_SCHEMA_VERSION}}}"#)
        );
        assert_eq!(
            serde_json::from_str::<LaunchConfig>(&rendered).expect("it re-parses"),
            bare
        );

        // And the version alone is a whole document, at every version this build
        // reads: a config that says nothing else is what a launch naming no
        // filters already means.
        for version in LAUNCH_CONFIG_SCHEMA_VERSIONS_READ {
            let minimal: LaunchConfig =
                serde_norway::from_str(&format!("schema_version: {version}\n"))
                    .expect("a bare config parses");
            assert_eq!(minimal.schema_version, version);
            assert!(minimal.filters.is_empty());
            assert_eq!(minimal.pr_author_graph, None);
            assert_eq!(minimal.node_validator, None);
        }
    }

    /// Every filter shape survives the wire, and an empty list is never written.
    #[test]
    fn a_launch_config_round_trips_without_losing_or_inventing_a_field() {
        let full = golden();
        let text = serde_norway::to_string(&full).expect("it serialises as YAML too");
        assert_eq!(
            serde_norway::from_str::<LaunchConfig>(&text).expect("it re-parses"),
            full
        );

        // The unfiltered profile is `{}` on the wire — both lists empty, and
        // neither written — so a reader can tell "admits everything" from a
        // profile that was never declared.
        let value: Value = serde_json::from_str(GOLDEN).expect("the golden is JSON");
        assert_eq!(
            value["filters"]["profiles"]["monitor"],
            serde_json::json!({})
        );
        assert!(
            value["filters"]["agentgraph"].get("include").is_none(),
            "an empty include was written out: {value}"
        );
    }

    /// A config already on disk carrying a blank `pr_author_graph` reads exactly
    /// as it always did.
    ///
    /// The regression this exists for: the blank-value refusal that arrived with
    /// `node_validator` was written for every key at once, and applied to
    /// `pr_author_graph` it turns down a document an operator wrote against a
    /// build that accepted it — a launch broken over a key its author never
    /// touched. Whatever a blank drafting graph meant at version 2 it goes on
    /// meaning: the value is read as written, `Some("")` and not `None`, and the
    /// document loads.
    ///
    /// Held at both versions that have the key, and with the surrounding block
    /// intact, because what has to keep working is the file as it is on disk
    /// rather than the key on its own.
    #[test]
    fn a_config_carrying_a_blank_drafting_graph_still_loads_as_it_always_did() {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-config-blank-drafting-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("a scratch directory");

        for version in [2, LAUNCH_CONFIG_SCHEMA_VERSION] {
            for written in ["\"\"", "\"   \""] {
                let path = root.join(format!("v{version}-{}.yaml", written.len()));
                std::fs::write(
                    &path,
                    format!(
                        "schema_version: {version}\n\
                         pr_author_graph: {written}\n\
                         filters:\n\
                        \x20 vcs:\n\
                        \x20   include:\n\
                        \x20     - kind: session-closed\n"
                    ),
                )
                .expect("the config is written");

                let read = LaunchConfig::load(&path).unwrap_or_else(|refusal| {
                    panic!(
                        "a schema-{version} config carrying a blank `pr_author_graph` no longer \
                         loads, which breaks every one already on disk: {refusal}"
                    )
                });
                assert_eq!(
                    read.pr_author_graph.as_deref(),
                    // As written, whitespace and all: this build does not decide
                    // for an operator what their blank value meant.
                    Some(written.trim_matches('"')),
                    "a blank drafting graph was read as something other than what the file said"
                );
                assert_eq!(read.schema_version, version);
                // The rest of the document is untouched by any of this.
                assert!(read.filters.vcs.is_some(), "the block was dropped");
                assert_eq!(read.node_validator, None);
            }
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// The version is refused by its number, an unknown key by its name, and a
    /// key an earlier version never had by *that* key's name.
    #[test]
    fn a_launch_config_this_build_cannot_read_is_refused_by_name() {
        let root = std::env::temp_dir().join(format!("onepipeline-config-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("a scratch directory");
        let written = |name: &str, body: &str| {
            let path = root.join(name);
            std::fs::write(&path, body).expect("the config is written");
            path
        };

        // A number this build has never written, told the versions it reads.
        let later = LaunchConfig::load(&written("later.yaml", "schema_version: 7\n"))
            .expect_err("a version this build does not read is refused");
        let said = later.to_string();
        assert!(said.contains("schema_version 7"), "{said}");
        for version in LAUNCH_CONFIG_SCHEMA_VERSIONS_READ {
            assert!(
                said.contains(&version.to_string()),
                "the refusal does not name version {version}, which this build reads: {said}"
            );
        }

        // A key the version this document declares never had: refused by that
        // key's name, because the key is what its author has to act on and the
        // alternative is a drafting graph nobody drafts with, or a validator
        // that checks nothing.
        //
        // Each key is refused by **its own** arrival version rather than by the
        // schema's current number: a version-2 config naming the drafting graph
        // version 2 introduced is a document this build reads, and a rule
        // written the other way would have turned it down the day an unrelated
        // key moved the schema on.
        for (key, arrived, value) in [
            ("pr_author_graph", 2, "./graphs/pr-author.yaml"),
            ("node_validator", 3, "./scripts/check-node.sh"),
        ] {
            let early = LaunchConfig::load(&written(
                &format!("early-{key}.yaml"),
                &format!("schema_version: {}\n{key}: {value}\n", arrived - 1),
            ))
            .expect_err("a key a declared version never had is refused");
            let said = early.to_string();
            assert!(said.contains(&format!("`{key}`")), "{said}");
            assert!(said.contains(&format!("schema {arrived} key")), "{said}");

            // And the same document at the version that has it is read.
            let read = LaunchConfig::load(&written(
                &format!("arrived-{key}.yaml"),
                &format!("schema_version: {arrived}\n{key}: {value}\n"),
            ))
            .expect("the version that declares the key reads it");
            let named = match key {
                "pr_author_graph" => read.pr_author_graph.as_deref(),
                _ => read.node_validator.as_deref(),
            };
            assert_eq!(named, Some(value));
        }

        // The key that arrives with this version, present and blank: a decision
        // half-written rather than a launch that declared nothing.
        let blank = LaunchConfig::load(&written(
            "blank-node-validator.yaml",
            &format!("schema_version: {LAUNCH_CONFIG_SCHEMA_VERSION}\nnode_validator: \"   \"\n"),
        ))
        .expect_err("a validator that names nothing is refused");
        let said = blank.to_string();
        assert!(
            said.contains("`node_validator`") && said.contains("names nothing"),
            "{said}"
        );

        let stray = LaunchConfig::load(&written(
            "stray.yaml",
            "schema_version: 1\nfilterz:\n  vcs: {}\n",
        ))
        .expect_err("a key this schema does not declare is refused");
        assert!(stray.to_string().contains("filterz"), "{stray}");

        // The filter grammar's own refusals reach here too: the config is one
        // more boundary the same spec crosses.
        let unusable = LaunchConfig::load(&written(
            "unusable.yaml",
            "schema_version: 1\nfilters:\n  vcs:\n    include:\n      - role: agent\n",
        ))
        .expect_err("a matcher field the grammar does not have is refused");
        assert!(unusable.to_string().contains("role"), "{unusable}");

        let missing = LaunchConfig::load(&root.join("nothing-here.yaml"))
            .expect_err("a file that is not there is refused");
        assert!(missing.to_string().contains("nothing-here"), "{missing}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
