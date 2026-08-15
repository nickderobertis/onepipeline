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

/// Every profile this build ships, in the order the contract lists them.
pub const SHIPPED_PROFILES: &[&str] = &[DEFAULT_PROFILE, MONITOR_PROFILE];

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

impl EventFilter {
    /// Read a filter from the text of a spec: JSON, or the YAML the grammar is
    /// written in, of which JSON is a subset.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] carrying the refusal, which names the matcher it is
    /// about — which list, and which position in it — because that is what an
    /// operator has to find in what they wrote.
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
    /// [`Error::Invalid`] for a file that cannot be read, a document that is not
    /// a filter, or a filter that could not be honoured — see
    /// [`validate`](Self::validate).
    pub fn read(spec: &str) -> Result<Self> {
        let filter = if spec.trim_start().starts_with('{') {
            Self::parse(spec)?
        } else {
            let document = std::fs::read_to_string(Path::new(spec)).map_err(|failure| {
                Error::Invalid(format!("cannot read the event filter {spec}: {failure}"))
            })?;
            Self::parse(&document)?
        };
        filter.validate()?;
        Ok(filter)
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
    Ok(EventFilter {
        include: matchers(object.get("include"), "include")?,
        exclude: matchers(object.get("exclude"), "exclude")?,
    })
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
