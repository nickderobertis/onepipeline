//! The two halves of `docs/plugin-protocol.md`, driven against each other.
//!
//! Nothing here stands in for the layer under test. The engine's half is the real
//! [`SubprocessSource`]; the plugin's half is the real [`serve`]; between them is a real
//! operating-system pipe with real JSON lines on it. What the pipe buys over a spawned
//! program — which the end-to-end journeys use, and which is the shape a user gets — is
//! the ability to hold the *plugin's* end and answer badly on purpose, which is the only
//! way to prove the refusals §6 owes.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write, pipe};
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use onetaskgraph_core::{
    MAX_LINE, RequestDeadline, SubprocessConfig, SubprocessSource, plugin_for, serve,
};
use onetaskgraph_plugin_api::{
    AssetWrite, Cursor, Direction, Document, DocumentQuery, ItemKind, ItemWrite, LabelFilter,
    Location, MetadataKey, MetadataRecord, NativeId, PageRequest, Priority, ProjectFilter,
    ProjectQuery, SecretResolver, SourceError, SourceName, Status, StatusCategory, Support, Task,
    TaskQuery, TaskRef, TaskSource, WriteSupport,
};
use secrecy::SecretString;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// The work every test here serves, small enough to page through in one assertion.
fn dataset() -> Value {
    json!({
        "tasks": [
            {"id": "T-1", "title": "Alpha", "content": "the engine core",
             "status": {"category": "todo", "name": "Todo"},
             "labels": [{"id": "L-1", "name": "bug"}], "project": "P-1"},
            {"id": "T-2", "title": "Beta", "content": "alpha in the body",
             "status": {"category": "done", "name": "Shipped"}, "labels": []},
            {"id": "T-3", "title": "Gamma", "content": "unrelated",
             "status": {"category": "todo", "name": "Todo"},
             "labels": [{"id": "L-1", "name": "bug"}]}
        ],
        "projects": [
            {"id": "P-1", "title": "Engine", "content": "the engine",
             "status": {"category": "in-progress", "name": "Doing"}, "labels": []},
            {"id": "P-2", "title": "Docs", "content": "alpha docs",
             "status": {"category": "todo", "name": "Todo"}, "labels": []}
        ],
        "labels": [{"id": "L-1", "name": "bug"}, {"id": "L-2", "name": "chore"}],
        "task_dependencies": [
            {"from": "T-1", "to": "T-2", "kind": "blocks"},
            {"from": "T-3", "to": "T-2", "kind": "related"}
        ],
        "project_dependencies": [{"from": "P-1", "to": "P-2", "kind": "blocks"}]
    })
}

/// The settings this build's reference host takes: which plugin, and its own block.
fn hosted_settings() -> Value {
    let mut config = dataset();
    config["capabilities"] = json!({"max_page_size": 2});
    json!({"kind": "in-memory", "config": config})
}

/// A name every test configures its source under.
fn name() -> SourceName {
    SourceName::new("work").expect("a usable name")
}

/// A resolver with nothing in it.
struct NoSecrets;

impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

/// The same source this build would have used in process, for answers to be compared to.
fn in_process() -> Box<dyn TaskSource> {
    let settings = hosted_settings();
    plugin_for("in-memory")
        .expect("the in-memory plugin is registered")
        .build(&name(), &settings["config"], &NoSecrets)
        .expect("the dataset is a valid block")
}

/// A source at the other end of a pipe, served by [`serve`] over the same dataset.
fn a_process_away(settings: Value) -> Result<SubprocessSource, SourceError> {
    let (to_engine, from_plugin) = pipe().expect("a pipe");
    let (to_plugin, from_engine) = pipe().expect("a pipe");
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let _ = runtime.block_on(serve(BufReader::new(to_plugin), from_plugin));
    });
    SubprocessSource::over(from_engine, to_engine, &name(), &settings, BTreeMap::new())
}

/// A source at the other end of a peer that answers exactly these lines, in order.
///
/// The point of it is the answers a correct plugin never gives: a version it was not
/// asked for, an envelope with both members, an id nobody sent.
fn scripted(answers: Vec<String>) -> Result<SubprocessSource, SourceError> {
    let (to_engine, mut from_liar) = pipe().expect("a pipe");
    let (to_liar, from_engine) = pipe().expect("a pipe");
    std::thread::spawn(move || {
        let mut asked = BufReader::new(to_liar);
        for answer in answers {
            let mut request = String::new();
            match asked.read_line(&mut request) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if writeln!(from_liar, "{answer}").is_err() || from_liar.flush().is_err() {
                return;
            }
        }
    });
    SubprocessSource::over(from_engine, to_engine, &name(), &json!({}), BTreeMap::new())
}

/// One page request, asking for `limit` rows from the start.
fn page(limit: u32) -> PageRequest {
    PageRequest {
        cursor: None,
        limit,
    }
}

/// A task query with no predicates at all.
fn everything() -> TaskQuery {
    TaskQuery {
        text: None,
        labels: LabelFilter::default(),
        statuses: Vec::new(),
        project: ProjectFilter::Any,
        priorities: Vec::new(),
        commented_since: None,
        metadata: Vec::new(),
        origin: None,
    }
}

/// The line `serve` writes in answer to `requests`, one per request.
fn served(requests: &[Value]) -> Vec<Value> {
    let mut input = String::new();
    for request in requests {
        input.push_str(&serde_json::to_string(request).expect("a request"));
        input.push('\n');
    }
    let mut output: Vec<u8> = Vec::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    runtime
        .block_on(serve(input.as_bytes(), &mut output))
        .expect("the streams are in memory and cannot fail");
    String::from_utf8(output)
        .expect("responses are UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("every answer is one JSON object"))
        .collect()
}

/// An `initialize` request over this build's reference host settings.
fn handshake(version: u32, settings: Value) -> Value {
    json!({
        "id": "0",
        "method": "initialize",
        "params": {
            "protocol_version": version,
            "engine": {"name": "onetaskgraph", "version": "0.1.0"},
            "source_name": "work",
            "config": settings,
            "secrets": {}
        }
    })
}

/// The `kind` of the error an answer carries.
fn refusal(answer: &Value) -> &str {
    answer["error"]["kind"].as_str().expect("an error envelope")
}

/// The message of the error an answer carries.
fn because(answer: &Value) -> &str {
    answer["error"]["message"]
        .as_str()
        .expect("a message to read")
}

#[tokio::test]
async fn a_source_a_process_away_answers_what_the_same_source_answers_in_process() {
    let here = in_process();
    let there = a_process_away(hosted_settings()).expect("the handshake succeeds");

    assert_eq!(
        there.query_tasks(&everything(), &page(50)).await,
        here.query_tasks(&everything(), &page(50)).await
    );
    let projects = ProjectQuery {
        text: None,
        labels: LabelFilter::default(),
        statuses: Vec::new(),
    };
    assert_eq!(
        there.query_projects(&projects, &page(50)).await,
        here.query_projects(&projects, &page(50)).await
    );
    assert_eq!(there.labels(&page(50)).await, here.labels(&page(50)).await);
    assert_eq!(
        there.get_task(&NativeId("T-1".to_owned())).await,
        here.get_task(&NativeId("T-1".to_owned())).await
    );
    assert_eq!(
        there.get_project(&NativeId("P-1".to_owned())).await,
        here.get_project(&NativeId("P-1".to_owned())).await
    );
    assert_eq!(there.health().await, here.health().await);
}

#[tokio::test]
async fn a_write_crosses_the_wire_and_lands_in_the_source_on_the_other_side() {
    let there = a_process_away(hosted_settings()).expect("the handshake succeeds");
    // Read at the handshake, exactly as capabilities are, so the engine refuses a copy
    // into an unwritable plugin before it reads anything.
    assert_eq!(there.writes(), WriteSupport::Supported);

    let item: Task = serde_json::from_value(json!({
        "id": "T-9", "title": "Written across a pipe", "content": "body",
        "status": {"category": "todo", "name": "Todo"}, "labels": [],
        "metadata": {"caller.count": 3, "caller.shape": {"nested": [true, null]}},
        "repositories": ["github.com/nickderobertis/onetaskgraph"]
    }))
    .expect("a task");
    let written = there
        .write_task(&ItemWrite {
            target: None,
            item: item.clone(),
            depends_on: Vec::new(),
        })
        .await
        .expect("the plugin takes the write");
    assert_eq!(written, NativeId("T-9".to_owned()));

    // What a source can do must not change because it is a process away: the value and
    // the JSON type of every caller-defined key survive the boundary in both directions.
    let read = there
        .get_task(&written)
        .await
        .expect("the plugin answers")
        .expect("the task is there");
    assert_eq!(read.title, "Written across a pipe");
    assert_eq!(read.metadata["caller.count"], json!(3));
    assert_eq!(
        read.metadata["caller.shape"],
        json!({"nested": [true, null]})
    );

    let updated = there
        .write_project(&ItemWrite {
            target: Some(NativeId("P-1".to_owned())),
            item: serde_json::from_value(json!({
                "id": "P-1", "title": "Renamed across a pipe",
                "status": {"category": "todo", "name": "Todo"}, "labels": []
            }))
            .expect("a project"),
            depends_on: Vec::new(),
        })
        .await
        .expect("the plugin takes the write");
    assert_eq!(updated, NativeId("P-1".to_owned()));
    assert_eq!(
        there.get_project(&updated).await.unwrap().unwrap().title,
        "Renamed across a pipe"
    );

    // And a refusal is the plugin's own, carried whole rather than flattened.
    let Err(SourceError::Refused { message }) = there
        .write_task(&ItemWrite {
            target: Some(NativeId("absent".to_owned())),
            item,
            depends_on: Vec::new(),
        })
        .await
    else {
        panic!("a target the hosted source does not hold must be refused");
    };
    assert!(
        message.contains("names no task this source holds"),
        "{message}"
    );
}

#[test]
fn a_project_status_write_is_answered_with_the_status_itself_or_null() {
    // §4.17a: unlike a task's (§4.17), the result is the `Status` with no `status` member
    // around it, and `null` — present, not absent — for a project the plugin does not hold.
    let request = |id: &str, project: &str| {
        json!({"id": id, "method": "set_project_status",
               "params": {"id": project, "category": "done"}})
    };
    let answers = served(&[
        handshake(2, hosted_settings()),
        request("1", "P-1"),
        request("2", "nope"),
        request("3", "T-1"),
    ]);

    assert_eq!(answers[0]["result"]["project_status"], json!(true));
    assert_eq!(
        answers[1],
        json!({"id": "1", "result": {"category": "done", "name": "done"}})
    );
    assert_eq!(answers[2], json!({"id": "2", "result": null}));
    assert_eq!(
        answers[3],
        json!({"id": "3", "result": null}),
        "a task's id names no project"
    );
}

#[tokio::test]
async fn a_project_status_read_back_across_the_wire_is_the_status_or_none() {
    let there = a_process_away(hosted_settings()).expect("the handshake succeeds");
    let written = there
        .set_project_status(&NativeId("P-1".to_owned()), StatusCategory::Done)
        .await
        .expect("the plugin takes the write");
    assert_eq!(
        written,
        in_process()
            .set_project_status(&NativeId("P-1".to_owned()), StatusCategory::Done)
            .await
            .expect("the same write in process"),
    );
    assert_eq!(
        written.map(|status| status.category),
        Some(StatusCategory::Done)
    );
    assert_eq!(
        there
            .get_project(&NativeId("P-1".to_owned()))
            .await
            .unwrap()
            .unwrap()
            .status
            .category,
        StatusCategory::Done
    );
    assert_eq!(
        there
            .set_project_status(&NativeId("nope".to_owned()), StatusCategory::Done)
            .await,
        Ok(None),
        "a `null` result is no such project, not a protocol violation"
    );

    // A plugin answering the wrapped shape `set_task_status` uses is not answering §4.17a.
    let wrapped = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities(), "writes": "supported",
               "project_status": true}})
        .to_string(),
        json!({"id": "1", "result": {"status": {"category": "done", "name": "Done"}}}).to_string(),
    ])
    .expect("the handshake succeeds");
    let Err(SourceError::Malformed { message }) = wrapped
        .set_project_status(&NativeId("P-1".to_owned()), StatusCategory::Done)
        .await
    else {
        panic!("a wrapped status is not the shape §4.17a promises");
    };
    assert!(message.contains("set_project_status"), "{message}");
}

#[tokio::test]
async fn a_plugin_that_says_nothing_about_writing_is_read_as_one_that_cannot() {
    // §3.3: the member is optional, and absent means unsupported — which is what a
    // version-1 plugin written before there was a write side sends.
    let silent = scripted(vec![
        json!({"id":"0","result":{"protocol_version":2,"kind":"ancient","capabilities":{
            "projects":"native","orphan_tasks":"native","filter_by_label":"native",
            "filter_by_status":"native","search_title":"native","search_content":"native",
            "task_dependencies":"both-directions","project_dependencies":"both-directions",
            "max_page_size":10}}})
        .to_string(),
    ])
    .expect("the handshake succeeds");
    assert_eq!(silent.writes(), WriteSupport::Unsupported);
}

