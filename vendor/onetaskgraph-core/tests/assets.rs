//! Image assets as a **Rust caller** reaches them: create a task holding assets, copy it with
//! [`Engine::copy`], and read back what landed.
//!
//! The journeys that drive the same verbs as a user does are in
//! `crates/onetaskgraph-e2e/tests/e2e/assets.rs`. These are the library half the copy verb owes:
//! a folder of Markdown holding the assets, and an `in-memory` destination declaring them
//! native, which serves each at a URL as a hosted destination does — and which a copy that
//! cannot finish has to leave exactly as it found it.

use std::path::Path;

use onetaskgraph_core::{
    Body, Config, CopyAction, CopyItems, CopyRequest, CopyScope, DocumentCreate, Engine,
    EngineError, GlobalId, TaskCreate,
};
use onetaskgraph_plugin_api::{
    Asset, AssetName, AssetPayload, AssetUploads, Capabilities, DependencyEdge, Direction, Health,
    Label, NativeId, Page, PageRequest, Project, ProjectQuery, SecretResolver, SourceError,
    SourceName, Task, TaskQuery, TaskSource, asset_sha256,
};
use secrecy::SecretString;
use serde_json::{Value, json};

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _var: &str) -> Option<SecretString> {
        None
    }
}

/// A folder of Markdown at `root` called `notes`, and an `in-memory` source called `into` whose
/// configuration is `into`.
fn engine(root: &Path, into: Value) -> Engine {
    let config = Config::from_document(json!({"sources": {
        "notes": {"plugin": "local-md", "config": {"root": root}},
        "into": {"plugin": "in-memory", "config": into},
    }}))
    .expect("a valid configuration");
    Engine::build(&config, &NoSecrets)
}

fn name(value: &str) -> AssetName {
    AssetName::new(value).expect("an asset name")
}

/// Create a task called `title` in `notes`, filed under `launch`, holding `shot.png`.
async fn create(engine: &Engine, title: &str, bytes: &[u8]) -> GlobalId {
    engine
        .create_task(&TaskCreate {
            source: SourceName::new("notes").expect("a name"),
            project: NativeId::from("launch"),
            title: title.to_owned(),
            body: Body::plain("![shot](./shot.png)\n"),
            status: None,
            labels: Vec::new(),
            repositories: Vec::new(),
            depends_on: Vec::new(),
            delivers: Vec::new(),
            metadata: Default::default(),
            assets: vec![AssetPayload::of(name("shot.png"), bytes.to_vec())],
            classification: Default::default(),
            assignee: None,
        })
        .await
        .expect("the task is created")
        .task
        .id
}

fn copy_of(items: &[GlobalId], scope: CopyScope) -> CopyRequest {
    CopyRequest {
        items: CopyItems::new(items.to_vec()).expect("at least one item"),
        scope,
        destination: SourceName::new("into").expect("a name"),
        target_task: None,
        match_by: None,
        recreate: false,
        create: false,
        dry_run: false,
        comments: false,
        no_link: false,
        expect: None,
        status_policy: onetaskgraph_core::StatusPolicy::Source,
        statuses: Vec::new(),
        conditions: Default::default(),
    }
}

/// The destination's copy of one task: its content and what it records it uploaded.
async fn landed(engine: &Engine, id: &GlobalId) -> (String, AssetUploads) {
    let task = engine
        .task(id)
        .await
        .expect("the task reads")
        .items
        .remove(0)
        .item;
    let uploads = AssetUploads::read(&task.metadata)
        .expect("a well-formed record")
        .expect("a record of what was uploaded");
    (task.content.unwrap_or_default(), uploads)
}

fn destination(action: &CopyAction) -> GlobalId {
    action
        .destination()
        .cloned()
        .unwrap_or_else(|| panic!("an item that landed, not {action:?}"))
}

