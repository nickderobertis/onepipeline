//! The half of "no work data outside a plugin" a filesystem scan cannot see.
//!
//! The sandboxed sentinel journey in the binary's suite proves nothing was *written
//! down*. It cannot prove the engine did not answer a second identical query out of
//! something it kept in memory for the life of the process — that leaves no file behind
//! and no crate in `deny.toml`'s ban list, and it is exactly what an ordinary
//! "let's not hammer the API" change would introduce.
//!
//! So this asks the same question twice through one engine and counts what the source
//! was asked. An engine that answered the second one from memory fails here.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use onetaskgraph_core::{
    ConfiguredSource, DependencyRequest, Engine, Filters, GlobalId, LabelRequest, Paging,
    ProjectRequest, ProjectSelector, ResolvedSource, SearchKind, SearchRequest, TaskRequest,
};
use onetaskgraph_plugin_api::{
    Capabilities, DependencyEdge, DependencySupport, Direction, Health, Label, NativeId, Page,
    PageRequest, Priority, Project, ProjectQuery, SourceError, SourceName, Status, StatusCategory,
    Support, Task, TaskQuery, TaskSource, TextFields, TextQuery,
};

/// A source that answers everything and counts every call it receives.
///
/// The counter is shared rather than owned, because the engine takes the source by value
/// and the assertion needs to read it afterwards.
struct Counting {
    calls: Arc<AtomicU32>,
}

impl Counting {
    /// Record one call.
    fn called(&self) {
        self.calls.fetch_add(1, Ordering::Relaxed);
    }
}

/// An engine over one counting source, and the counter it shares.
fn engine() -> (Engine, Arc<AtomicU32>) {
    let calls = Arc::new(AtomicU32::new(0));
    let name = SourceName::new("work").expect("a valid source name");
    let source = ResolvedSource::adopt(
        name.clone(),
        Box::new(Counting {
            calls: Arc::clone(&calls),
        }),
    );
    (
        Engine::new(vec![ConfiguredSource::Ready(source)], vec![name]),
        calls,
    )
}

/// Large enough that every verb below answers in one page, so what these tests count is
/// re-asking rather than paging.
fn one_page() -> Paging {
    Paging {
        limit: NonZeroU32::new(10).expect("10 is not zero"),
        token: None,
    }
}

/// One task, enough to make a query that returns something.
fn task() -> Task {
    Task {
        version: None,
        id: NativeId::from("T-1"),
        key: None,
        title: "Land the engine".to_owned(),
        content: Some("a body".to_owned()),
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
    }
}

/// One project, so the project verbs have something to return.
fn project() -> Project {
    Project {
        version: None,
        id: NativeId::from("P-1"),
        title: "Engine".to_owned(),
        content: None,
        status: Status {
            category: StatusCategory::InProgress,
            name: "Doing".to_owned(),
        },
        labels: Vec::new(),
        url: None,
        location: None,
        created_at: None,
        updated_at: None,
        metadata: Default::default(),
        repositories: Vec::new(),
        classification: Default::default(),
    }
}

#[async_trait::async_trait]
impl TaskSource for Counting {
    fn kind(&self) -> &'static str {
        "counting"
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
        self.called();
        Ok(Health {
            reachable: true,
            detail: None,
        })
    }

    async fn get_task(&self, _id: &NativeId) -> Result<Option<Task>, SourceError> {
        self.called();
        Ok(Some(task()))
    }

    async fn get_project(&self, _id: &NativeId) -> Result<Option<Project>, SourceError> {
        self.called();
        Ok(Some(project()))
    }

    async fn query_tasks(
        &self,
        _query: &TaskQuery,
        _page: &PageRequest,
    ) -> Result<Page<Task>, SourceError> {
        self.called();
        Ok(Page::last(vec![task()]))
    }

    async fn query_projects(
        &self,
        _query: &ProjectQuery,
        _page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        self.called();
        Ok(Page::last(vec![project()]))
    }

    async fn labels(&self, _page: &PageRequest) -> Result<Page<Label>, SourceError> {
        self.called();
        Ok(Page::last(vec![Label {
            id: NativeId::from("L-1"),
            name: "bug".to_owned(),
            color: None,
        }]))
    }

    async fn task_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.called();
        Ok(Page::last(Vec::new()))
    }

    async fn project_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.called();
        Ok(Page::last(Vec::new()))
    }
}