/// A handshake answer from a plugin that can be written through, with `stored_forms` as given.
fn writable_handshake(stored_forms: bool) -> String {
    let mut result = json!({"protocol_version":2,"kind":"rewriting","writes":"supported",
        "capabilities":{
            "projects":"native","orphan_tasks":"native","filter_by_label":"native",
            "filter_by_status":"native","search_title":"native","search_content":"native",
            "task_dependencies":"both-directions","project_dependencies":"both-directions",
            "max_page_size":10}});
    if stored_forms {
        result["stored_forms"] = json!(true);
    }
    json!({"id":"0","result":result}).to_string()
}

/// A task to write, holding `content`.
fn task_holding(content: &str) -> ItemWrite<Task> {
    ItemWrite {
        target: None,
        item: serde_json::from_value(json!({
            "id": "T-1", "title": "Rewritten", "content": content,
            "status": {"category": "done", "name": "done"}, "labels": []
        }))
        .expect("a task"),
        depends_on: Vec::new(),
    }
}

#[tokio::test]
async fn a_plugin_that_says_nothing_about_stored_forms_is_never_asked_and_its_answer_never_read() {
    // §3.16: absent means the plugin stores what it is given. The peer answers the handshake and
    // one write, and nothing else — so a `stored_status` sent to it would find no answer and
    // fail rather than read as the status given.
    let silent = scripted(vec![
        writable_handshake(false),
        r#"{"id":"1","result":{"id":"I-1","stored_content":"* one"}}"#.to_owned(),
    ])
    .expect("the handshake succeeds");
    let status = Status {
        category: StatusCategory::Done,
        name: "done".to_owned(),
    };
    assert_eq!(
        silent.stored_status(ItemKind::Task, &status).await.unwrap(),
        status
    );
    let id = silent.write_task(&task_holding("- one")).await.unwrap();
    assert_eq!(silent.stored_content(MetadataRecord::Task, &id), None);
}

#[tokio::test]
async fn a_plugin_declaring_stored_forms_is_asked_and_what_its_write_says_it_stores_is_kept_once() {
    // The peer answers `stored_status` and three writes, and nothing else: the retired
    // `stored_form` is never sent, so a request for it would find no answer and fail.
    let declaring = scripted(vec![
        writable_handshake(true),
        r#"{"id":"1","result":{"status":{"category":"done","name":"Shipped"}}}"#.to_owned(),
        r#"{"id":"2","result":{"id":"I-1","stored_content":"* one"}}"#.to_owned(),
        r#"{"id":"3","result":{"id":"I-2"}}"#.to_owned(),
        r#"{"id":"4","result":{"id":"I-3","stored_content":7}}"#.to_owned(),
    ])
    .expect("the handshake succeeds");
    let status = Status {
        category: StatusCategory::Done,
        name: "done".to_owned(),
    };
    assert_eq!(
        declaring
            .stored_status(ItemKind::Project, &status)
            .await
            .unwrap()
            .name,
        "Shipped"
    );
    let id = declaring.write_task(&task_holding("* one")).await.unwrap();
    assert_eq!(
        declaring
            .stored_content(MetadataRecord::Task, &id)
            .as_deref(),
        Some("* one")
    );
    // Taken: a second ask answers nothing until the next write says something.
    assert_eq!(declaring.stored_content(MetadataRecord::Task, &id), None);
    let quiet = declaring.write_task(&task_holding("* two")).await.unwrap();
    assert_eq!(declaring.stored_content(MetadataRecord::Task, &quiet), None);
    let Err(SourceError::Malformed { message }) =
        declaring.write_task(&task_holding("* three")).await
    else {
        panic!("a stored_content that is not a string is malformed");
    };
    assert!(message.contains("write_task"), "{message}");
}

#[tokio::test]
async fn an_asset_write_keeps_what_its_answer_says_is_stored_and_refuses_one_not_a_string() {
    // §4.9a: an asset-carrying write's answer is `{id, content}`, and §4.9 puts `stored_content`
    // beside them for a plugin declaring `stored_forms` — taken out before the rest is read, so
    // it is held to the same shape as an ordinary write's.
    let mut handshake: Value = serde_json::from_str(&writable_handshake(true)).expect("JSON");
    handshake["result"]["capabilities"]["assets"] = json!("native");
    let declaring = scripted(vec![
        handshake.to_string(),
        r#"{"id":"1","result":{"id":"I-1","content":"* one","stored_content":"* one"}}"#.to_owned(),
        r#"{"id":"2","result":{"id":"I-2","content":"* two","stored_content":["* two"]}}"#
            .to_owned(),
    ])
    .expect("the handshake succeeds");
    let assets = AssetWrite::default();
    let written = declaring
        .write_task_with_assets(&task_holding("* one"), None, &assets)
        .await
        .expect("a well-formed answer");
    assert_eq!(
        declaring
            .stored_content(MetadataRecord::Task, &written.id)
            .as_deref(),
        Some("* one")
    );
    let Err(SourceError::Malformed { message }) = declaring
        .write_task_with_assets(&task_holding("* two"), None, &assets)
        .await
    else {
        panic!("a stored_content that is not a string is malformed");
    };
    assert!(message.contains("write_task"), "{message}");
    assert_eq!(
        declaring.stored_content(MetadataRecord::Task, &NativeId("I-2".into())),
        None,
        "nothing is kept of an answer that was refused"
    );
}

#[test]
fn the_reference_host_answers_the_stored_forms_golden_exactly() {
    // The checked-in golden of the members added to version 2 without a bump (§6): this
    // build's own host speaks exactly those shapes, its handshake still version 2 — so a
    // version-2 engine and plugin keep shaking hands — and a write it has nothing to say about
    // carries no `stored_content` at all.
    let golden: Value = serde_json::from_str(include_str!("fixtures/stored-forms-wire.json"))
        .expect("the golden is JSON");
    let expected = &golden["reference_host"];
    let requests = &golden["requests"];
    let answers = served(&[
        handshake(2, hosted_settings()),
        json!({"id":"1","method":"stored_status","params":requests["stored_status"]}),
        json!({"id":"2","method":"stored_status","params":{
            "kind":"task","status":{"category":"done","name":"Finished"}}}),
        json!({"id":"3","method":"write_task","params":{"write":{"target":null,"item":{
            "id":"T-9","title":"Golden","content":"- one",
            "status":{"category":"todo","name":"Todo"},"labels":[]}}}}),
    ]);
    let mut initialize = answers[0]["result"].clone();
    initialize
        .as_object_mut()
        .expect("a handshake result")
        .remove("capabilities");
    assert_eq!(initialize, expected["initialize"]);
    assert_eq!(
        answers[1]["result"],
        json!({"status": {"category": "done", "name": "done"}})
    );
    assert_eq!(answers[2]["result"], expected["stored_status"]);
    assert_eq!(answers[3]["result"], expected["write_task"]);
}

#[test]
fn the_reference_host_declares_stored_forms_and_answers_stored_status_but_not_the_retired_stored_form()
 {
    let answers = served(&[
        handshake(2, hosted_settings()),
        json!({"id":"1","method":"stored_form","params":{"content":"- one"}}),
        json!({"id":"2","method":"stored_status","params":{
            "kind":"task","status":{"category":"done","name":"Finished"}}}),
    ]);
    assert_eq!(answers[0]["result"]["stored_forms"], json!(true));
    // `stored_form` is retired (§4.25): the host no longer answers it, as no engine sends it.
    assert!(answers[1].get("error").is_some(), "{}", answers[1]);
    assert_eq!(
        answers[2]["result"],
        json!({"status": {"category": "done", "name": "Finished"}})
    );
}

#[tokio::test]
async fn a_hosted_source_reports_its_own_kind_and_its_own_capabilities() {
    let there = a_process_away(hosted_settings()).expect("the handshake succeeds");

    // The transport is not the kind: what a caller sees is the source that answered, so a
    // plan naming `in-memory` is telling the truth about where the rows came from.
    assert_eq!(there.kind(), "in-memory");
    assert_eq!(there.capabilities(), in_process().capabilities());
    assert_eq!(there.capabilities().max_page_size, 2);
}

