//! The document read path is the engine's own, driven as a library call.
//!
//! Not through the command line, deliberately: this product is exposed three ways from one
//! engine, and a document read that lived in the binary would strand the Rust caller that
//! links the crate — and would answer a script and a shell differently the first time the
//! two drifted. So these drive `Engine::documents` and `Engine::document` directly, over
//! several configured sources at once, and a command line that reached plugins itself
//! could not make them pass.
//!
//! The journeys that prove the same behaviour end to end drive the compiled binary; see
//! `crates/onetaskgraph-e2e/tests/e2e/`.

use std::num::NonZeroU32;

use onetaskgraph_core::{
    Config, DocumentFilters, DocumentRequest, Engine, EngineError, GlobalId, Paging, Predicate,
    ProjectSelector, Qualified, QueryResponse, SourcePlan,
};
use onetaskgraph_plugin_api::{
    Document, LabelFilter, Location, NativeId, SecretResolver, SourceName, TextFields, TextQuery,
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

fn page(limit: u32) -> Paging {
    Paging {
        limit: NonZeroU32::new(limit).expect("a non-zero limit"),
        token: None,
    }
}

/// The documents every source below serves: one in each project, one in neither, and one
/// location of each shape so a consumer can be shown telling them apart.
fn documents() -> Value {
    json!([
        {"id": "D-1", "title": "Design review", "content": "the engine core, reviewed",
         "project": "P-1", "labels": [{"id": "L-1", "name": "spec"}],
         "location": {"url": "https://example.invalid/D-1"}},
        {"id": "D-2", "title": "Runbook", "content": "how to reach the design review",
         "project": "P-2", "labels": [{"id": "L-2", "name": "ops"}],
         "location": {"path": "/srv/notes/D-2.md"}},
        {"id": "D-3", "title": "Loose note", "content": "filed nowhere",
         "project": null, "labels": []}
    ])
}

fn work(capabilities: Value) -> Value {
    json!({
        "capabilities": capabilities,
        "projects": [
            {"id": "P-1", "title": "Engine", "content": null,
             "status": {"category": "in-progress", "name": "Doing"}, "labels": []},
            {"id": "P-2", "title": "Docs", "content": null,
             "status": {"category": "todo", "name": "Todo"}, "labels": []}
        ],
        "tasks": [],
        "documents": documents(),
    })
}

/// Three sources at once: one that applies every document predicate itself, one that
/// applies none of them, and one that has no documents at all.
fn engine() -> Engine {
    let sources = json!({
        "native": {"plugin": "in-memory", "config": work(json!({"documents": "native"}))},
        "compensated": {"plugin": "in-memory", "config": work(json!({
            "documents": "native",
            "filter_by_label": "unsupported",
            "search_title": "unsupported",
            "search_content": "unsupported",
            "max_page_size": 2,
        }))},
        // No `documents:` list and no declaration, which is a source with none.
        "documentless": {"plugin": "in-memory", "config": json!({
            "projects": [{"id": "P-1", "title": "Engine", "content": null,
                          "status": {"category": "todo", "name": "Todo"}, "labels": []}],
            "tasks": [],
        })},
    });
    let config = Config::from_document(json!({"sources": sources})).expect("a valid configuration");
    Engine::build(&config, &NoSecrets)
}

fn request(filters: DocumentFilters, project: ProjectSelector, limit: u32) -> DocumentRequest {
    DocumentRequest {
        sources: Vec::new(),
        filters,
        project,
        paging: page(limit),
    }
}

/// The qualified ids of one page, in the order the engine merged them.
fn ids(response: &QueryResponse<Qualified<Document>>) -> Vec<String> {
    response
        .items
        .iter()
        .map(|document| document.id.to_string())
        .collect()
}

/// One source's plan entry.
fn plan_for<'a>(response: &'a QueryResponse<Qualified<Document>>, source: &str) -> &'a SourcePlan {
    response
        .plan
        .per_source
        .iter()
        .find(|plan| plan.source == name(source))
        .unwrap_or_else(|| panic!("no plan entry for {source}: {:#?}", response.plan))
}

#[tokio::test]
async fn a_document_query_crosses_every_configured_source_at_once() {
    let response = engine()
        .documents(&request(
            DocumentFilters::default(),
            ProjectSelector::Any,
            20,
        ))
        .await
        .expect("the request addresses configured sources");

    // Both document-bearing sources answer, interleaved by the engine's own round-robin.
    assert_eq!(
        ids(&response),
        [
            "compensated:D-1",
            "native:D-1",
            "compensated:D-2",
            "native:D-2",
            "compensated:D-3",
            "native:D-3",
        ]
    );
    assert!(
        response.errors.is_empty(),
        "a source that holds no documents is not a source that failed: {:?}",
        response.errors
    );
}

#[tokio::test]
async fn a_source_that_declares_no_documents_is_left_unasked_rather_than_asked_and_excused() {
    let response = engine()
        .documents(&request(
            DocumentFilters::default(),
            ProjectSelector::Any,
            20,
        ))
        .await
        .expect("the request addresses configured sources");

    let unasked = plan_for(&response, "documentless");
    assert_eq!(
        unasked.unavailable,
        [Predicate::Document],
        "the plan says this source has no documents: {unasked:?}"
    );
    // Not asked, rather than asked and refused: a page fetched from it would be a read the
    // handshake already said would be refused.
    assert_eq!(unasked.pages_fetched, 0, "{unasked:?}");
    assert!(
        response.errors.is_empty(),
        "reporting it as holding none is not reporting it as having failed: {:?}",
        response.errors
    );
}

