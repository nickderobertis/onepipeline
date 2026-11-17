//! A dependency of a task in a source not declared private on a task in a source that is,
//! kept on the private side.
//!
//! A write anywhere not declared private withholds every id naming an item of a source that
//! is (see [`Engine::withholds`]), so the public task cannot hold such an edge itself: holding
//! it would name the private task where the private task must not be named. The private task
//! may name the public one freely, so the copy that withholds the edge records it there
//! instead, under [`DEPENDED_ON_BY_KEY`], and every read of the graph — `task deps` from
//! either end, `project graph`, and so the plan a caller loads from them — merges what the
//! private side records with what each source holds natively.
//!
//! The record lives in the private source's own store, at the user's explicit request — a
//! copy — and is read back only by asking that source again, so it is a write into a plugin
//! rather than state the engine keeps.
//!
//! A source declared private that could not be asked — its credential absent or refused, it
//! unreachable or rate-limited — leaves such a read answering what every other source holds and
//! naming that source, rather than failing a read of tasks none of which live there: a reader
//! who may not see a private source still reads the public graph. A source that answered — with
//! a record that does not say what it names, a refusal of the request, or word that its own
//! configuration is wrong — still fails the read, because that is an edge lost or a store to
//! fix, not a source unasked. A caller that needs every edge asks for
//! [`Completeness::Required`] and is refused.

use std::collections::BTreeMap;

use onetaskgraph_plugin_api::{
    DependencyEdge, DependencyEndpoint, DependencyKind, ItemKind, MetadataKey, MetadataMatch,
    NativeId, SourceError, SourceName, Task,
};
use serde_json::{Map, Value};

use super::local::ProjectSelector;
use super::{Engine, EngineError, Filters, Paging, TaskRequest};
use crate::GlobalId;
use crate::config::SourceVisibility;
use crate::plan::SourceFailure;

/// The reserved key a task of a source declared private records, under it, the tasks of
/// sources not declared private that depend on it.
///
/// A JSON object keyed by each dependent's qualified id, whose value is that dependent as a
/// [`DependencyEndpoint`] naming a task, so a read can ask a source, by an ordinary metadata
/// match on the endpoint's `id`, which of its tasks one id depends on. Store owned like every
/// key in the namespace: a copy keeps it, nothing a caller types can name it, and a copy never
/// carries one item's entry onto another.
pub(crate) const DEPENDED_ON_BY_KEY: &str = "onetaskgraph.depended_on_by";

/// The tasks one private task records as depending on it, each once.
///
/// Keyed by the id's own rendering, because that is the key the record is spelled under and
/// [`GlobalId`] has no order of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Dependents(BTreeMap<String, GlobalId>);

impl Dependents {
    /// Add `dependent`, once however often it is added.
    pub(crate) fn insert(&mut self, dependent: GlobalId) {
        self.0.insert(dependent.to_string(), dependent);
    }

    pub(crate) fn contains(&self, dependent: &GlobalId) -> bool {
        self.0.contains_key(&dependent.to_string())
    }

    /// Every one of them, in the order the record spells them.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &GlobalId> {
        self.0.values()
    }

    /// These, without every one of `decided`, and with every one of `named`: what a task holds
    /// once a copy that decided `decided` found `named` depending on it.
    pub(crate) fn applied(&self, decided: &Self, named: &Self) -> Self {
        self.iter()
            .filter(|dependent| !decided.contains(dependent))
            .chain(named.iter())
            .cloned()
            .collect()
    }
}

impl FromIterator<GlobalId> for Dependents {
    fn from_iter<I: IntoIterator<Item = GlobalId>>(items: I) -> Self {
        let mut dependents = Self::default();
        for dependent in items {
            dependents.insert(dependent);
        }
        dependents
    }
}

