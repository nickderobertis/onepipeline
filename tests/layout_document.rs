//! The published planner-channel document against the layout this engine
//! compiles in.
//!
//! `schemas/planner-channel.json` declares the layout's preparation in the bus's
//! step vocabulary, while the engine runs [`PlannerChannel`]'s `prepare`, which
//! is code. Two properties hold the two to one another:
//!
//! - **Recorded histories**: every record the directories under
//!   `tests/recorded/channel/` hold, offered through a bus of each layout into
//!   an empty directory, writes the same files byte for byte.
//! - **Offers**: every shape of offer this crate's own layout tests drive —
//!   routed, kept, stamped and refused — is prepared the same way by both, and
//!   where the step vocabulary cannot say what the code says, the difference is
//!   asserted here case by case rather than left to be found.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use onemessagebus::{
    Allowlist, Author, Bus, Config, Freshness, Layout, Layouts, LinkResolver, OpWord, QueueName,
    SchemaLink, TransportKinds,
};
use onepipeline::channel::layout::{
    bundle_json, PlannerChannel, COMMANDS, COMMAND_OUTCOMES, FILES, PLANNER_CHANNEL, REPLIES,
    SURFACES,
};
use serde_json::{json, Value};

const RECORDED: &[&str] = &[
    "domain-driven-modularity-2",
    "onemessagebus-repair-2",
    "onemessagebus-repair-2-pending",
    "onemessagebus-repair-2-pending-bus",
    "waiting-updates",
];

/// A scratch directory of this process's own, removed when it is dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "onepipeline-layout-document-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn queue(name: &str) -> QueueName {
    name.parse().expect("a queue name")
}

/// The document, written where a host's configuration would link it, and
/// resolved the way the bus resolves a link.
fn linked(scratch: &Path) -> Arc<dyn Layout> {
    let path = scratch.join("planner-channel.json");
    std::fs::write(&path, bundle_json()).expect("the document is written");
    let link = SchemaLink::parse(path.to_str().expect("a UTF-8 path")).expect("a link");
    let resolved = LinkResolver::new(None)
        .resolve(&link, Freshness::CachedFirst)
        .expect("the document resolves");
    Layouts::new()
        .with_linked(std::slice::from_ref(&resolved))
        .expect("the document links")
        .get(PLANNER_CHANNEL)
        .cloned()
        .expect("the document declares the planner channel")
}

/// A bus of `layout` keeping its queues in `dir`.
fn bus(layout: Arc<dyn Layout>, dir: &Path) -> Bus {
    Config::local(dir, Some(PLANNER_CHANNEL))
        .resolve(&Layouts::new().with(layout), &TransportKinds::builtin())
        .expect("the bus resolves")
}

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| serde_json::from_str(line).expect("a recorded line is JSON"))
                .collect()
        })
        .unwrap_or_default()
}

/// Every record a recorded directory holds, offered to a bus of each layout,
/// writes the same channel directory byte for byte.
#[test]
fn every_recorded_history_offered_through_the_document_writes_what_the_compiled_layout_writes() {
    let scratch = Scratch::new("recorded");
    let document = linked(scratch.path());
    for name in RECORDED {
        let recorded = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/recorded/channel")
            .join(name);
        let compiled_dir = scratch.path().join(format!("{name}-compiled"));
        let linked_dir = scratch.path().join(format!("{name}-linked"));
        let compiled = bus(Arc::new(PlannerChannel), &compiled_dir);
        let linked = bus(Arc::clone(&document), &linked_dir);

        let mut offers: Vec<(&str, Value)> = Vec::new();
        for mut line in lines(&recorded.join("surfaces.jsonl")) {
            let fields = line.as_object_mut().expect("a surface line is an object");
            if fields.remove("event") == Some(json!("queued")) {
                offers.push((SURFACES, line));
            }
        }
        for line in lines(&recorded.join("replies.jsonl")) {
            // Offered framed, as the log holds it, and bare, as a planner
            // writes one — which is what routes by halves.
            offers.push((REPLIES, line["reply"].clone()));
            offers.push((REPLIES, line));
        }
        offers.extend(
            lines(&recorded.join("commands.jsonl"))
                .into_iter()
                .map(|line| (COMMANDS, line)),
        );
        offers.extend(
            lines(&recorded.join("command-outcomes.jsonl"))
                .into_iter()
                .map(|line| (COMMAND_OUTCOMES, line)),
        );
        assert!(!offers.is_empty(), "{name}: the recording holds nothing");
        for (target, record) in offers {
            let one = compiled.send(&queue(target), record.clone());
            let other = linked.send(&queue(target), record.clone());
            assert_eq!(
                one.as_ref().map(Vec::len).map_err(ToString::to_string),
                other.as_ref().map(Vec::len).map_err(ToString::to_string),
                "{name}: {record} offered to {target}"
            );
        }
        for file in FILES {
            let read = |dir: &Path| stamped_out(&std::fs::read_to_string(dir.join(file)).ok());
            let written = read(&linked_dir);
            // The one difference `docs/contract.md` states for a routed record:
            // a bare envelope that omits its author routes a command record that
            // omits it too. Read as the engine writes it, and nothing else is.
            let written = if file == "commands.jsonl" {
                written.map(|text| author_as_the_engine_writes_it(&text))
            } else {
                written
            };
            assert_eq!(
                read(&compiled_dir),
                written,
                "{name}: the document's bus wrote a different {file}"
            );
        }
    }
}

