//! What the engine holds a source's relation answers to before it reports or writes anything:
//! one answer per task asked, each about the task it was asked for. A source answering
//! otherwise — too few or too many answers, or one task's relations under another's id — is
//! refused as malformed, through `task relations` and the link verbs alike, so no task's
//! content, links or version is lent to another.

use onetaskgraph_core::{
    ConfiguredSource, Engine, EngineError, GlobalId, LinkBatch, LinkItem, LinkOutcome,
    ResolvedSource,
};
use onetaskgraph_plugin_api::{
    BatchAnswer, BatchUpdate, Capabilities, DependencyEdge, DependencySupport, Direction, Health,
    Label, NativeId, Page, PageRequest, Priority, Project, ProjectQuery, Relations, SourceError,
    SourceName, Status, StatusCategory, Support, Task, TaskQuery, TaskSource, TaskUpdateOutcome,
    WriteSupport,
};

/// How a [`Bent`] source answers a relation read.
#[derive(Clone, Copy)]
enum Answer {
    /// One answer fewer than the ids asked.
    TooFew,
    /// One answer more than the ids asked.
    TooMany,
    /// Each id answered with another task's relations.
    Swapped,
    /// `A` answered with another task's relations, every other id rightly, and each endpoint
    /// write answered with the task it sent.
    OneAnother,
    /// Every relation read answered rightly, and each endpoint write answered `Written` with
    /// another task than the one it sent.
    WrittenAsAnother,
}

/// A source whose relation reads are bent as [`Answer`] says, and which answers nothing else.
struct Bent(Answer);

fn task(id: &str) -> Task {
    Task {
        version: Some(format!("v-{id}")),
        id: NativeId::from(id),
        key: None,
        title: id.to_owned(),
        content: Some(format!("The {id} body.")),
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

fn relations(id: &str) -> Option<Relations> {
    Some(Relations {
        task: task(id),
        parent: None,
        children: Vec::new(),
        children_complete: true,
    })
}

#[async_trait::async_trait]
impl TaskSource for Bent {
    fn kind(&self) -> &'static str {
        "bent"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            projects: Support::Unsupported,
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
            filter_by_label: Support::Unsupported,
            filter_by_status: Support::Unsupported,
            search_title: Support::Unsupported,
            search_content: Support::Unsupported,
            task_dependencies: DependencySupport::BothDirections,
            project_dependencies: DependencySupport::BothDirections,
            max_page_size: 10,
            assignees: Support::Unsupported,
        }
    }

    fn writes(&self) -> WriteSupport {
        WriteSupport::Supported
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
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }

    async fn query_projects(
        &self,
        _query: &ProjectQuery,
        _page: &PageRequest,
    ) -> Result<Page<Project>, SourceError> {
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }

    async fn labels(&self, _page: &PageRequest) -> Result<Page<Label>, SourceError> {
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }

    async fn task_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }

    async fn project_dependencies(
        &self,
        _id: &NativeId,
        _direction: Direction,
        _page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }

    async fn read_relations(
        &self,
        ids: &[NativeId],
    ) -> Vec<Result<Option<Relations>, SourceError>> {
        let mut answers: Vec<Result<Option<Relations>, SourceError>> =
            ids.iter().map(|id| Ok(relations(&id.0))).collect();
        match self.0 {
            Answer::TooFew => {
                answers.pop();
            }
            Answer::TooMany => answers.push(Ok(relations("extra"))),
            Answer::Swapped => answers.reverse(),
            Answer::OneAnother => {
                if let Some(at) = ids.iter().position(|id| id.0 == "A") {
                    answers[at] = Ok(relations("elsewhere"));
                }
            }
            Answer::WrittenAsAnother => {}
        }
        answers
    }

    async fn update_tasks(
        &self,
        updates: &[BatchUpdate],
    ) -> Vec<Result<BatchAnswer<TaskUpdateOutcome>, SourceError>> {
        updates
            .iter()
            .map(|update| {
                let answered = match self.0 {
                    Answer::OneAnother => update.id.0.clone(),
                    _ => format!("not-{}", update.id.0),
                };
                Ok(BatchAnswer::Written(TaskUpdateOutcome {
                    task: task(&answered),
                    written: Default::default(),
                    delivers_before: Vec::new(),
                }))
            })
            .collect()
    }
}

