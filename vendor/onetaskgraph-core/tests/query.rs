//! What the engine does with a query, against real sources.
//!
//! Almost everything here drives the real `in-memory` plugin through a real
//! configuration document, because the behaviour under test is the engine's *choice* —
//! push this predicate down, narrow that one here — and a source written for the test
//! would be a source that agrees with whatever the engine happens to do. Two sources are
//! written here, and both are instrumentation at the boundary rather than a stand-in for
//! the layer under test: one that pauses at a rendezvous, because no plugin can be made
//! to pause, and one that answers a cursor with the same cursor, because no correct
//! plugin does.
//!
//! The journeys that prove the same behaviour end to end drive the compiled binary; see
//! `crates/onetaskgraph-e2e/tests/e2e/`.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use onetaskgraph_core::{
    Config, ConfiguredSource, DependencyRequest, Engine, EngineError, Filters, GlobalId,
    LabelRequest, PageToken, Paging, Predicate, ProjectRequest, ProjectSelector, QueryPlan,
    ResolvedSource, SearchKind, SearchRequest, SourcePlan, TaskRequest,
};
use onetaskgraph_plugin_api::{
    Capabilities, Cursor, DependencyEdge, DependencySupport, Direction, Health, Label, LabelFilter,
    NativeId, Page, PageRequest, Priority, Project, ProjectQuery, SecretResolver, SourceError,
    SourceName, Status, StatusCategory, Support, Task, TaskQuery, TaskSource, TextFields,
    TextQuery,
};
use secrecy::SecretString;
use serde_json::{Value, json};
use tokio::sync::Barrier;

/// No source in this crate's tests needs a credential.
struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

/// A source name, or the test's own fault.
fn name(value: &str) -> SourceName {
    SourceName::new(value).expect("a valid source name")
}

/// An engine over a configuration document's `sources:` block.
fn engine_over(sources: Value) -> Engine {
    let config =
        Config::from_document(json!({ "sources": sources })).expect("a valid configuration");
    Engine::build(&config, &NoSecrets)
}

/// One page of `limit` items, from the beginning.
fn page(limit: u32) -> Paging {
    Paging {
        limit: NonZeroU32::new(limit).expect("a non-zero limit"),
        token: None,
    }
}

/// The same page, resumed.
fn resumed(limit: u32, token: PageToken) -> Paging {
    Paging {
        limit: NonZeroU32::new(limit).expect("a non-zero limit"),
        token: Some(token),
    }
}

/// The shared fixture: four tasks, three labels, two projects, one dependency graph.
fn work() -> Value {
    json!({
        "tasks": [
            {"id": "T-1", "title": "Alpha engine", "content": "the engine core",
             "status": {"category": "todo", "name": "Todo"},
             "labels": [{"id": "L-1", "name": "bug"}, {"id": "L-3", "name": "core"}],
             "project": "P-1"},
            {"id": "T-2", "title": "Beta", "content": "alpha in the body",
             "status": {"category": "done", "name": "Shipped"},
             "labels": [{"id": "L-2", "name": "chore"}], "project": "P-1"},
            {"id": "T-3", "title": "Gamma", "content": "unrelated",
             "status": {"category": "todo", "name": "Todo"},
             "labels": [{"id": "L-1", "name": "bug"}]},
            {"id": "T-4", "title": "Delta docs", "content": "documentation",
             "status": {"category": "in-progress", "name": "Doing"},
             "labels": [{"id": "L-3", "name": "core"}], "project": "P-2"}
        ],
        "projects": [
            {"id": "P-1", "title": "Engine", "content": "the engine",
             "status": {"category": "in-progress", "name": "Doing"}, "labels": []},
            {"id": "P-2", "title": "Docs", "content": "alpha docs",
             "status": {"category": "todo", "name": "Todo"}, "labels": []}
        ],
        "labels": [
            {"id": "L-1", "name": "bug"},
            {"id": "L-2", "name": "chore"},
            {"id": "L-3", "name": "core"}
        ],
        "task_dependencies": [
            {"from": "T-1", "to": "T-2", "kind": "blocks"},
            {"from": "T-3", "to": "T-2", "kind": "blocks"},
            {"from": "T-4", "to": "T-2", "kind": "related"}
        ],
        "project_dependencies": [{"from": "P-1", "to": "P-2", "kind": "blocks"}]
    })
}

/// The same fixture under a source that applies nothing and pages two rows at a time.
fn compensated(overrides: Value) -> Value {
    let mut config = work();
    config["capabilities"] = overrides;
    config
}

/// The capability block a compensating source declares.
fn nothing_native() -> Value {
    json!({
        "filter_by_label": "unsupported",
        "filter_by_status": "unsupported",
        "search_title": "unsupported",
        "search_content": "unsupported",
        "orphan_tasks": "unsupported",
        "task_dependencies": "forward-only",
        "project_dependencies": "forward-only",
        "max_page_size": 2
    })
}

/// A task request over every configured source.
fn tasks(filters: Filters, project: ProjectSelector, paging: Paging) -> TaskRequest {
    TaskRequest {
        sources: Vec::new(),
        filters,
        project,
        priorities: Vec::new(),
        commented_since: None,
        metadata: Vec::new(),
        origin: None,
        include_members: false,
        paging,
    }
}

/// The plan entry for one source.
fn entry<'a>(plan: &'a QueryPlan, source: &str) -> &'a SourcePlan {
    plan.per_source
        .iter()
        .find(|entry| entry.source.as_str() == source)
        .unwrap_or_else(|| panic!("the plan has no entry for {source}: {plan:?}"))
}

/// The qualified ids a response carries, in order.
fn ids<T>(items: &[onetaskgraph_core::Qualified<T>]) -> Vec<String> {
    items.iter().map(|item| item.id.to_string()).collect()
}

#[tokio::test]
async fn one_query_against_two_sources_of_different_capability_returns_one_answer_by_two_plans() {
    // The whole reason capability declaration exists. `fast` filters by label itself;
    // `slow` ignores the predicate and returns the wider set, which the engine narrows.
    // Both answers are correct and the plan says which route each took.
    let engine = engine_over(json!({
        "fast": {"plugin": "in-memory", "config": work()},
        "slow": {"plugin": "in-memory", "config": compensated(nothing_native())},
    }));

    let response = engine
        .tasks(&tasks(
            Filters {
                labels: LabelFilter {
                    all_of: vec!["bug".to_owned()],
                    ..LabelFilter::default()
                },
                ..Filters::default()
            },
            ProjectSelector::Any,
            page(50),
        ))
        .await
        .expect("the query runs");

    assert_eq!(
        ids(&response.items),
        ["fast:T-1", "slow:T-1", "fast:T-3", "slow:T-3"],
        "both sources must return exactly the tasks carrying `bug`"
    );
    assert_eq!(
        entry(&response.plan, "fast").pushed_down,
        [Predicate::Label]
    );
    assert!(entry(&response.plan, "fast").applied_locally.is_empty());
    assert_eq!(
        entry(&response.plan, "slow").applied_locally,
        [Predicate::Label]
    );
    assert!(entry(&response.plan, "slow").pushed_down.is_empty());
    assert!(response.errors.is_empty());
}