/// `text`, a command log, with every record that omits its author given the
/// author an omitted one means, where the engine writes it: after the id.
fn author_as_the_engine_writes_it(text: &str) -> String {
    text.lines()
        .map(|line| match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(fields)) if !fields.contains_key("author") => {
                let mut written = serde_json::Map::new();
                for (key, value) in fields {
                    let id = key == "id";
                    written.insert(key, value);
                    if id {
                        written.insert("author".to_owned(), json!("planner"));
                    }
                }
                Value::Object(written).to_string()
            }
            _ => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` with every `"at"` stamp — the one member each writer stamps with its
/// own clock — read as the same instant.
fn stamped_out(text: &Option<String>) -> Option<String> {
    text.as_ref().map(|text| {
        text.lines()
            .map(|line| match serde_json::from_str::<Value>(line) {
                Ok(Value::Object(mut fields)) if fields.contains_key("at") => {
                    fields.insert("at".to_owned(), json!(0));
                    Value::Object(fields).to_string()
                }
                _ => line.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// The allowlist the offers below are prepared under: the layout's own, plus a
/// configured `sentinel` granted a finding and refused an attest and the
/// completion, as `tests/channel_layout.rs` configures it.
fn grants(layout: &dyn Layout) -> Allowlist<OpWord> {
    let mut grants = layout.allowlist();
    let sentinel = Author::from("sentinel");
    grants.grant(sentinel.clone(), OpWord("finding".to_owned()));
    grants.refuse(
        sentinel.clone(),
        &OpWord("attest".to_owned()),
        "only a person may attest",
    );
    grants.refuse(
        sentinel,
        &OpWord("complete".to_owned()),
        "the planner decides completion",
    );
    grants
}

type Prepared = Result<Vec<(String, Value)>, String>;

/// What `layout` prepares `record` offered to `target` as, each stamp read as
/// the same instant.
fn prepared(layout: &dyn Layout, target: &str, record: &Value) -> Prepared {
    layout
        .prepare(&queue(target), record.clone(), &grants(layout))
        .map(|routed| {
            routed
                .into_iter()
                .map(|(queue, mut record)| {
                    if let Value::Object(fields) = &mut record {
                        // The id a numbered queue allocates on push, whatever
                        // an offer carried: the engine writes a placeholder.
                        if target != SURFACES && fields.get("id") == Some(&json!(0)) {
                            fields.shift_remove("id");
                        }
                        for stamp in ["at", "queued_at"] {
                            if fields.get(stamp).is_some_and(Value::is_u64) {
                                fields.insert(stamp.to_owned(), json!(0));
                            }
                        }
                    }
                    (queue.to_string(), record)
                })
                .collect()
        })
}

/// Every offer both prepare alike.
#[test]
fn every_offer_is_prepared_by_the_document_as_the_compiled_layout_prepares_it() {
    let scratch = Scratch::new("offers");
    let document = linked(scratch.path());
    let compiled = PlannerChannel;
    let alike: &[(&str, Value)] = &[
        // Routed by its halves, a version-2 envelope read at 3.
        (
            REPLIES,
            json!({"version": 2, "author": "sentinel", "completion": false, "message": "go on", "commands": [{"op": "finding", "message": "look"}]}),
        ),
        (
            REPLIES,
            json!({"version": 3, "author": "sentinel", "commands": [{"op": "finding", "message": "look"}]}),
        ),
        (REPLIES, json!({"message": "carry on"})),
        (REPLIES, json!({})),
        (REPLIES, json!({"author": "sentinel", "message": "noted"})),
        // Refused, in the engine's words.
        (
            REPLIES,
            json!({"version": 3, "author": "sentinel", "commands": [{"op": "attest", "ref": "x"}]}),
        ),
        (REPLIES, json!({"author": "sentinel", "completion": true})),
        (
            REPLIES,
            json!({"version": 1, "commands": [{"op": "cancel", "id": "a"}]}),
        ),
        (
            REPLIES,
            json!({"version": 3, "commands": [{"op": "frobnicate"}]}),
        ),
        (REPLIES, json!({"author": "nobody", "message": "hi"})),
        // A framed reply: checked and kept whole, a version-2 edit included.
        (
            REPLIES,
            json!({"id": 0, "reply": {"message": "carry on"}, "at": 5}),
        ),
        (
            REPLIES,
            json!({"id": 0, "reply": {"version": 2, "commands": [{"op": "cancel", "id": "a"}]}, "at": 5}),
        ),
        (
            REPLIES,
            json!({"id": 0, "reply": {"author": "sentinel", "completion": true}, "at": 5}),
        ),
        (
            COMMANDS,
            json!({"id": 0, "author": "sentinel", "commands": [{"op": "settle", "id": "a"}]}),
        ),
        (
            COMMANDS,
            json!({"id": 0, "commands": [{"op": "cancel", "id": "a"}]}),
        ),
        // A surface: stamped, and its `about` read as its workstream.
        (
            SURFACES,
            json!({"id": 0, "kind": "finding", "message": "m", "source": "proposal", "blocking": false}),
        ),
        (
            SURFACES,
            json!({"id": 0, "kind": "question", "message": "m", "source": "proposal", "blocking": true, "about": "build"}),
        ),
        (SURFACES, json!("text")),
        (COMMAND_OUTCOMES, json!({"id": 3, "applied": true})),
    ];
    for (target, record) in alike {
        assert_eq!(
            prepared(&compiled, target, record),
            prepared(document.as_ref(), target, record),
            "{record} offered to {target}"
        );
    }
}

/// Where the bus's step vocabulary cannot say what the compiled-in preparation
/// does, the two differ — and only as `docs/contract.md` states, each case
/// asserted as it is so a difference that grew or went away fails here.
///
/// Two are gaps in the vocabulary for routed records: no step sets a member to
/// a value when it is absent, so a routed command record keeps an omitted
/// author omitted; and none drops a member that holds its default, so a routed
/// reply keeps an `author: planner` the engine drops. Each reads back as the
/// same record, because `planner` is what an omitted author means. The rest
/// are refusals of malformed envelopes, made in the bus's words: a schema's,
/// or the grant step's.
#[test]
fn where_the_step_vocabulary_cannot_say_what_the_code_does_the_document_differs_as_stated() {
    let scratch = Scratch::new("differences");
    let document = linked(scratch.path());
    let compiled = PlannerChannel;
    let routed = |pairs: &[(&str, Value)]| -> Prepared {
        Ok(pairs
            .iter()
            .map(|(queue, record)| ((*queue).to_owned(), record.clone()))
            .collect())
    };
    let refused = |why: &str| -> Prepared { Err(why.to_owned()) };
    let edit = json!([{"op": "cancel", "id": "a"}]);
    let differing: Vec<(&str, Value, Prepared, Prepared)> = vec![
        (
            REPLIES,
            json!({"version": 3, "message": "go on", "commands": edit}),
            routed(&[
                (COMMANDS, json!({"author": "planner", "commands": edit})),
                (
                    REPLIES,
                    json!({"reply": {"version": 3, "message": "go on", "commands": edit}, "at": 0}),
                ),
            ]),
            routed(&[
                (COMMANDS, json!({"commands": edit})),
                (
                    REPLIES,
                    json!({"reply": {"version": 3, "message": "go on", "commands": edit}, "at": 0}),
                ),
            ]),
        ),
        (
            REPLIES,
            json!({"author": "planner", "message": "hi"}),
            routed(&[(REPLIES, json!({"reply": {"message": "hi"}, "at": 0}))]),
            routed(&[(
                REPLIES,
                json!({"reply": {"author": "planner", "message": "hi"}, "at": 0}),
            )]),
        ),
        (
            REPLIES,
            json!({"message": "hi", "context": "retired"}),
            refused(
                "the reply is malformed: unknown field `context`, expected one of `version`, \
                 `author`, `completion`, `message`, `reason`, `commands`",
            ),
            refused(
                "the reply is malformed: Additional properties are not allowed ('context' was \
                 unexpected)",
            ),
        ),
        (
            REPLIES,
            json!({"id": 0, "reply": [3], "at": 5}),
            refused("the reply is malformed: a reply envelope is a JSON object"),
            refused("the reply is malformed: [3] is not of type \"object\""),
        ),
        (
            COMMANDS,
            json!({"id": 0, "author": 7, "commands": []}),
            refused("the envelope's author: invalid type: integer `7`, expected a string"),
            refused("the envelope's author: `author` is not text"),
        ),
        // Passed by the document's steps, and refused by the queue's schema when
        // the bus pushes it — see below.
        (
            COMMANDS,
            json!({"id": 0, "author": "planner"}),
            refused("a command envelope carries `commands`"),
            routed(&[(COMMANDS, json!({"author": "planner"}))]),
        ),
    ];
    for (target, record, from_code, from_document) in differing {
        assert_eq!(
            (
                prepared(&compiled, target, &record),
                prepared(document.as_ref(), target, &record)
            ),
            (from_code, from_document),
            "{record} offered to {target}"
        );
    }
    let pushed = bus(Arc::clone(&document), &scratch.path().join("pushed"))
        .send(&queue(COMMANDS), json!({"author": "planner"}))
        .expect_err("a command envelope with no commands is refused by the queue's schema");
    assert!(pushed.to_string().contains("commands"), "{pushed}");
}