/// The dependents `metadata` records, or a message saying what about the entry cannot be read.
///
/// # Errors
///
/// An entry that is not an object, or holds a value that is not that same qualified id as a
/// task endpoint: an edge that cannot be read is refused rather than read as none.
pub(crate) fn recorded(metadata: &BTreeMap<String, Value>) -> Result<Dependents, String> {
    let Some(held) = metadata.get(DEPENDED_ON_BY_KEY) else {
        return Ok(Dependents::default());
    };
    let Some(entries) = held.as_object() else {
        return Err(format!(
            "{DEPENDED_ON_BY_KEY} is not an object of the tasks that depend on it"
        ));
    };
    entries
        .iter()
        .map(|(id, value)| {
            let refused = || {
                format!(
                    "{DEPENDED_ON_BY_KEY} records {id:?} as {value}, which is not that \
                     qualified task id as a task endpoint"
                )
            };
            let endpoint: DependencyEndpoint =
                serde_json::from_value(value.clone()).map_err(|_| refused())?;
            if !endpoint.is_qualified() || endpoint.kind != ItemKind::Task || endpoint.id() != id {
                return Err(refused());
            }
            id.parse::<GlobalId>().map_err(|_| refused())
        })
        .collect()
}

/// `metadata` recording exactly `dependents`, and without the key when there are none.
pub(crate) fn record(metadata: &mut BTreeMap<String, Value>, dependents: &Dependents) {
    if dependents.0.is_empty() {
        metadata.remove(DEPENDED_ON_BY_KEY);
        return;
    }
    let entries: Map<String, Value> = dependents
        .0
        .iter()
        .map(|(id, dependent)| {
            let endpoint = DependencyEndpoint::new(dependent.to_string(), ItemKind::Task)
                .expect("a qualified id is a valid endpoint");
            (
                id.clone(),
                serde_json::to_value(endpoint).expect("an endpoint renders as JSON"),
            )
        })
        .collect();
    metadata.insert(DEPENDED_ON_BY_KEY.to_owned(), Value::Object(entries));
}

/// The match a task recording `dependent` among its dependents holds.
pub(crate) fn naming(dependent: &GlobalId) -> MetadataMatch {
    let id = dependent.to_string();
    MetadataMatch::new(DEPENDED_ON_BY_KEY, vec![id.clone(), "id".to_owned()], id)
        .expect("the key and a qualified id are never empty")
}

/// Hold `task`, found by [`naming`] `dependent`, to a record that reads and names it.
///
/// # Errors
///
/// A record [`recorded`] refuses, or one the match found that does not name `dependent` as a
/// task — an entry holding its id under another kind, say — so a malformed record is never
/// read as an edge.
pub(crate) fn names(task: &Task, dependent: &GlobalId) -> Result<(), SourceError> {
    let malformed = |message: String| SourceError::Malformed {
        message: format!("task {}: {message}", task.id),
    };
    if recorded(&task.metadata)
        .map_err(malformed)?
        .contains(dependent)
    {
        Ok(())
    } else {
        Err(malformed(format!(
            "{DEPENDED_ON_BY_KEY} matched {dependent} without recording it as a task endpoint"
        )))
    }
}

/// The edge `dependent` → `prerequisite`, with the prerequisite native to the source reporting
/// it.
pub(crate) fn edge(dependent: &GlobalId, prerequisite: NativeId) -> DependencyEdge {
    DependencyEdge {
        from: DependencyEndpoint::new(dependent.to_string(), ItemKind::Task)
            .expect("a qualified id is a valid endpoint"),
        to: DependencyEndpoint::from_native(prerequisite, ItemKind::Task),
        kind: DependencyKind::Blocks,
    }
}

/// The edges `task`'s own record says depend on it, each from a qualified dependent to
/// `task` native to its source.
///
/// # Errors
///
/// As [`recorded`].
pub(crate) fn recorded_edges(task: &Task) -> Result<Vec<DependencyEdge>, SourceError> {
    Ok(recorded(&task.metadata)
        .map_err(|message| SourceError::Malformed {
            message: format!("task {}: {message}", task.id),
        })?
        .iter()
        .map(|dependent| edge(dependent, task.id.clone()))
        .collect())
}

/// Whether `error` says a source could not be asked at all — its credential absent or refused,
/// it unreachable, or it asking to be asked later — rather than that it answered: with a record
/// that cannot be read, a refusal of the request, or word that its configuration is wrong.
pub(crate) fn could_not_answer(error: &SourceError) -> bool {
    matches!(
        error,
        SourceError::Auth { .. }
            | SourceError::Unavailable { .. }
            | SourceError::RateLimited { .. }
    )
}