#[tokio::test]
async fn a_half_capable_source_is_not_asked_to_search_at_all() {
    // A `title-or-content` search pushed to a source that searches titles but not bodies
    // would come back missing every row matching only in the body — narrower than the
    // truth, which is the one error compensation cannot repair.
    let engine = engine_over(json!({
        "half": {"plugin": "in-memory", "config": compensated(json!({"search_content": "unsupported"}))},
    }));

    let response = engine
        .tasks(&tasks(
            Filters {
                text: Some(TextQuery {
                    terms: "alpha".to_owned(),
                    fields: TextFields::TitleOrContent,
                }),
                ..Filters::default()
            },
            ProjectSelector::Any,
            page(50),
        ))
        .await
        .expect("the query runs");

    assert_eq!(
        ids(&response.items),
        ["half:T-1", "half:T-2"],
        "the body-only match must survive"
    );
    assert_eq!(
        entry(&response.plan, "half").applied_locally,
        [Predicate::SearchTitle, Predicate::SearchContent]
    );
    assert!(entry(&response.plan, "half").pushed_down.is_empty());
}

#[tokio::test]
async fn an_emulated_reverse_walk_answers_exactly_what_a_native_one_does() {
    // Two sources over the same dependency graph, one declaring `both-directions` and
    // one `forward-only`. The engine's bounded scan must produce the native answer edge
    // for edge, and must say it emulated it.
    let engine = engine_over(json!({
        "native": {"plugin": "in-memory", "config": work()},
        "scanned": {"plugin": "in-memory", "config": compensated(nothing_native())},
    }));

    let mut answers = Vec::new();
    for source in ["native", "scanned"] {
        let response = engine
            .task_dependencies(&DependencyRequest {
                id: format!("{source}:T-2").parse().expect("a qualified id"),
                direction: Direction::DependedOnBy,
                completeness: onetaskgraph_core::Completeness::NamingUnasked,
                paging: page(50),
            })
            .await
            .expect("the walk runs");

        let plan = entry(&response.plan, source);
        if source == "native" {
            assert_eq!(plan.pushed_down, [Predicate::ReverseDependencies]);
            assert!(plan.emulated.is_empty());
        } else {
            assert_eq!(plan.emulated, [Predicate::ReverseDependencies]);
            assert!(plan.pushed_down.is_empty());
            assert!(
                plan.pages_fetched > 1,
                "a scan of a two-row-per-page source costs more than one page"
            );
        }

        answers.push(
            response
                .items
                .iter()
                .map(|edge| {
                    format!(
                        "{}->{} {:?}",
                        edge.from.id.native, edge.to.id.native, edge.kind
                    )
                })
                .collect::<Vec<_>>(),
        );
    }

    assert_eq!(
        answers[0], answers[1],
        "the emulated reverse walk must match the native one edge for edge"
    );
    assert_eq!(answers[0].len(), 3, "T-2 is depended on by three tasks");
}

#[tokio::test]
async fn a_source_with_no_projects_reports_the_predicate_unavailable_rather_than_guessing() {
    let engine = engine_over(json!({
        "flat": {"plugin": "in-memory", "config": compensated(json!({"projects": "unsupported"}))},
        "deep": {"plugin": "in-memory", "config": work()},
    }));

    let response = engine
        .projects(&ProjectRequest {
            sources: Vec::new(),
            filters: Filters::default(),
            paging: page(50),
        })
        .await
        .expect("the query runs");

    assert_eq!(ids(&response.items), ["deep:P-1", "deep:P-2"]);
    assert_eq!(
        entry(&response.plan, "flat").unavailable,
        [Predicate::Project]
    );
    assert_eq!(entry(&response.plan, "flat").pages_fetched, 0);
    assert!(entry(&response.plan, "deep").unavailable.is_empty());
}

#[tokio::test]
async fn a_walk_returns_every_row_once_whatever_the_page_size_and_repeats_itself_exactly() {
    let sources = json!({
        "fast": {"plugin": "in-memory", "config": work()},
        "slow": {"plugin": "in-memory", "config": compensated(nothing_native())},
    });

    let whole = engine_over(sources.clone())
        .tasks(&tasks(Filters::default(), ProjectSelector::Any, page(50)))
        .await
        .expect("the query runs");
    assert_eq!(whole.items.len(), 8);
    assert!(whole.next.is_none());

    for limit in [1u32, 2, 3, 7] {
        let engine = engine_over(sources.clone());
        let mut seen = Vec::new();
        let mut token = None;
        let mut pages = 0;
        loop {
            let response = engine
                .tasks(&tasks(
                    Filters::default(),
                    ProjectSelector::Any,
                    match token {
                        None => page(limit),
                        Some(token) => resumed(limit, token),
                    },
                ))
                .await
                .expect("the query runs");
            pages += 1;
            assert!(
                response.items.len() as u32 <= limit,
                "a page never overfills"
            );
            seen.extend(ids(&response.items));
            token = response.next;
            if token.is_none() {
                break;
            }
            assert!(pages < 50, "a walk of eight rows must terminate");
        }

        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            8,
            "limit {limit} returned {} row(s) with {} distinct",
            seen.len(),
            sorted.len()
        );

        // Within one source the order is that source's own, whatever the page size.
        let of_fast: Vec<&String> = seen.iter().filter(|id| id.starts_with("fast:")).collect();
        assert_eq!(of_fast, ["fast:T-1", "fast:T-2", "fast:T-3", "fast:T-4"]);
    }
}