#[tokio::test]
async fn a_copy_hands_a_serving_destination_the_assets_and_lands_its_rewritten_references() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = engine(root.path(), json!({"capabilities": {"assets": "native"}}));
    let shot = vec![7_u8; 2048];
    let task = create(&engine, "Pictured", &shot).await;

    let report = engine
        .copy(&copy_of(std::slice::from_ref(&task), CopyScope::Tasks))
        .await
        .expect("the copy lands");
    let copied = destination(&report.items[0].action);
    let (content, uploads) = landed(&engine, &copied).await;
    let upload = &uploads.0[&name("shot.png")];
    assert_eq!(upload.sha256, asset_sha256(&shot));
    assert_eq!(content, format!("![shot]({})\n", upload.url));
    let held = engine
        .task_without_comments(&copied)
        .await
        .expect("the task reads")
        .assets
        .expect("the assets it holds");
    assert_eq!(held.len(), 1);
    assert!(
        held[0].path.is_none(),
        "a serving destination keeps no path"
    );

    // The same bytes again: nothing differs, so nothing is written.
    let again = engine
        .copy(&copy_of(std::slice::from_ref(&task), CopyScope::Tasks))
        .await
        .expect("the copy runs");
    assert!(matches!(
        again.items[0].action,
        CopyAction::Unchanged { .. }
    ));
}

#[tokio::test]
async fn a_copy_into_a_destination_that_stores_no_assets_is_refused_before_anything_is_written() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = engine(root.path(), json!({}));
    let task = create(&engine, "Pictured", &[1, 2, 3]).await;
    let refused = engine
        .copy(&copy_of(std::slice::from_ref(&task), CopyScope::Tasks))
        .await
        .expect_err("refused");
    assert_eq!(
        refused,
        EngineError::AssetsUnsupported {
            name: "into".to_owned(),
            kind: "in-memory".to_owned(),
            record: task.to_string(),
            asset: "shot.png".to_owned(),
        }
    );
}

#[tokio::test]
async fn a_copy_that_cannot_finish_puts_back_the_assets_of_each_task_it_changed() {
    let root = tempfile::tempdir().expect("a folder");
    std::fs::create_dir_all(root.path().join("projects")).expect("a projects folder");
    std::fs::write(
        root.path().join("projects/launch.md"),
        "---\ntitle: Launch\nstatus: todo\n---\n",
    )
    .expect("a project");
    // The second task's update is applied and then refused, so a re-copy that changed the first
    // fails after the first has landed.
    let engine = engine(
        root.path(),
        json!({"capabilities": {"assets": "native", "half_written_titles": ["Second"]}}),
    );
    let first = create(&engine, "First", &[1; 64]).await;
    let second = create(&engine, "Second", &[2; 64]).await;
    let project = GlobalId::new(
        SourceName::new("notes").expect("a name"),
        NativeId::from("launch"),
    );
    let projects = CopyScope::Projects {
        tasks: true,
        documents: false,
    };
    let report = engine
        .copy(&copy_of(std::slice::from_ref(&project), projects.clone()))
        .await
        .expect("the first copy lands");
    let copied = report
        .items
        .iter()
        .find(|outcome| outcome.source == first)
        .map(|outcome| destination(&outcome.action))
        .expect("the first task landed");
    let before = landed(&engine, &copied).await;

    // Both tasks' pictures change at their source; the re-copy writes the first, then fails on
    // the second.
    for (task, bytes) in [(&first, [3; 64]), (&second, [4; 64])] {
        std::fs::write(
            root.path()
                .join(format!("tasks/{}.assets/shot.png", task.native)),
            bytes,
        )
        .expect("new bytes");
    }
    engine
        .copy(&copy_of(std::slice::from_ref(&project), projects))
        .await
        .expect_err("the second task's update is refused");

    // The first task holds exactly what it held before: its content, its record of what was
    // uploaded, and the bytes served at that record's URL.
    assert_eq!(landed(&engine, &copied).await, before);
    let held = engine
        .task_without_comments(&copied)
        .await
        .expect("the task reads")
        .assets
        .expect("the assets it holds");
    assert_eq!(held[0].sha256, asset_sha256(&[1; 64]));
    assert!(
        before.1.0[&name("shot.png")]
            .url
            .contains(&asset_sha256(&[1; 64])),
        "{:?}",
        before.1
    );
}

