//! A source's `routes`: where an item written to it goes instead, chosen by the item's
//! repositories.
//!
//! Engine-owned rather than plugin-owned: `routes` sits beside `plugin` and `config` in a
//! source's entry, and a plugin's own block never sees it. Every writer reaches the store,
//! so the rule is held here once rather than restated, and drifting, in each of them.
//!
//! An entry is one of three kinds. A **repository** entry, `{repositories, to}`, matches an
//! item when **every** one of the item's repositories matches at least one of its patterns;
//! an item with no repositories matches none. A **classification** entry,
//! `{classification, to}`, matches every item of that classification. A **combined** entry,
//! `{repositories, classification, to}`, matches an item of that classification every one of
//! whose repositories matches one of its patterns — and, like a repository entry, never an
//! item naming none.
//!
//! **Safety is decided before matching.** An item that may only be written somewhere
//! private is never placed in a source not declared private: for it, only entries sending
//! to a private source, or to one no layer gives a plugin, are tried. When none places it,
//! it stays in the source itself — and when that source is not private, the write is
//! refused there rather than sent anywhere less safe.
//!
//! **The kind decides the order, wherever an entry is written.** Among the entries an item
//! may try, combined entries are tried first, then classification entries, then repository
//! entries, each kind in the order written, and no match leaves the item in the source
//! itself. A placement names the entry by its index as written.

use std::collections::{BTreeMap, BTreeSet};

use onetaskgraph_plugin_api::{Classification, Repository, SourceName};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ConfigError, SourceVisibility};

/// One pattern over a normalized repository origin, `host/owner/name`, where a `*` segment
/// matches exactly one whole segment of the origin.
///
/// Kept as it was written, because that is how `config show` and every refusal spell it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryPattern(String);

impl RepositoryPattern {
    /// One pattern, once it is established it is one.
    ///
    /// # Errors
    ///
    /// Returns why when the pattern is not itself spelled as a normalized origin — the
    /// shape [`Repository`] holds every origin to, so a pattern no origin could match is
    /// refused by the one rule that defines an origin rather than by a restatement of it —
    /// or when a segment holds a `*` beside other characters.
    pub fn new(pattern: impl Into<String>) -> Result<Self, String> {
        let pattern = pattern.into();
        let valid = Repository::try_from(pattern.clone()).is_ok()
            && pattern
                .split('/')
                .all(|segment| segment == "*" || !segment.contains('*'));
        valid.then_some(Self(pattern.clone())).ok_or_else(|| {
            format!(
                "{pattern:?} is not a repository pattern: a pattern is host/owner/name, each \
                 segment either a literal or a lone `*` matching one whole segment, with no \
                 scheme and no .git suffix"
            )
        })
    }

    /// Whether `origin` is one this pattern names.
    #[must_use]
    pub fn matches(&self, origin: &Repository) -> bool {
        let pattern: Vec<&str> = self.0.split('/').collect();
        let origin: Vec<&str> = origin.as_str().split('/').collect();
        pattern.len() == origin.len()
            && pattern
                .iter()
                .zip(&origin)
                .all(|(pattern, origin)| *pattern == "*" || pattern == origin)
    }

    /// The pattern as it was written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One entry of a source's `routes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    purpose: Option<RoutePurpose>,
    matcher: Matcher,
    to: SourceName,
}

/// An operation's routing identity. Producers name their own purpose; legacy writers
/// leave it absent. A purpose is a nonempty lowercase identifier separated by hyphens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct RoutePurpose(String);

impl RoutePurpose {
    /// Validate a producer's routing identity.
    ///
    /// # Errors
    ///
    /// Refuses empty identifiers and segments containing anything but lowercase ASCII letters.
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_lowercase()))
        {
            Ok(Self(value))
        } else {
            Err("a route purpose is lowercase words separated by hyphens".to_owned())
        }
    }

    /// The producer's identifier as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RoutePurpose {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RoutePurpose> for String {
    fn from(value: RoutePurpose) -> Self {
        value.0
    }
}

/// What one entry matches on: its repositories, its classification, or both together, so
/// an entry naming neither cannot be held.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Matcher {
    /// Items every one of whose repositories one of these patterns matches. Never empty.
    Repositories(Vec<RepositoryPattern>),
    /// Every item of this classification.
    Classification(Classification),
    /// Items of this classification every one of whose repositories one of these patterns
    /// matches. Never empty.
    Combined(Classification, Vec<RepositoryPattern>),
}

impl Route {
    /// The operation this entry serves, or the legacy unscoped operation.
    #[must_use]
    pub fn purpose(&self) -> Option<&RoutePurpose> {
        self.purpose.as_ref()
    }

    /// The patterns, in the order written — empty for a classification entry.
    #[must_use]
    pub fn repositories(&self) -> &[RepositoryPattern] {
        match &self.matcher {
            Matcher::Repositories(patterns) | Matcher::Combined(_, patterns) => patterns,
            Matcher::Classification(_) => &[],
        }
    }

    /// The classification a classification or combined entry matches, or `None` for a
    /// repository entry.
    #[must_use]
    pub fn classification(&self) -> Option<Classification> {
        match &self.matcher {
            Matcher::Repositories(_) => None,
            Matcher::Classification(classification) | Matcher::Combined(classification, _) => {
                Some(*classification)
            }
        }
    }

