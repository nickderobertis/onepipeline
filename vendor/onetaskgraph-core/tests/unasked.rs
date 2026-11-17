//! A project's graph as a **Rust caller** loads it when a source declared private stops
//! answering part way: what that source answered stands, it is named once, it is not asked
//! again, and a caller asking for a complete graph is refused.
//!
//! The source that stops answering is real `in-memory` work behind a wrapper that fails its
//! reads from the second on — a fault at the plugin boundary, which is the one thing a test
//! cannot ask a real hosted source for on cue. The graph is the real `Engine::project_graph`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use onetaskgraph_core::config::{Routes, SourceVisibility};
use onetaskgraph_core::{
    Completeness, ConfiguredSource, Engine, EngineError, GlobalId, ProjectGraphRequest,
    ResolvedSource,
};
use onetaskgraph_plugin_api::{
    Capabilities, DependencyEdge, Health, Label, NativeId, Page, PageRequest, Project,
    ProjectQuery, SecretResolver, SourceError, SourceName, SourcePlugin, Task, TaskQuery,
    TaskSource,
};
use secrecy::SecretString;
use serde_json::{Value, json};

/// No source here needs a credential.
struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

fn name(value: &str) -> SourceName {
    SourceName::new(value).expect("a valid source name")
}

fn id(value: &str) -> GlobalId {
    value.parse().expect("a qualified id")
}

fn built(source: &str, config: &Value) -> Box<dyn TaskSource> {
    onetaskgraph_in_memory::Plugin
        .build(&name(source), config, &NoSecrets)
        .expect("the in-memory plugin builds")
}