/// A plain task and a plain document in `notes` holding no asset, filed under `launch`.
async fn plain(engine: &Engine, title: &str) -> (GlobalId, GlobalId) {
    let notes = SourceName::new("notes").expect("a name");
    let task = engine
        .create_task(&TaskCreate {
            source: notes.clone(),
            project: NativeId::from("launch"),
            title: title.to_owned(),
            body: Body::plain("no picture yet\n"),
            status: None,
            labels: Vec::new(),
            repositories: Vec::new(),
            depends_on: Vec::new(),
            delivers: Vec::new(),
            metadata: Default::default(),
            assets: Vec::new(),
            classification: Default::default(),
            assignee: None,
        })
        .await
        .expect("the task is created")
        .task
        .id;
    let document = engine
        .create_document(&DocumentCreate {
            source: notes,
            project: NativeId::from("launch"),
            title: title.to_owned(),
            id: None,
            body: Body::plain("no picture yet\n"),
            labels: Vec::new(),
            repositories: Vec::new(),
            metadata: Default::default(),
            assets: Vec::new(),
            classification: Default::default(),
        })
        .await
        .expect("the document is created")
        .id;
    (task, document)
}

/// Give the record whose file is `file` under `root` a reference to `shot.png`, holding `bytes`.
fn picture(root: &Path, file: &str, bytes: &[u8]) {
    let path = root.join(file);
    let text = std::fs::read_to_string(&path).expect("the record's file");
    std::fs::write(&path, text.replace("no picture yet", "![shot](./shot.png)"))
        .expect("the reference");
    let directory = path.with_extension("assets");
    std::fs::create_dir_all(&directory).expect("its asset directory");
    std::fs::write(directory.join("shot.png"), bytes).expect("the asset");
}

#[tokio::test]
async fn a_copy_that_cannot_finish_takes_away_the_assets_it_added_to_tasks_and_documents() {
    let root = tempfile::tempdir().expect("a folder");
    std::fs::create_dir_all(root.path().join("projects")).expect("a projects folder");
    std::fs::write(
        root.path().join("projects/launch.md"),
        "---\ntitle: Launch\nstatus: todo\n---\n",
    )
    .expect("a project");
    let engine = engine(
        root.path(),
        json!({"capabilities": {
            "assets": "native", "documents": "native", "half_written_titles": ["Second"]
        }}),
    );
    let (first_task, first_document) = plain(&engine, "First").await;
    let (second_task, second_document) = plain(&engine, "Second").await;
    let first_copy = engine
        .copy(&copy_of(
            &[first_task.clone(), second_task.clone()],
            CopyScope::Tasks,
        ))
        .await
        .expect("the tasks land with no assets");
    let documents = engine
        .copy(&copy_of(
            &[first_document.clone(), second_document.clone()],
            CopyScope::Documents,
        ))
        .await
        .expect("the documents land with no assets");
    let copied_task = destination(&first_copy.items[0].action);
    let copied_document = destination(&documents.items[0].action);
    let before_task = engine
        .task(&copied_task)
        .await
        .expect("read")
        .items
        .remove(0)
        .item;
    let before_document = engine
        .document(&copied_document)
        .await
        .expect("read")
        .items
        .remove(0)
        .item;

    // Each first record gains a picture at its source; each second record changes too, and its
    // update is refused after it was applied, so each copy is undone.
    for (file, bytes) in [
        (format!("tasks/{}.md", first_task.native), [5; 32]),
        (format!("tasks/{}.md", second_task.native), [6; 32]),
        (format!("documents/{}.md", first_document.native), [7; 32]),
        (format!("documents/{}.md", second_document.native), [8; 32]),
    ] {
        picture(root.path(), &file, &bytes);
    }
    engine
        .copy(&copy_of(&[first_task, second_task], CopyScope::Tasks))
        .await
        .expect_err("the second task's update is refused");
    engine
        .copy(&copy_of(
            &[first_document, second_document],
            CopyScope::Documents,
        ))
        .await
        .expect_err("the second document's update is refused");

    let task = engine
        .task_without_comments(&copied_task)
        .await
        .expect("read");
    assert_eq!(task.response.items[0].item, before_task);
    assert_eq!(
        task.assets,
        Some(Vec::new()),
        "the asset the copy added is gone"
    );
    let document = engine
        .document_detail(&copied_document)
        .await
        .expect("read");
    assert_eq!(document.response.items[0].item, before_document);
    assert_eq!(
        document.assets,
        Some(Vec::new()),
        "the asset the copy added is gone"
    );
}