#[tokio::test]
async fn a_source_that_cannot_be_built_leaves_the_others_answering_and_is_named() {
    // Linear is configured without its credential and must not stop the configured
    // `in-memory` source from answering.
    let engine = engine_over(json!({
        "broken": {"plugin": "linear", "config": {}},
        "work": {"plugin": "in-memory", "config": work()},
    }));

    let response = engine
        .tasks(&tasks(Filters::default(), ProjectSelector::Any, page(50)))
        .await
        .expect("the query runs");

    assert_eq!(
        ids(&response.items),
        ["work:T-1", "work:T-2", "work:T-3", "work:T-4"]
    );
    assert_eq!(response.errors.len(), 1);
    assert_eq!(response.errors[0].source.as_str(), "broken");
    assert!(
        response.errors[0]
            .error
            .to_string()
            .contains("LINEAR_API_KEY"),
        "{:?}",
        response.errors[0]
    );

    let listed = engine.listing();
    assert_eq!(listed.len(), 2);
    assert!(matches!(
        listed[0].state,
        onetaskgraph_core::SourceState::Unavailable { .. }
    ));
}

#[tokio::test]
async fn a_request_naming_a_source_nothing_configures_is_refused_with_the_names_that_exist() {
    let engine = engine_over(json!({"work": {"plugin": "in-memory", "config": work()}}));
    let Err(EngineError::UnknownSource {
        name: asked,
        configured,
    }) = engine
        .tasks(&TaskRequest {
            sources: vec![name("elsewhere")],
            filters: Filters::default(),
            project: ProjectSelector::Any,
            priorities: Vec::new(),
            commented_since: None,
            metadata: Vec::new(),
            origin: None,
            include_members: false,
            paging: page(10),
        })
        .await
    else {
        panic!("a source nothing configures must be refused");
    };
    assert_eq!(asked, "elsewhere");
    assert_eq!(configured, "work");

    let nothing = engine_over(json!({}));
    assert!(matches!(
        nothing
            .tasks(&tasks(Filters::default(), ProjectSelector::Any, page(10)))
            .await,
        Err(EngineError::NoSources)
    ));
    assert!(matches!(
        nothing
            .task(&"work:T-1".parse::<GlobalId>().expect("an id"))
            .await,
        Err(EngineError::NoSources)
    ));
}

#[tokio::test]
async fn labels_and_search_and_orphans_answer_across_sources() {
    let engine = engine_over(json!({
        "fast": {"plugin": "in-memory", "config": work()},
        "slow": {"plugin": "in-memory", "config": compensated(nothing_native())},
    }));

    let labels = engine
        .labels(&LabelRequest {
            sources: Vec::new(),
            paging: page(50),
        })
        .await
        .expect("the query runs");
    assert_eq!(
        ids(&labels.items),
        [
            "fast:L-1", "slow:L-1", "fast:L-2", "slow:L-2", "fast:L-3", "slow:L-3"
        ]
    );

    // A search over both entities reads two streams from each source, and the plan is
    // still one entry per source.
    let hits = engine
        .search(&SearchRequest {
            sources: vec![name("fast")],
            text: TextQuery {
                terms: "alpha".to_owned(),
                fields: TextFields::TitleOrContent,
            },
            kind: SearchKind::Both,
            paging: page(50),
        })
        .await
        .expect("the query runs");
    assert_eq!(hits.plan.per_source.len(), 1);
    assert_eq!(hits.items.len(), 3, "two tasks and one project match");

    // An orphan is a first-class case: T-3 belongs to no project, in both sources, and
    // `slow` cannot select it itself.
    let orphans = engine
        .tasks(&tasks(
            Filters::default(),
            ProjectSelector::Orphans,
            page(50),
        ))
        .await
        .expect("the query runs");
    assert_eq!(ids(&orphans.items), ["fast:T-3", "slow:T-3"]);
    assert_eq!(
        entry(&orphans.plan, "slow").applied_locally,
        [Predicate::Project]
    );
    assert_eq!(
        entry(&orphans.plan, "fast").pushed_down,
        [Predicate::Project]
    );
}

#[tokio::test]
async fn a_qualified_project_filter_narrows_the_query_to_that_project_s_own_source() {
    let engine = engine_over(json!({
        "fast": {"plugin": "in-memory", "config": work()},
        "slow": {"plugin": "in-memory", "config": compensated(nothing_native())},
    }));

    let response = engine
        .tasks(&tasks(
            Filters::default(),
            ProjectSelector::Qualified("fast:P-1".parse().expect("a qualified id")),
            page(50),
        ))
        .await
        .expect("the query runs");

    assert_eq!(ids(&response.items), ["fast:T-1", "fast:T-2"]);
    assert_eq!(
        response.plan.per_source.len(),
        1,
        "no other source can hold a project of `fast`, so no other source is asked"
    );

    // A bare id is asked of every selected source instead.
    let bare = engine
        .tasks(&tasks(
            Filters::default(),
            ProjectSelector::Native(NativeId::from("P-1")),
            page(50),
        ))
        .await
        .expect("the query runs");
    assert_eq!(
        ids(&bare.items),
        ["fast:T-1", "slow:T-1", "fast:T-2", "slow:T-2"]
    );
    assert_eq!(bare.plan.per_source.len(), 2);
}

#[tokio::test]
async fn a_label_filter_may_ask_for_any_of_several_names() {
    // `any_of` is not reachable from the command line, where a repeated `--label`
    // narrows. It is part of the contract every SDK is generated against, so it is
    // proven here rather than left to a caller to discover.
    let engine = engine_over(json!({
        "slow": {"plugin": "in-memory", "config": compensated(nothing_native())},
    }));

    let response = engine
        .tasks(&tasks(
            Filters {
                labels: LabelFilter {
                    any_of: vec!["chore".to_owned(), "core".to_owned()],
                    none_of: vec!["bug".to_owned()],
                    ..LabelFilter::default()
                },
                ..Filters::default()
            },
            ProjectSelector::Any,
            page(50),
        ))
        .await
        .expect("the query runs");

    assert_eq!(ids(&response.items), ["slow:T-2", "slow:T-4"]);
}

/// A source that will not answer until another source has been asked too.
///
/// Instrumentation at the boundary rather than a stand-in for the engine: no plugin can
/// be made to pause, and pausing is the only way to observe from outside whether two
/// sources were consulted at once or one after the other.
struct Rendezvous {
    /// Both sources wait here; it releases only when both have arrived.
    barrier: Arc<Barrier>,
}