#[tokio::test]
async fn an_existing_stream_applies_its_deadline_after_initialization() {
    let (to_engine, mut from_plugin) = pipe().expect("a pipe");
    let (to_plugin, from_engine) = pipe().expect("a pipe");
    std::thread::spawn(move || {
        let mut requests = BufReader::new(to_plugin);
        let mut request = String::new();
        requests.read_line(&mut request).expect("the handshake");
        writeln!(
            from_plugin,
            "{}",
            json!({"id": "0", "result": {"protocol_version": 2,
                "kind": "silent", "capabilities": capabilities()}})
        )
        .expect("the handshake answer");
        from_plugin.flush().expect("the answer is visible");
        request.clear();
        requests
            .read_line(&mut request)
            .expect("the health request");
        std::thread::park();
    });
    let source = SubprocessSource::over_with_request_deadline(
        from_engine,
        to_engine,
        &name(),
        &json!({}),
        BTreeMap::new(),
        RequestDeadline::from_millis(NonZeroU64::new(20).expect("positive")),
    )
    .expect("the handshake succeeds");

    let SourceError::Unavailable { message } = source.health().await.expect_err("it times out")
    else {
        panic!("a request deadline is a reachability failure");
    };
    assert!(
        message.contains("health") && message.contains("20 milliseconds"),
        "{message}"
    );
    let again = Instant::now();
    assert!(
        matches!(
            source.labels(&page(1)).await,
            Err(SourceError::Unavailable { .. })
        ),
        "a later request is bounded too"
    );
    assert!(
        again.elapsed() < Duration::from_secs(10),
        "the later request hung"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_request_deadline_turns_a_silent_child_into_a_named_source_error() {
    // POSIX supplies one ubiquitous executable that can answer the real handshake and
    // then stay silent. Windows lacks an equivalent system program; the transport code
    // is platform-neutral and its functional journeys still run there.
    let answer = json!({"id": "0", "result": {"protocol_version": 2,
        "kind": "silent", "capabilities": capabilities()}})
    .to_string();
    // Twenty milliseconds bounds the request under test, against a child already running.
    // The handshake before it carries the child's own start-up — `/bin/sh` reaching its
    // first `read` — which a loaded host stretches well past that, so it takes the
    // protocol's default and the asserted deadline reaches the silent request alone.
    let source = SubprocessSource::connect_with_deadlines(
        "/bin/sh",
        &[
            "-c".to_owned(),
            "read -r _; printf '%s\\n' \"$1\"; read -r _; while :; do :; done".to_owned(),
            "_".to_owned(),
            answer,
        ],
        &name(),
        &json!({}),
        BTreeMap::new(),
        RequestDeadline::DEFAULT,
        RequestDeadline::from_millis(NonZeroU64::new(20).expect("positive")),
    )
    .expect("the handshake succeeds");

    let started = Instant::now();
    let SourceError::Unavailable { message } = source.health().await.expect_err("it times out")
    else {
        panic!("a deadline is a reachability failure");
    };
    assert!(
        message.contains("health") && message.contains("20 milliseconds"),
        "{message}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the deadline did not hang"
    );
    // An expired request kills the child this path owns, closing the connection for good; a
    // child left alive would answer every later call with a fresh timeout of its own. So
    // polling for a refusal that names no deadline is what tells the two apart while the
    // child takes its moment to die.
    let again = Instant::now();
    loop {
        let Err(SourceError::Unavailable { message }) = source.health().await else {
            panic!("the timed-out connection stays closed");
        };
        if !message.contains("did not answer") {
            break;
        }
        assert!(
            again.elapsed() < Duration::from_secs(5),
            "a later call kept waiting on a child the expired deadline should have \
             killed: {message}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_handshake_slower_than_the_request_deadline_still_connects() {
    // The test above asserts the short bound; this one asserts that it stops where the
    // handshake begins. Its child waits a whole second before reading — fifty times that
    // bound, and far past any scheduler delay — so the handshake succeeding is evidence
    // that the two spans are bounded apart, rather than evidence that this host was fast.
    // One deadline for both is what held a real child's start-up to twenty milliseconds.
    let answer = json!({"id": "0", "result": {"protocol_version": 2,
        "kind": "slow-to-start", "capabilities": capabilities()}})
    .to_string();
    let source = SubprocessSource::connect_with_deadlines(
        "/bin/sh",
        &[
            "-c".to_owned(),
            "sleep 1; read -r _; printf '%s\\n' \"$1\"; while :; do :; done".to_owned(),
            "_".to_owned(),
            answer,
        ],
        &name(),
        &json!({}),
        BTreeMap::new(),
        RequestDeadline::DEFAULT,
        RequestDeadline::from_millis(NonZeroU64::new(20).expect("positive")),
    )
    .expect("a handshake is not held to the request deadline");

    // The generous handshake replaced nothing: what comes after it is still held to twenty
    // milliseconds, against the same child that took a second to reach its first read.
    let SourceError::Unavailable { message } = source.health().await.expect_err("it times out")
    else {
        panic!("a deadline is a reachability failure");
    };
    assert!(
        message.contains("health") && message.contains("20 milliseconds"),
        "{message}"
    );
}

#[tokio::test]
async fn both_dependency_directions_cross_the_wire_unchanged() {
    let here = in_process();
    let there = a_process_away(hosted_settings()).expect("the handshake succeeds");

    for direction in [Direction::DependsOn, Direction::DependedOnBy] {
        assert_eq!(
            there
                .task_dependencies(&NativeId("T-2".to_owned()), direction, &page(50))
                .await,
            here.task_dependencies(&NativeId("T-2".to_owned()), direction, &page(50))
                .await,
            "task dependencies differ for {direction:?}"
        );
        assert_eq!(
            there
                .project_dependencies(&NativeId("P-2".to_owned()), direction, &page(50))
                .await,
            here.project_dependencies(&NativeId("P-2".to_owned()), direction, &page(50))
                .await,
            "project dependencies differ for {direction:?}"
        );
    }
}

#[tokio::test]
async fn a_walk_across_the_wire_pages_to_exhaustion_and_stops() {
    let there = a_process_away(hosted_settings()).expect("the handshake succeeds");
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = there
            .query_tasks(
                &everything(),
                &PageRequest {
                    cursor: cursor.clone(),
                    limit: 2,
                },
            )
            .await
            .expect("a page");
        seen.extend(page.items.into_iter().map(|task| task.id.0));
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, ["T-1", "T-2", "T-3"]);
}

#[tokio::test]
async fn a_predicate_crosses_the_wire_and_the_hosted_source_applies_it() {
    let there = a_process_away(hosted_settings()).expect("the handshake succeeds");
    let query = TaskQuery {
        text: None,
        labels: LabelFilter {
            any_of: vec!["bug".to_owned()],
            ..LabelFilter::default()
        },
        statuses: Vec::new(),
        project: ProjectFilter::Any,
        priorities: Vec::new(),
        commented_since: None,
        metadata: Vec::new(),
        origin: None,
    };

    let kept: Vec<String> = there
        .query_tasks(&query, &page(50))
        .await
        .expect("a page")
        .items
        .into_iter()
        .map(|task| task.id.0)
        .collect();
    assert_eq!(kept, ["T-1", "T-3"]);
}

#[tokio::test]
async fn an_id_that_names_nothing_is_null_rather_than_a_failure() {
    let there = a_process_away(hosted_settings()).expect("the handshake succeeds");

    assert_eq!(there.get_task(&NativeId("nope".to_owned())).await, Ok(None));
    assert_eq!(
        there.get_project(&NativeId("nope".to_owned())).await,
        Ok(None)
    );
}

#[test]
fn a_protocol_version_this_plugin_does_not_know_is_refused_by_name() {
    let answers = served(&[handshake(3, hosted_settings())]);

    assert_eq!(
        answers.len(),
        1,
        "the plugin exits after refusing: {answers:?}"
    );
    assert_eq!(refusal(&answers[0]), "config");
    assert!(
        because(&answers[0]).contains("version 3") && because(&answers[0]).contains("version 2"),
        "the refusal names both versions: {}",
        because(&answers[0])
    );
}

#[test]
fn a_request_before_the_handshake_is_refused_rather_than_answered() {
    let answers =
        served(&[json!({"id": "1", "method": "labels", "params": {"page": {"limit": 5}}})]);

    assert_eq!(refusal(&answers[0]), "malformed");
    assert!(
        because(&answers[0]).contains("before the handshake"),
        "{}",
        because(&answers[0])
    );
}

#[test]
fn a_plugin_declaring_no_path_accepts_a_document_directory_and_answers_as_it_did() {
    let mut located = handshake(2, hosted_settings());
    located["params"]["document_dir"] = json!(std::env::temp_dir());
    let answers = served(&[
        located,
        json!({"id": "1", "method": "get_task", "params": {"id": "T-1"}}),
    ]);

    assert!(answers[0]["result"].is_object(), "{answers:?}");
    assert_eq!(
        answers[1]["result"]["task"]["title"], "Alpha",
        "{answers:?}"
    );
}

#[test]
fn a_document_directory_that_is_not_absolute_is_refused_at_the_handshake() {
    let mut located = handshake(2, hosted_settings());
    located["params"]["document_dir"] = json!("relative/to/nothing");
    let answers = served(&[located]);

    assert_eq!(refusal(&answers[0]), "config", "{answers:?}");
    assert!(
        because(&answers[0]).contains("relative/to/nothing")
            && because(&answers[0]).contains("not an absolute path"),
        "{}",
        because(&answers[0])
    );
}

#[test]
fn a_second_handshake_on_one_connection_is_refused() {
    let answers = served(&[
        handshake(2, hosted_settings()),
        handshake(2, hosted_settings()),
    ]);

    assert!(answers[0]["result"].is_object(), "{answers:?}");
    assert_eq!(refusal(&answers[1]), "malformed");
    assert!(
        because(&answers[1]).contains("already initialized"),
        "{}",
        because(&answers[1])
    );
}

#[test]
fn a_line_with_no_request_id_is_skipped_and_the_next_request_still_answered() {
    let mut input = String::from("this is not JSON at all\n");
    input.push_str("{\"method\":\"labels\"}\n");
    input.push('\n');
    input.push_str(&serde_json::to_string(&handshake(2, hosted_settings())).expect("a request"));
    input.push('\n');
    let mut output: Vec<u8> = Vec::new();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(serve(input.as_bytes(), &mut output))
        .expect("in-memory streams cannot fail");

    let answered = String::from_utf8(output).expect("UTF-8");
    assert_eq!(
        answered.lines().count(),
        1,
        "only the handshake is answerable: {answered}"
    );
    assert!(answered.contains("\"protocol_version\":2"), "{answered}");
}

#[test]
fn a_line_that_is_addressed_but_is_not_a_request_is_answered_rather_than_dropped() {
    let answers = served(&[json!({"id": "9", "method": 7})]);

    assert_eq!(answers[0]["id"], "9");
    assert_eq!(refusal(&answers[0]), "malformed");
    assert!(
        because(&answers[0]).contains("not a request envelope"),
        "{}",
        because(&answers[0])
    );
}

#[test]
fn a_method_this_version_does_not_have_is_refused_by_name() {
    let answers = served(&[
        handshake(2, hosted_settings()),
        json!({"id": "1", "method": "delete_everything", "params": {}}),
    ]);

    assert_eq!(refusal(&answers[1]), "malformed");
    assert!(
        because(&answers[1]).contains("delete_everything"),
        "{}",
        because(&answers[1])
    );
}

#[test]
fn parameters_of_the_wrong_shape_are_refused_naming_the_method() {
    let answers = served(&[
        handshake(2, hosted_settings()),
        json!({"id": "1", "method": "get_task", "params": {}}),
    ]);

    assert_eq!(refusal(&answers[1]), "malformed");
    assert!(
        because(&answers[1]).contains("get_task"),
        "{}",
        because(&answers[1])
    );
}

#[test]
fn settings_naming_no_plugin_of_this_build_are_refused_with_the_kinds_it_knows() {
    let answers = served(&[handshake(2, json!({"kind": "jira", "config": {}}))]);

    assert_eq!(refusal(&answers[0]), "config");
    assert!(
        because(&answers[0]).contains("jira") && because(&answers[0]).contains("in-memory"),
        "{}",
        because(&answers[0])
    );
}

#[test]
fn settings_this_host_cannot_read_are_refused_for_what_they_are_missing() {
    let answers = served(&[handshake(2, json!({"config": {}}))]);

    assert_eq!(refusal(&answers[0]), "config");
    assert!(
        because(&answers[0]).contains("kind"),
        "{}",
        because(&answers[0])
    );
}

#[test]
fn a_hosted_plugin_that_refuses_to_build_answers_with_its_own_error() {
    let answers = served(&[handshake(2, json!({"kind": "linear", "config": {}}))]);

    // Linear has no forwarded credential, proving a build failure crosses the wire as
    // the plugin's own authentication error rather than as a transport failure.
    assert!(answers[0]["error"].is_object(), "{answers:?}");
    assert!(
        because(&answers[0]).contains("LINEAR_API_KEY"),
        "{}",
        because(&answers[0])
    );
}

#[test]
fn a_plugin_answering_in_a_version_the_engine_did_not_ask_for_is_refused() {
    let error = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 3, "kind": "made-up",
               "capabilities": capabilities()}})
        .to_string(),
    ])
    .expect_err("a version the engine does not speak");

    let SourceError::Config { message } = error else {
        panic!("a version disagreement is a configuration refusal: {error:?}");
    };
    assert!(
        message.contains("version 2") && message.contains("version 3"),
        "{message}"
    );
}

#[test]
fn a_plugin_that_omits_the_protocol_version_is_refused_rather_than_assumed() {
    let error = scripted(vec![
        json!({"id": "0", "result": {"kind": "made-up", "capabilities": capabilities()}})
            .to_string(),
    ])
    .expect_err("an unstated version");

    let SourceError::Config { message } = error else {
        panic!("an unstated version is a configuration refusal: {error:?}");
    };
    assert!(message.contains("did not say"), "{message}");
}

#[test]
fn a_handshake_answer_that_is_not_an_initialize_result_is_malformed() {
    let error = scripted(vec![json!({"id": "0", "result": {"kind": 7}}).to_string()])
        .expect_err("a result of the wrong shape");

    assert!(matches!(error, SourceError::Malformed { .. }), "{error:?}");
}

#[test]
fn a_plugin_kind_with_no_name_is_rejected_at_the_handshake_boundary() {
    for kind in ["", " \t"] {
        let error = scripted(vec![
            json!({"id": "0", "result": {"protocol_version": 2, "kind": kind,
                   "capabilities": capabilities()}})
            .to_string(),
        ])
        .expect_err("a plugin kind must name something");

        let SourceError::Malformed { message } = error else {
            panic!("an invalid handshake field is malformed: {error:?}");
        };
        assert!(message.contains("plugin kind"), "{message}");
    }
}

#[test]
fn a_handshake_answer_carrying_both_members_is_a_protocol_violation() {
    let error = scripted(vec![
        json!({"id": "0", "result": {}, "error": {"kind": "auth", "message": "no"}}).to_string(),
    ])
    .expect_err("both members at once");

    let SourceError::Malformed { message } = error else {
        panic!("both members at once is a violation: {error:?}");
    };
    assert!(message.contains("both a result and an error"), "{message}");
}

#[test]
fn a_handshake_the_plugin_refuses_is_reported_as_the_plugin_worded_it() {
    let error = scripted(vec![
        json!({"id": "0", "error": {"kind": "auth", "message": "the token expired"}}).to_string(),
    ])
    .expect_err("a refused handshake");

    assert_eq!(
        error,
        SourceError::Auth {
            message: "the token expired".to_owned()
        }
    );
}

#[test]
fn a_plugin_that_says_nothing_at_all_is_reported_as_unreachable() {
    let error = scripted(Vec::new()).expect_err("nothing was answered");

    assert!(
        matches!(error, SourceError::Unavailable { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_plugin_answering_an_id_nobody_asked_is_a_protocol_violation() {
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities()}})
        .to_string(),
        json!({"id": "999", "result": {"reachable": true}}).to_string(),
    ])
    .expect("the handshake succeeds");

    let SourceError::Malformed { message } = source.health().await.expect_err("a wrong id") else {
        panic!("an id nobody sent is a violation");
    };
    assert!(message.contains("999"), "{message}");
}

#[tokio::test]
async fn a_plugin_line_that_is_not_json_is_quoted_back_at_a_readable_length() {
    let long = "x".repeat(500);
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities()}})
        .to_string(),
        long,
    ])
    .expect("the handshake succeeds");

    let SourceError::Malformed { message } = source.health().await.expect_err("not JSON") else {
        panic!("a line that is not JSON is a violation");
    };
    assert!(message.contains("(truncated)"), "{message}");
    assert!(message.len() < 500, "the quote is cut short: {message}");
}

#[tokio::test]
async fn a_delete_result_is_read_as_an_object_rather_than_as_exactly_the_empty_one() {
    // §4.10 answers `{}`, and §2.1 is what says how strictly to read that: a reader ignores
    // members it does not know, at every level, so a later version can add an optional one
    // without a version bump. A decoder refusing the unknown member here would refuse that
    // plugin outright — and would be the only one of this boundary that did. What is
    // refused is an answer that is not an object at all, which no version of §4.10 permits.
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities()}})
        .to_string(),
        json!({"id": "1", "result": {"removed_at": "2026-08-30T00:00:00Z"}}).to_string(),
        json!({"id": "2", "result": "done"}).to_string(),
    ])
    .expect("the handshake succeeds");

    source
        .delete_task(&NativeId("T-1".to_owned()))
        .await
        .expect("a member this version does not know is ignored, not refused");

    let SourceError::Malformed { message } = source
        .delete_project(&NativeId("P-1".to_owned()))
        .await
        .expect_err("a result that is not an object")
    else {
        panic!("a delete result that is not an object is a protocol violation");
    };
    assert!(message.contains("delete_project"), "{message}");
}