#[tokio::test]
async fn a_rendering_edited_by_hand_keeps_its_provenance_verbatim_when_its_references_are_served() {
    let root = tempfile::tempdir().expect("a folder");
    std::fs::create_dir_all(root.path().join("tasks/edited.assets")).expect("a folder");
    // A rendering whose recorded body_digest no longer matches its content: a hand edit.
    let entry = json!({
        "template": "/templates/task.md", "digest": format!("sha256:{}", "a".repeat(64)),
        "body_digest": format!("sha256:{}", "b".repeat(64)),
        "answers_digest": format!("sha256:{}", "c".repeat(64)),
    });
    std::fs::write(
        root.path().join("tasks/edited.md"),
        format!(
            "---\ntitle: Edited\nstatus: todo\nmetadata:\n  onetaskgraph.template: {entry}\n---\n\
             ![shot](./shot.png) edited by hand\n"
        ),
    )
    .expect("a task");
    std::fs::write(root.path().join("tasks/edited.assets/shot.png"), [9; 16]).expect("an asset");
    let engine = engine(root.path(), json!({"capabilities": {"assets": "native"}}));
    let task: GlobalId = "notes:edited".parse().expect("an id");
    let report = engine
        .copy(&copy_of(std::slice::from_ref(&task), CopyScope::Tasks))
        .await
        .expect("the copy lands");
    let copied = destination(&report.items[0].action);
    let held = engine
        .task(&copied)
        .await
        .expect("read")
        .items
        .remove(0)
        .item;
    assert!(
        held.content
            .as_deref()
            .unwrap_or_default()
            .contains("in-memory://"),
        "the reference is served"
    );
    assert_eq!(
        held.metadata["onetaskgraph.template"], entry,
        "carried verbatim"
    );
}

#[tokio::test]
async fn a_project_render_given_assets_is_refused_before_anything_is_read() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = engine(root.path(), json!({}));
    let refused = engine
        .render_project(
            &"notes:launch".parse().expect("an id"),
            &onetaskgraph_core::RenderRequest {
                assets: vec![AssetPayload::of(name("shot.png"), vec![1])],
                ..Default::default()
            },
        )
        .await
        .expect_err("refused");
    assert_eq!(
        refused,
        EngineError::AssetNotReferenced {
            record: "project notes:launch".to_owned(),
            asset: "shot.png".to_owned(),
        }
    );
}

/// Take away the reference and the asset of the record whose file is `file` under `root`.
fn unpicture(root: &Path, file: &str) {
    let path = root.join(file);
    let text = std::fs::read_to_string(&path).expect("the record's file");
    std::fs::write(&path, text.replace("![shot](./shot.png)", "no picture now"))
        .expect("the reference gone");
    std::fs::remove_dir_all(path.with_extension("assets")).expect("its assets gone");
}

#[tokio::test]
async fn a_copy_of_a_record_that_dropped_its_assets_takes_them_away_and_an_undo_puts_them_back() {
    let root = tempfile::tempdir().expect("a folder");
    std::fs::create_dir_all(root.path().join("projects")).expect("a projects folder");
    std::fs::write(
        root.path().join("projects/launch.md"),
        "---\ntitle: Launch\nstatus: todo\n---\n",
    )
    .expect("a project");
    let engine = engine(
        root.path(),
        json!({"capabilities": {"assets": "native", "half_written_titles": ["Second"]}}),
    );
    let first = create(&engine, "First", &[1; 32]).await;
    let second = create(&engine, "Second", &[2; 32]).await;
    let project: GlobalId = "notes:launch".parse().expect("an id");
    let projects = CopyScope::Projects {
        tasks: true,
        documents: false,
    };
    let report = engine
        .copy(&copy_of(std::slice::from_ref(&project), projects.clone()))
        .await
        .expect("the first copy lands");
    let copied = report
        .items
        .iter()
        .find(|outcome| outcome.source == first)
        .map(|outcome| destination(&outcome.action))
        .expect("the first task landed");
    let before = landed(&engine, &copied).await;

    // Both tasks drop their pictures; the second's update is refused after it was applied, so
    // the first's removal is put back.
    for task in [&first, &second] {
        unpicture(root.path(), &format!("tasks/{}.md", task.native));
    }
    engine
        .copy(&copy_of(std::slice::from_ref(&project), projects.clone()))
        .await
        .expect_err("the second task's update is refused");
    assert_eq!(landed(&engine, &copied).await, before);
    let held = engine
        .task_without_comments(&copied)
        .await
        .expect("read")
        .assets
        .expect("listed");
    assert_eq!(held[0].sha256, asset_sha256(&[1; 32]));

    // Copied on its own, the first task lands without the asset it no longer references.
    engine
        .copy(&copy_of(std::slice::from_ref(&first), CopyScope::Tasks))
        .await
        .expect("the copy lands");
    let task = engine.task_without_comments(&copied).await.expect("read");
    assert_eq!(task.assets, Some(Vec::new()));
    assert_eq!(
        task.response.items[0].item.content.as_deref(),
        Some("no picture now\n")
    );
    assert!(
        !task.response.items[0]
            .item
            .metadata
            .contains_key("onetaskgraph.assets"),
        "a record holding no asset records nothing about assets"
    );
}

