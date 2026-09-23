//! The agent stack's event vocabulary: the words every producer's envelopes are
//! written in, declared once here because this is the one crate that names them
//! all.
//!
//! `onemessagebus` knows the *shape* of an envelope — a version, a stamp, a
//! stream, a sequence number, a source, a kind, the labels, the payload, the
//! artifacts — and nothing about agents. A [`Vocabulary`] supplies the words as
//! types, and [`Agent`] is this stack's: the three [`Source`]s, the four
//! [`Phase`]s, the six reserved [`Labels`], the [`MatchFields`] a matcher may
//! name, and the schema families the stack registers — the event envelope at its
//! two versions among them.
//!
//! **Declared here rather than taken from a shared profile crate.** Each
//! producer declares its own vocabulary over the same core — `onevcs` owns
//! [`Phase`] and the word `vcs`, `oneagentgraph` owns the word `agentgraph` —
//! and `onepipeline` is the one crate that links both, so the closed set of all
//! three sources is only nameable here. A sibling's envelope crosses into this
//! one at its JSON form (`src/vcs.rs` and `src/agentgraph.rs` do the crossing),
//! and `tests/contract.rs` holds the shapes to one another so a producer that
//! moved is a refusal here rather than a field silently dropped.
//!
//! **The bytes do not move.** Every type here serializes to what the stack has
//! always written: `tests/recorded/bus/` holds a stream from each producer,
//! copied byte for byte from `onemessagebus` at tag
//! `onemessagebus-agent-v0.8.0`, and `tests/recorded.rs` reads every line back
//! through these types and re-serializes it with no byte changed.

use std::fmt;

use onemessagebus::{Message, Registry, Reserved, SchemaId, Vocabulary};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use onemessagebus::ArtifactRef;
/// The four phases of a change's life, declared by the producer that classifies
/// its own kinds into them.
///
/// `onevcs` stamps `phase` on every event it writes and classifies its kinds
/// into these four; this crate stamps it on nothing it writes and relays it as
/// that producer wrote it. So the type is that producer's, re-exported, rather
/// than a second enum here that the first could come apart from.
pub use onevcs::Phase;

/// The agent stack's vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Agent;

/// The reserved label keys, in wire order.
pub const RESERVED_LABELS: &[Reserved] = &[
    Reserved::text("run_id"),
    Reserved::integer("round"),
    Reserved::text("node"),
    Reserved::text("step"),
    Reserved::text("member"),
    Reserved::text("persona"),
];

/// The reserved top-level dimensions: `phase` alone.
pub const DIMENSIONS: &[Reserved] = &[Reserved::word("phase")];

impl Vocabulary for Agent {
    type Source = Source;
    type Dimensions = Dimensions;
    type Labels = Labels;
    type Fields = MatchFields;

    const NAME: &'static str = "agent";
    const RESERVED: &'static [Reserved] = RESERVED_LABELS;
    const DIMENSIONS: &'static [Reserved] = DIMENSIONS;
    const DEFAULT_SOURCE: &'static str = "pipeline";

    /// The envelope version each producer writes against: `pipeline` moved to
    /// 2 when its journal's record shapes did; `agentgraph` and `vcs` write 1.
    /// A relayed envelope keeps its producer's number.
    fn write_version(source: &Self::Source) -> u32 {
        match source {
            Source::Agentgraph | Source::Vcs => 1,
            Source::Pipeline => 2,
        }
    }
}

/// One event of the agent stack: the core's envelope over the agent vocabulary.
pub type Envelope = onemessagebus::Envelope<Agent>;

/// Which envelopes pass: the core's filter over the agent vocabulary.
pub type EventFilter = onemessagebus::Filter<Agent>;

/// One matcher of an [`EventFilter`]: `source`, `kind`, and the
/// [`MatchFields`] — `phase` and the five reserved labels a matcher may name.
pub type Matcher = onemessagebus::Matcher<Agent>;

/// The core's emitter over the agent vocabulary.
pub type Emitter = onemessagebus::Emitter<Agent>;

/// The core's reader over the agent vocabulary.
pub type Reader = onemessagebus::Reader<Agent>;

/// The core's merge over the agent vocabulary.
pub type Merge = onemessagebus::Merge<Agent>;

// Closed, unlike each producer's own open source: this crate is the one that
// links every producer, so the whole set is nameable here. A word outside it is
// a stream this crate has no reader for, which is why a sibling envelope
// carrying one is refused at the crossing and reported rather than dropped.
//
// The doc comment below is deliberately the one `onemessagebus-agent` 0.8.0
// carried, word for word: `schemars` writes it into `agent.event-envelope@{1,2}`
// and `agent.event-filter@1` as the `Source` definition's `description`, and
// `tests/registry.rs` holds every document this crate registers to a capture of
// what that release registered. The note above is a comment for that reason —
// prose that belongs to the reader of this file rather than to the published
// schema.
/// The library that produced an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// `oneagentgraph`.
    Agentgraph,
    /// `onevcs`.
    Vcs,
    /// `onepipeline`.
    Pipeline,
}