/// A source that answers its first `answers` task reads and is unreachable for every one
/// after, counting every task read it is asked.
struct StopsAnswering {
    inner: Box<dyn TaskSource>,
    answers: u32,
    asked: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl TaskSource for StopsAnswering {
    fn kind(&self) -> &'static str {
        "stops-answering"
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    async fn health(&self) -> Result<Health, SourceError> {
        self.inner.health().await
    }

    async fn get_task(&self, id: &NativeId) -> Result<Option<Task>, SourceError> {
        self.inner.get_task(id).await
    }

    async fn get_project(&self, id: &NativeId) -> Result<Option<Project>, SourceError> {
        self.inner.get_project(id).await
    }

    async fn query_tasks(
        &self,
        query: &TaskQuery,
        page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        if self.asked.fetch_add(1, Ordering::Relaxed) >= self.answers {
            return Err(SourceError::Unavailable {
                message: "the vault stopped answering".to_owned(),
            });
        }
        self.inner.query_tasks(query, page).await
    }

    async fn query_projects(
        &self,
        query: &ProjectQuery,
        page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        self.inner.query_projects(query, page).await
    }

    async fn labels(&self, page: &PageRequest) -> Result<Page<Label>, SourceError> {
        self.inner.labels(page).await
    }

    async fn task_dependencies(
        &self,
        id: &NativeId,
        direction: onetaskgraph_plugin_api::Direction,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.inner.task_dependencies(id, direction, page).await
    }

    async fn project_dependencies(
        &self,
        id: &NativeId,
        direction: onetaskgraph_plugin_api::Direction,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.inner.project_dependencies(id, direction, page).await
    }
}

fn task(id: &str, title: &str, extra: Value) -> Value {
    let mut task = json!({
        "id": id,
        "title": title,
        "status": {"category": "todo", "name": "Todo"},
        "labels": [],
    });
    task.as_object_mut()
        .expect("a task")
        .extend(extra.as_object().expect("extra keys").clone());
    task
}

/// A task of `vault` recording `site:<dependent>` as depending on it.
fn recorder(id: &str, dependent: &str) -> Value {
    let dependent = format!("site:{dependent}");
    task(
        id,
        id,
        json!({"metadata": {"onetaskgraph.depended_on_by": {
            dependent.clone(): {"id": dependent, "kind": "task"},
        }}}),
    )
}

/// The public `site`, holding project `P-1` and its tasks `T-1`, `T-2` and `T-3` — each landed
/// there by a copy that left an edge of it on the private side, so it carries a copy's record and
/// not the mark of one whose edges are all its own — beside the private `vault`, whose `V-1`,
/// `V-2` and `V-3` each record one of them depending on it, and which answers its first
/// `answers` task reads alone.
fn engine(answers: u32) -> (Engine, Arc<AtomicU32>) {
    engine_marked(answers, json!({}))
}

/// [`engine`], with `site`'s tasks carrying `metadata` beside a copy's record.
fn engine_marked(answers: u32, metadata: Value) -> (Engine, Arc<AtomicU32>) {
    let mut recorded = json!({"onetaskgraph.copied_destination_version": "sha256:00"});
    recorded
        .as_object_mut()
        .expect("an object")
        .extend(metadata.as_object().expect("metadata").clone());
    let tasks: Vec<Value> = ["T-1", "T-2", "T-3"]
        .map(|id| task(id, id, json!({"project": "P-1", "metadata": recorded})))
        .into();
    let site = built(
        "site",
        &json!({
            "projects": [{"id": "P-1", "title": "Plan",
                          "status": {"category": "todo", "name": "Todo"}, "labels": []}],
            "tasks": tasks,
        }),
    );
    let vault = built(
        "vault",
        &json!({"tasks": [recorder("V-1", "T-1"), recorder("V-2", "T-2"), recorder("V-3", "T-3")]}),
    );
    let asked = Arc::new(AtomicU32::new(0));
    let vault = StopsAnswering {
        inner: vault,
        answers,
        asked: Arc::clone(&asked),
    };
    let engine = Engine::new(
        vec![
            ConfiguredSource::Ready(ResolvedSource::adopt(name("site"), site)),
            ConfiguredSource::Ready(ResolvedSource::adopt(name("vault"), Box::new(vault))),
        ],
        vec![name("site"), name("vault")],
    )
    .with_routes(Routes::default().with_visibility(BTreeMap::from([
        (name("site"), SourceVisibility::Public),
        (name("vault"), SourceVisibility::Private),
    ])));
    (engine, asked)
}

fn request(completeness: Completeness) -> ProjectGraphRequest {
    ProjectGraphRequest {
        project: id("site:P-1"),
        direction: None,
        group_by: None,
        completeness,
    }
}

/// The `(prerequisite, dependent)` pairs a graph draws, from its JSON form.
fn edges(graph: &onetaskgraph_core::ProjectGraph) -> Vec<(String, String)> {
    let document = serde_json::to_value(graph).expect("a graph renders as JSON");
    let mut edges: Vec<(String, String)> = document["edges"]
        .as_array()
        .expect("the edges")
        .iter()
        .map(|edge| {
            (
                edge["from"].as_str().expect("a prerequisite").to_owned(),
                edge["to"].as_str().expect("a dependent").to_owned(),
            )
        })
        .collect();
    edges.sort();
    edges
}

#[tokio::test]
async fn a_private_source_that_answers_every_read_draws_every_edge_it_records() {
    let (engine, asked) = engine(u32::MAX);
    let graph = engine
        .project_graph(&request(Completeness::Required))
        .await
        .expect("every source answers");
    assert!(graph.is_complete());
    assert_eq!(
        edges(&graph),
        [
            ("vault:V-1", "site:T-1"),
            ("vault:V-2", "site:T-2"),
            ("vault:V-3", "site:T-3")
        ]
        .map(|(from, to)| (from.to_owned(), to.to_owned()))
    );
    assert_eq!(asked.load(Ordering::Relaxed), 3, "one read per task");
}

#[tokio::test]
async fn a_private_source_that_stops_answering_keeps_what_it_answered_and_is_not_asked_again() {
    let (engine, asked) = engine(1);
    let graph = engine
        .project_graph(&request(Completeness::NamingUnasked))
        .await
        .expect("a graph without what vault could not be asked");

    assert!(!graph.is_complete());
    let unanswered: Vec<(&str, &SourceError)> = graph
        .unanswered()
        .iter()
        .map(|entry| {
            let failure = entry.source().expect("a private source not asked");
            (failure.source.as_str(), &failure.error)
        })
        .collect();
    assert_eq!(
        unanswered,
        [(
            "vault",
            &SourceError::Unavailable {
                message: "the vault stopped answering".to_owned()
            }
        )],
        "named once, however many tasks it was not asked about"
    );
    assert_eq!(
        edges(&graph),
        [("vault:V-1".to_owned(), "site:T-1".to_owned())],
        "the edge it answered before it stopped is drawn"
    );
    assert_eq!(
        asked.load(Ordering::Relaxed),
        2,
        "asked for the first task, failing the second, and not asked for the third"
    );
}

#[tokio::test]
async fn a_complete_graph_is_refused_once_a_private_source_stops_answering() {
    let (engine, _) = engine(1);
    let refused = engine
        .project_graph(&request(Completeness::Required))
        .await
        .expect_err("a complete graph is refused");
    assert!(
        matches!(
            &refused,
            EngineError::SourceFailed { name, error: SourceError::Unavailable { .. } }
                if name == "vault"
        ),
        "{refused:?}"
    );
}

/// A task whose copy marked every edge of it as on its own record is never asked about of a
/// private source: the graph draws nothing the vault records, and the vault is sent nothing.
#[tokio::test]
async fn a_task_whose_edges_are_on_its_record_is_not_asked_about_of_a_private_source() {
    let (engine, asked) = engine_marked(u32::MAX, json!({"onetaskgraph.edges_on_record": 1}));
    let graph = engine
        .project_graph(&request(Completeness::Required))
        .await
        .expect("a graph that asks the vault nothing");
    assert!(graph.is_complete());
    assert_eq!(edges(&graph), Vec::<(String, String)>::new());
    assert_eq!(
        asked.load(Ordering::Relaxed),
        0,
        "the vault was sent nothing"
    );
}
