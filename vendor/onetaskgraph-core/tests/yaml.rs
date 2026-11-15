//! The engine and the local Markdown source read YAML alike.
//!
//! Each declares its own serde-saphyr options, because a plugin never depends on the engine,
//! so this is the gate that keeps the two from drifting apart: the same spellings, read as a
//! template's answers by the engine and as a task's metadata by the source, come back as the
//! same values — and as the values pinned here, so a reader that moves is seen to move.

use onetaskgraph_core::{Answers, TemplateLoader};
use onetaskgraph_plugin_api::{NativeId, SecretResolver, SourceName, SourcePlugin};
use secrecy::SecretString;
use serde_json::{Value, json};

/// One flow mapping holding every spelling whose reading a YAML reader chooses.
const SPELLINGS: &str = "{word: yes, short: n, toggle: off, octal: 010, grouped: 1_000, \
                         huge: 123456789012345678901234567890, binary: !!binary aGVsbG8=, \
                         local: !local x, '<<': {a: 1}, stamp: 2026-10-13}";

struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn get(&self, _: &str) -> Option<SecretString> {
        None
    }
}

/// `SPELLINGS` as the engine reads an answers file.
fn as_answers() -> Value {
    let template = TemplateLoader::new()
        .with_template(
            "t.md",
            "---\nonetaskgraph_template: 1\nvariables:\n  v: {description: d, type: object}\n---\n",
        )
        .load_name("t.md")
        .expect("the template loads");
    let answers = Answers::from_yaml(&format!("v: {SPELLINGS}\n")).expect("the answers read");
    template.resolve(&answers).expect("the answers resolve")["v"].clone()
}

/// `SPELLINGS` as the local Markdown source reads a task's metadata.
async fn as_metadata() -> Value {
    let root = tempfile::tempdir().expect("a folder");
    std::fs::create_dir_all(root.path().join("tasks")).expect("the tasks folder");
    std::fs::write(
        root.path().join("tasks/a.md"),
        format!("---\ntitle: A\nstatus: todo\nmetadata:\n  myapp.v: {SPELLINGS}\n---\n"),
    )
    .expect("a task");
    let source = onetaskgraph_local_md::Plugin
        .build(
            &SourceName::new("work").expect("a name"),
            &json!({ "root": root.path() }),
            &NoSecrets,
        )
        .expect("the folder builds");
    let task = source
        .get_task(&NativeId("a".to_owned()))
        .await
        .expect("the task reads")
        .expect("the task is held");
    task.metadata["myapp.v"].clone()
}

#[tokio::test]
async fn the_engine_and_the_markdown_source_read_every_spelling_alike() {
    let answers = as_answers();
    assert_eq!(answers, as_metadata().await, "the two readers agree");
    assert_eq!(
        answers,
        json!({
            "word": "yes",
            "short": "n",
            "toggle": "off",
            "octal": 10.0,
            "grouped": 1000,
            "huge": 1.2345678901234568e29,
            "binary": "hello",
            "local": "x",
            "<<": {"a": 1},
            "stamp": "2026-10-13",
        })
    );
}