#[tokio::test]
async fn an_upload_record_of_the_right_shape_with_a_bad_digest_or_no_url_is_refused_and_left_as_it_was()
 {
    let good = "a".repeat(64);
    for (sha256, url) in [
        ("A".repeat(64), "in-memory://kept.png"),
        ("abc".to_owned(), "in-memory://kept.png"),
        (good, ""),
    ] {
        let root = tempfile::tempdir().expect("a folder");
        let recorded = json!({"kept.png": {"sha256": sha256, "url": url}});
        let engine = engine(
            root.path(),
            json!({
                "capabilities": {"assets": "native", "documents": "native"},
                "tasks": [{
                    "id": "landed", "title": "Pictured", "content": "old",
                    "status": {"category": "todo", "name": "Todo"}, "labels": [],
                    "metadata": {
                        "onetaskgraph.origin": "notes:pictured",
                        "onetaskgraph.assets": recorded,
                    },
                }],
            }),
        );
        let task = create(&engine, "Pictured", &[1; 8]).await;
        let refused = engine
            .copy(&copy_of(std::slice::from_ref(&task), CopyScope::Tasks))
            .await
            .expect_err("refused");
        assert!(
            refused.to_string().contains("onetaskgraph.assets")
                && refused.to_string().contains("kept.png"),
            "{refused}"
        );
        let held = engine
            .task(&"into:landed".parse().expect("an id"))
            .await
            .expect("read")
            .items
            .remove(0)
            .item;
        assert_eq!(held.content.as_deref(), Some("old"));
        assert_eq!(held.metadata["onetaskgraph.assets"], recorded);
    }
}

#[tokio::test]
async fn a_record_holding_a_malformed_upload_record_is_refused_and_left_as_it_was() {
    let root = tempfile::tempdir().expect("a folder");
    // The copy's destination already holds the task's counterpart, whose record of what it
    // serves is not one.
    let engine = engine(
        root.path(),
        json!({
            "capabilities": {"assets": "native", "documents": "native"},
            "tasks": [{
                "id": "landed", "title": "Pictured", "content": "old",
                "status": {"category": "todo", "name": "Todo"}, "labels": [],
                "metadata": {
                    "onetaskgraph.origin": "notes:pictured",
                    "onetaskgraph.assets": "not a record",
                },
            }],
        }),
    );
    let task = create(&engine, "Pictured", &[1; 8]).await;
    let refused = engine
        .copy(&copy_of(std::slice::from_ref(&task), CopyScope::Tasks))
        .await
        .expect_err("refused");
    assert!(
        refused.to_string().contains("onetaskgraph.assets"),
        "{refused}"
    );
    let held = engine
        .task(&"into:landed".parse().expect("an id"))
        .await
        .expect("read")
        .items
        .remove(0)
        .item;
    assert_eq!(held.content.as_deref(), Some("old"));

    // A document of the folder holding one is refused by a replacement and by a render.
    std::fs::create_dir_all(root.path().join("documents")).expect("a documents folder");
    let file = root.path().join("documents/held.md");
    let text = "---\ntitle: Held\nmetadata:\n  onetaskgraph.assets: 3\n  \
                onetaskgraph.template: {template: /nowhere.md, digest: \"sha256:00\", \
                body_digest: \"sha256:00\", answers_digest: \"sha256:00\"}\n---\nkept\n";
    std::fs::write(&file, text).expect("a document");
    let replaced = engine
        .create_document(&DocumentCreate {
            source: SourceName::new("notes").expect("a name"),
            project: NativeId::from("launch"),
            title: "Held".to_owned(),
            id: Some(NativeId::from("held")),
            body: Body::plain("replaced\n"),
            labels: Vec::new(),
            repositories: Vec::new(),
            metadata: Default::default(),
            assets: Vec::new(),
            classification: Default::default(),
        })
        .await
        .expect_err("refused");
    assert!(
        replaced.to_string().contains("onetaskgraph.assets"),
        "{replaced}"
    );
    let template = root.path().join("template.md");
    std::fs::write(&template, "---\nonetaskgraph_template: 1\n---\nrendered\n")
        .expect("a template");
    let rendered = engine
        .render_document(
            &"notes:held".parse().expect("an id"),
            &onetaskgraph_core::RenderRequest {
                template: onetaskgraph_core::RenderTemplate::Given(
                    onetaskgraph_core::TemplateInput::File {
                        path: template,
                        search_path: Vec::new(),
                    },
                ),
                ..Default::default()
            },
        )
        .await
        .expect_err("refused");
    assert!(
        rendered.to_string().contains("onetaskgraph.assets"),
        "{rendered}"
    );
    assert_eq!(std::fs::read_to_string(&file).expect("the document"), text);
}