impl Source {
    /// Every source, in the order a refusal lists them.
    #[must_use]
    pub const fn every() -> [Source; 3] {
        [Source::Agentgraph, Source::Vcs, Source::Pipeline]
    }

    /// The word this source travels as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Agentgraph => "agentgraph",
            Source::Vcs => "vcs",
            Source::Pipeline => "pipeline",
        }
    }

    /// The envelope version a producer of this source writes against.
    #[must_use]
    pub fn write_version(self) -> u32 {
        Agent::write_version(&self)
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The agent envelope's reserved top-level dimensions: `phase`, optional and
/// omitted from the wire when absent, exactly as this crate relays it and
/// `onevcs` stamps it.
///
/// Carried between `kind` and `labels` on the wire. Refuses any other
/// top-level key, which is what makes an agent envelope reject an unknown
/// field by name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Dimensions {
    /// Which part of a change's life the event belongs to, as its producer
    /// classified it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
}

impl Dimensions {
    /// No phase.
    #[must_use]
    pub const fn none() -> Self {
        Self { phase: None }
    }

    /// At `phase`.
    #[must_use]
    pub const fn at(phase: Phase) -> Self {
        Self { phase: Some(phase) }
    }
}

impl From<Phase> for Dimensions {
    fn from(phase: Phase) -> Self {
        Self::at(phase)
    }
}

impl From<Option<Phase>> for Dimensions {
    fn from(phase: Option<Phase>) -> Self {
        Self { phase }
    }
}

/// The reserved label keys, plus whatever else a producer stamped.
///
/// Reserved keys are absent rather than empty when unknown, so an enricher can
/// tell "not stamped" from "stamped empty". The extras are flattened beside
/// them, in the order they were stamped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Labels {
    /// The run this event belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The round within the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u64>,
    /// The graph node being executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// The step within a node that runs several in sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// Which member of a conversation produced the event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// The persona that member is running under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    /// Free-form extras beyond the reserved keys above, carried untouched.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Message for Labels {
    const SCHEMA: SchemaId = SchemaId::literal("agent", "labels", 1);
}

/// What an agent matcher may name beside `source` and `kind`: `phase` and the
/// reserved labels, each by exact equality against what the envelope carries.
/// `round` is deliberately not among them, as the grammar says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MatchFields {
    /// The phase the envelope was stamped at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
    /// The `run_id` label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The `node` label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// The `step` label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// The `member` label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// The `persona` label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
}

/// What the agent envelope carries that the core's field names do not spell:
/// the phase, by name.
pub trait AgentEnvelope {
    /// The phase the envelope was stamped at, if any.
    fn phase(&self) -> Option<Phase>;
}

impl AgentEnvelope for Envelope {
    fn phase(&self) -> Option<Phase> {
        self.dimensions.phase
    }
}

/// [`EventFilter::allows`] spelled in the agent vocabulary's own terms.
pub trait AgentFilter {
    /// Whether an envelope of `source`, `kind`, `labels` and `phase` passes.
    fn admits(&self, source: Source, kind: &str, labels: &Labels, phase: Option<Phase>) -> bool;
}

impl AgentFilter for EventFilter {
    fn admits(&self, source: Source, kind: &str, labels: &Labels, phase: Option<Phase>) -> bool {
        self.allows(&source, kind, &Dimensions { phase }, labels)
    }
}

/// The event envelope's family: `agent.event-envelope`.
pub const EVENT_ENVELOPE_FAMILY: &str = "agent.event-envelope";

/// The event envelope versions this build reads, newest first.
pub const EVENT_ENVELOPE_READS: &[u32] = &[2, 1];

/// `agent.artifact-ref@1`: the core's [`ArtifactRef`], registered under this id
/// by this crate.
pub const ARTIFACT_REF: SchemaId = SchemaId::literal("agent", "artifact-ref", 1);

/// `agent.event-filter@1`: the agent [`EventFilter`].
pub const EVENT_FILTER: SchemaId = SchemaId::literal("agent", "event-filter", 1);

/// The event envelope's schema at `version`: the generated document with `v`
/// pinned to that version, so a payload checks against the version it declares.
#[must_use]
pub fn event_envelope_schema(version: u32) -> Value {
    let mut schema = schemars::schema_for!(Envelope).to_value();
    if let Some(v) = schema.pointer_mut("/properties/v") {
        if let Some(object) = v.as_object_mut() {
            object.insert("const".to_owned(), Value::from(version));
        }
    }
    schema
}