#[async_trait::async_trait]
impl TaskSource for Rendezvous {
    fn kind(&self) -> &'static str {
        "rendezvous"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            projects: Support::Native,
            documents: Support::Unsupported,
            comments: Support::Unsupported,
            assets: Support::Unsupported,
            priority: Support::Unsupported,
            filter_by_priority: Support::Unsupported,
            filter_by_comment_activity: Support::Unsupported,
            filter_by_metadata: Support::Unsupported,
            filter_by_origin: Support::Unsupported,
            reconcile_status: Support::Unsupported,
            complete_by_issue: Support::Unsupported,
            orphan_tasks: Support::Native,
            filter_by_label: Support::Native,
            filter_by_status: Support::Native,
            search_title: Support::Native,
            search_content: Support::Native,
            task_dependencies: DependencySupport::BothDirections,
            project_dependencies: DependencySupport::BothDirections,
            max_page_size: 10,
            assignees: Support::Unsupported,
        }
    }

    async fn health(&self) -> Result<Health, SourceError> {
        Ok(Health {
            reachable: true,
            detail: None,
        })
    }

    async fn get_task(&self, _id: &NativeId) -> Result<Option<Task>, SourceError> {
        Ok(None)
    }

    async fn get_project(&self, _id: &NativeId) -> Result<Option<Project>, SourceError> {
        Ok(None)
    }

    async fn query_tasks(
        &self,
        _query: &TaskQuery,
        _page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        self.barrier.wait().await;
        Ok(Page::last(vec![Task {
            version: None,
            id: NativeId::from("T-1"),
            key: None,
            title: "Waited".to_owned(),
            content: None,
            status: Status {
                category: StatusCategory::Todo,
                name: "Todo".to_owned(),
            },
            priority: Priority::None,
            labels: Vec::new(),
            project: None,
            url: None,
            location: None,
            created_at: None,
            updated_at: None,
            metadata: Default::default(),
            repositories: Vec::new(),
            delivers: Vec::new(),
            delivered_by: Vec::new(),
            classification: Default::default(),
            assignees: None,
        }]))
    }

    async fn query_projects(
        &self,
        _query: &ProjectQuery,
        _page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn labels(&self, _page: &PageRequest) -> Result<Page<Label>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn task_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn project_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page::last(Vec::new()))
    }
}

#[tokio::test]
async fn the_sources_are_consulted_at_once_rather_than_one_after_another() {
    // Each source waits at a two-party rendezvous before answering. An engine that asked
    // them one after the other would block on the first for ever, so the timeout below
    // is what turns "not concurrent" into a failed test rather than a hung suite.
    let barrier = Arc::new(Barrier::new(2));
    let sources: Vec<ConfiguredSource> = ["one", "two"]
        .into_iter()
        .map(|source| {
            ConfiguredSource::Ready(ResolvedSource::adopt(
                name(source),
                Box::new(Rendezvous {
                    barrier: Arc::clone(&barrier),
                }),
            ))
        })
        .collect();
    let engine = Engine::new(sources, vec![name("one"), name("two")]);

    let response = tokio::time::timeout(
        Duration::from_secs(10),
        engine.tasks(&tasks(Filters::default(), ProjectSelector::Any, page(10))),
    )
    .await
    .expect("the engine consults its sources at once; a sequential one never gets here")
    .expect("the query runs");

    assert_eq!(ids(&response.items), ["one:T-1", "two:T-1"]);
}

/// A source that answers every cursor with the cursor it was given.
///
/// No correct plugin does this, which is the point: an engine that trusted it would walk
/// for ever, and a command that hangs is worse than one that says what went wrong.
struct Stuck {
    /// Counted so the test can show the walk stopped rather than ran away.
    calls: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl TaskSource for Stuck {
    fn kind(&self) -> &'static str {
        "stuck"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            projects: Support::Native,
            documents: Support::Unsupported,
            comments: Support::Unsupported,
            assets: Support::Unsupported,
            priority: Support::Unsupported,
            filter_by_priority: Support::Unsupported,
            filter_by_comment_activity: Support::Unsupported,
            filter_by_metadata: Support::Unsupported,
            filter_by_origin: Support::Unsupported,
            reconcile_status: Support::Unsupported,
            complete_by_issue: Support::Unsupported,
            orphan_tasks: Support::Native,
            filter_by_label: Support::Native,
            filter_by_status: Support::Native,
            search_title: Support::Native,
            search_content: Support::Native,
            task_dependencies: DependencySupport::BothDirections,
            project_dependencies: DependencySupport::BothDirections,
            max_page_size: 10,
            assignees: Support::Unsupported,
        }
    }

    async fn health(&self) -> Result<Health, SourceError> {
        Ok(Health {
            reachable: true,
            detail: None,
        })
    }

    async fn get_task(&self, _id: &NativeId) -> Result<Option<Task>, SourceError> {
        Ok(None)
    }

    async fn get_project(&self, _id: &NativeId) -> Result<Option<Project>, SourceError> {
        Ok(None)
    }

    async fn query_tasks(
        &self,
        _query: &TaskQuery,
        page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(Page {
            items: Vec::new(),
            next: Some(page.cursor.clone().unwrap_or(Cursor(String::new()))),
        })
    }

    async fn query_projects(
        &self,
        _query: &ProjectQuery,
        _page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn labels(&self, _page: &PageRequest) -> Result<Page<Label>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn task_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn project_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page::last(Vec::new()))
    }
}

#[tokio::test]
async fn a_source_that_never_advances_its_cursor_is_reported_rather_than_walked_for_ever() {
    let calls = Arc::new(AtomicU32::new(0));
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name("stuck"),
            Box::new(Stuck {
                calls: Arc::clone(&calls),
            }),
        ))],
        vec![name("stuck")],
    );

    let response = tokio::time::timeout(
        Duration::from_secs(10),
        engine.tasks(&tasks(Filters::default(), ProjectSelector::Any, page(10))),
    )
    .await
    .expect("the walk stops instead of running away")
    .expect("the query runs");

    assert_eq!(response.errors.len(), 1);
    assert!(
        response.errors[0].error.to_string().contains(
            "the source returned the cursor it was given while one of its streams was \
             being walked"
        ),
        "{:?}",
        response.errors[0]
    );
    assert!(calls.load(Ordering::Relaxed) <= 3, "the walk stopped early");
}

/// A source that answers a listing by copy origin with every project it holds.
///
/// No correct plugin does this either: the method states it answers the projects copied from
/// that origin and none other. The engine checks the origin again over what came back, so a
/// source answering wider than it was asked cannot widen the caller's answer.
struct WiderByOrigin;

impl WiderByOrigin {
    /// Its three projects: one copied from `plan:P-1`, one from another origin, one from none.
    fn projects() -> Vec<Project> {
        [
            ("P-1", Some("plan:P-1")),
            ("P-2", Some("plan:P-2")),
            ("P-3", None),
        ]
        .into_iter()
        .map(|(id, origin)| {
            serde_json::from_value(json!({
                "id": id,
                "title": id,
                "status": {"category": "todo", "name": "Todo"},
                "labels": [],
                "metadata": origin
                    .map(|origin| json!({ TaskQuery::ORIGIN_KEY: origin }))
                    .unwrap_or_else(|| json!({})),
            }))
            .expect("a project")
        })
        .collect()
    }
}