    /// The configured source an item this entry matches goes to.
    #[must_use]
    pub fn to(&self) -> &SourceName {
        &self.to
    }

    /// Whether every one of `repositories` matches one of this entry's patterns — `false`
    /// for an item naming none, and for a classification entry. A combined entry asks this of
    /// its patterns alone; whether its classification is the item's is
    /// [`Routes::place_classified`]'s to decide.
    #[must_use]
    pub fn matches(&self, repositories: &[Repository]) -> bool {
        let patterns = self.repositories();
        !patterns.is_empty()
            && !repositories.is_empty()
            && repositories
                .iter()
                .all(|origin| patterns.iter().any(|pattern| pattern.matches(origin)))
    }

    /// Whether this entry places an item of `classification` with `repositories`.
    fn places(&self, classification: Classification, repositories: &[Repository]) -> bool {
        match &self.matcher {
            Matcher::Repositories(_) => self.matches(repositories),
            Matcher::Classification(matched) => *matched == classification,
            Matcher::Combined(matched, _) => {
                *matched == classification && self.matches(repositories)
            }
        }
    }

    /// The tier this entry is tried in: combined, then classification, then repository.
    fn tier(&self) -> u8 {
        match &self.matcher {
            Matcher::Combined(..) => 0,
            Matcher::Classification(_) => 1,
            Matcher::Repositories(_) => 2,
        }
    }
}

/// Where one item written to a source lands, and which of its routes put it there.
///
/// `route` is always written, `null` included: a reader of a routed copy's report is told
/// that no entry matched rather than left to infer it from an absent key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Placement {
    /// The source the item lands in: the routed one, or the source itself.
    pub destination: SourceName,
    /// The index of the route entry that matched, or `None` when none did and the item
    /// stays in the source itself.
    pub route: Option<u32>,
}

/// What `sources route` answers: where an item with the repositories asked about, written
/// to `source`, would land — read from configuration alone, never from a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceRoute {
    /// The source the item would be written to.
    pub source: SourceName,
    /// Where it would land, and the route entry that would send it there.
    #[serde(flatten)]
    pub placement: Placement,
    /// Every source an item written to `source` could land in, whatever its repositories and
    /// classification: `source` itself first, then each source its routes name as `to`, in
    /// the order they are written, each once — the set a routed copy reads ahead from.
    pub reachable: Vec<SourceName>,
}

/// Every configured source's routes, by source name, and who each source is declared
/// readable by. A source with no routes has no entry in the first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Routes(
    BTreeMap<SourceName, Vec<Route>>,
    BTreeMap<SourceName, SourceVisibility>,
    /// Sources a route may name that no layer gives a `plugin`: placed to like any other
    /// destination, so the command following the route refuses it rather than skipping it.
    BTreeSet<SourceName>,
);

impl Routes {
    /// Where a public item with `repositories` written to `source` lands.
    #[must_use]
    pub fn place(&self, source: &SourceName, repositories: &[Repository]) -> Placement {
        self.place_classified(source, Classification::Public, repositories)
    }

    /// Where an item of `classification` with `repositories`, written to `source`, lands —
    /// by the module's rule: safety first, then combined entries, then classification
    /// entries, then repository entries, each in the order written, then the source itself.
    #[must_use]
    pub fn place_classified(
        &self,
        source: &SourceName,
        classification: Classification,
        repositories: &[Repository],
    ) -> Placement {
        self.place_for(source, classification, repositories, None)
    }