#[tokio::test]
async fn a_task_detail_whose_members_contradict_each_other_is_a_protocol_violation() {
    // §4.4a answers a task with exactly one of a page and a comment refusal, or no task and
    // neither. Every other shape decodes — each member is optional on the wire — and would
    // otherwise be read as something §4.4a never says: comments for no task, or a task whose
    // comments were neither read nor refused, which a copy would take as having none.
    let task = json!({"id": "T-1", "title": "Alpha", "content": "",
                      "status": {"category": "todo", "name": "Todo"}, "labels": []});
    let comments = json!({"items": [], "next": null});
    let refused = json!({"kind": "refused", "message": "a draft has no comments"});
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities(), "task_details": true}})
        .to_string(),
        json!({"id": "1", "result": {"task": task, "page": comments}}).to_string(),
        json!({"id": "2", "result": {"task": task, "comments_error": refused}}).to_string(),
        json!({"id": "3", "result": {"task": null}}).to_string(),
        json!({"id": "4", "result": {"task": null, "page": comments}}).to_string(),
        json!({"id": "5", "result": {"task": task}}).to_string(),
        json!({"id": "6", "result": {"task": task, "page": comments, "comments_error": refused}})
            .to_string(),
    ])
    .expect("the handshake succeeds");
    let ids = [NativeId("T-1".to_owned())];
    let mut read = Vec::new();
    for _ in 0..6 {
        read.extend(source.get_task_details(&ids, Some(&page(50))).await);
    }

    let read_whole = read[0].as_ref().expect("a task and its page").as_ref();
    assert!(
        read_whole.is_some_and(|detail| matches!(detail.comments, Some(Ok(Some(_))))),
        "{read_whole:?}"
    );
    let read_refused = read[1].as_ref().expect("a task and its refusal").as_ref();
    assert!(
        read_refused.is_some_and(|detail| matches!(
            detail.comments,
            Some(Err(SourceError::Refused { .. }))
        )),
        "{read_refused:?}"
    );
    assert!(matches!(read[2], Ok(None)), "{:?}", read[2]);
    for (answer, named) in [
        (&read[3], "comments for no task"),
        (&read[4], "neither a page nor a comment refusal"),
        (&read[5], "both a page and a comment refusal"),
    ] {
        let Err(SourceError::Malformed { message }) = answer else {
            panic!("expected a protocol violation naming {named:?}, got {answer:?}");
        };
        assert!(message.contains(named), "{message}");
    }
}

#[tokio::test]
async fn a_task_detail_about_another_task_than_the_one_asked_for_is_a_protocol_violation() {
    let task = |id: &str| {
        json!({"id": id, "title": "Alpha", "content": "",
               "status": {"category": "todo", "name": "Todo"}, "labels": []})
    };
    let comments = json!({"items": [], "next": null});
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities(), "task_details": true}})
        .to_string(),
        json!({"id": "1", "result": {"task": task("T-2"), "page": comments}}).to_string(),
        json!({"id": "2", "result": {"task": task("T-1"), "page": comments}}).to_string(),
    ])
    .expect("the handshake succeeds");
    let ids = [NativeId("T-1".to_owned())];

    let other = source.get_task_details(&ids, Some(&page(50))).await;
    let Err(SourceError::Malformed { message }) = &other[0] else {
        panic!("an answer about T-2 is not T-1's: {other:?}");
    };
    assert!(
        message.contains("asked for task T-1 and answered with task T-2"),
        "{message}"
    );
    let asked = source.get_task_details(&ids, Some(&page(50))).await;
    assert!(
        matches!(&asked[0], Ok(Some(detail)) if detail.task.id == ids[0]),
        "{asked:?}"
    );
}

#[tokio::test]
async fn a_comment_copy_naming_no_task_is_refused_before_an_older_peer_is_sent_anything() {
    // A peer whose handshake predates `comment_copies` is handed a copy as a marked comment
    // through `add_comment`; an origin the engine would read back as none is refused first, so
    // the peer — which answers the handshake and nothing else — is never asked.
    let mut declared = capabilities();
    declared["comments"] = json!("native");
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": declared, "writes": "supported"}})
        .to_string(),
    ])
    .expect("the handshake succeeds");
    let copy = onetaskgraph_plugin_api::CommentCopy {
        body: onetaskgraph_plugin_api::CommentBody::new("Copied text.").unwrap(),
        origin: onetaskgraph_plugin_api::CommentOrigin {
            task: "I_kw1".to_owned(),
            id: NativeId("IC_7".to_owned()),
            author: None,
            created_at: None,
            updated_at: None,
            url: None,
        },
    };
    let refused = source
        .write_comment_copy(&NativeId("T-1".to_owned()), None, &copy)
        .await
        .expect_err("an origin naming no task is never written");
    assert!(
        matches!(&refused, SourceError::Malformed { message } if message.contains("qualified task id")),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_document_refusal_crosses_the_wire_as_the_hosted_plugin_s_own_reason() {
    // Both halves of §4.11 and §4.12, driven against each other over a real pipe. The
    // engine's side forwards these four the way it forwards every other method, and the
    // reason that comes back is the *hosted* plugin's — `in-memory`, not `subprocess` —
    // because this side holds no opinion about documents any more than it holds one about
    // anything else.
    let source = a_process_away(hosted_settings()).expect("the handshake succeeds");

    // What the engine reads once, at the handshake, and never asks again.
    assert_eq!(source.capabilities().documents, Support::Unsupported);

    for refusal in [
        source
            .get_document(&NativeId("D-1".to_owned()))
            .await
            .map(|found| format!("{found:?}")),
        source
            .query_documents(&DocumentQuery::default(), &page(2))
            .await
            .map(|answered| format!("{answered:?}")),
    ] {
        let Err(SourceError::Refused { message }) = refusal else {
            panic!("a source with no documents refuses a document read: {refusal:?}");
        };
        assert_eq!(message, "the in-memory plugin has no documents");
    }

    // The two writes refuse for the same reason the two reads do, and not for the write
    // one: this hosted source *is* writable, so "cannot be written" would be false — it
    // has no document side to write into, which is what it says.
    assert!(source.writes().is_supported());
    for refusal in [
        source
            .write_document(&ItemWrite {
                target: None,
                item: filed(),
                depends_on: Vec::new(),
            })
            .await
            .map(|id| format!("{id:?}")),
        source
            .delete_document(&NativeId("D-1".to_owned()))
            .await
            .map(|()| String::new()),
    ] {
        let Err(SourceError::Refused { message }) = refusal else {
            panic!("a source with no document side refuses a document write: {refusal:?}");
        };
        assert_eq!(message, "the in-memory plugin has no documents");
    }
}

#[tokio::test]
async fn a_document_a_peer_really_answers_with_crosses_the_wire_whole() {
    // The refusal is what every source of this build gives, so it is what the shared
    // journey drives. This is the other half: a peer that *does* have documents, answering
    // all four methods successfully over a real pipe, so the engine's own encoding and
    // decoding of them is exercised rather than assumed. Nothing registered here can play
    // that peer yet — no plugin implements documents — which is exactly why it is scripted.
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": documentary(), "writes": "supported"}})
        .to_string(),
        json!({"id": "1", "result": {"document": wire_document()}}).to_string(),
        json!({"id": "2", "result": {"items": [wire_document()], "next": "b2Zmc2V0PTE"}})
            .to_string(),
        json!({"id": "3", "result": {"id": "D-9"}}).to_string(),
        json!({"id": "4", "result": {}}).to_string(),
    ])
    .expect("the handshake succeeds");

    assert_eq!(source.capabilities().documents, Support::Native);

    let found = source
        .get_document(&NativeId("D-1".to_owned()))
        .await
        .expect("the peer holds it")
        .expect("a document, not a null");
    assert_eq!(found.title, "Why the store holds a document");
    assert_eq!(found.project, Some(NativeId("P-1".to_owned())));
    // The member a consumer acts on without knowing the backend, decoded as the one key
    // that was present rather than as a bare string.
    assert_eq!(
        found.location,
        Some(Location::Path("/home/someone/notes/design.md".to_owned()))
    );

    let page = source
        .query_documents(&DocumentQuery::default(), &page(5))
        .await
        .expect("the peer answers a page");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, NativeId("D-1".to_owned()));
    assert_eq!(page.next, Some(Cursor("b2Zmc2V0PTE".to_owned())));

    let written = source
        .write_document(&ItemWrite {
            target: None,
            item: filed(),
            depends_on: Vec::new(),
        })
        .await
        .expect("the peer takes it");
    assert_eq!(written, NativeId("D-9".to_owned()));

    source
        .delete_document(&NativeId("D-9".to_owned()))
        .await
        .expect("the peer removes it");
}

/// A handshake from a peer that says it has documents, unlike anything this build hosts.
fn documentary() -> Value {
    let mut declared = capabilities();
    declared["documents"] = json!("native");
    declared
}

/// One document as a peer puts it on the wire.
fn wire_document() -> Value {
    json!({
        "id": "D-1",
        "title": "Why the store holds a document",
        "content": "A person cannot review a plan node by node.",
        "project": "P-1",
        "labels": [{"id": "L-1", "name": "design", "color": null}],
        "url": "https://example.invalid/D-1",
        "location": {"path": "/home/someone/notes/design.md"},
        "created_at": null,
        "updated_at": null
    })
}

#[tokio::test]
async fn a_handshake_that_says_nothing_about_documents_is_read_as_having_none() {
    // §2.1 and §4.2 together: `documents` is an optional member with a documented default,
    // so a plugin written before there were documents — which is what `capabilities()`
    // below spells, omitting it — is read as the document-free source it is rather than
    // refused for a member it has never heard of.
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities()}})
        .to_string(),
    ])
    .expect("the handshake succeeds");

    assert_eq!(source.capabilities().documents, Support::Unsupported);
}

/// The document a write test hands the far side, which never gets as far as holding one.
fn filed() -> Document {
    Document {
        version: None,
        id: NativeId("D-1".to_owned()),
        title: "Why the store holds a document".to_owned(),
        content: None,
        project: Some(NativeId("P-1".to_owned())),
        labels: Vec::new(),
        url: None,
        location: None,
        created_at: None,
        updated_at: None,
        metadata: BTreeMap::new(),
        repositories: Vec::new(),
        classification: Default::default(),
    }
}

#[tokio::test]
async fn a_plugin_that_stops_answering_fails_this_call_and_every_later_one() {
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities()}})
        .to_string(),
    ])
    .expect("the handshake succeeds");

    assert!(
        matches!(source.health().await, Err(SourceError::Unavailable { .. })),
        "the first call after the plugin left is unavailable"
    );
    assert!(
        matches!(
            source.labels(&page(5)).await,
            Err(SourceError::Unavailable { .. })
        ),
        "and so is the next one, rather than waiting for an answer nothing will send"
    );
    assert!(
        matches!(source.health().await, Err(SourceError::Unavailable { .. })),
        "and every one after that, from the record that there is no worker left"
    );
}

#[test]
fn a_program_that_does_not_exist_is_reported_with_the_path_that_was_tried() {
    let error = SubprocessSource::connect(
        "onetaskgraph-no-such-plugin-program",
        &[],
        &name(),
        &json!({}),
        BTreeMap::new(),
    )
    .expect_err("nothing to run");

    let SourceError::Unavailable { message } = error else {
        panic!("a program that will not run is unreachable: {error:?}");
    };
    assert!(
        message.contains("onetaskgraph-no-such-plugin-program"),
        "{message}"
    );
}

/// A real program every host running these tests is guaranteed to have: this one.
///
/// It is emphatically *not* a plugin, and that is the point. Proving what the engine does
/// when it spawns something that does not speak the protocol needs a real process — a real
/// `spawn`, real pipes, a real exit and a real kill on drop — and the test binary is the
/// one such program that is present on every platform this suite runs on without being
/// built, shipped or installed for the purpose.
fn not_a_plugin(argument: &str) -> Result<SubprocessSource, SourceError> {
    let program = std::env::current_exe().expect("this test binary has a path");
    SubprocessSource::connect(
        &program.to_string_lossy(),
        &[argument.to_owned()],
        &name(),
        &json!({}),
        BTreeMap::new(),
    )
}

#[test]
fn a_spawned_program_that_does_not_speak_the_protocol_is_refused_rather_than_waited_on() {
    // `--list` makes the test binary print its own test names and exit: a real program,
    // really spawned, that does not speak this protocol. Which refusal it earns is a race
    // it is not worth pretending away — the engine may get the non-envelope line, or it
    // may find the program already gone when it writes — so what this asserts is the part
    // that is not racy, and the part that matters: the run is refused, promptly, and
    // nothing becomes a source. The wording of a non-envelope answer is pinned
    // deterministically by `a_handshake_answer_that_is_not_a_line_of_json_is_a_violation`.
    let Err(error) = not_a_plugin("--list") else {
        panic!("a program that is not a plugin cannot become a source");
    };

    assert!(
        matches!(
            error,
            SourceError::Malformed { .. } | SourceError::Unavailable { .. }
        ),
        "a program that does not speak the protocol is refused as one that cannot: {error:?}"
    );
}

#[test]
fn a_spawned_program_that_fails_at_once_is_reported_with_what_it_wrote() {
    // An argument no test harness accepts: the program exits without writing a response,
    // having explained itself on standard error — which is the one place the reason is.
    let Err(error) = not_a_plugin("--definitely-not-a-flag") else {
        panic!("a program that exits at once cannot become a source");
    };

    let SourceError::Unavailable { message } = error else {
        panic!("a program that answered nothing is unreachable: {error:?}");
    };
    assert!(
        message.contains("definitely-not-a-flag"),
        "the plugin's own words reach the user: {message}"
    );
}