#[async_trait::async_trait]
impl TaskSource for WiderByOrigin {
    fn kind(&self) -> &'static str {
        "wider-by-origin"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            filter_by_origin: Support::Native,
            ..Stuck {
                calls: Arc::new(AtomicU32::new(0)),
            }
            .capabilities()
        }
    }

    async fn health(&self) -> Result<Health, SourceError> {
        Ok(Health {
            reachable: true,
            detail: None,
        })
    }

    async fn get_task(&self, _id: &NativeId) -> Result<Option<Task>, SourceError> {
        Ok(None)
    }

    async fn get_project(&self, _id: &NativeId) -> Result<Option<Project>, SourceError> {
        Ok(None)
    }

    async fn query_tasks(
        &self,
        _query: &TaskQuery,
        _page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn query_projects(
        &self,
        _query: &ProjectQuery,
        _page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        Ok(Page::last(Self::projects()))
    }

    async fn query_projects_by_origin(
        &self,
        _origin: &str,
        _page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        Ok(Page::last(Self::projects()))
    }

    async fn labels(&self, _page: &PageRequest) -> Result<Page<Label>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn task_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn project_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page::last(Vec::new()))
    }
}

#[tokio::test]
async fn a_listing_by_origin_keeps_only_that_origins_projects_whatever_the_source_answers() {
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name("wide"),
            Box::new(WiderByOrigin),
        ))],
        vec![name("wide")],
    );
    let origin = GlobalId::new(name("plan"), NativeId("P-1".to_owned()));

    let copies = engine
        .projects_by_origin(
            &ProjectRequest {
                sources: Vec::new(),
                filters: Filters::default(),
                paging: page(10),
            },
            &origin,
        )
        .await
        .expect("the listing runs");

    assert_eq!(
        ids(&copies.items),
        ["wide:P-1"],
        "only the project copied from that origin is listed"
    );
    assert_eq!(
        entry(&copies.plan, "wide").pushed_down,
        [Predicate::Origin],
        "the origin was still asked of the source"
    );
}

/// A forward-only source whose forward edges answer every cursor with the cursor they
/// were given.
///
/// Answering `DependedOnBy` here is the engine's own bounded scan: it walks this source's
/// tasks and reads each one's forward edges, keeping the ones that point back. That inner
/// read is a pagination loop of its own, and it is the one loop no walk of a source's own
/// stream covers — the outer walk sees cursors that advance while the inner one is asked
/// the same cursor for ever.
struct StuckEdges;

#[async_trait::async_trait]
impl TaskSource for StuckEdges {
    fn kind(&self) -> &'static str {
        "stuck-edges"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            projects: Support::Native,
            documents: Support::Unsupported,
            comments: Support::Unsupported,
            assets: Support::Unsupported,
            priority: Support::Unsupported,
            filter_by_priority: Support::Unsupported,
            filter_by_comment_activity: Support::Unsupported,
            filter_by_metadata: Support::Unsupported,
            filter_by_origin: Support::Unsupported,
            reconcile_status: Support::Unsupported,
            complete_by_issue: Support::Unsupported,
            orphan_tasks: Support::Native,
            filter_by_label: Support::Native,
            filter_by_status: Support::Native,
            search_title: Support::Native,
            search_content: Support::Native,
            // Forward-only, which is what makes a reverse read a scan rather than a
            // question this source could answer itself.
            task_dependencies: DependencySupport::ForwardOnly,
            project_dependencies: DependencySupport::ForwardOnly,
            max_page_size: 10,
            assignees: Support::Unsupported,
        }
    }

    async fn health(&self) -> Result<Health, SourceError> {
        Ok(Health {
            reachable: true,
            detail: None,
        })
    }

    async fn get_task(&self, id: &NativeId) -> Result<Option<Task>, SourceError> {
        Ok(Some(one_task(id)))
    }

    async fn get_project(&self, _id: &NativeId) -> Result<Option<Project>, SourceError> {
        Ok(None)
    }

    async fn query_tasks(
        &self,
        _query: &TaskQuery,
        _page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        // One task, so the scan has exactly one item's forward edges to read.
        Ok(Page::last(vec![one_task(&NativeId::from("T-1"))]))
    }

    async fn query_projects(
        &self,
        _query: &ProjectQuery,
        _page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn labels(&self, _page: &PageRequest) -> Result<Page<Label>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn task_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page {
            items: Vec::new(),
            next: Some(page.cursor.clone().unwrap_or(Cursor("start".to_owned()))),
        })
    }

    async fn project_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page::last(Vec::new()))
    }
}

/// The one task [`StuckEdges`] holds, under whichever id it was asked for.
fn one_task(id: &NativeId) -> Task {
    Task {
        version: None,
        id: id.clone(),
        key: None,
        title: "Alpha".to_owned(),
        content: None,
        status: Status {
            category: StatusCategory::Todo,
            name: "Todo".to_owned(),
        },
        priority: Priority::None,
        labels: Vec::new(),
        project: None,
        url: None,
        location: None,
        created_at: None,
        updated_at: None,
        metadata: std::collections::BTreeMap::new(),
        repositories: Vec::new(),
        delivers: Vec::new(),
        delivered_by: Vec::new(),
        classification: Default::default(),
        assignees: None,
    }
}

#[tokio::test]
async fn a_source_that_never_advances_its_edge_cursor_stops_the_scan_that_answers_in_reverse() {
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name("scanned"),
            Box::new(StuckEdges),
        ))],
        vec![name("scanned")],
    );

    let response = tokio::time::timeout(
        Duration::from_secs(10),
        engine.task_dependencies(&DependencyRequest {
            id: "scanned:T-1".parse().expect("a qualified id"),
            direction: Direction::DependedOnBy,
            completeness: onetaskgraph_core::Completeness::NamingUnasked,
            paging: page(10),
        }),
    )
    .await
    .expect("the scan stops instead of running away")
    .expect("the walk runs");

    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert!(
        response.errors[0].error.to_string().contains(
            "the source returned the cursor it was given while its forward edges were \
             being scanned"
        ),
        "{:?}",
        response.errors[0]
    );
}