#[tokio::test]
async fn a_regenerate_on_a_serving_source_stores_its_assets_and_points_its_references_at_them() {
    let root = tempfile::tempdir().expect("a folder");
    let template = root.path().join("pictured.md");
    std::fs::write(
        &template,
        "---\nonetaskgraph_template: 1\nvariables:\n  caption:\n    description: What it shows\n\
         ---\n![{{ caption }}](./shot.png)\n",
    )
    .expect("a template");
    let engine = engine(
        root.path(),
        json!({
            "capabilities": {"assets": "native", "documents": "native"},
            "tasks": [{"id": "T-1", "title": "Pictured", "content": "old",
                       "status": {"category": "todo", "name": "Todo"}, "labels": []}],
            "documents": [{"id": "D-1", "title": "Pictured", "content": "old", "labels": []}],
        }),
    );
    let mut answers = onetaskgraph_core::Answers::new();
    answers.set("caption".to_owned(), json!("after"));
    let request = onetaskgraph_core::RenderRequest {
        template: onetaskgraph_core::RenderTemplate::Given(
            onetaskgraph_core::TemplateInput::File {
                path: template,
                search_path: Vec::new(),
            },
        ),
        answers,
        dry_run: false,
        assets: vec![AssetPayload::of(name("shot.png"), vec![4; 16])],
    };
    let task: GlobalId = "into:T-1".parse().expect("an id");
    let document: GlobalId = "into:D-1".parse().expect("an id");
    assert!(
        engine
            .render_task(&task, &request)
            .await
            .expect("renders")
            .changed
    );
    assert!(
        engine
            .render_document(&document, &request)
            .await
            .expect("renders")
            .changed
    );

    let (content, uploads) = landed(&engine, &task).await;
    let upload = &uploads.0[&name("shot.png")];
    assert_eq!(upload.sha256, asset_sha256(&[4; 16]));
    assert_eq!(content, format!("![after]({})\n", upload.url));
    let detail = engine.document_detail(&document).await.expect("read");
    let held = &detail.response.items[0].item;
    assert_eq!(held.content.as_deref(), Some(content.as_str()));
    assert_eq!(
        detail.assets.expect("listed")[0].sha256,
        asset_sha256(&[4; 16])
    );
    // The rendering vouches for the content as it landed, its references served.
    assert_eq!(
        held.metadata["onetaskgraph.template"]["body_digest"],
        json!(onetaskgraph_plugin_api::body_digest(&content))
    );
}

/// Create a task called `title` in `notes` whose content references every one of `assets`,
/// holding each.
async fn holding(engine: &Engine, title: &str, assets: &[(&str, &[u8])]) -> GlobalId {
    let content: String = assets
        .iter()
        .map(|(file, _)| format!("![{file}](./{file})\n"))
        .collect();
    engine
        .create_task(&TaskCreate {
            source: SourceName::new("notes").expect("a name"),
            project: NativeId::from("launch"),
            title: title.to_owned(),
            body: Body::plain(&content),
            status: None,
            labels: Vec::new(),
            repositories: Vec::new(),
            depends_on: Vec::new(),
            delivers: Vec::new(),
            metadata: Default::default(),
            assets: assets
                .iter()
                .map(|(file, bytes)| AssetPayload::of(name(file), bytes.to_vec()))
                .collect(),
            classification: Default::default(),
            assignee: None,
        })
        .await
        .expect("the task is created")
        .task
        .id
}