#[cfg(unix)]
#[test]
fn a_silent_handshake_is_stopped_at_its_configured_deadline() {
    // This probe uses the POSIX shell solely to create a portable-on-Unix child that
    // remains alive without writing. Windows has no corresponding system executable;
    // the runtime path itself is platform-neutral and the later-request deadline is
    // exercised over Rust pipes on every platform above.
    let started = Instant::now();
    let Err(error) = subprocess_plugin().build(
        &name(),
        &json!({"command": "/bin/sh", "args": ["-c", "while :; do :; done"],
                "deadline_ms": 20}),
        &NoSecrets,
    ) else {
        panic!("a silent handshake reaches its configured deadline");
    };

    let SourceError::Unavailable { message } = error else {
        panic!("a handshake deadline is a reachability failure: {error:?}");
    };
    assert!(
        message.contains("initialize") && message.contains("20 milliseconds"),
        "{message}"
    );
    // Unlike the request-deadline probes above, this span includes spawning the child, which
    // on a saturated host — the pre-push gate runs every affected suite at once — has taken
    // over a second on its own. A deadline that was never armed hangs rather than failing
    // this, so what the bound catches is one armed at the wrong scale: twenty milliseconds
    // read as twenty seconds is still well past it.
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the handshake hung"
    );
}

#[cfg(unix)]
#[test]
fn a_rate_limited_handshake_keeps_its_wait_and_gains_what_the_plugin_wrote() {
    // A plugin that refuses the handshake has usually said why on standard error and
    // nowhere else, and the engine appends that to the refusal it reports. `rate-limited`
    // was the one kind that could not take it — the variant had no message — so what the
    // plugin wrote was dropped. It has one now.
    //
    // What must survive alongside it is `retry_after_seconds`: that is the piece of this
    // refusal the engine acts on, and a diagnostic that replaced it with prose would leave
    // the caller with a reason and no wait. A real child writes both halves.
    let script = r#"read -r _request
printf '%s\n' 'the board is refusing bursts; slow down' >&2
printf '%s\n' '{"id":"0","error":{"kind":"rate-limited","retry_after_seconds":45}}'"#;
    let error = SubprocessSource::connect(
        "/bin/sh",
        &["-c".to_owned(), script.to_owned()],
        &name(),
        &json!({}),
        BTreeMap::new(),
    )
    .expect_err("a handshake the plugin rate-limited");

    let SourceError::RateLimited {
        retry_after_seconds,
        message,
    } = error
    else {
        panic!("a rate-limited handshake reported as {error:?}");
    };
    assert_eq!(
        retry_after_seconds,
        Some(45),
        "the wait the plugin asked for was replaced by the diagnostic"
    );
    let said = message.expect("what the plugin wrote reaches the caller");
    assert!(
        said.contains("the board is refusing bursts"),
        "the plugin's own words were dropped: {said}"
    );
    assert!(
        said.contains("the source rate-limited the request"),
        "the refusal it was appended to is no longer in it: {said}"
    );
}

#[cfg(unix)]
#[test]
fn a_rate_limited_handshake_from_a_silent_plugin_reads_exactly_as_it_did_before() {
    // The other half: a plugin that wrote nothing has nothing to add, and the refusal it
    // earns is the one shape every caller was already written against.
    let script = r#"read -r _request
printf '%s\n' '{"id":"0","error":{"kind":"rate-limited","retry_after_seconds":45}}'"#;
    let error = SubprocessSource::connect(
        "/bin/sh",
        &["-c".to_owned(), script.to_owned()],
        &name(),
        &json!({}),
        BTreeMap::new(),
    )
    .expect_err("a handshake the plugin rate-limited");

    assert_eq!(
        error,
        SourceError::RateLimited {
            retry_after_seconds: Some(45),
            message: None,
        }
    );
    assert_eq!(error.to_string(), "the source rate-limited the request");
}

#[cfg(unix)]
#[test]
fn a_spawned_plugin_inherits_no_unrelated_host_environment() {
    // `HOME` is deliberately not one of the named credentials handed to `connect`.
    // A real child shell reports which side of the boundary it observed through the
    // protocol's own error envelope, so this proves the spawned process's environment
    // rather than inspecting the `Command` that built it.
    let script = r#"read -r _request
if [ -z "${HOME+x}" ]; then
  printf '%s\n' '{"id":"0","error":{"kind":"auth","message":"environment cleared"}}'
else
  printf '%s\n' '{"id":"0","error":{"kind":"auth","message":"HOME leaked"}}'
fi"#;
    let error = SubprocessSource::connect(
        "/bin/sh",
        &["-c".to_owned(), script.to_owned()],
        &name(),
        &json!({}),
        BTreeMap::new(),
    )
    .expect_err("the probe refuses after reporting its environment");

    assert_eq!(
        error,
        SourceError::Auth {
            message: "environment cleared".to_owned()
        }
    );
}

#[tokio::test]
async fn a_source_is_named_in_diagnostics_without_its_connection() {
    let source = a_process_away(hosted_settings()).expect("the handshake succeeds");

    let shown = format!("{source:?}");
    assert!(shown.contains("in-memory"), "{shown}");
    assert!(
        !shown.contains("Connection"),
        "a live child and a forwarded credential stay out of it: {shown}"
    );
}

#[test]
fn a_handshake_answer_that_is_not_a_line_of_json_is_a_violation() {
    let Err(error) = scripted(vec!["not JSON at all".to_owned()]) else {
        panic!("a handshake that is not JSON builds nothing");
    };

    let SourceError::Malformed { message } = error else {
        panic!("a handshake line that is not JSON is a violation: {error:?}");
    };
    assert!(message.contains("not a response envelope"), "{message}");
}

#[tokio::test]
async fn an_answer_that_is_json_but_not_the_promised_shape_is_a_violation() {
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities()}})
        .to_string(),
        json!({"id": "1", "result": {"reachable": "yes please"}}).to_string(),
    ])
    .expect("the handshake succeeds");

    let SourceError::Malformed { message } = source.health().await.expect_err("a wrong shape")
    else {
        panic!("a result of the wrong shape is a violation");
    };
    assert!(message.contains("health"), "the method is named: {message}");
}

#[tokio::test]
async fn an_answer_carrying_both_members_after_the_handshake_is_a_violation() {
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities()}})
        .to_string(),
        json!({"id": "1", "result": {"reachable": true},
               "error": {"kind": "auth", "message": "no"}})
        .to_string(),
    ])
    .expect("the handshake succeeds");

    let SourceError::Malformed { message } = source.health().await.expect_err("both members")
    else {
        panic!("both members at once is a violation");
    };
    assert!(message.contains("both a result and an error"), "{message}");
}

/// A peer that records the handshake it was sent before answering it.
///
/// What crosses the wire is the thing worth asserting on: §3.1 says the handshake carries
/// only the variables this source's configuration names, and the only way to know that is
/// to read the line the engine actually wrote.
fn capturing(secrets: BTreeMap<String, String>) -> (SubprocessSource, Value) {
    let (to_engine, mut from_peer) = pipe().expect("a pipe");
    let (to_peer, from_engine) = pipe().expect("a pipe");
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let recorder = std::sync::Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut asked = BufReader::new(to_peer);
        let mut request = String::new();
        if asked.read_line(&mut request).is_err() {
            return;
        }
        recorder
            .lock()
            .expect("nothing else holds this")
            .push_str(&request);
        let answer = json!({"id": "0", "result": {"protocol_version": 2, "kind": "recorder",
                            "capabilities": capabilities()}});
        let _ = writeln!(from_peer, "{answer}");
        let _ = from_peer.flush();
    });
    let source = SubprocessSource::over(
        from_engine,
        to_engine,
        &name(),
        &json!({"root": "/somewhere"}),
        secrets,
    )
    .expect("the recorder answers the handshake");
    let line = seen.lock().expect("the thread has finished").clone();
    let handshake: Value = serde_json::from_str(line.trim()).expect("one JSON request");
    (source, handshake)
}

#[test]
fn the_handshake_carries_the_named_credentials_the_settings_and_the_source_name() {
    let mut secrets = BTreeMap::new();
    secrets.insert("LINEAR_API_KEY".to_owned(), "a value".to_owned());
    let (_source, handshake) = capturing(secrets);

    let params = &handshake["params"];
    assert_eq!(params["protocol_version"], 2);
    assert_eq!(params["source_name"], "work");
    assert_eq!(params["config"], json!({"root": "/somewhere"}));
    // Exactly the one named variable, so a plugin cannot be handed a credential its
    // configuration never asked for.
    assert_eq!(params["secrets"], json!({"LINEAR_API_KEY": "a value"}));
}

#[test]
fn a_handshake_carrying_no_named_credentials_forwards_none_at_all() {
    let (_source, handshake) = capturing(BTreeMap::new());

    assert_eq!(handshake["params"]["secrets"], json!({}));
}

#[test]
fn a_handshake_answer_addressed_to_another_request_is_a_violation() {
    let Err(error) = scripted(vec![
        json!({"id": "17", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities()}})
        .to_string(),
    ]) else {
        panic!("an answer to a request nobody sent builds nothing");
    };

    let SourceError::Malformed { message } = error else {
        panic!("an envelope addressed elsewhere is a violation: {error:?}");
    };
    assert!(
        message.contains("\"17\"") && message.contains("\"0\""),
        "{message}"
    );
}

#[test]
fn a_secret_name_that_could_never_be_a_variable_is_refused_at_the_field() {
    let Err(error) = subprocess_plugin().build(
        &name(),
        &json!({"command": "onetaskgraph-no-such-plugin-program", "secrets": ["not a name"]}),
        &NoSecrets,
    ) else {
        panic!("an unusable variable name builds nothing");
    };

    let SourceError::Config { message } = error else {
        panic!("an unusable variable name is a configuration refusal: {error:?}");
    };
    assert!(message.contains("not a name"), "{message}");
}

#[tokio::test]
async fn a_plugin_that_never_ends_its_line_has_the_connection_closed_on_it() {
    // One byte past the bound, and no newline: the shape of a peer that would otherwise
    // decide how much memory this process uses.
    let unbounded = "x".repeat(usize::try_from(MAX_LINE).expect("the bound fits a usize") + 1);
    let (to_engine, mut from_liar) = pipe().expect("a pipe");
    let (to_liar, from_engine) = pipe().expect("a pipe");
    std::thread::spawn(move || {
        let mut asked = BufReader::new(to_liar);
        let mut request = String::new();
        let _ = asked.read_line(&mut request);
        let answer = json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
                            "capabilities": capabilities()}});
        let _ = writeln!(from_liar, "{answer}");
        let _ = from_liar.flush();
        let mut second = String::new();
        let _ = asked.read_line(&mut second);
        let _ = from_liar.write_all(unbounded.as_bytes());
        let _ = from_liar.flush();
        // Held open: the engine must refuse on the length rather than on the close.
        let mut parked = String::new();
        let _ = asked.read_line(&mut parked);
    });
    let source =
        SubprocessSource::over(from_engine, to_engine, &name(), &json!({}), BTreeMap::new())
            .expect("the handshake succeeds");

    let SourceError::Malformed { message } = source.health().await.expect_err("an endless line")
    else {
        panic!("a line that never ends is a violation");
    };
    assert!(
        message.contains(&MAX_LINE.to_string()),
        "the refusal names the bound: {message}"
    );
}

#[test]
fn a_request_that_never_ends_its_line_closes_the_connection_rather_than_being_framed() {
    let mut input = serde_json::to_string(&handshake(2, hosted_settings())).expect("a request");
    input.push('\n');
    input.push_str(&"x".repeat(usize::try_from(MAX_LINE).expect("the bound fits a usize") + 1));

    let mut output: Vec<u8> = Vec::new();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(serve(input.as_bytes(), &mut output))
        .expect("an in-memory stream cannot fail");

    let answered = String::from_utf8(output).expect("UTF-8");
    // The handshake is answered; nothing after the unterminated line is, because nothing
    // after it can be framed as a request rather than as the tail of one.
    assert_eq!(answered.lines().count(), 1, "{answered}");
    assert!(answered.contains("\"protocol_version\":2"), "{answered}");
}

/// A `Capabilities` the scripted answers can carry, in the protocol's own spelling.
fn capabilities() -> Value {
    json!({
        "projects": "native",
        "orphan_tasks": "native",
        "filter_by_label": "native",
        "filter_by_status": "native",
        "search_title": "native",
        "search_content": "native",
        "task_dependencies": "both-directions",
        "project_dependencies": "both-directions",
        "max_page_size": 50
    })
}