#[tokio::test]
async fn a_status_filter_is_pushed_down_or_narrowed_here_and_answers_the_same_either_way() {
    let engine = engine_over(json!({
        "fast": {"plugin": "in-memory", "config": work()},
        "slow": {"plugin": "in-memory", "config": compensated(nothing_native())},
    }));

    let response = engine
        .tasks(&tasks(
            Filters {
                statuses: vec![StatusCategory::Todo, StatusCategory::InProgress],
                ..Filters::default()
            },
            ProjectSelector::Any,
            page(50),
        ))
        .await
        .expect("the query runs");

    assert_eq!(
        ids(&response.items),
        [
            "fast:T-1", "slow:T-1", "fast:T-3", "slow:T-3", "fast:T-4", "slow:T-4"
        ]
    );
    assert_eq!(
        entry(&response.plan, "fast").pushed_down,
        [Predicate::Status]
    );
    assert_eq!(
        entry(&response.plan, "slow").applied_locally,
        [Predicate::Status]
    );
}

#[tokio::test]
async fn a_project_query_is_filtered_here_when_the_source_filters_nothing() {
    let engine = engine_over(json!({
        "slow": {"plugin": "in-memory", "config": compensated(nothing_native())},
    }));

    // Every project predicate at once, all of them left to the engine.
    let filtered = engine
        .projects(&ProjectRequest {
            sources: Vec::new(),
            filters: Filters {
                text: Some(TextQuery {
                    terms: "docs".to_owned(),
                    fields: TextFields::Title,
                }),
                labels: LabelFilter::default(),
                statuses: vec![StatusCategory::Todo],
            },
            paging: page(50),
        })
        .await
        .expect("the query runs");
    assert_eq!(ids(&filtered.items), ["slow:P-2"]);
    assert_eq!(
        entry(&filtered.plan, "slow").applied_locally,
        [Predicate::Status, Predicate::SearchTitle]
    );

    // A label predicate over projects narrows to nothing, because this fixture's
    // projects carry no labels — the wider set was returned and then narrowed.
    let labelled = engine
        .projects(&ProjectRequest {
            sources: Vec::new(),
            filters: Filters {
                labels: LabelFilter {
                    all_of: vec!["core".to_owned()],
                    ..LabelFilter::default()
                },
                ..Filters::default()
            },
            paging: page(50),
        })
        .await
        .expect("the query runs");
    assert!(labelled.items.is_empty());
    assert_eq!(
        entry(&labelled.plan, "slow").applied_locally,
        [Predicate::Label]
    );
}

#[tokio::test]
async fn a_search_covers_the_fields_and_the_entities_it_was_asked_for() {
    let engine = engine_over(json!({
        "fast": {"plugin": "in-memory", "config": work()},
        "slow": {"plugin": "in-memory", "config": compensated(nothing_native())},
    }));

    for (fields, kind, expected) in [
        (
            TextFields::Title,
            SearchKind::Tasks,
            vec!["fast:T-1", "slow:T-1"],
        ),
        (
            TextFields::Content,
            SearchKind::Tasks,
            vec!["fast:T-2", "slow:T-2"],
        ),
        (
            TextFields::TitleOrContent,
            SearchKind::Projects,
            vec!["fast:P-2", "slow:P-2"],
        ),
    ] {
        let response = engine
            .search(&SearchRequest {
                sources: Vec::new(),
                text: TextQuery {
                    terms: "alpha".to_owned(),
                    fields,
                },
                kind,
                paging: page(50),
            })
            .await
            .expect("the query runs");

        let found: Vec<String> = response
            .items
            .iter()
            .map(|hit| match hit {
                onetaskgraph_core::SearchHit::Task(task) => task.id.to_string(),
                onetaskgraph_core::SearchHit::Project(project) => project.id.to_string(),
            })
            .collect();
        assert_eq!(found, expected, "{fields:?} over {kind:?}");
    }
}

#[tokio::test]
async fn project_dependencies_walk_both_ways_and_the_reverse_is_emulated_when_it_has_to_be() {
    let engine = engine_over(json!({
        "native": {"plugin": "in-memory", "config": work()},
        "scanned": {"plugin": "in-memory", "config": compensated(nothing_native())},
    }));

    for source in ["native", "scanned"] {
        let forward = engine
            .project_dependencies(&DependencyRequest {
                id: format!("{source}:P-1").parse().expect("a qualified id"),
                direction: Direction::DependsOn,
                completeness: onetaskgraph_core::Completeness::NamingUnasked,
                paging: page(50),
            })
            .await
            .expect("the walk runs");
        assert_eq!(forward.items.len(), 1);
        assert_eq!(forward.items[0].to.to_string(), format!("{source}:P-2"));

        let reverse = engine
            .project_dependencies(&DependencyRequest {
                id: format!("{source}:P-2").parse().expect("a qualified id"),
                direction: Direction::DependedOnBy,
                completeness: onetaskgraph_core::Completeness::NamingUnasked,
                paging: page(50),
            })
            .await
            .expect("the walk runs");
        assert_eq!(reverse.items.len(), 1);
        assert_eq!(reverse.items[0].from.to_string(), format!("{source}:P-1"));

        let plan = entry(&reverse.plan, source);
        if source == "native" {
            assert_eq!(plan.pushed_down, [Predicate::ReverseDependencies]);
        } else {
            assert_eq!(plan.emulated, [Predicate::ReverseDependencies]);
        }
    }
}

#[tokio::test]
async fn showing_one_item_answers_it_or_says_plainly_that_there_is_none() {
    let engine = engine_over(json!({"work": {"plugin": "in-memory", "config": work()}}));

    let task = engine
        .task(&"work:T-1".parse().expect("a qualified id"))
        .await
        .expect("the query runs");
    assert_eq!(ids(&task.items), ["work:T-1"]);
    assert_eq!(entry(&task.plan, "work").pages_fetched, 1);

    let project = engine
        .project(&"work:P-1".parse().expect("a qualified id"))
        .await
        .expect("the query runs");
    assert_eq!(ids(&project.items), ["work:P-1"]);

    for missing in ["work:NOPE", "work:P-9"] {
        let id: GlobalId = missing.parse().expect("a qualified id");
        assert!(
            engine
                .task(&id)
                .await
                .expect("the query runs")
                .items
                .is_empty()
        );
        assert!(
            engine
                .project(&id)
                .await
                .expect("the query runs")
                .items
                .is_empty()
        );
    }
}