/// What a dependency read owes when a source declared private could not be asked what it
/// records of the items read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Completeness {
    /// Answer every edge the sources that could answer hold, and name each source declared
    /// private that could not be asked: a reader without that source's credential still reads
    /// the rest of the graph.
    #[default]
    NamingUnasked,
    /// Refuse an answer that lacks what any source records, as any other failed read is
    /// refused.
    Required,
}

impl Engine {
    /// Whether `failure`, reported by a dependency read of an item of `subject`, is a source
    /// declared private that could not be asked what it records of that item — so the read
    /// answered every edge the other sources hold, and lacks only what that source records.
    ///
    /// `false` for a failure of `subject` itself, of any source not asked for its records, and
    /// for anything a source answered with: each of those leaves the answer wrong rather than
    /// short of one private source.
    pub(crate) fn records_unasked(&self, subject: &SourceName, failure: &SourceFailure) -> bool {
        &failure.source != subject
            && could_not_answer(&failure.error)
            && self.recording_sources(subject).contains(&failure.source)
    }

    /// Whether a dependency read of `task`, held by `source`, asks the sources declared private
    /// what they record of it: only when `source` is not declared private itself, `task` was
    /// landed by a copy — it carries [`MetadataKey::COPIED_DESTINATION_VERSION_KEY`] — and that
    /// copy did not mark every edge of it as on its own record, with
    /// [`MetadataKey::EDGES_ON_RECORD_KEY`]. Nothing else can have had an edge of it kept on a
    /// private source's side, so nothing else costs a private source a request.
    pub(crate) fn reads_recorded_for(&self, source: &SourceName, task: &Task) -> bool {
        self.declared(source) != SourceVisibility::Private
            && task
                .metadata
                .contains_key(MetadataKey::COPIED_DESTINATION_VERSION_KEY)
            && !task.metadata.contains_key(MetadataKey::EDGES_ON_RECORD_KEY)
    }

    /// The sources a forward dependency read of task `native` of `near` asks what they record of
    /// it: [`recording_sources`](Self::recording_sources) when
    /// [`reads_recorded_for`](Self::reads_recorded_for) holds of the task as `near` reads it now,
    /// and none otherwise. A task `near` cannot be asked for is asked of every one, because
    /// what it records cannot then be ruled out.
    pub(crate) async fn recording_sources_for(
        &self,
        near: &SourceName,
        native: &NativeId,
    ) -> Vec<SourceName> {
        let recording = self.recording_sources(near);
        if recording.is_empty() {
            return recording;
        }
        let Some(source) = self.ready_source(near) else {
            return recording;
        };
        match source.get_task(native).await {
            Ok(Some(task)) if self.reads_recorded_for(near, &task) => recording,
            Ok(_) => Vec::new(),
            Err(_) => recording,
        }
    }

    /// Every configured source that may record a dependent from `near`: each one declared
    /// private but `near` itself, and none at all when `near` is declared private, because a
    /// private task holds its own edges wherever they lead.
    pub(crate) fn recording_sources(&self, near: &SourceName) -> Vec<SourceName> {
        if self.declared(near) == SourceVisibility::Private {
            return Vec::new();
        }
        self.sources
            .iter()
            .map(|source| source.name().clone())
            .filter(|name| name != near && self.declared(name) == SourceVisibility::Private)
            .collect()
    }

    /// Every task of a source declared private that records `dependent` as depending on it,
    /// read whole from each of [`recording_sources`](Self::recording_sources).
    ///
    /// # Errors
    ///
    /// A source that could not be built or failed a read, and a record that does not say what
    /// it names: a prerequisite that cannot be read is refused rather than reported absent.
    pub(crate) async fn recorded_prerequisites(
        &self,
        dependent: &GlobalId,
    ) -> Result<Vec<GlobalId>, EngineError> {
        let sources = self.recording_sources(&dependent.source);
        self.recorded_prerequisites_in(dependent, &sources).await
    }