/// A resolver holding exactly one variable.
struct OneSecret(&'static str);

impl SecretResolver for OneSecret {
    fn get(&self, var: &str) -> Option<SecretString> {
        (var == self.0).then(|| SecretString::from("a value nothing prints"))
    }
}

/// The `subprocess` plugin, as a configuration reaches it.
fn subprocess_plugin() -> Box<dyn onetaskgraph_plugin_api::SourcePlugin> {
    plugin_for("subprocess").expect("the subprocess plugin is registered")
}

#[test]
fn a_source_with_no_command_is_refused_before_anything_is_spawned() {
    let Err(error) = subprocess_plugin().build(&name(), &json!({"command": "   "}), &NoSecrets)
    else {
        panic!("a command that names nothing builds nothing");
    };

    let SourceError::Config { message } = error else {
        panic!("an empty command is a configuration refusal: {error:?}");
    };
    assert!(message.contains("command"), "{message}");
}

#[test]
fn a_subprocess_block_defaults_its_deadline_and_refuses_zero() {
    let config: SubprocessConfig =
        serde_json::from_value(json!({"command": "plugin"})).expect("the minimal block is valid");
    assert_eq!(config.deadline_ms, RequestDeadline::DEFAULT.milliseconds());

    let error =
        serde_json::from_value::<SubprocessConfig>(json!({"command": "plugin", "deadline_ms": 0}))
            .expect_err("zero is not a deadline");
    assert!(error.to_string().contains("nonzero u64"), "{error}");
}

#[test]
fn a_block_that_is_not_this_plugins_shape_is_refused_naming_the_source() {
    let Err(error) = subprocess_plugin().build(&name(), &json!({"command": 7}), &NoSecrets) else {
        panic!("a command of the wrong type builds nothing");
    };

    let SourceError::Config { message } = error else {
        panic!("a block of the wrong shape is a configuration refusal: {error:?}");
    };
    assert!(message.contains("work"), "the source is named: {message}");
}

#[test]
fn a_named_credential_nothing_defines_is_refused_naming_the_variable() {
    let Err(error) = subprocess_plugin().build(
        &name(),
        &json!({"command": "onetaskgraph-no-such-plugin-program",
                "secrets": ["LINEAR_API_KEY"]}),
        &NoSecrets,
    ) else {
        panic!("a credential nothing defines builds nothing");
    };

    let SourceError::Auth { message } = error else {
        panic!("an absent credential is an authentication refusal: {error:?}");
    };
    assert!(message.contains("LINEAR_API_KEY"), "{message}");
}

#[test]
fn a_named_credential_that_resolves_is_forwarded_and_the_run_gets_as_far_as_spawning() {
    let Err(error) = subprocess_plugin().build(
        &name(),
        &json!({"command": "onetaskgraph-no-such-plugin-program",
                "secrets": ["LINEAR_API_KEY"]}),
        &OneSecret("LINEAR_API_KEY"),
    ) else {
        panic!("the program does not exist, so nothing builds");
    };

    // Reaching the spawn is the assertion: a credential that resolved is one this build
    // never refused, and the only thing left to fail on is the program itself.
    assert!(
        matches!(error, SourceError::Unavailable { .. }),
        "resolution passed and the spawn failed: {error:?}"
    );
}

/// A peer that answers each request under that request's own id with the next of `results`,
/// the handshake first, and records every request it is sent — including any it is sent after
/// it has run out of answers.
fn recording(
    results: Vec<Value>,
) -> (
    Result<SubprocessSource, SourceError>,
    Arc<Mutex<Vec<Value>>>,
) {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&heard);
    let (to_engine, mut from_peer) = pipe().expect("a pipe");
    let (to_peer, from_engine) = pipe().expect("a pipe");
    std::thread::spawn(move || {
        let mut asked = BufReader::new(to_peer);
        let mut results = results.into_iter();
        loop {
            let mut line = String::new();
            match asked.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let request: Value = serde_json::from_str(&line).expect("a request line");
            let id = request["id"].clone();
            kept.lock().expect("the record").push(request);
            let Some(result) = results.next() else {
                continue;
            };
            let answer = json!({"id": id, "result": result});
            if writeln!(from_peer, "{answer}").is_err() || from_peer.flush().is_err() {
                return;
            }
        }
    });
    (
        SubprocessSource::over(from_engine, to_engine, &name(), &json!({}), BTreeMap::new()),
        heard,
    )
}

/// One task as a write carries it.
fn written(value: Value) -> Task {
    serde_json::from_value(value).expect("a task")
}

#[tokio::test]
async fn a_plugin_whose_handshake_lists_no_statuses_is_never_handed_queued_or_a_narrow_write() {
    let (source, heard) = recording(vec![
        json!({"protocol_version": 2, "kind": "earlier", "capabilities": capabilities(),
               "writes": "supported"}),
        json!({"items": [], "next": null}),
    ]);
    let source = source.expect("the handshake completes");

    // Every category the engine knows is listed in its own handshake.
    let handshake = heard.lock().expect("the record")[0].clone();
    assert_eq!(
        handshake["params"]["statuses"],
        json!([
            "draft",
            "backlog",
            "todo",
            "queued",
            "in-progress",
            "done",
            "cancelled",
            "unknown"
        ])
    );

    // A query for `queued` alone is not sent: nothing such a plugin holds can be in it.
    let only = TaskQuery {
        statuses: vec![StatusCategory::Queued],
        ..everything()
    };
    let answered = source
        .query_tasks(&only, &page(10))
        .await
        .expect("answered");
    assert!(answered.items.is_empty() && answered.next.is_none());
    // One naming a category it knows beside `queued` is sent that category alone.
    let mixed = TaskQuery {
        statuses: vec![StatusCategory::Todo, StatusCategory::Queued],
        ..everything()
    };
    source
        .query_tasks(&mixed, &page(10))
        .await
        .expect("answered");

    let refused = |error: SourceError| match error {
        SourceError::Refused { message } => message,
        other => panic!("expected a refusal by name, got {other:?}"),
    };
    let queued = written(json!({"id": "T-1", "title": "Alpha", "content": null,
        "status": {"category": "queued", "name": "Queued"}, "labels": []}));
    let message = refused(
        source
            .write_task(&ItemWrite {
                target: None,
                item: queued,
                depends_on: Vec::new(),
            })
            .await
            .expect_err("never handed queued"),
    );
    assert!(
        message.contains("\"earlier\" plugin's handshake does not list the status category queued"),
        "{message}"
    );
    let delivering = written(json!({"id": "T-1", "title": "Alpha", "content": null,
        "status": {"category": "todo", "name": "Todo"}, "labels": [], "delivers": ["T-2"]}));
    let message = refused(
        source
            .write_task(&ItemWrite {
                target: None,
                item: delivering,
                depends_on: Vec::new(),
            })
            .await
            .expect_err("never handed a list it would drop"),
    );
    assert!(
        message.contains("does not say it answers the narrow task writes"),
        "{message}"
    );
    let message = refused(
        source
            .set_task_status(&NativeId::from("T-1"), StatusCategory::Todo)
            .await
            .expect_err("never sent the method"),
    );
    assert!(
        message.contains("does not send it set_task_status"),
        "{message}"
    );
    let message = refused(
        source
            .set_delivered_by(&NativeId::from("T-1"), &[])
            .await
            .expect_err("never sent the method"),
    );
    assert!(
        message.contains("does not send it set_delivered_by"),
        "{message}"
    );

    let heard = heard.lock().expect("the record").clone();
    let methods: Vec<&str> = heard
        .iter()
        .map(|request| request["method"].as_str().unwrap())
        .collect();
    assert_eq!(methods, ["initialize", "query_tasks"], "{heard:?}");
    assert_eq!(heard[1]["params"]["query"]["statuses"], json!(["todo"]));
}

#[tokio::test]
async fn the_narrow_task_writes_cross_the_wire_and_land_in_the_hosted_source() {
    let there = a_process_away(hosted_settings()).expect("connects");
    let id = NativeId::from("T-3");
    assert_eq!(
        there
            .set_task_status(&id, StatusCategory::Queued)
            .await
            .expect("answered"),
        Some(Status {
            category: StatusCategory::Queued,
            name: "queued".to_owned()
        })
    );
    let queued = TaskQuery {
        statuses: vec![StatusCategory::Queued],
        ..everything()
    };
    let found: Vec<String> = there
        .query_tasks(&queued, &page(2))
        .await
        .expect("answered")
        .items
        .into_iter()
        .map(|task| task.id.0)
        .collect();
    assert_eq!(found, ["T-3"]);

    let by = vec![TaskRef::new("plan:P-1").expect("a task id")];
    assert_eq!(
        there.set_delivered_by(&id, &by).await.expect("answered"),
        Some(())
    );
    let held = there.get_task(&id).await.expect("answered").expect("held");
    assert_eq!(held.delivered_by, by);
    assert_eq!(held.status.category, StatusCategory::Queued);

    let nothing = NativeId::from("T-9");
    assert_eq!(
        there
            .set_task_status(&nothing, StatusCategory::Done)
            .await
            .expect("answered"),
        None
    );
    assert_eq!(
        there
            .set_delivered_by(&nothing, &[])
            .await
            .expect("answered"),
        None
    );
}

#[test]
fn an_engine_that_lists_no_statuses_is_told_queued_as_unknown_under_its_own_name() {
    let mut settings = hosted_settings();
    settings["config"]["tasks"][0]["status"] = json!({"category": "queued", "name": "Queued"});
    let get = json!({"id": "1", "method": "get_task", "params": {"id": "T-1"}});
    let list = json!({"id": "2", "method": "query_tasks",
        "params": {"query": {"text": null, "labels": {"any_of": [], "all_of": [], "none_of": []}, "statuses": [], "project": "any"},
                   "page": {"cursor": null, "limit": 2}}});
    let set = json!({"id": "3", "method": "set_task_status",
        "params": {"id": "T-3", "category": "queued"}});
    settings["config"]["projects"][0]["status"] = json!({"category": "queued", "name": "Queued"});
    let project = json!({"id": "4", "method": "get_project", "params": {"id": "P-1"}});
    let projects = json!({"id": "5", "method": "query_projects",
        "params": {"query": {"text": null, "labels": {"any_of": [], "all_of": [], "none_of": []}, "statuses": []},
                   "page": {"cursor": null, "limit": 2}}});

    let earlier = served(&[
        handshake(2, settings.clone()),
        get.clone(),
        list.clone(),
        set,
        project,
        projects,
    ]);
    assert_eq!(
        earlier[4]["result"]["project"]["status"],
        json!({"category": "unknown", "name": "Queued"}),
        "{earlier:?}"
    );
    assert_eq!(
        earlier[5]["result"]["items"][0]["status"],
        json!({"category": "unknown", "name": "Queued"}),
        "{earlier:?}"
    );
    assert_eq!(earlier[0]["result"]["task_updates"], json!(true));
    assert_eq!(
        earlier[0]["result"]["statuses"],
        json!([
            "draft",
            "backlog",
            "todo",
            "queued",
            "in-progress",
            "done",
            "cancelled",
            "unknown"
        ])
    );
    assert_eq!(
        earlier[1]["result"]["task"]["status"],
        json!({"category": "unknown", "name": "Queued"})
    );
    assert_eq!(
        earlier[2]["result"]["items"][0]["status"],
        json!({"category": "unknown", "name": "Queued"})
    );
    assert_eq!(
        earlier[3]["result"]["status"],
        json!({"category": "unknown", "name": "queued"})
    );

    let mut current = handshake(2, settings);
    current["params"]["statuses"] = json!([
        "draft",
        "backlog",
        "todo",
        "queued",
        "in-progress",
        "done",
        "cancelled",
        "unknown"
    ]);
    let now = served(&[current, get]);
    assert_eq!(
        now[1]["result"]["task"]["status"],
        json!({"category": "queued", "name": "Queued"})
    );
}

#[tokio::test]
async fn a_plugin_whose_handshake_does_not_declare_metadata_updates_is_refused_without_being_asked()
{
    let (source, heard) = recording(vec![json!({
        "protocol_version": 2, "kind": "earlier",
        "capabilities": capabilities(),
        "writes": "supported", "task_updates": true
    })]);
    let source = source.expect("the handshake completes");
    let id = NativeId::from("T-1");
    let key = MetadataKey::new("myapp.review").expect("a caller key");
    let value = json!({"approved": true});

    let refusals = [
        (
            "task",
            source
                .set_task_metadata(&id, &key, &value)
                .await
                .map(|_| ()),
        ),
        (
            "project",
            source
                .set_project_metadata(&id, &key, &value)
                .await
                .map(|_| ()),
        ),
        (
            "document",
            source
                .set_document_metadata(&id, &key, &value)
                .await
                .map(|_| ()),
        ),
    ];
    for (record, refusal) in refusals {
        assert_eq!(
            refusal.expect_err("never sent the method"),
            SourceError::Refused {
                message: format!(
                    "the earlier plugin cannot write a {record}'s metadata on its own"
                )
            }
        );
    }
    let heard = heard.lock().expect("the record").clone();
    let methods: Vec<&str> = heard
        .iter()
        .map(|request| request["method"].as_str().unwrap())
        .collect();
    assert_eq!(methods, ["initialize"], "{heard:?}");
}