fn engine(answer: Answer) -> Engine {
    let name = SourceName::new("bent").expect("a valid source name");
    let source = ResolvedSource::adopt(name.clone(), Box::new(Bent(answer)));
    Engine::new(vec![ConfiguredSource::Ready(source)], vec![name])
}

fn id(native: &str) -> GlobalId {
    format!("bent:{native}").parse().expect("a qualified id")
}

#[tokio::test]
async fn a_relation_read_answered_with_another_count_or_another_task_is_refused_as_malformed() {
    for (answer, named) in [
        (Answer::TooFew, "1 relation reads for the 2 tasks asked"),
        (Answer::TooMany, "3 relation reads for the 2 tasks asked"),
        (
            Answer::Swapped,
            "asked for the relations of A, it answered with those of B",
        ),
    ] {
        let refused = engine(answer)
            .task_relations(&[id("A"), id("B")], false)
            .await
            .expect_err("a bent answer is refused");
        let EngineError::SourceFailed {
            error: SourceError::Malformed { message },
            ..
        } = &refused
        else {
            panic!("not refused as malformed: {refused:?}");
        };
        assert!(message.contains(named), "{message}");
    }
}

#[tokio::test]
async fn a_link_between_tasks_a_source_answers_for_wrongly_is_written_nowhere() {
    for answer in [Answer::TooFew, Answer::Swapped] {
        let written = engine(answer)
            .add_links(&LinkBatch {
                schema_version: 1,
                links: vec![LinkItem {
                    from: "bent:A".to_owned(),
                    to: "bent:B".to_owned(),
                    link_type: "caused-by".to_owned(),
                }],
            })
            .await
            .expect("the batch is answered");
        assert_eq!(written.results[0].outcome, LinkOutcome::Error);
        let reason = written.results[0].reason.as_deref().unwrap_or_default();
        assert!(reason.contains("bent"), "{reason}");
    }
}

#[tokio::test]
async fn an_endpoint_write_a_source_answers_with_another_task_is_never_answered_written() {
    let written = engine(Answer::WrittenAsAnother)
        .add_links(&LinkBatch {
            schema_version: 1,
            links: vec![LinkItem {
                from: "bent:A".to_owned(),
                to: "bent:B".to_owned(),
                link_type: "caused-by".to_owned(),
            }],
        })
        .await
        .expect("the batch is answered");
    let result = &written.results[0];
    assert_eq!(result.outcome, LinkOutcome::Error, "{result:?}");
    let reason = result.reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("answered the write of bent:A with the task not-A"),
        "{reason}"
    );
    assert!(reason.contains("not-B"), "{reason}");
}

#[tokio::test]
async fn a_relation_read_answering_one_id_with_another_task_fails_that_link_alone() {
    let written = engine(Answer::OneAnother)
        .add_links(&LinkBatch {
            schema_version: 1,
            links: vec![
                LinkItem {
                    from: "bent:A".to_owned(),
                    to: "bent:B".to_owned(),
                    link_type: "caused-by".to_owned(),
                },
                LinkItem {
                    from: "bent:C".to_owned(),
                    to: "bent:D".to_owned(),
                    link_type: "caused-by".to_owned(),
                },
            ],
        })
        .await
        .expect("the batch is answered");
    assert_eq!(written.results[0].outcome, LinkOutcome::Error);
    let reason = written.results[0].reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("asked for the relations of A, it answered with those of elsewhere"),
        "{reason}"
    );
    assert_eq!(
        written.results[1].outcome,
        LinkOutcome::Written,
        "{:?}",
        written.results[1]
    );
}