    /// [`recorded_prerequisites`](Self::recorded_prerequisites), asked of `sources` alone.
    ///
    /// # Errors
    ///
    /// As [`recorded_prerequisites`](Self::recorded_prerequisites).
    pub(crate) async fn recorded_prerequisites_in(
        &self,
        dependent: &GlobalId,
        sources: &[SourceName],
    ) -> Result<Vec<GlobalId>, EngineError> {
        let mut prerequisites = Vec::new();
        for source in sources.iter().cloned() {
            let mut request = TaskRequest {
                sources: vec![source],
                filters: Filters::default(),
                project: ProjectSelector::Any,
                priorities: Vec::new(),
                commented_since: None,
                metadata: vec![naming(dependent)],
                origin: None,
                include_members: false,
                paging: Paging {
                    limit: super::copy::PROJECT_PAGE,
                    token: None,
                },
            };
            loop {
                let response = self.tasks(&request).await?;
                if let Some(failure) = response.errors.into_iter().next() {
                    return Err(EngineError::SourceFailed {
                        name: failure.source.to_string(),
                        error: failure.error,
                    });
                }
                for task in response.items {
                    names(&task.item, dependent).map_err(|error| EngineError::SourceFailed {
                        name: task.id.source.to_string(),
                        error,
                    })?;
                    prerequisites.push(task.id);
                }
                match response.next {
                    Some(token) => request.paging.token = Some(token),
                    None => break,
                }
            }
        }
        Ok(prerequisites)
    }

    /// [`recorded_prerequisites`](Self::recorded_prerequisites), with each source that could
    /// not be asked added to `unasked` instead of failing the read, and not asked again once it
    /// is there — unless `completeness` is [`Completeness::Required`], when it fails the read as
    /// it always did. What such a source answered for an earlier dependent stays answered.
    ///
    /// # Errors
    ///
    /// As [`recorded_prerequisites`](Self::recorded_prerequisites), less a source that could
    /// not be asked when the answer need not be complete.
    pub(crate) async fn recorded_prerequisites_answered(
        &self,
        dependent: &GlobalId,
        completeness: Completeness,
        unasked: &mut Vec<SourceFailure>,
    ) -> Result<Vec<GlobalId>, EngineError> {
        let mut prerequisites = Vec::new();
        for source in self.recording_sources(&dependent.source) {
            if unasked.iter().any(|failure| failure.source == source) {
                continue;
            }
            match self
                .recorded_prerequisites_in(dependent, std::slice::from_ref(&source))
                .await
            {
                Ok(found) => prerequisites.extend(found),
                Err(EngineError::SourceFailed { name, error })
                    if completeness == Completeness::NamingUnasked
                        && name == source.as_str()
                        && could_not_answer(&error) =>
                {
                    unasked.push(SourceFailure { source, error });
                }
                Err(error) => return Err(error),
            }
        }
        Ok(prerequisites)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{DEPENDED_ON_BY_KEY, Dependents, naming, record, recorded};
    use crate::GlobalId;

    fn id(text: &str) -> GlobalId {
        text.parse().expect("a qualified id")
    }

    /// What one write records reads back, and the match for each dependent holds of it.
    #[test]
    fn a_recorded_dependent_reads_back_and_is_found_by_its_match() {
        let dependents: Dependents = [id("board:12"), id("notes:roll.out")].into_iter().collect();
        let mut metadata = BTreeMap::new();
        record(&mut metadata, &dependents);
        assert_eq!(recorded(&metadata), Ok(dependents.clone()));
        for dependent in dependents.iter() {
            assert!(naming(dependent).holds(&metadata), "{metadata:?}");
        }
        assert!(!naming(&id("board:13")).holds(&metadata));
        record(&mut metadata, &Dependents::default());
        assert!(metadata.is_empty(), "no dependents leaves no key");
    }

    /// An entry that does not say what it names is refused, never read as no dependents.
    #[test]
    fn an_entry_that_is_not_its_own_task_endpoint_is_refused() {
        for held in [
            json!(["board:12"]),
            json!({"board:12": "board:12"}),
            json!({"board:12": {"id": "board:13", "kind": "task"}}),
            json!({"board:12": {"id": "board:12", "kind": "project"}}),
            json!({"unqualified": {"id": "unqualified", "kind": "task"}}),
        ] {
            let metadata = BTreeMap::from([(DEPENDED_ON_BY_KEY.to_owned(), held.clone())]);
            assert!(recorded(&metadata).is_err(), "{held} is refused");
        }
    }
}