#[tokio::test]
async fn a_plugin_declaring_metadata_updates_is_sent_the_key_and_value_as_json() {
    let (source, heard) = recording(vec![
        json!({"protocol_version": 2, "kind": "later", "capabilities": capabilities(),
               "writes": "supported", "metadata_updates": true}),
        json!({"task": null}),
    ]);
    let source = source.expect("the handshake completes");
    let answered = source
        .set_task_metadata(
            &NativeId::from("T-9"),
            &MetadataKey::new("myapp.review").expect("a caller key"),
            &json!([1, "two", null]),
        )
        .await
        .expect("answered");
    assert_eq!(answered, None);
    let heard = heard.lock().expect("the record").clone();
    assert_eq!(
        heard[1],
        json!({"id": heard[1]["id"], "method": "set_task_metadata",
               "params": {"id": "T-9", "key": "myapp.review", "value": [1, "two", null]}})
    );
}

#[tokio::test]
async fn the_narrow_metadata_writes_cross_the_wire_and_land_in_the_hosted_source() {
    let mut settings = hosted_settings();
    settings["config"]["capabilities"]["documents"] = json!("native");
    settings["config"]["tasks"][0]["metadata"] = json!({"myapp.kept": 1});
    settings["config"]["documents"] = json!([
        {"id": "D-1", "title": "Design", "content": "text", "labels": [],
         "metadata": {"myapp.kept": 1}}
    ]);
    let there = a_process_away(settings).expect("connects");
    let key = MetadataKey::new("myapp.review").expect("a caller key");

    let task = there
        .set_task_metadata(&NativeId::from("T-1"), &key, &json!({"by": "nick"}))
        .await
        .expect("answered")
        .expect("held");
    assert_eq!(
        serde_json::to_value(&task.metadata).expect("plain data"),
        json!({"myapp.kept": 1, "myapp.review": {"by": "nick"}})
    );
    assert_eq!(
        there
            .get_task(&NativeId::from("T-1"))
            .await
            .expect("answered"),
        Some(task)
    );

    let project = there
        .set_project_metadata(&NativeId::from("P-1"), &key, &json!(false))
        .await
        .expect("answered")
        .expect("held");
    assert_eq!(project.metadata["myapp.review"], json!(false));
    assert_eq!(
        there
            .get_project(&NativeId::from("P-1"))
            .await
            .expect("answered"),
        Some(project)
    );

    let document = there
        .set_document_metadata(&NativeId::from("D-1"), &key, &Value::Null)
        .await
        .expect("answered")
        .expect("held");
    assert_eq!(
        serde_json::to_value(&document.metadata).expect("plain data"),
        json!({"myapp.kept": 1, "myapp.review": null})
    );
    assert_eq!(
        there
            .get_document(&NativeId::from("D-1"))
            .await
            .expect("answered"),
        Some(document)
    );

    for record in ["task", "project", "document"] {
        let nothing = NativeId::from("X-9");
        let answered = match record {
            "task" => there
                .set_task_metadata(&nothing, &key, &json!(1))
                .await
                .map(|held| held.is_none()),
            "project" => there
                .set_project_metadata(&nothing, &key, &json!(1))
                .await
                .map(|held| held.is_none()),
            _ => there
                .set_document_metadata(&nothing, &key, &json!(1))
                .await
                .map(|held| held.is_none()),
        };
        assert!(answered.expect("answered"), "{record} X-9 is not held");
    }
}

#[tokio::test]
async fn a_handshake_written_before_priorities_is_never_handed_a_priority_or_a_content_write() {
    // The literal handshake a pre-priority plugin sends: no `priority`, no
    // `filter_by_priority` and no `content_updates`. It is read as a source holding no
    // priority, and the seam itself refuses — before anything is sent — a narrow priority
    // write, a narrow content write, and a task write carrying a priority it would drop.
    let (source, heard) = recording(vec![json!({
        "protocol_version": 2, "kind": "earlier",
        "capabilities": capabilities(),
        "writes": "supported", "task_updates": true, "metadata_updates": true
    })]);
    let source = source.expect("the handshake completes");
    assert_eq!(source.capabilities().priority, Support::Unsupported);
    assert_eq!(
        source.capabilities().filter_by_priority,
        Support::Unsupported
    );
    let id = NativeId::from("T-1");
    assert_eq!(
        source
            .set_task_priority(&id, Priority::High)
            .await
            .expect_err("never sent"),
        SourceError::Refused {
            message: "the earlier plugin cannot write a task's priority on its own".to_owned()
        }
    );
    assert_eq!(
        source
            .set_task_content(&id, "body")
            .await
            .expect_err("never sent"),
        SourceError::Refused {
            message: "the earlier plugin cannot write a task's content on its own".to_owned()
        }
    );
    let mut task: Task = serde_json::from_value(json!({
        "id": "T-1", "title": "Alpha", "content": null,
        "status": {"category": "todo", "name": "Todo"}, "labels": [], "project": null,
        "url": null, "created_at": null, "updated_at": null
    }))
    .expect("a task");
    task.priority = Priority::Urgent;
    assert!(
        source
            .write_task(&ItemWrite {
                target: None,
                item: task,
                depends_on: Vec::new(),
            })
            .await
            .is_err(),
        "a task carrying a priority never reaches a plugin that would drop it"
    );
    let heard = heard.lock().expect("the record").clone();
    let methods: Vec<&str> = heard
        .iter()
        .map(|request| request["method"].as_str().unwrap())
        .collect();
    assert_eq!(methods, ["initialize"], "{heard:?}");
}

#[tokio::test]
async fn the_narrow_priority_and_content_writes_cross_the_wire_and_land_in_the_hosted_source() {
    let mut settings = hosted_settings();
    settings["config"]["tasks"][0]["priority"] = json!("low");
    let there = a_process_away(settings).expect("connects");
    assert_eq!(there.capabilities().priority, Support::Native);
    let id = NativeId::from("T-1");
    let before = there.get_task(&id).await.expect("answered").expect("held");
    assert_eq!(before.priority, Priority::Low);

    assert_eq!(
        there
            .set_task_priority(&id, Priority::Urgent)
            .await
            .expect("answered"),
        Some(Priority::Urgent)
    );
    assert_eq!(
        there
            .set_task_content(&id, "replaced\r\nbody\n")
            .await
            .expect("answered"),
        Some(())
    );
    let after = there.get_task(&id).await.expect("answered").expect("held");
    assert_eq!(
        after,
        Task {
            priority: Priority::Urgent,
            content: Some("replaced\r\nbody\n".to_owned()),
            version: after.version.clone(),
            ..before
        }
    );

    // A priority filter crosses the wire to the hosted source, which applies it.
    let urgent = there
        .query_tasks(
            &TaskQuery {
                priorities: vec![Priority::Urgent],
                ..everything()
            },
            &page(2),
        )
        .await
        .expect("answered");
    assert_eq!(
        urgent
            .items
            .iter()
            .map(|task| task.id.0.as_str())
            .collect::<Vec<_>>(),
        ["T-1"]
    );

    let nothing = NativeId::from("X-9");
    assert_eq!(
        there
            .set_task_priority(&nothing, Priority::None)
            .await
            .expect("answered"),
        None
    );
    assert_eq!(
        there
            .set_task_content(&nothing, "x")
            .await
            .expect("answered"),
        None
    );
}

#[tokio::test]
async fn a_narrow_priority_or_content_answer_that_does_not_say_what_it_wrote_is_malformed() {
    let mut declared = capabilities();
    declared["priority"] = json!("native");
    let handshake = json!({"protocol_version": 2, "kind": "later", "capabilities": declared,
                           "writes": "supported", "content_updates": true});
    let id = NativeId::from("T-1");

    // An answer leaving the member out says nothing about whether the task is there.
    let (source, _) = recording(vec![handshake.clone(), json!({})]);
    let refused = source
        .expect("the handshake completes")
        .set_task_priority(&id, Priority::Low)
        .await
        .expect_err("an answer without its member");
    assert!(
        matches!(&refused, SourceError::Malformed { message }
            if message.contains("set_task_priority") && message.contains("priority")),
        "{refused:?}"
    );
    let (source, _) = recording(vec![handshake.clone(), json!({})]);
    assert!(matches!(
        source
            .expect("the handshake completes")
            .set_task_content(&id, "x")
            .await,
        Err(SourceError::Malformed { .. })
    ));

    // And one naming a task it was not asked to write is not that write landing.
    let (source, heard) = recording(vec![handshake, json!({"id": "T-2"})]);
    let refused = source
        .expect("the handshake completes")
        .set_task_content(&id, "the body")
        .await
        .expect_err("the wrong task");
    assert!(
        matches!(&refused, SourceError::Malformed { message }
            if message.contains("T-2") && message.contains("T-1")),
        "{refused:?}"
    );
    let heard = heard.lock().expect("the record").clone();
    assert_eq!(
        heard[1],
        json!({"id": heard[1]["id"], "method": "set_task_content",
               "params": {"id": "T-1", "content": "the body"}})
    );
}

/// The targeted update this seam's tests put to a hosted task.
fn settling() -> onetaskgraph_plugin_api::TaskUpdate {
    onetaskgraph_plugin_api::TaskUpdate {
        status: Some(Status {
            category: StatusCategory::Done,
            name: "Shipped".to_owned(),
        }),
        metadata_set: BTreeMap::from([(
            MetadataKey::new("caller.settled").expect("a caller key"),
            json!({"at": 3}),
        )]),
        ..onetaskgraph_plugin_api::TaskUpdate::default()
    }
}

#[tokio::test]
async fn a_targeted_update_crosses_the_wire_to_a_host_that_declares_it() {
    use onetaskgraph_plugin_api::UpdatedField;

    let there = a_process_away(hosted_settings()).expect("the handshake succeeds");
    let id = NativeId::from("T-1");
    let outcome = there
        .update_task(&id, &settling())
        .await
        .expect("the plugin takes the update")
        .expect("the task is there");
    assert_eq!(
        outcome.written,
        std::collections::BTreeSet::from([UpdatedField::Status, UpdatedField::Metadata])
    );
    assert_eq!(outcome.task.status.name, "Shipped");
    assert_eq!(
        outcome.task.title, "Alpha",
        "a field the update did not name moved"
    );
    let read = there.get_task(&id).await.expect("a read").expect("held");
    assert_eq!(
        read, outcome.task,
        "the outcome is what the plugin now holds"
    );

    // The same update again is no write at all, and an absent task is `null`.
    let again = there
        .update_task(&id, &settling())
        .await
        .expect("the plugin takes the update")
        .expect("the task is there");
    assert!(again.written.is_empty(), "{:?}", again.written);
    assert_eq!(
        there
            .update_task(&NativeId::from("X-9"), &settling())
            .await
            .expect("answered"),
        None
    );
}

#[tokio::test]
async fn a_plugin_whose_handshake_predates_the_targeted_update_is_updated_through_one_write() {
    // §3.10: absent means `false`, and such a plugin is never sent `update_task` — it is
    // updated through the reads and the `write_task` it already answers.
    let held = json!({"id": "T-1", "title": "Alpha", "content": "body",
                      "status": {"category": "todo", "name": "Todo"}, "labels": []});
    let mut landed = held.clone();
    landed["status"] = json!({"category": "done", "name": "Shipped"});
    landed["metadata"] = json!({"caller.settled": {"at": 3}});
    let handshake = json!({"protocol_version": 2, "kind": "earlier", "capabilities": capabilities(),
                           "writes": "supported"});
    let (source, heard) = recording(vec![
        handshake,
        json!({"task": held}),
        json!({"items": [], "next": null}),
        json!({"id": "T-1"}),
        json!({"task": landed}),
    ]);
    let outcome = source
        .expect("the handshake completes")
        .update_task(&NativeId::from("T-1"), &settling())
        .await
        .expect("the update lands")
        .expect("the task is there");
    assert_eq!(outcome.task.status.name, "Shipped");
    let heard = heard.lock().expect("the record").clone();
    let methods: Vec<&str> = heard
        .iter()
        .map(|request| request["method"].as_str().unwrap())
        .collect();
    assert_eq!(
        methods,
        vec![
            "initialize",
            "get_task",
            "task_dependencies",
            "write_task",
            "get_task"
        ]
    );
    assert_eq!(heard[3]["params"]["write"]["target"], json!("T-1"));
    assert_eq!(heard[3]["params"]["write"]["item"]["title"], json!("Alpha"));
}

#[tokio::test]
async fn a_targeted_update_naming_what_the_handshake_did_not_declare_is_refused_unsent() {
    // A plugin that declares `targeted_updates` but lists no status category past the first
    // vocabulary, holds no priority and does not declare `task_updates` is not sent an
    // update naming any of those.
    let (source, heard) = recording(vec![json!({
        "protocol_version": 2, "kind": "later", "capabilities": capabilities(),
        "writes": "supported", "targeted_updates": true
    })]);
    let source = source.expect("the handshake completes");
    let id = NativeId::from("T-1");
    for (update, names) in [
        (
            onetaskgraph_plugin_api::TaskUpdate {
                status: Some(Status {
                    category: StatusCategory::Queued,
                    name: "Queued".to_owned(),
                }),
                ..onetaskgraph_plugin_api::TaskUpdate::default()
            },
            "status category queued",
        ),
        (
            onetaskgraph_plugin_api::TaskUpdate {
                priority: Some(onetaskgraph_plugin_api::Priority::High),
                ..onetaskgraph_plugin_api::TaskUpdate::default()
            },
            "priority",
        ),
        (
            onetaskgraph_plugin_api::TaskUpdate {
                delivers: Some(Vec::new()),
                ..onetaskgraph_plugin_api::TaskUpdate::default()
            },
            "an update naming delivers",
        ),
    ] {
        let error = source
            .update_task(&id, &update)
            .await
            .expect_err("never sent the method");
        let SourceError::Refused { message } = &error else {
            panic!("answered as {error:?}");
        };
        assert!(message.contains(names), "{message}");
    }
    let heard = heard.lock().expect("the record").clone();
    let methods: Vec<&str> = heard
        .iter()
        .map(|request| request["method"].as_str().unwrap())
        .collect();
    assert_eq!(methods, ["initialize"], "{heard:?}");
}