/// The five ids this crate owns of the event vocabulary, registered into
/// `registry`: the envelope at each version it reads, the artifact reference,
/// the filter and the labels.
///
/// These are exactly the ids [`EVENTS_BUNDLE_PATH`] publishes, and both readers
/// take them from here, so what a record validates against and what a host's
/// bus links are one set generated from one set of types.
///
/// # Errors
///
/// Never for this build's own documents; each is generated from its type and
/// each id is distinct. Returned rather than panicked so a caller composing
/// several vocabularies reports the collision it made.
pub fn register_events(registry: &mut Registry) -> Result<(), onemessagebus::RegistryError> {
    for version in EVENT_ENVELOPE_READS {
        registry.register_schema(
            SchemaId::literal("agent", "event-envelope", *version),
            event_envelope_schema(*version),
        )?;
    }
    registry.register_schema(ARTIFACT_REF, schemars::schema_for!(ArtifactRef).to_value())?;
    registry.register_schema(EVENT_FILTER, schemars::schema_for!(EventFilter).to_value())?;
    registry.register::<Labels>()?;
    Ok(())
}

/// The agent stack's registry: every schema the stack registers that is not one
/// crate's own payload, at every version this build reads.
///
/// Three owners, named here because this is the crate that links all three —
/// which is the whole of what changed when the shared profile crate went. The
/// event families are [`register_events`]'s, above. `agent.note@1` is the note
/// contract's message, and its type is **onejudge's**
/// ([`onejudge::note::Note`]), registered from there rather than restated. The
/// transport plugin protocol's three shapes are the core's, registered under its
/// own `onemessagebus` namespace so a client in another language validates
/// against them.
///
/// Both registries this crate builds — [`crate::payload::registry`] and
/// [`crate::channel::layout::registry`] — start from this, so they list the same
/// ids with the same documents as the profile crate's `registry()` did at
/// `onemessagebus-agent` 0.8.0. `tests/registry.rs` holds them to a capture of
/// exactly that.
///
/// # Panics
///
/// Never for this build's own documents; each is generated from its type.
#[must_use]
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    register_events(&mut registry).expect("the event vocabulary's schemas register");
    registry
        .register::<onejudge::note::Note>()
        .expect("the note schema registers");
    onemessagebus::transport::register_protocol(&mut registry)
        .expect("the transport plugin protocol's schemas register");
    registry
}

/// Where the published event bundle is committed, relative to the repository
/// root: what a host's configuration links at
/// `https://raw.githubusercontent.com/nickderobertis/onepipeline/v<version>/<this>`.
///
/// The event vocabulary is this crate's, so the stack's own schemas for it are
/// published from here the way `schemas/planner-channel.json` publishes the
/// channel's layout. A reader in another language validates an envelope, a
/// filter, an artifact reference or a label map against these documents rather
/// than against a second reading of this module.
pub const EVENTS_BUNDLE_PATH: &str = "schemas/events.json";

/// The version the published event bundle declares — what a link's pin is held
/// against. Raised with any change to what the bundle says: its major where a
/// stream one version writes is one the other cannot read.
pub const EVENTS_BUNDLE_VERSION: &str = "1.0.0";

/// The five ids [`EVENTS_BUNDLE_PATH`] publishes, in the order the bundle lists
/// them: the envelope at each version this build reads, the filter, the
/// artifact reference and the labels.
#[must_use]
pub fn events_bundle_ids() -> Vec<SchemaId> {
    let mut ids: Vec<SchemaId> = EVENT_ENVELOPE_READS
        .iter()
        .rev()
        .map(|version| SchemaId::literal("agent", "event-envelope", *version))
        .collect();
    ids.push(EVENT_FILTER);
    ids.push(ARTIFACT_REF);
    ids.push(SchemaId::literal("agent", "labels", 1));
    ids
}

/// The schema bundle published at [`EVENTS_BUNDLE_PATH`]: the documents
/// [`register_events`] generates, at [`EVENTS_BUNDLE_VERSION`].
///
/// # Panics
///
/// Never for this build's own documents: each is generated from its type, and
/// each id appears once.
#[must_use]
pub fn events_bundle() -> onemessagebus::SchemaBundle {
    let registry = {
        let mut registry = Registry::new();
        register_events(&mut registry).expect("the event vocabulary's schemas register");
        registry
    };
    let schemas = events_bundle_ids()
        .into_iter()
        .map(|id| onemessagebus::sdk_schema::RegistryDocument {
            schema: registry
                .schema(&id)
                .cloned()
                .unwrap_or_else(|| unreachable!("the event vocabulary registers {id}")),
            id,
        })
        .collect();
    onemessagebus::SchemaBundle::new(
        EVENTS_BUNDLE_VERSION
            .parse::<onemessagebus::BundleVersion>()
            .unwrap_or_else(|why| unreachable!("{why}")),
        Some(
            "The agent stack's event envelope, filter, artifact reference and labels, as \
             onepipeline declares them."
                .to_owned(),
        ),
        schemas,
    )
    .unwrap_or_else(|why| panic!("the events bundle is not a schema bundle: {why}"))
}

/// [`events_bundle`] as the text committed at [`EVENTS_BUNDLE_PATH`]:
/// pretty-printed, with a trailing newline.
///
/// # Panics
///
/// Never: a bundle serializes.
#[must_use]
pub fn events_bundle_json() -> String {
    let mut text = serde_json::to_string_pretty(&events_bundle()).expect("a bundle serializes");
    text.push('\n');
    text
}