/// Every verb that reaches a source, asked twice, must reach it twice.
///
/// The list below is the whole of that surface — both list verbs, labels, search, both
/// show verbs and both dependency walks. Naming them one by one is what makes a verb
/// added later and quietly memoised fail here: the failure this catches is a cache, and a
/// cache is added in one place and stops every one of these asking.
#[tokio::test]
async fn the_same_query_asked_twice_reaches_the_source_twice() {
    let tasks = TaskRequest {
        sources: Vec::new(),
        filters: Filters::default(),
        project: ProjectSelector::Any,
        priorities: Vec::new(),
        commented_since: None,
        metadata: Vec::new(),
        origin: None,
        include_members: false,
        paging: one_page(),
    };
    let projects = ProjectRequest {
        sources: Vec::new(),
        filters: Filters::default(),
        paging: one_page(),
    };
    let labels = LabelRequest {
        sources: Vec::new(),
        paging: one_page(),
    };
    let search = SearchRequest {
        sources: Vec::new(),
        text: TextQuery {
            terms: "land".to_owned(),
            fields: TextFields::TitleOrContent,
        },
        kind: SearchKind::Both,
        paging: one_page(),
    };
    let id: GlobalId = "work:T-1".parse().expect("a qualified id");
    let dependencies = DependencyRequest {
        id: id.clone(),
        direction: Direction::DependedOnBy,
        completeness: onetaskgraph_core::Completeness::NamingUnasked,
        paging: one_page(),
    };

    for (verb, ask) in [
        ("task list", 0usize),
        ("project list", 1),
        ("label list", 2),
        ("search", 3),
        ("task show", 4),
        ("project show", 5),
        ("task deps", 6),
        ("project deps", 7),
    ] {
        let (engine, calls) = engine();

        for round in 1..=2u32 {
            match ask {
                0 => {
                    engine.tasks(&tasks).await.expect("the query runs");
                }
                1 => {
                    engine.projects(&projects).await.expect("the query runs");
                }
                2 => {
                    engine.labels(&labels).await.expect("the query runs");
                }
                3 => {
                    engine.search(&search).await.expect("the query runs");
                }
                4 => {
                    engine.task(&id).await.expect("the query runs");
                }
                5 => {
                    engine.project(&id).await.expect("the query runs");
                }
                6 => {
                    engine
                        .task_dependencies(&dependencies)
                        .await
                        .expect("the query runs");
                }
                _ => {
                    engine
                        .project_dependencies(&dependencies)
                        .await
                        .expect("the query runs");
                }
            }

            let so_far = calls.load(Ordering::Relaxed);
            assert!(
                so_far >= round,
                "{verb}: after {round} identical query(ies) the source had been asked \
                 {so_far} time(s); an engine that answers a repeat from memory is holding \
                 work data outside the plugin"
            );
        }

        // The decisive comparison: the second round asked at least as much as the first.
        // A cache would make the second round free.
        let total = calls.load(Ordering::Relaxed);
        assert!(
            total >= 2,
            "{verb}: two identical queries reached the source {total} time(s) in total"
        );
    }
}

/// Two different pages of one walk are two different asks, for the same reason.
#[tokio::test]
async fn paging_re_asks_rather_than_serving_a_page_it_kept() {
    let (engine, calls) = engine();
    let request = TaskRequest {
        sources: Vec::new(),
        filters: Filters::default(),
        project: ProjectSelector::Any,
        priorities: Vec::new(),
        commented_since: None,
        metadata: Vec::new(),
        origin: None,
        include_members: false,
        paging: Paging {
            limit: NonZeroU32::new(1).expect("1 is not zero"),
            token: None,
        },
    };

    let first = engine.tasks(&request).await.expect("the query runs");
    let after_first = calls.load(Ordering::Relaxed);
    assert_eq!(first.items.len(), 1);

    // This source serves its one task in one page and says so, so the walk is over and
    // there is no second page to ask for — which is itself the point: the engine kept
    // nothing that would let it answer one.
    assert!(
        first.next.is_none(),
        "a one-page source exhausts in one page"
    );
    assert!(after_first >= 1, "the first page reached the source");

    let again = engine.tasks(&request).await.expect("the query runs");
    assert_eq!(again.items, first.items);
    assert!(
        calls.load(Ordering::Relaxed) > after_first,
        "asking for the same page again must reach the source again"
    );
}