#[tokio::test]
async fn a_targeted_update_answered_for_another_task_is_malformed() {
    let handshake = json!({"protocol_version": 2, "kind": "later", "capabilities": capabilities(),
                           "writes": "supported", "targeted_updates": true});
    let other = json!({"task": {"id": "T-2", "title": "Beta", "content": null,
                                 "status": {"category": "done", "name": "Shipped"},
                                 "labels": []},
                       "written": ["status"], "delivers_before": []});
    let (source, heard) = recording(vec![handshake.clone(), json!({"outcome": other})]);
    let refused = source
        .expect("the handshake completes")
        .update_task(&NativeId::from("T-1"), &settling())
        .await
        .expect_err("the wrong task");
    assert!(
        matches!(&refused, SourceError::Malformed { message }
            if message.contains("T-2") && message.contains("T-1")),
        "{refused:?}"
    );
    assert_eq!(
        heard.lock().expect("the record")[1]["method"],
        "update_task"
    );

    // Nor is one saying it wrote a field the update never named.
    let mut unnamed = other.clone();
    unnamed["task"]["id"] = json!("T-1");
    unnamed["written"] = json!(["status", "title"]);
    let (source, _) = recording(vec![handshake.clone(), json!({"outcome": unnamed})]);
    let refused = source
        .expect("the handshake completes")
        .update_task(&NativeId::from("T-1"), &settling())
        .await
        .expect_err("a field the update did not name");
    assert!(
        matches!(&refused, SourceError::Malformed { message }
            if message.contains("title") && message.contains("did not name")),
        "{refused:?}"
    );

    // An answer leaving the member out says nothing about whether the task is there.
    let (source, _) = recording(vec![handshake, json!({})]);
    assert!(matches!(
        source
            .expect("the handshake completes")
            .update_task(&NativeId::from("T-1"), &settling())
            .await,
        Err(SourceError::Malformed { .. })
    ));
}

#[test]
fn a_served_plugin_takes_the_copy_link_key_only_with_a_value_that_is_links() {
    // `onetaskgraph.copies` is the one reserved key §4.18 admits, and only as an object of
    // destination source names to qualified ids of that source: anything else is refused at
    // the boundary, before the hosted source is asked, so no plugin holds a link a later copy
    // could not read.
    let write = |value: Value| {
        json!({"id": "1", "method": "set_task_metadata",
               "params": {"id": "T-1", "key": "onetaskgraph.copies", "value": value}})
    };
    for value in [
        json!("notes:T-1"),
        json!({"notes": 5}),
        json!({"notes": "elsewhere:T-1"}),
        json!({"notes": "not qualified"}),
    ] {
        let answers = served(&[handshake(2, hosted_settings()), write(value.clone())]);
        assert_eq!(
            refusal(&answers[1]),
            "malformed",
            "{value}: {:#}",
            answers[1]
        );
    }

    // The shape `docs/metadata.md` documents for the key is its own example, taken from that
    // page rather than restated, so the page and this boundary cannot come apart.
    let page = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/metadata.md"),
    )
    .expect("docs/metadata.md is readable");
    let section = &page[page
        .find("### `onetaskgraph.copies`")
        .expect("the section on the link")..];
    let example = &section[section.find("```json\n").expect("its example") + "```json\n".len()..];
    let documented: Value = serde_json::from_str(&example[..example.find("```").unwrap()])
        .expect("the documented example is JSON");
    let answers = served(&[handshake(2, hosted_settings()), write(documented.clone())]);
    assert_eq!(
        answers[1]["result"]["task"]["metadata"]["onetaskgraph.copies"], documented,
        "the documented example is refused: {:#}",
        answers[1]
    );

    let link = json!({"notes": "notes:T-1", "board": "board:I_1"});
    let answers = served(&[handshake(2, hosted_settings()), write(link.clone())]);
    assert_eq!(
        answers[1]["result"]["task"]["metadata"]["onetaskgraph.copies"], link,
        "{:#}",
        answers[1]
    );
    // And every other key in the namespace is refused as it always was.
    let other = served(&[
        handshake(2, hosted_settings()),
        json!({"id": "1", "method": "set_task_metadata",
               "params": {"id": "T-1", "key": "onetaskgraph.origin", "value": "x:y"}}),
    ]);
    assert_eq!(refusal(&other[1]), "malformed", "{:#}", other[1]);
}

#[test]
fn a_served_plugin_takes_the_copy_location_key_only_with_a_value_that_is_locations() {
    // `onetaskgraph.copy_locations` is the other reserved key §4.18 admits, and only as an
    // object of destination source names to an id of that source and a location: anything else
    // is refused at the boundary, before the hosted source is asked.
    let write = |value: Value| {
        json!({"id": "1", "method": "set_task_metadata",
               "params": {"id": "T-1", "key": "onetaskgraph.copy_locations", "value": value}})
    };
    for value in [
        json!("notes:T-1"),
        json!({"notes": "notes:T-1"}),
        json!({"notes": {"id": "elsewhere:T-1", "location": {"path": "/tmp/T-1.md"}}}),
        json!({"notes": {"id": "notes:T-1"}}),
        json!({"notes": {"id": "notes:T-1", "location": "/tmp/T-1.md"}}),
    ] {
        let answers = served(&[handshake(2, hosted_settings()), write(value.clone())]);
        assert_eq!(
            refusal(&answers[1]),
            "malformed",
            "{value}: {:#}",
            answers[1]
        );
    }

    // The documented example, taken from `docs/metadata.md` rather than restated.
    let page = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/metadata.md"),
    )
    .expect("docs/metadata.md is readable");
    let section = &page[page
        .find("### `onetaskgraph.copy_locations`")
        .expect("the section on the locations")..];
    let example = &section[section.find("```json\n").expect("its example") + "```json\n".len()..];
    let documented: Value = serde_json::from_str(&example[..example.find("```").unwrap()])
        .expect("the documented example is JSON");
    let answers = served(&[handshake(2, hosted_settings()), write(documented.clone())]);
    assert_eq!(
        answers[1]["result"]["task"]["metadata"]["onetaskgraph.copy_locations"], documented,
        "the documented example is refused: {:#}",
        answers[1]
    );
}

#[test]
fn the_reference_host_declares_end_command_and_answers_it() {
    let answers = served(&[
        handshake(2, hosted_settings()),
        json!({"id": "1", "method": "end_command", "params": {}}),
    ]);
    assert_eq!(
        answers[0]["result"]["ends_commands"],
        json!(true),
        "{:#}",
        answers[0]
    );
    assert_eq!(answers[1], json!({"id": "1", "result": {}}));
}

#[test]
fn the_reference_host_takes_end_command_params_only_as_an_object() {
    let answers = served(&[
        handshake(2, hosted_settings()),
        json!({"id": "1", "method": "end_command", "params": null}),
        json!({"id": "2", "method": "end_command", "params": []}),
        json!({"id": "3", "method": "end_command", "params": "now"}),
        json!({"id": "4", "method": "end_command", "params": {"from": "a newer engine"}}),
    ]);
    for refused in &answers[1..4] {
        assert_eq!(refusal(refused), "malformed", "{refused:#}");
        assert!(
            because(refused).contains("the parameters of end_command"),
            "{refused:#}"
        );
    }
    assert_eq!(
        answers[4],
        json!({"id": "4", "result": {}}),
        "a member it does not know is ignored"
    );
}

#[tokio::test]
async fn end_command_crosses_the_wire_and_the_hosted_source_still_answers() {
    let there = a_process_away(hosted_settings()).expect("the handshake succeeds");
    there.end_command().await.expect("the command ends");
    let task = there
        .get_task(&NativeId("T-1".into()))
        .await
        .expect("a read after the call")
        .expect("T-1 is held");
    assert_eq!(task.title, "Alpha");
}

#[tokio::test]
async fn a_plugin_that_does_not_declare_end_command_is_never_sent_it() {
    // The peer answers the handshake and then hangs up: a request sent to it would fail.
    let source = scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities()}})
        .to_string(),
    ])
    .expect("the handshake succeeds");
    source
        .end_command()
        .await
        .expect("nothing is sent to a plugin that holds nothing to drop");
}

/// A peer that shakes hands declaring `end_command`, then answers every request with an empty
/// result and records each request it was sent.
fn recording_end_commands() -> (SubprocessSource, Arc<Mutex<Vec<Value>>>) {
    let (to_engine, mut from_peer) = pipe().expect("a pipe");
    let (to_peer, from_engine) = pipe().expect("a pipe");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&asked);
    std::thread::spawn(move || {
        let mut lines = BufReader::new(to_peer);
        loop {
            let mut line = String::new();
            match lines.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let request: Value = serde_json::from_str(&line).expect("a request");
            let result = if request["method"] == "initialize" {
                json!({"protocol_version": 2, "kind": "made-up",
                       "capabilities": capabilities(), "ends_commands": true})
            } else {
                json!({})
            };
            recorded.lock().unwrap().push(request.clone());
            let answer = json!({"id": request["id"], "result": result});
            if writeln!(from_peer, "{answer}").is_err() || from_peer.flush().is_err() {
                return;
            }
        }
    });
    let source =
        SubprocessSource::over(from_engine, to_engine, &name(), &json!({}), BTreeMap::new())
            .expect("the handshake succeeds");
    (source, asked)
}

/// A peer declaring `end_command` that refuses it with `message`.
fn refusing_end_command(message: &str) -> SubprocessSource {
    scripted(vec![
        json!({"id": "0", "result": {"protocol_version": 2, "kind": "made-up",
               "capabilities": capabilities(), "ends_commands": true}})
        .to_string(),
        json!({"id": "1", "error": {"kind": "unavailable", "message": message}}).to_string(),
    ])
    .expect("the handshake succeeds")
}

fn ready(name: &str, source: impl TaskSource + 'static) -> onetaskgraph_core::ConfiguredSource {
    onetaskgraph_core::ConfiguredSource::Ready(onetaskgraph_core::ResolvedSource::adopt(
        SourceName::new(name).unwrap(),
        Box::new(source),
    ))
}

#[tokio::test]
async fn the_engine_asks_every_source_past_a_failure_and_names_the_first_that_failed() {
    let (recorded, asked) = recording_end_commands();
    let engine = onetaskgraph_core::Engine::new(
        vec![
            ready(
                "first",
                refusing_end_command("the first source's board stayed held"),
            ),
            ready("between", recorded),
            ready(
                "last",
                refusing_end_command("the last source's board stayed held"),
            ),
        ],
        vec![name()],
    );
    let refused = engine
        .end_command()
        .await
        .expect_err("two sources could not drop what they held");
    let onetaskgraph_core::EngineError::SourceFailed { name, error } = &refused else {
        panic!("named as the source that failed: {refused:?}");
    };
    assert_eq!(name, "first", "the first failure is the one reported");
    assert!(
        error
            .to_string()
            .contains("the first source's board stayed held"),
        "{error}"
    );
    let methods = asked
        .lock()
        .unwrap()
        .iter()
        .map(|request| request["method"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        methods,
        [json!("initialize"), json!("end_command")],
        "the source after the failure was still asked"
    );
}

#[tokio::test]
async fn a_repositories_update_reaches_a_host_by_rewriting_the_task_and_moves_nothing_else() {
    use onetaskgraph_plugin_api::{Repository, UpdatedField};

    // A peer that declares targeted updates predates `repositories`, and one that did not know
    // the member would drop it: the host rewrites the task whole instead, so the list lands.
    let there = a_process_away(hosted_settings()).expect("the handshake succeeds");
    let id = NativeId::from("T-1");
    let before = there.get_task(&id).await.expect("a read").expect("held");
    let widgets = Repository::try_from("github.com/acme/widgets".to_owned()).expect("an origin");
    let outcome = there
        .update_task(
            &id,
            &onetaskgraph_plugin_api::TaskUpdate {
                repositories: Some(vec![widgets.clone()]),
                ..onetaskgraph_plugin_api::TaskUpdate::default()
            },
        )
        .await
        .expect("the update lands")
        .expect("the task is there");
    assert_eq!(
        outcome.written,
        std::collections::BTreeSet::from([UpdatedField::Repositories])
    );
    let read = there.get_task(&id).await.expect("a read").expect("held");
    assert_eq!(read.repositories, vec![widgets]);
    assert_eq!(
        Task {
            repositories: before.repositories.clone(),
            version: before.version.clone(),
            ..read
        },
        before,
        "nothing but the repositories moved"
    );
}