    /// Place an item using only entries of the requested purpose. Visibility restrictions
    /// and matcher precedence are identical to legacy routing; a scoped entry never
    /// changes an unrelated writer's destination.
    #[must_use]
    pub fn place_for(
        &self,
        source: &SourceName,
        classification: Classification,
        repositories: &[Repository],
        purpose: Option<&RoutePurpose>,
    ) -> Placement {
        let entries: Vec<(usize, &Route)> = self
            .0
            .get(source)
            .map(|routes| {
                routes
                    .iter()
                    .enumerate()
                    .filter(|(_, route)| {
                        route.purpose.as_ref() == purpose
                            && (classification.is_public()
                                || self.is_private(&route.to)
                                || self.2.contains(&route.to))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let chosen = entries
            .iter()
            .filter(|(_, route)| route.places(classification, repositories))
            .min_by_key(|(index, route)| (route.tier(), *index));
        chosen.map_or_else(
            || Placement {
                destination: source.clone(),
                route: None,
            },
            |(index, route)| Placement {
                destination: route.to.clone(),
                route: Some(u32::try_from(*index).unwrap_or(u32::MAX)),
            },
        )
    }

    /// Whether `source` holds a classification entry a private item may take: one that sends
    /// every private item it is written to somewhere declared private, whatever its
    /// repositories. A combined entry never answers this: it is asked with no repositories,
    /// which no combined entry matches.
    #[must_use]
    pub fn classifies_private(&self, source: &SourceName) -> bool {
        self.place_classified(source, Classification::Private, &[])
            .route
            .is_some()
    }

    /// Whether `source` is declared private.
    #[must_use]
    pub fn is_private(&self, source: &SourceName) -> bool {
        self.1.get(source) == Some(&SourceVisibility::Private)
    }

    /// Who `source` is declared readable by: `unknown` for a source nothing declares.
    #[must_use]
    pub fn visibility(&self, source: &SourceName) -> SourceVisibility {
        self.1.get(source).copied().unwrap_or_default()
    }

    /// The same routes, holding each source to the visibility `declared` gives it.
    #[must_use]
    pub fn with_visibility(mut self, declared: BTreeMap<SourceName, SourceVisibility>) -> Self {
        self.1 = declared;
        self
    }

    /// The same routes, knowing `incomplete` names sources no layer gives a `plugin`.
    #[must_use]
    pub(super) fn with_incomplete(mut self, incomplete: BTreeSet<SourceName>) -> Self {
        self.2 = incomplete;
        self
    }

    /// Whether `source` declares any legacy, unscoped route.
    #[must_use]
    pub fn routes(&self, source: &SourceName) -> bool {
        self.of(source).iter().any(|route| route.purpose.is_none())
    }

    /// Whether any source declares a legacy, unscoped route.
    #[must_use]
    pub fn any(&self) -> bool {
        self.0
            .values()
            .flatten()
            .any(|route| route.purpose.is_none())
    }

    /// Every source an item written to `source` could land in: the source itself first,
    /// then each distinct route target in the order its entries are written.
    #[must_use]
    pub fn reachable(&self, source: &SourceName) -> Vec<SourceName> {
        let mut reachable = vec![source.clone()];
        for route in self
            .of(source)
            .iter()
            .filter(|route| route.purpose.is_none())
        {
            if !reachable.contains(&route.to) {
                reachable.push(route.to.clone());
            }
        }
        reachable
    }

    /// One source's routes, in order.
    #[must_use]
    pub fn of(&self, source: &SourceName) -> &[Route] {
        self.0.get(source).map_or(&[], Vec::as_slice)
    }

    /// Routes built in code, for a caller holding sources it did not resolve from a
    /// configuration document. Checked by the same rules a document's are.
    ///
    /// # Errors
    ///
    /// As [`check`].
    pub fn new(
        routes: BTreeMap<SourceName, Vec<(Vec<String>, SourceName)>>,
        configured: &[SourceName],
    ) -> Result<Self, ConfigError> {
        let mut built = BTreeMap::new();
        for (source, entries) in routes {
            let mut parsed = Vec::new();
            for (index, (patterns, to)) in entries.into_iter().enumerate() {
                let key = entry_key(&source, index);
                parsed.push(Route {
                    purpose: None,
                    matcher: Matcher::Repositories(patterns_of(&key, patterns)?),
                    to,
                });
            }
            built.insert(source, parsed);
        }
        let routes = Self(built, BTreeMap::new(), BTreeSet::new());
        check(&routes, configured)?;
        Ok(routes)
    }

    /// One source's classification entries, built in code beside [`Routes::new`]'s: each
    /// sends every item of its classification to its `to`, tried before any repository
    /// entry. Checked by the same rules, and a `private` entry sending to a source this
    /// value does not hold as private is refused.
    ///
    /// # Errors
    ///
    /// As [`check`] and [`check_classified`].
    pub fn classified(
        mut self,
        source: SourceName,
        entries: Vec<(Classification, SourceName)>,
        configured: &[SourceName],
    ) -> Result<Self, ConfigError> {
        let routes = self.0.entry(source).or_default();
        let mut classified: Vec<Route> = entries
            .into_iter()
            .map(|(classification, to)| Route {
                purpose: None,
                matcher: Matcher::Classification(classification),
                to,
            })
            .collect();
        classified.append(routes);
        *routes = classified;
        check(&self, configured)?;
        check_classified(&self, &self.1)?;
        Ok(self)
    }

    /// One source's combined entries, built in code beside [`Routes::classified`]'s: each
    /// sends an item of its classification every one of whose repositories one of its
    /// patterns matches to its `to`, tried before any classification or repository entry.
    /// Put first among the source's entries, in the order given. Checked by the same rules a
    /// document's are: the patterns as a repository entry's, the `to` by [`check`], and a
    /// `private` entry sending to a source this value does not hold as private is refused.
    ///
    /// # Errors
    ///
    /// As [`check`] and [`check_classified`], and an empty or malformed pattern list as a
    /// repository entry's.
    pub fn combined(
        mut self,
        source: SourceName,
        entries: Vec<(Classification, Vec<String>, SourceName)>,
        configured: &[SourceName],
    ) -> Result<Self, ConfigError> {
        let mut combined = Vec::new();
        for (index, (classification, patterns, to)) in entries.into_iter().enumerate() {
            let key = entry_key(&source, index);
            combined.push(Route {
                purpose: None,
                matcher: Matcher::Combined(classification, patterns_of(&key, patterns)?),
                to,
            });
        }
        let routes = self.0.entry(source).or_default();
        combined.append(routes);
        *routes = combined;
        check(&self, configured)?;
        check_classified(&self, &self.1)?;
        Ok(self)
    }

    pub(super) fn insert(&mut self, source: SourceName, routes: Vec<Route>) {
        if !routes.is_empty() {
            self.0.insert(source, routes);
        }
    }
}

fn entry_key(source: &SourceName, index: usize) -> String {
    format!("sources.{source}.routes.{index}")
}

/// Read one source's `routes` as a document, an environment variable or `--set` spells it.
///
/// A list is the document's spelling. A mapping keyed by each entry's index is what the
/// environment and `--set` produce — `--set sources.plans.routes.0.to=linear` — because
/// neither can spell a list of objects, and both are read the same way. One pattern where
/// a list of them is expected is read as that one-pattern list, for the reason
/// `default_sources` accepts one name.
pub(super) fn parse(source: &SourceName, value: &Value) -> Result<Vec<Route>, ConfigError> {
    let base = format!("sources.{source}.routes");
    let entries: Vec<(usize, &Value)> = match value {
        Value::Null => return Ok(Vec::new()),
        Value::Array(entries) => entries.iter().enumerate().collect(),
        Value::Object(entries) => {
            let mut indexed = Vec::new();
            for (key, entry) in entries {
                let index = key.parse::<usize>().map_err(|_| {
                    ConfigError::setting(
                        format!("{base}.{key}"),
                        "a route is addressed by its index in the list, and this is not one",
                        format!(
                            "write the routes as a list, or address each entry by its index — \
                             `--set {base}.0.to=<source>`."
                        ),
                    )
                })?;
                indexed.push((index, entry));
            }
            indexed.sort_by_key(|(index, _)| *index);
            for (position, (index, _)) in indexed.iter().enumerate() {
                if *index != position {
                    return Err(ConfigError::setting(
                        format!("{base}.{index}"),
                        format!(
                            "the routes are numbered from 0 with no gaps, and entry {position} \
                             is missing"
                        ),
                        format!("set {base}.{position} as well, or renumber the entries."),
                    ));
                }
            }
            indexed
        }
        _ => {
            return Err(ConfigError::setting(
                base,
                "routes is a list of entries, each naming `repositories` and `to`",
                "write it as a list — see the README's section on routes.",
            ));
        }
    };
    entries
        .into_iter()
        .map(|(index, entry)| parse_entry(&entry_key(source, index), entry))
        .collect()
}

/// One entry, refused naming its key when it is not `{repositories, to}`,
/// `{classification, to}` or `{repositories, classification, to}`.
fn parse_entry(key: &str, entry: &Value) -> Result<Route, ConfigError> {
    let Value::Object(fields) = entry else {
        return Err(ConfigError::setting(
            key,
            "a route is a mapping holding `to` and `repositories`, `classification` or both",
            "write the entry as `{repositories: [host/owner/*], to: <source>}`, \
             `{classification: private, to: <source>}` or both matchers together.",
        ));
    };
    if let Some(unknown) = fields.keys().find(|field| {
        !["repositories", "classification", "to", "purpose"].contains(&field.as_str())
    }) {
        return Err(ConfigError::setting(
            format!("{key}.{unknown}"),
            "unknown field; a route holds `to` and `repositories`, `classification` or both, \
             with optional `purpose: human-task`",
            "remove it.",
        ));
    }
    let to = destination_of(key, fields)?;
    let purpose = fields
        .get("purpose")
        .map(|value| {
            serde_json::from_value::<RoutePurpose>(value.clone()).map_err(|error| {
                ConfigError::setting(
                    format!("{key}.purpose"),
                    error.to_string(),
                    "write `human-task` or omit purpose for legacy routing.",
                )
            })
        })
        .transpose()?;
    let classification = fields
        .get("classification")
        .map(|classification| {
            classification
                .as_str()
                .and_then(|spelled| spelled.parse::<Classification>().ok())
                .ok_or_else(|| {
                    ConfigError::setting(
                        format!("{key}.classification"),
                        format!("{classification} is not a classification"),
                        "write `public` or `private`.",
                    )
                })
        })
        .transpose()?;
    if let (Some(classification), false) = (classification, fields.contains_key("repositories")) {
        return Ok(Route {
            purpose,
            matcher: Matcher::Classification(classification),
            to,
        });
    }
    let patterns = match fields.get("repositories") {
        None => {
            return Err(ConfigError::setting(
                format!("{key}.repositories"),
                "a route names the repositories it matches, and this one names none",
                "list at least one pattern, such as `github.com/example-org/*`.",
            ));
        }
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Array(many)) => many
            .iter()
            .map(|pattern| {
                pattern.as_str().map(str::to_owned).ok_or_else(|| {
                    ConfigError::setting(
                        format!("{key}.repositories"),
                        format!("{pattern} is not a pattern; each is a string"),
                        "write each pattern as host/owner/name, quoted if need be.",
                    )
                })
            })
            .collect::<Result<_, _>>()?,
        Some(other) => {
            return Err(ConfigError::setting(
                format!("{key}.repositories"),
                format!("{other} is not a list of patterns"),
                "write it as a list of host/owner/name patterns.",
            ));
        }
    };
    let patterns = patterns_of(key, patterns)?;
    Ok(Route {
        purpose,
        matcher: match classification {
            Some(classification) => Matcher::Combined(classification, patterns),
            None => Matcher::Repositories(patterns),
        },
        to,
    })
}

/// The `to` of one entry, refused naming it when it is absent or not a source name.
fn destination_of(
    key: &str,
    fields: &serde_json::Map<String, Value>,
) -> Result<SourceName, ConfigError> {
    match fields.get("to") {
        Some(Value::String(to)) => SourceName::new(to.clone()).map_err(|error| {
            ConfigError::setting(
                format!("{key}.to"),
                error.to_string(),
                "name a configured source; `onetaskgraph config show` lists them.",
            )
        }),
        _ => Err(ConfigError::setting(
            format!("{key}.to"),
            "a route names the configured source it sends an item to, and this one names \
             none",
            "set `to` to a configured source's name.",
        )),
    }
}

/// The patterns of one entry, refused naming it when there are none or one is malformed.
fn patterns_of(key: &str, patterns: Vec<String>) -> Result<Vec<RepositoryPattern>, ConfigError> {
    if patterns.is_empty() {
        return Err(ConfigError::setting(
            format!("{key}.repositories"),
            "a route names the repositories it matches, and this one's list is empty — an \
             entry matching nothing is a mistake rather than a rule",
            "list at least one pattern, such as `github.com/example-org/*`, or remove the entry.",
        ));
    }
    patterns
        .into_iter()
        .map(|pattern| {
            RepositoryPattern::new(pattern).map_err(|problem| {
                ConfigError::setting(
                    format!("{key}.repositories"),
                    problem,
                    "write the pattern as host/owner/name, using `*` for one whole segment — \
                     `github.com/example-org/*`.",
                )
            })
        })
        .collect()
}

/// Refuse an entry whose `to` names no configured source, the source itself, or a source
/// that routes on its own.
///
/// No chains: a route's target places what it is given where it is, so where an item lands
/// is always one lookup away from where it was sent, and no cycle can be written down.
///
/// # Errors
///
/// [`ConfigError::Setting`] naming the source and the entry.
pub(super) fn check(routes: &Routes, configured: &[SourceName]) -> Result<(), ConfigError> {
    for (source, entries) in &routes.0 {
        for (index, route) in entries.iter().enumerate() {
            let key = format!("{}.to", entry_key(source, index));
            if !configured.contains(&route.to) {
                return Err(ConfigError::setting(
                    key,
                    format!(
                        "route {index} of source {source} sends to {:?}, which no source is \
                         configured as",
                        route.to.as_str()
                    ),
                    format!(
                        "name one of the configured sources ({}), or configure {:?}.",
                        configured
                            .iter()
                            .map(SourceName::as_str)
                            .collect::<Vec<_>>()
                            .join(", "),
                        route.to.as_str()
                    ),
                ));
            }
            if &route.to == source {
                return Err(ConfigError::setting(
                    key,
                    format!(
                        "route {index} of source {source} sends to {source} itself, which is \
                         where an item no route matches already stays"
                    ),
                    "name another source, or remove the entry.",
                ));
            }
            if !routes.of(&route.to).is_empty() {
                return Err(ConfigError::setting(
                    key,
                    format!(
                        "route {index} of source {source} sends to {}, which has routes of its \
                         own; a route never chains",
                        route.to
                    ),
                    format!(
                        "send to the source {} would route to directly, or remove {}'s routes.",
                        route.to, route.to
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Refuse a `classification: private` entry — classification or combined — whose `to` is not
/// declared private.
///
/// Such an entry could never be taken — a private item is placed only where it may be
/// written — so writing one down is a mistake about where private work goes, refused when
/// the configuration is read rather than discovered at a write.
///
/// # Errors
///
/// [`ConfigError::Setting`] naming the source and the entry.
pub(super) fn check_classified(
    routes: &Routes,
    declared: &BTreeMap<SourceName, SourceVisibility>,
) -> Result<(), ConfigError> {
    for (source, entries) in &routes.0 {
        for (index, route) in entries.iter().enumerate() {
            // An incomplete destination's visibility is declared, if anywhere, in a layer this
            // merge did not read; the command that would follow the entry refuses it instead.
            if route.classification() == Some(Classification::Private)
                && !routes.2.contains(&route.to)
                && declared.get(&route.to) != Some(&SourceVisibility::Private)
            {
                return Err(ConfigError::setting(
                    format!("{}.to", entry_key(source, index)),
                    format!(
                        "route {index} of source {source} sends private items to {}, which is \
                         not declared private",
                        route.to
                    ),
                    format!(
                        "declare `visibility: private` on {} if its backend is private, or \
                         send private items to a source that is.",
                        route.to
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(text: &str) -> Repository {
        Repository::try_from(text.to_owned()).expect("an origin")
    }

    #[test]
    fn a_star_matches_exactly_one_whole_segment() {
        let pattern = RepositoryPattern::new("github.com/example-org/*").expect("a pattern");
        assert!(pattern.matches(&origin("github.com/example-org/api")));
        assert!(!pattern.matches(&origin("github.com/example-org/api/sub")));
        assert!(!pattern.matches(&origin("github.com/example-orgx/api")));
        assert!(!pattern.matches(&origin("gitlab.com/example-org/api")));
    }

    #[test]
    fn a_malformed_pattern_is_refused() {
        for bad in [
            "github.com/example-org",
            "github.com/example*/api",
            "https://github.com/a/b",
            "github.com//b",
            "github.com/a/b.git",
        ] {
            assert!(RepositoryPattern::new(bad).is_err(), "{bad} is refused");
        }
    }

    #[test]
    fn an_entry_matches_only_when_every_repository_does() {
        let route = Route {
            purpose: None,
            matcher: Matcher::Repositories(vec![
                RepositoryPattern::new("github.com/example-org/*").unwrap(),
            ]),
            to: SourceName::new("linear").unwrap(),
        };
        assert!(route.matches(&[origin("github.com/example-org/a")]));
        assert!(!route.matches(&[
            origin("github.com/example-org/a"),
            origin("github.com/nickderobertis/b")
        ]));
        assert!(!route.matches(&[]));
    }

    fn name(text: &str) -> SourceName {
        SourceName::new(text.to_owned()).expect("a name")
    }

    /// `plans` routes private items to `vault`, the entry `classified` puts first, then
    /// widgetco work to `team`; `vault` is the one source declared private.
    fn classified() -> Routes {
        let configured = [name("plans"), name("team"), name("vault")];
        Routes::new(
            BTreeMap::from([(
                name("plans"),
                vec![(vec!["github.com/widgetco/*".to_owned()], name("team"))],
            )]),
            &configured,
        )
        .expect("repository routes")
        .with_visibility(BTreeMap::from([(name("vault"), SourceVisibility::Private)]))
        .classified(
            name("plans"),
            vec![(Classification::Private, name("vault"))],
            &configured,
        )
        .expect("a classification route")
    }

    #[test]
    fn safety_is_decided_before_any_repository_entry_matches() {
        let routes = classified();
        // A private widgetco item never follows the repository entry to a source not declared
        // private: the classification entry places it. The `routes` journeys in
        // `crates/onetaskgraph-e2e/tests/e2e/boundary.rs` hold it to that with the repository
        // entry written first.
        assert_eq!(
            routes.place_classified(
                &name("plans"),
                Classification::Private,
                &[origin("github.com/widgetco/api")]
            ),
            Placement {
                destination: name("vault"),
                route: Some(0)
            }
        );
        // A public one keeps the repository placement it always had.
        assert_eq!(
            routes.place_classified(
                &name("plans"),
                Classification::Public,
                &[origin("github.com/widgetco/api")]
            ),
            Placement {
                destination: name("team"),
                route: Some(1)
            }
        );
        assert_eq!(
            routes.place(&name("plans"), &[]),
            Placement {
                destination: name("plans"),
                route: None
            }
        );
    }

    #[test]
    fn a_private_item_with_no_private_destination_stays_where_it_was_sent() {
        let routes = classified();
        // `team` routes nothing and is not private: the item stays, and is refused there.
        assert_eq!(
            routes
                .place_classified(&name("team"), Classification::Private, &[])
                .destination,
            name("team")
        );
    }

    #[test]
    fn only_a_classification_entry_places_every_private_item_whatever_its_repositories() {
        let configured = [name("plans"), name("vault")];
        let by_repository = Routes::new(
            BTreeMap::from([(
                name("plans"),
                vec![(vec!["github.com/hiddenco/*".to_owned()], name("vault"))],
            )]),
            &configured,
        )
        .expect("repository routes")
        .with_visibility(BTreeMap::from([(name("vault"), SourceVisibility::Private)]));
        assert!(!by_repository.classifies_private(&name("plans")));
        assert!(classified().classifies_private(&name("plans")));
        assert!(!classified().classifies_private(&name("team")));
    }

    #[test]
    fn a_private_classification_entry_naming_a_source_not_declared_private_is_refused() {
        let configured = [name("plans"), name("team")];
        let refused = Routes::default()
            .classified(
                name("plans"),
                vec![(Classification::Private, name("team"))],
                &configured,
            )
            .expect_err("team is not declared private");
        assert!(
            refused.to_string().contains("not declared private"),
            "{refused}"
        );
    }

    /// The routes a configuration document builds for `site`, written as `routes`, beside
    /// `public` and the private `vault` and `special` — every source in memory.
    fn configured(routes: Value) -> Routes {
        crate::Config::from_document(serde_json::json!({"sources": {
            "site": {"plugin": "in-memory", "routes": routes},
            "public": {"plugin": "in-memory"},
            "team": {"plugin": "in-memory"},
            "vault": {"plugin": "in-memory", "visibility": "private"},
            "special": {"plugin": "in-memory", "visibility": "private"},
        }}))
        .expect("the configuration loads")
        .routes()
    }

    /// The configuration's own refusal of `routes` on `site`, as text.
    fn refused(routes: Value) -> String {
        crate::Config::from_document(serde_json::json!({"sources": {
            "site": {"plugin": "in-memory", "routes": routes},
            "public": {"plugin": "in-memory"},
            "vault": {"plugin": "in-memory", "visibility": "private"},
            "chained": {"plugin": "in-memory", "visibility": "private",
                        "routes": [{"classification": "private", "to": "vault"}]},
        }}))
        .expect_err("the configuration is refused")
        .to_string()
    }

    fn placed(destination: &str, route: u32) -> Placement {
        Placement {
            destination: name(destination),
            route: Some(route),
        }
    }

    fn stays(source: &str) -> Placement {
        Placement {
            destination: name(source),
            route: None,
        }
    }

    /// The README's example: one private repository to its own source, the rest of the
    /// owner's private work to another, every other private item to `vault`, and a team's
    /// work wherever it is — written with the repository entry first and the combined ones
    /// last, so only the kind can be what orders them.
    fn example() -> Routes {
        configured(serde_json::json!([
            {"repositories": ["github.com/example-org/*"], "to": "public"},
            {"classification": "private", "to": "vault"},
            {"repositories": ["github.com/example-org/special"], "classification": "private",
             "to": "special"},
            {"repositories": ["github.com/example-org/*"], "classification": "private",
             "to": "vault"},
        ]))
    }

    #[test]
    fn a_combined_entry_matches_only_its_classification_with_every_repository_matching() {
        let routes = configured(serde_json::json!([
            {"repositories": ["github.com/example-org/*"], "classification": "private",
             "to": "vault"},
        ]));
        let site = name("site");
        let private = Classification::Private;
        assert_eq!(
            routes.place_classified(&site, private, &[origin("github.com/example-org/api")]),
            placed("vault", 0)
        );
        // Not its classification.
        assert_eq!(
            routes.place_classified(
                &site,
                Classification::Public,
                &[origin("github.com/example-org/api")]
            ),
            stays("site")
        );
        // One repository the patterns do not name.
        assert_eq!(
            routes.place_classified(
                &site,
                private,
                &[
                    origin("github.com/example-org/api"),
                    origin("github.com/widgetco/api")
                ]
            ),
            stays("site")
        );
        // No repository at all.
        assert_eq!(routes.place_classified(&site, private, &[]), stays("site"));
        assert!(!routes.classifies_private(&site));
    }

    #[test]
    fn the_kind_orders_the_entries_wherever_each_is_written_and_the_index_is_as_written() {
        let routes = example();
        let site = name("site");
        let private = Classification::Private;
        // The repository-specific combined entry, written above the owner-wide one.
        assert_eq!(
            routes.place_classified(&site, private, &[origin("github.com/example-org/special")]),
            placed("special", 2)
        );
        // The owner's other private work: the second combined entry, before the
        // classification-only entry written above it.
        assert_eq!(
            routes.place_classified(&site, private, &[origin("github.com/example-org/api")]),
            placed("vault", 3)
        );
        // A private item no combined entry matches takes the classification entry.
        assert_eq!(
            routes.place_classified(&site, private, &[origin("github.com/widgetco/api")]),
            placed("vault", 1)
        );
        assert_eq!(
            routes.place_classified(&site, private, &[]),
            placed("vault", 1)
        );
        assert!(routes.classifies_private(&site));
        // A public item of the same owner: the repository entry, which no combined private
        // entry shadows.
        assert_eq!(
            routes.place(&site, &[origin("github.com/example-org/special")]),
            placed("public", 0)
        );
        assert_eq!(routes.place(&site, &[]), stays("site"));
    }

    #[test]
    fn within_one_kind_the_entry_written_first_wins() {
        let routes = configured(serde_json::json!([
            {"repositories": ["github.com/example-org/*"], "classification": "private",
             "to": "vault"},
            {"repositories": ["github.com/example-org/special"], "classification": "private",
             "to": "special"},
        ]));
        assert_eq!(
            routes.place_classified(
                &name("site"),
                Classification::Private,
                &[origin("github.com/example-org/special")]
            ),
            placed("vault", 0)
        );
    }

    #[test]
    fn a_public_combined_entry_outranks_a_public_classification_entry_and_never_a_private_item() {
        let routes = configured(serde_json::json!([
            {"classification": "public", "to": "team"},
            {"repositories": ["github.com/example-org/*"], "classification": "public",
             "to": "public"},
        ]));
        let site = name("site");
        assert_eq!(
            routes.place(&site, &[origin("github.com/example-org/api")]),
            placed("public", 1)
        );
        assert_eq!(
            routes.place(&site, &[origin("github.com/widgetco/api")]),
            placed("team", 0)
        );
        assert_eq!(
            routes.place_classified(
                &site,
                Classification::Private,
                &[origin("github.com/example-org/api")]
            ),
            stays("site")
        );
    }

    #[test]
    fn a_private_item_tries_only_entries_sending_where_it_may_be_written() {
        // A public combined entry to a public source, and a repository entry to one, never
        // take a private item; a source no layer gives a plugin may be named and is tried.
        let routes = crate::Config::from_document(serde_json::json!({"sources": {
            "site": {"plugin": "in-memory", "routes": [
                {"repositories": ["github.com/example-org/*"], "to": "public"},
                {"repositories": ["github.com/example-org/*"], "classification": "private",
                 "to": "elsewhere"},
            ]},
            "public": {"plugin": "in-memory"},
            "elsewhere": {"visibility": "private"},
        }}))
        .expect("an incomplete destination still loads")
        .routes();
        assert_eq!(
            routes.place_classified(
                &name("site"),
                Classification::Private,
                &[origin("github.com/example-org/api")]
            ),
            placed("elsewhere", 1)
        );
        // The same entries with the combined one gone leave a private item where it was sent.
        let routes = configured(serde_json::json!([
            {"repositories": ["github.com/example-org/*"], "to": "public"},
        ]));
        assert_eq!(
            routes.place_classified(
                &name("site"),
                Classification::Private,
                &[origin("github.com/example-org/api")]
            ),
            stays("site")
        );
    }

    #[test]
    fn a_configuration_with_no_combined_entry_places_as_it_did() {
        // Classification before repository, whatever their order, exactly as before.
        let routes = configured(serde_json::json!([
            {"repositories": ["github.com/widgetco/*"], "to": "team"},
            {"classification": "private", "to": "vault"},
        ]));
        let site = name("site");
        assert_eq!(
            routes.place_classified(
                &site,
                Classification::Private,
                &[origin("github.com/widgetco/api")]
            ),
            placed("vault", 1)
        );
        assert_eq!(
            routes.place(&site, &[origin("github.com/widgetco/api")]),
            placed("team", 0)
        );
    }

    #[test]
    fn a_combined_entry_is_refused_at_load_naming_the_source_and_the_entry() {
        let combined = |repositories: Value, to: &str| {
            serde_json::json!([
                {"classification": "private", "to": "vault"},
                {"repositories": repositories, "classification": "private", "to": to},
            ])
        };
        let one = serde_json::json!(["github.com/example-org/*"]);
        for (routes, wanted) in [
            (combined(one.clone(), "public"), "not declared private"),
            (combined(one.clone(), "nowhere"), "no source is configured"),
            (combined(one.clone(), "site"), "itself"),
            (combined(one, "chained"), "routes of its own"),
            (combined(serde_json::json!([]), "vault"), "empty"),
            (
                combined(serde_json::json!(["github.com/example*/x"]), "vault"),
                "not a repository pattern",
            ),
        ] {
            let refusal = refused(routes.clone());
            assert!(
                refusal.contains("sources.site.routes.1") && refusal.contains(wanted),
                "{routes}: {refusal}"
            );
        }
        // A combined entry naming a classification that is none is refused by that key.
        let refusal = refused(serde_json::json!([
            {"repositories": ["github.com/example-org/*"], "classification": "secret",
             "to": "vault"},
        ]));
        assert!(
            refusal.contains("sources.site.routes.0.classification"),
            "{refusal}"
        );
    }

    /// The sources [`configured`] declares, and their visibility, for building in code.
    fn declared() -> ([SourceName; 5], BTreeMap<SourceName, SourceVisibility>) {
        (
            [
                name("site"),
                name("public"),
                name("team"),
                name("vault"),
                name("special"),
            ],
            BTreeMap::from([
                (name("vault"), SourceVisibility::Private),
                (name("special"), SourceVisibility::Private),
            ]),
        )
    }

    #[test]
    fn combined_routes_built_in_code_place_as_the_same_entries_loaded_do() {
        let (configured, visibility) = declared();
        let built = Routes::new(
            BTreeMap::from([(
                name("site"),
                vec![(vec!["github.com/example-org/*".to_owned()], name("public"))],
            )]),
            &configured,
        )
        .expect("repository routes")
        .with_visibility(visibility)
        .classified(
            name("site"),
            vec![(Classification::Private, name("vault"))],
            &configured,
        )
        .expect("a classification route")
        .combined(
            name("site"),
            vec![
                (
                    Classification::Private,
                    vec!["github.com/example-org/special".to_owned()],
                    name("special"),
                ),
                (
                    Classification::Private,
                    vec!["github.com/example-org/*".to_owned()],
                    name("vault"),
                ),
            ],
            &configured,
        )
        .expect("combined routes");
        // The same entries, written in the order the constructors leave them.
        let loaded = self::configured(serde_json::json!([
            {"repositories": ["github.com/example-org/special"], "classification": "private",
             "to": "special"},
            {"repositories": ["github.com/example-org/*"], "classification": "private",
             "to": "vault"},
            {"classification": "private", "to": "vault"},
            {"repositories": ["github.com/example-org/*"], "to": "public"},
        ]));
        let site = name("site");
        assert_eq!(built.of(&site), loaded.of(&site));
        for classification in [Classification::Private, Classification::Public] {
            for repositories in [
                vec![origin("github.com/example-org/special")],
                vec![origin("github.com/example-org/api")],
                vec![origin("github.com/widgetco/api")],
                vec![],
            ] {
                assert_eq!(
                    built.place_classified(&site, classification, &repositories),
                    loaded.place_classified(&site, classification, &repositories),
                    "{classification:?} {repositories:?}"
                );
            }
        }
        assert_eq!(
            built.place_classified(
                &site,
                Classification::Private,
                &[origin("github.com/example-org/special")]
            ),
            placed("special", 0)
        );
        // Both accessors answer for a combined entry.
        let entry = &built.of(&site)[1];
        assert_eq!(entry.classification(), Some(Classification::Private));
        assert_eq!(
            entry
                .repositories()
                .iter()
                .map(RepositoryPattern::as_str)
                .collect::<Vec<_>>(),
            vec!["github.com/example-org/*"]
        );
        assert_eq!(entry.to(), &name("vault"));
    }

    #[test]
    fn combined_routes_built_in_code_are_refused_as_a_documents_are() {
        let (configured, visibility) = declared();
        let chained = Routes::new(
            BTreeMap::from([(
                name("team"),
                vec![(vec!["github.com/widgetco/*".to_owned()], name("public"))],
            )]),
            &configured,
        )
        .expect("a route on team");
        for (base, patterns, to, wanted) in [
            (
                Routes::default(),
                vec!["github.com/example-org/*"],
                "public",
                "not declared private",
            ),
            (
                Routes::default(),
                vec!["github.com/example-org/*"],
                "nowhere",
                "no source is configured",
            ),
            (
                Routes::default(),
                vec!["github.com/example-org/*"],
                "site",
                "itself",
            ),
            (
                chained,
                vec!["github.com/example-org/*"],
                "team",
                "routes of its own",
            ),
            (Routes::default(), vec![], "vault", "empty"),
            (
                Routes::default(),
                vec!["github.com/example*/x"],
                "vault",
                "not a repository pattern",
            ),
        ] {
            let refusal = base
                .with_visibility(visibility.clone())
                .combined(
                    name("site"),
                    vec![(
                        Classification::Private,
                        patterns.iter().map(|&pattern| pattern.to_owned()).collect(),
                        name(to),
                    )],
                    &configured,
                )
                .expect_err("refused")
                .to_string();
            assert!(
                refusal.contains("sources.site.routes.0") && refusal.contains(wanted),
                "{to} {patterns:?}: {refusal}"
            );
        }
    }
}