#[tokio::test]
async fn a_verb_addressed_at_a_source_that_never_built_reports_it_and_returns_nothing() {
    // Every single-source verb: the source is configured, so the id is not a typo, but it
    // could not be built — which is a failure to report, not a row to invent.
    let engine = engine_over(json!({"broken": {"plugin": "linear", "config": {}}}));
    let id: GlobalId = "broken:T-1".parse().expect("a qualified id");

    let task = engine.task(&id).await.expect("the query runs");
    assert!(task.items.is_empty());
    assert_eq!(task.errors.len(), 1);
    assert_eq!(task.errors[0].source.as_str(), "broken");
    assert!(task.plan.per_source.is_empty());

    let project = engine.project(&id).await.expect("the query runs");
    assert_eq!(project.errors.len(), 1);

    let deps = engine
        .task_dependencies(&DependencyRequest {
            id: id.clone(),
            direction: Direction::DependsOn,
            completeness: onetaskgraph_core::Completeness::NamingUnasked,
            paging: page(10),
        })
        .await
        .expect("the query runs");
    assert!(deps.items.is_empty());
    assert_eq!(deps.errors.len(), 1);

    let project_deps = engine
        .project_dependencies(&DependencyRequest {
            id,
            direction: Direction::DependedOnBy,
            completeness: onetaskgraph_core::Completeness::NamingUnasked,
            paging: page(10),
        })
        .await
        .expect("the query runs");
    assert_eq!(project_deps.errors.len(), 1);
}

#[tokio::test]
async fn a_dependency_walk_pages_like_every_other_walk() {
    let engine = engine_over(json!({"work": {"plugin": "in-memory", "config": work()}}));

    let mut seen = Vec::new();
    let mut token = None;
    loop {
        let response = engine
            .task_dependencies(&DependencyRequest {
                id: "work:T-2".parse().expect("a qualified id"),
                direction: Direction::DependedOnBy,
                completeness: onetaskgraph_core::Completeness::NamingUnasked,
                paging: match token {
                    None => page(1),
                    Some(token) => resumed(1, token),
                },
            })
            .await
            .expect("the walk runs");
        seen.extend(response.items.iter().map(|edge| edge.from.to_string()));
        token = response.next;
        if token.is_none() {
            break;
        }
        assert!(seen.len() < 10, "a three-edge walk must terminate");
    }
    assert_eq!(seen, ["work:T-1", "work:T-3", "work:T-4"]);
}

/// A source with more rows than any page, which records every page size it is asked for.
///
/// Instrumentation at the boundary, like [`Rendezvous`]: what a source is *asked* for is
/// the only thing about the engine's memory bound that is observable from outside it, and
/// no plugin reports that.
struct Recording {
    /// Every `limit` this source has been handed, in order.
    asked: Arc<std::sync::Mutex<Vec<u32>>>,
    /// The ceiling it declares.
    ceiling: u32,
    /// How many tasks it holds.
    rows: u32,
    /// How many rows beyond the page it was asked for it hands back anyway — a plugin
    /// defect the contract forbids and the engine must not simply absorb.
    overshoot: u32,
}

impl Recording {
    /// One page of `self.rows` tasks, starting at the decimal offset `cursor` encodes.
    fn slice(&self, page: &PageRequest) -> Page<Task> {
        self.asked
            .lock()
            .expect("no test panics here")
            .push(page.limit);
        let start: u32 = page.cursor.as_ref().map_or(0, |Cursor(raw)| {
            raw.parse().expect("this source's own cursor")
        });
        let end = start
            .saturating_add(page.limit.min(self.ceiling).saturating_add(self.overshoot))
            .min(self.rows);
        let items = (start..end)
            .map(|index| Task {
                version: None,
                id: NativeId::from(format!("T-{index}")),
                key: None,
                title: format!("task {index}"),
                content: None,
                status: Status {
                    category: StatusCategory::Todo,
                    name: "Todo".to_owned(),
                },
                priority: Priority::None,
                labels: Vec::new(),
                project: None,
                url: None,
                location: None,
                created_at: None,
                updated_at: None,
                metadata: Default::default(),
                repositories: Vec::new(),
                delivers: Vec::new(),
                delivered_by: Vec::new(),
                classification: Default::default(),
                assignees: None,
            })
            .collect();
        Page {
            items,
            next: (end < self.rows).then(|| Cursor(end.to_string())),
        }
    }
}

#[async_trait::async_trait]
impl TaskSource for Recording {
    fn kind(&self) -> &'static str {
        "recording"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            projects: Support::Native,
            documents: Support::Unsupported,
            comments: Support::Unsupported,
            assets: Support::Unsupported,
            priority: Support::Unsupported,
            filter_by_priority: Support::Unsupported,
            filter_by_comment_activity: Support::Unsupported,
            filter_by_metadata: Support::Unsupported,
            filter_by_origin: Support::Unsupported,
            reconcile_status: Support::Unsupported,
            complete_by_issue: Support::Unsupported,
            orphan_tasks: Support::Native,
            filter_by_label: Support::Native,
            filter_by_status: Support::Native,
            search_title: Support::Native,
            search_content: Support::Native,
            task_dependencies: DependencySupport::BothDirections,
            project_dependencies: DependencySupport::BothDirections,
            max_page_size: self.ceiling,
            assignees: Support::Unsupported,
        }
    }

    async fn health(&self) -> Result<Health, SourceError> {
        Ok(Health {
            reachable: true,
            detail: None,
        })
    }

    async fn get_task(&self, _id: &NativeId) -> Result<Option<Task>, SourceError> {
        Ok(None)
    }

    async fn get_project(&self, _id: &NativeId) -> Result<Option<Project>, SourceError> {
        Ok(None)
    }

    async fn query_tasks(
        &self,
        _query: &TaskQuery,
        page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        Ok(self.slice(page))
    }

    async fn query_projects(
        &self,
        _query: &ProjectQuery,
        _page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn labels(&self, _page: &PageRequest) -> Result<Page<Label>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn task_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page::last(Vec::new()))
    }

    async fn project_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page::last(Vec::new()))
    }
}