#[tokio::test]
async fn task_assets_answers_every_asset_a_task_holds_with_its_bytes_and_none_for_one_holding_none()
{
    let root = tempfile::tempdir().expect("a folder");
    let engine = engine(root.path(), json!({}));
    let diagram = vec![1_u8, 2, 3, 250];
    let photo = vec![9_u8; 4096];
    let pictured = holding(
        &engine,
        "Pictured",
        &[("diagram.png", &diagram), ("photo.jpg", &photo)],
    )
    .await;
    let plain = holding(&engine, "Plain", &[]).await;

    let held = engine
        .task_assets(&pictured)
        .await
        .expect("its assets read");
    // Compared by digest, so a mismatch prints two lines rather than every byte.
    let read: Vec<(String, String)> = held
        .iter()
        .map(|asset| (asset.name.to_string(), asset_sha256(&asset.bytes)))
        .collect();
    assert_eq!(
        read,
        vec![
            ("diagram.png".to_owned(), asset_sha256(&diagram)),
            ("photo.jpg".to_owned(), asset_sha256(&photo))
        ]
    );
    assert_eq!(
        engine.task_assets(&plain).await.expect("its assets read"),
        Vec::new()
    );
    // A source that keeps no assets answers none for a task it holds.
    let config = Config::from_document(json!({"sources": {"memory": {
        "plugin": "in-memory",
        "config": {"tasks": [{"id": "T-1", "title": "Kept", "content": null,
                               "status": {"category": "todo", "name": "todo"}, "labels": []}]},
    }}}))
    .expect("a valid configuration");
    let memory = Engine::build(&config, &NoSecrets);
    assert_eq!(
        memory
            .task_assets(&GlobalId::new(
                SourceName::new("memory").expect("a name"),
                NativeId::from("T-1")
            ))
            .await
            .expect("its assets read"),
        Vec::new()
    );
}

#[tokio::test]
async fn task_assets_refuses_a_missing_task_and_an_unreadable_source_naming_the_id_and_the_reason()
{
    let root = tempfile::tempdir().expect("a folder");
    let engine = engine(root.path(), json!({}));
    let missing = GlobalId::new(
        SourceName::new("notes").expect("a name"),
        NativeId::from("nowhere"),
    );
    let error = engine
        .task_assets(&missing)
        .await
        .expect_err("no such task");
    assert_eq!(error.id, missing);
    assert!(
        matches!(&error.error, EngineError::NoSuchTask { id } if id == "notes:nowhere"),
        "{error:?}"
    );
    assert!(error.to_string().contains("notes:nowhere"), "{error}");

    let unknown: GlobalId = "unknown:T-1".parse().expect("a qualified id");
    let error = engine
        .task_assets(&unknown)
        .await
        .expect_err("an unknown source");
    assert!(error.to_string().contains("unknown:T-1"), "{error}");
    assert!(matches!(error.error, EngineError::UnknownSource { .. }));

    // The task's asset directory replaced by a file: its source can no longer list them.
    let broken = holding(&engine, "Broken", &[("shot.png", &[5, 6, 7])]).await;
    let directory = root.path().join("tasks/broken.assets");
    std::fs::remove_dir_all(&directory).expect("the asset directory");
    std::fs::write(&directory, "not a directory").expect("a file in its place");
    let error = engine
        .task_assets(&broken)
        .await
        .expect_err("an unreadable asset directory");
    assert_eq!(error.id, broken);
    assert!(
        matches!(&error.error, EngineError::SourceFailed { name, .. } if name == "notes"),
        "{error:?}"
    );
    let message = error.to_string();
    assert!(message.contains("notes:broken"), "{message}");
    assert!(message.contains("broken.assets"), "{message}");

    // A source that could not be built at all: its root is not there.
    let config = Config::from_document(json!({"sources": {"gone": {
        "plugin": "local-md", "config": {"root": root.path().join("absent")},
    }}}))
    .expect("a valid configuration");
    let gone = GlobalId::new(
        SourceName::new("gone").expect("a name"),
        NativeId::from("T-1"),
    );
    let error = Engine::build(&config, &NoSecrets)
        .task_assets(&gone)
        .await
        .expect_err("an unbuilt source");
    assert!(
        matches!(&error.error, EngineError::SourceUnavailable { name, .. } if name == "gone"),
        "{error:?}"
    );
    let message = error.to_string();
    assert!(message.contains("gone:T-1"), "{message}");
    assert!(message.contains("cannot canonicalize root"), "{message}");
}