#[tokio::test]
async fn a_document_predicate_goes_down_to_the_source_that_applies_it_and_is_narrowed_here_otherwise()
 {
    let filters = DocumentFilters {
        labels: LabelFilter {
            all_of: vec!["spec".to_owned()],
            ..LabelFilter::default()
        },
        ..DocumentFilters::default()
    };
    let response = engine()
        .documents(&request(filters, ProjectSelector::Any, 20))
        .await
        .expect("the request addresses configured sources");

    // One correct answer, two plans: the same rows whoever applied the predicate.
    assert_eq!(ids(&response), ["compensated:D-1", "native:D-1"]);
    assert_eq!(
        plan_for(&response, "native").pushed_down,
        [Predicate::Label]
    );
    assert_eq!(
        plan_for(&response, "compensated").applied_locally,
        [Predicate::Label]
    );
    assert!(
        plan_for(&response, "compensated").pushed_down.is_empty(),
        "a predicate the source ignores is not also pushed to it"
    );
}

#[tokio::test]
async fn a_document_search_narrows_in_the_engine_for_a_source_that_searches_neither_field() {
    let filters = DocumentFilters {
        text: Some(TextQuery {
            terms: "design review".to_owned(),
            fields: TextFields::TitleOrContent,
        }),
        ..DocumentFilters::default()
    };
    let response = engine()
        .documents(&request(filters, ProjectSelector::Any, 20))
        .await
        .expect("the request addresses configured sources");

    // D-1 matches in its title and D-2 in its body, so a source that searched only one
    // half would come back short — which is why a half-capable source is not asked at all.
    assert_eq!(
        ids(&response),
        [
            "compensated:D-1",
            "native:D-1",
            "compensated:D-2",
            "native:D-2"
        ]
    );
    assert_eq!(
        plan_for(&response, "native").pushed_down,
        [Predicate::SearchTitle, Predicate::SearchContent]
    );
    assert_eq!(
        plan_for(&response, "compensated").applied_locally,
        [Predicate::SearchTitle, Predicate::SearchContent]
    );
}

#[tokio::test]
async fn a_document_list_narrows_to_one_project_and_to_the_documents_in_none() {
    let engine = engine();

    let filed = engine
        .documents(&request(
            DocumentFilters::default(),
            ProjectSelector::Qualified(GlobalId::new(name("native"), NativeId("P-2".to_owned()))),
            20,
        ))
        .await
        .expect("the request addresses configured sources");
    // A qualified project id names one project of one source, so no other source is asked.
    assert_eq!(ids(&filed), ["native:D-2"]);

    let orphans = engine
        .documents(&request(
            DocumentFilters::default(),
            ProjectSelector::Orphans,
            20,
        ))
        .await
        .expect("the request addresses configured sources");
    assert_eq!(ids(&orphans), ["compensated:D-3", "native:D-3"]);
}

#[tokio::test]
async fn one_document_is_read_by_its_qualified_id_and_carries_the_location_its_source_gave_it() {
    let engine = engine();

    let linked = engine
        .document(&GlobalId::new(name("native"), NativeId("D-1".to_owned())))
        .await
        .expect("the id names a configured source");
    assert_eq!(
        linked.items[0].item.location,
        Some(Location::Url("https://example.invalid/D-1".to_owned()))
    );

    let filed = engine
        .document(&GlobalId::new(name("native"), NativeId("D-2".to_owned())))
        .await
        .expect("the id names a configured source");
    assert_eq!(
        filed.items[0].item.location,
        Some(Location::Path("/srv/notes/D-2.md".to_owned()))
    );

    // A source with no documents holds none rather than failing, and says so in the plan.
    let none = engine
        .document(&GlobalId::new(
            name("documentless"),
            NativeId("D-1".to_owned()),
        ))
        .await
        .expect("the id names a configured source");
    assert!(none.items.is_empty() && none.errors.is_empty(), "{none:?}");
    assert_eq!(
        none.plan.per_source[0].unavailable,
        [Predicate::Document],
        "{:#?}",
        none.plan
    );
}

#[tokio::test]
async fn a_document_walk_pages_to_exhaustion_in_a_stable_order() {
    let engine = engine();
    let mut walked = Vec::new();
    let mut request = request(DocumentFilters::default(), ProjectSelector::Any, 2);
    loop {
        let response = engine
            .documents(&request)
            .await
            .expect("the request addresses configured sources");
        walked.extend(ids(&response));
        match response.next {
            Some(token) => request.paging.token = Some(token),
            None => break,
        }
    }
    assert_eq!(
        walked,
        [
            "compensated:D-1",
            "native:D-1",
            "compensated:D-2",
            "native:D-2",
            "compensated:D-3",
            "native:D-3",
        ]
    );
}

#[tokio::test]
async fn a_document_read_naming_a_source_nothing_configures_is_refused_before_anything_is_asked() {
    let refusal = engine()
        .document(&GlobalId::new(name("absent"), NativeId("D-1".to_owned())))
        .await
        .expect_err("an unconfigured source is not a source that holds none");
    let EngineError::UnknownSource { name: named, .. } = refusal else {
        panic!("an unconfigured source name is refused as one: {refusal:?}");
    };
    assert_eq!(named, "absent");
}