#[tokio::test]
async fn a_walk_asks_for_one_page_at_a_time_and_stops_when_the_callers_page_is_full() {
    // The observable half of "at most one source page plus the caller's page". A source
    // holding a thousand rows must not be drained to answer a request for five: the walk
    // asks for pages no larger than the source's own ceiling and stops as soon as it has
    // what the caller asked for.
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name("wide"),
            Box::new(Recording {
                asked: Arc::clone(&asked),
                ceiling: 3,
                rows: 1_000,
                overshoot: 0,
            }),
        ))],
        vec![name("wide")],
    );

    let response = engine
        .tasks(&tasks(Filters::default(), ProjectSelector::Any, page(5)))
        .await
        .expect("the query runs");

    assert_eq!(response.items.len(), 5);
    let asked = asked.lock().expect("no test panics here").clone();
    assert!(
        asked.iter().all(|limit| *limit <= 3),
        "no request may exceed the source's declared ceiling: {asked:?}"
    );
    assert_eq!(
        asked.len(),
        2,
        "five rows out of three-row pages is two pages, and then the walk stops: {asked:?}"
    );
    assert_eq!(
        entry(&response.plan, "wide").pages_fetched,
        2,
        "and the plan reports exactly what was pulled"
    );

    // Resuming reaches the source again and still walks one page at a time.
    let next = response.next.expect("a thousand rows do not fit in five");
    let resumed_response = engine
        .tasks(&tasks(
            Filters::default(),
            ProjectSelector::Any,
            resumed(5, next),
        ))
        .await
        .expect("the query runs");
    assert_eq!(
        ids(&resumed_response.items),
        (5..10)
            .map(|index| format!("wide:T-{index}"))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_page_token_this_configuration_cannot_honour_is_refused_for_what_it_says() {
    // `PageToken::parse` establishes only that a string is this engine's resume document.
    // Whether what the document *says* can be honoured needs the configuration, so it is
    // checked here — and each of these would otherwise resume half a walk and look like a
    // short page rather than a mistake.
    let engine = engine_over(json!({
        "work": {"plugin": "in-memory", "config": compensated(nothing_native())},
    }));

    for (document, problem) in [
        (
            json!([{"source": "elsewhere", "stream": "items"}]),
            "does not have",
        ),
        (
            json!([{"source": "work", "stream": "items", "skip": 999}]),
            "serves at most",
        ),
        (
            json!([
                {"source": "work", "stream": "items"},
                {"source": "work", "stream": "items", "skip": 1},
            ]),
            "two places to resume",
        ),
        // The one that would otherwise answer *successfully* and wrongly: a token a
        // search minted names streams `task list` does not read, so every source would
        // drop out of the walk and the page would come back empty and exhausted.
        (
            json!([{"source": "work", "stream": "tasks"}]),
            "this command does not read",
        ),
    ] {
        let raw = document_for_task_list(&engine, &document).await;
        let token = PageToken::parse(hex(&raw)).expect("it is this engine's own document");
        let Err(EngineError::Token { message }) = engine
            .tasks(&tasks(
                Filters::default(),
                ProjectSelector::Any,
                resumed(5, token),
            ))
            .await
        else {
            panic!("{raw} must be refused");
        };
        assert!(message.contains(problem), "{raw}: {message}");
    }
}

/// One string as the lower-case hex a page token is spelled in.
///
/// Here rather than reached for from the engine: a token is opaque by construction and
/// there is deliberately no public way to build one from parts, so a test that needs a
/// *specific* token spells it the way a caller pasting one off a terminal would.
fn hex(document: &str) -> String {
    document
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A token document whose streams are `streams` and whose query is the one `engine`
/// fingerprints for a plain `task list`.
///
/// The fingerprint is lifted from a token the engine really minted rather than computed
/// here, because computing it would mean a test reimplementing the thing under test — and
/// one that drifted would start passing for the wrong reason. What these tests are about
/// is the *streams* half of the document, so they need the query half to be the one the
/// query they resume actually carries.
async fn document_for_task_list(engine: &Engine, streams: &Value) -> String {
    let minted = engine
        .tasks(&tasks(Filters::default(), ProjectSelector::Any, page(1)))
        .await
        .expect("the query runs")
        .next
        .expect("the fixture holds more rows than one page");
    let decoded = String::from_utf8(
        minted
            .as_str()
            .as_bytes()
            .chunks(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("hex is ascii"), 16)
                    .expect("a token is hex")
            })
            .collect(),
    )
    .expect("a token's document is UTF-8");
    let query =
        serde_json::from_str::<Value>(&decoded).expect("a token's document is JSON")["query"]
            .clone();
    serde_json::to_string(&json!({"query": query, "streams": streams}))
        .expect("a resume document renders")
}

#[tokio::test]
async fn a_search_refuses_a_token_from_a_walk_over_entities_it_is_not_covering() {
    // The mirror of the case above. `--kind task` reading a token a `--kind both` walk
    // minted would silently drop the project half and report an exhausted walk.
    let engine = engine_over(json!({
        "work": {"plugin": "in-memory", "config": work()},
    }));
    let both = SearchRequest {
        sources: Vec::new(),
        text: TextQuery {
            terms: "alpha".to_owned(),
            fields: TextFields::TitleOrContent,
        },
        kind: SearchKind::Both,
        paging: page(1),
    };

    let first = engine.search(&both).await.expect("the query runs");
    let token = first.next.expect("more than one row matches");

    let narrowed = SearchRequest {
        kind: SearchKind::Tasks,
        paging: resumed(1, token.clone()),
        ..both.clone()
    };
    let Err(EngineError::Token { message }) = engine.search(&narrowed).await else {
        panic!("a token naming the project half must not be read by a task-only search");
    };
    assert!(message.contains("this command does not read"), "{message}");

    // And the same token against the same scope resumes, so the check refuses a mismatch
    // rather than every token.
    let resumed_both = SearchRequest {
        paging: resumed(1, token),
        ..both
    };
    let second = engine.search(&resumed_both).await.expect("the query runs");
    assert_eq!(second.items.len(), 1);
    assert_ne!(second.items, first.items);
}

#[tokio::test]
async fn a_source_that_serves_a_larger_page_than_it_was_asked_for_is_refused() {
    // "A source may return fewer, never more" is the contract's rule, and a source is
    // external code — a subprocess-hosted plugin is somebody else's program. An over-long
    // page breaks the one bound the engine holds by construction, so it is named as the
    // plugin defect it is rather than absorbed, and rather than truncated, which would
    // narrow the answer silently.
    let engine = Engine::new(
        vec![ConfiguredSource::Ready(ResolvedSource::adopt(
            name("greedy"),
            Box::new(Recording {
                asked: Arc::new(std::sync::Mutex::new(Vec::new())),
                ceiling: 3,
                rows: 1_000,
                overshoot: 2,
            }),
        ))],
        vec![name("greedy")],
    );

    let response = engine
        .tasks(&tasks(Filters::default(), ProjectSelector::Any, page(5)))
        .await
        .expect("the query runs");

    assert!(response.items.is_empty());
    assert_eq!(response.errors.len(), 1);
    assert!(
        response.errors[0]
            .error
            .to_string()
            .contains("may return fewer than it was asked for and never more"),
        "{:?}",
        response.errors[0]
    );
}