/// A source losing an asset between listing and fetching it. The engine is driven through
/// its public API; this peer supplies the two contradictory answers a real store may give
/// when an image disappears during a read.
struct LosingAsset {
    inner: Box<dyn TaskSource>,
    unavailable: bool,
}

#[async_trait::async_trait]
impl TaskSource for LosingAsset {
    fn kind(&self) -> &'static str {
        self.inner.kind()
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
        direction: Direction,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.inner.task_dependencies(id, direction, page).await
    }
    async fn project_dependencies(
        &self,
        id: &NativeId,
        direction: Direction,
        page: &PageRequest,
    ) -> Result<Page<DependencyEdge>, SourceError> {
        self.inner.project_dependencies(id, direction, page).await
    }

    async fn task_assets(&self, _id: &NativeId) -> Result<Vec<Asset>, SourceError> {
        Ok(vec![Asset {
            name: name("shot.png"),
            sha256: asset_sha256(b"image"),
            content_type: name("shot.png").content_type(),
            path: None,
        }])
    }
    async fn task_asset(
        &self,
        _id: &NativeId,
        _name: &AssetName,
    ) -> Result<Option<Vec<u8>>, SourceError> {
        if self.unavailable {
            Err(SourceError::Unavailable {
                message: "image store disconnected".to_owned(),
            })
        } else {
            Ok(None)
        }
    }
}

#[tokio::test]
async fn task_assets_refuses_an_image_that_disappears_or_cannot_be_fetched_after_listing() {
    for unavailable in [false, true] {
        let source = SourceName::new("remote").expect("a source name");
        let inner = onetaskgraph_core::plugin_for("in-memory")
            .expect("registered")
            .build(
                &source,
                &json!({"tasks": [{"id": "T-1", "title": "Pictured",
                "status": {"category": "todo", "name": "Todo"}, "labels": []}]}),
                &NoSecrets,
            )
            .expect("a source");
        let engine = Engine::new(
            vec![onetaskgraph_core::ConfiguredSource::Ready(
                onetaskgraph_core::ResolvedSource::adopt(
                    source.clone(),
                    Box::new(LosingAsset { inner, unavailable }),
                ),
            )],
            vec![source],
        );
        let id: GlobalId = "remote:T-1".parse().expect("an id");
        let error = engine
            .task_assets(&id)
            .await
            .expect_err("the image cannot be read");
        let message = error.to_string();
        assert!(message.contains("remote:T-1"), "{message}");
        assert!(
            message.contains(if unavailable {
                "image store disconnected"
            } else {
                "shot.png but holds no bytes"
            }),
            "{message}"
        );
        assert!(matches!(error.error, EngineError::SourceFailed { .. }));
    }
}

#[tokio::test]
async fn task_assets_returns_unreferenced_images_after_references_in_name_order() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = engine(root.path(), json!({}));
    let id = holding(&engine, "Pictured", &[("shot.png", b"shot")]).await;
    let directory = root.path().join("tasks/pictured.assets");
    std::fs::write(directory.join("z.png"), b"last").expect("an unreferenced image");
    std::fs::write(directory.join("a.png"), b"first").expect("an unreferenced image");
    let held = engine.task_assets(&id).await.expect("all held images");
    assert_eq!(
        held.iter()
            .map(|asset| (asset.name.as_str(), asset.bytes.as_slice()))
            .collect::<Vec<_>>(),
        vec![
            ("shot.png", b"shot".as_slice()),
            ("a.png", b"first".as_slice()),
            ("z.png", b"last".as_slice())
        ]
    );
}

#[tokio::test]
async fn task_assets_preserves_a_task_read_failure_and_its_qualified_id() {
    let root = tempfile::tempdir().expect("a folder");
    let engine = engine(root.path(), json!({}));
    let id = holding(&engine, "Broken", &[]).await;
    std::fs::write(
        root.path().join("tasks/broken.md"),
        "---\ntitle: [unterminated\n---\n",
    )
    .expect("a damaged task file");
    let error = engine
        .task_assets(&id)
        .await
        .expect_err("the task cannot be read");
    let message = error.to_string();
    assert!(message.contains("notes:broken"), "{message}");
    assert!(message.contains("broken.md"), "{message}");
    assert!(matches!(error.error, EngineError::SourceFailed { .. }));
}
