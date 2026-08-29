//! Not a double at all: a **real** `onetaskgraph` source, and a label-strict one.
//!
//! `onetaskgraph`'s hosted destinations differ in what they will accept, and the one this
//! product's boards actually live on — `github-projects` — refuses a write outright when
//! the item's labels differ from the labels being written: *"GitHub issue labels differ
//! from the labels being written"*. A projection that dropped a label would therefore stop
//! reaching such a board the moment anybody labelled one of its issues, permanently and
//! silently. Nothing in the offline tier can reach GitHub, and a store that accepts every
//! write cannot show the difference — so this is that destination's *rule*, over a real
//! store.
//!
//! It is a source rather than a stand-in for one. It speaks `onetaskgraph`'s own stdio
//! plugin protocol on the wire the engine spawns it on, and every read and every write it
//! serves is served by the real `local-md` plugin, hosted in the shipped
//! `onetaskgraph-source` program that this process proxies. The only thing it adds is the
//! refusal: a `write_task` or `write_project` onto an item this store already holds is
//! refused, without reaching the store, when the labels being written are not the labels
//! that item carries.
//!
//! Its `config:` block — which a `subprocess` source hands over verbatim as `settings:` —
//! names both halves:
//!
//! * `host` — the `onetaskgraph-source` executable that hosts the real `local-md` plugin.
//! * `root` — the folder of Markdown that plugin serves.
//!
//! **What is typed here and what is not** is the protocol's own division. The handshake,
//! the write, and the read a write is judged against are what this program *interprets*,
//! so all three are parsed into the shapes below and a line that is not one of them is a
//! boundary failure rather than something to guess at. A request of any other method is
//! what it *relays*, and relaying is the whole of what it does with one: its parameters
//! stay the JSON they arrived as, because narrowing a message this program only carries
//! would make it refuse a method the hosted plugin serves and it does not — which is the
//! one failure a proxy must not have.
//!
//! **`refused` is the only error kind spelled here**, and it is the one this source
//! genuinely means. Every other way this program can fail is a plugin that stops: §1 of the
//! protocol has the engine report *that* as `unavailable`, quoting whatever the plugin
//! wrote to standard error, so there is nothing to restate and nothing to drift.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use serde::Deserialize;
use serde_json::{json, Value};

/// The refusal this destination makes, worded as the shipped one is.
const DIFFER: &str = "destination labels differ from the labels being written";

// llmlint: ignore-block[boundary_inputs_validated] These shapes deliberately **ignore**
// members they do not know, because §2.1 of the plugin protocol requires it: that is how a
// later protocol version adds an optional field without a version bump, and a peer that
// refused one would break against a build newer than this program. What is validated is
// what this program acts on — every field below is required and typed, and a native id
// this store could never hold is refused by name.

/// The handshake, as far as this program reads it: its own settings, and the fields the
/// hosted plugin's handshake has to carry through unchanged.
#[derive(Deserialize)]
struct Handshake {
    id: Value,
    params: HandshakeParams,
}

#[derive(Deserialize)]
struct HandshakeParams {
    protocol_version: u32,
    #[serde(default)]
    engine: Value,
    source_name: String,
    config: Settings,
}

/// This source's own `config:` block: what to host, and what it serves.
#[derive(Deserialize)]
struct Settings {
    host: PathBuf,
    root: PathBuf,
}

/// One request off the engine's wire.
///
/// `params` stays JSON deliberately: every method but the two writes is relayed, and a
/// proxy that narrowed a message it only carries would refuse what the hosted plugin
/// serves.
#[derive(Deserialize)]
struct Request {
    id: Value,
    method: String,
    params: Value,
}

/// A write, which is the one request this program interprets rather than relays.
#[derive(Deserialize)]
struct WriteParams {
    write: ItemWrite,
}

#[derive(Deserialize)]
struct ItemWrite {
    /// The item at **this** source to update, or `null` to create one. A plugin never
    /// speaks in qualified ids (§3.2), so this is the store's own native id.
    target: Option<NativeId>,
    item: WrittenItem,
}

#[derive(Deserialize)]
struct WrittenItem {
    labels: Vec<Label>,
}

/// The hosted plugin's answer to a `get_task` or `get_project`.
#[derive(Deserialize)]
struct Held {
    /// `null` inside where the store holds no such item, which is the protocol's answer
    /// for one that is simply not there rather than an error.
    result: HeldItem,
}

#[derive(Deserialize)]
struct HeldItem {
    #[serde(rename = "task", alias = "project")]
    item: Option<HeldLabels>,
}

#[derive(Deserialize)]
struct HeldLabels {
    labels: Vec<Label>,
}

/// One label, compared whole — id, name and colour — exactly as the shipped destination
/// compares the labels it holds against the labels it is handed.
#[derive(Deserialize, PartialEq, Eq)]
struct Label {
    id: String,
    name: String,
    #[serde(default)]
    color: Option<String>,
}

/// A source's own opaque identifier for one item: any non-empty string, colons included.
///
/// A newtype rather than a `String` because an empty one names nothing any store could
/// hold, and a write whose target was blank would otherwise be compared against whatever
/// the store answered for it.
#[derive(Deserialize)]
#[serde(try_from = "String")]
struct NativeId(String);

impl TryFrom<String> for NativeId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err("a native id is not empty".to_owned());
        }
        Ok(Self(value))
    }
}

// llmlint: ignore-end[boundary_inputs_validated]

impl std::fmt::Display for Label {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.name)
    }
}

/// One end of the pipe to the hosted `local-md` plugin.
struct Host {
    input: std::process::ChildStdin,
    output: BufReader<std::process::ChildStdout>,
    child: std::process::Child,
}

impl Host {
    /// One request out and its response back. Strictly sequential: this proxy never has
    /// more than one request outstanding, so a response is always the one just asked for.
    fn ask(&mut self, request: &Value) -> Option<Value> {
        writeln!(self.input, "{request}").ok()?;
        self.input.flush().ok()?;
        let mut line = String::new();
        match self.output.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => serde_json::from_str(&line).ok(),
        }
    }
}

/// Stop, saying why where the protocol says a stopping plugin says it.
///
/// §1: a plugin that exits before answering a request has failed it, and the engine reports
/// that as `unavailable` quoting standard error. So this program never spells that kind
/// itself — it says the sentence and goes.
fn stop(why: &str) -> ExitCode {
    eprintln!("label-strict-source: {why}");
    ExitCode::FAILURE
}

/// Refuse one request: this source understood it and will not do it.
fn refuse(id: &Value, message: &str) {
    let response = json!({"id": id, "error": {"kind": "refused", "message": message}});
    println!("{response}");
}

fn main() -> ExitCode {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let Some(Ok(first)) = lines.next() else {
        return ExitCode::SUCCESS;
    };
    let handshake: Handshake = match serde_json::from_str(&first) {
        Ok(value) => value,
        Err(error) => return stop(&format!("unreadable handshake: {error}")),
    };

    let spawned = Command::new(&handshake.params.config.host)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            return stop(&format!(
                "cannot run {}: {error}",
                handshake.params.config.host.display()
            ))
        }
    };
    let mut host = Host {
        input: child.stdin.take().expect("a piped standard input"),
        output: BufReader::new(child.stdout.take().expect("a piped standard output")),
        child,
    };

    // The hosted plugin's own handshake, in the version the engine asked for and under the
    // name the engine gave this source, so what it reports is the real `local-md` source's
    // capabilities rather than a second opinion about them.
    let hosted = json!({
        "id": "strict-initialize",
        "method": "initialize",
        "params": {
            "protocol_version": handshake.params.protocol_version,
            "engine": handshake.params.engine,
            "source_name": handshake.params.source_name,
            "config": {
                "kind": "local-md",
                "config": {"root": handshake.params.config.root},
            },
            "secrets": {},
        },
    });
    let Some(mut answered) = host.ask(&hosted) else {
        return stop("the hosted local-md plugin did not start");
    };
    answered["id"] = handshake.id;
    println!("{answered}");

    let ending = relay(&mut host, lines);
    drop(host.input);
    let _ = host.child.wait();
    ending
}

/// Carry every later request to the hosted plugin, refusing the writes this source refuses.
fn relay(host: &mut Host, lines: impl Iterator<Item = std::io::Result<String>>) -> ExitCode {
    for line in lines {
        let Ok(line) = line else {
            return stop("standard input could not be read");
        };
        // A line this side cannot read is a violation of the framing rather than a request
        // to answer: there is no id to answer under, and inventing one would be a second
        // response to somebody.
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => return stop(&format!("unreadable request: {error}")),
        };
        match judged(host, &request) {
            Judgement::Refused(why) => refuse(&request.id, &why),
            Judgement::Unreadable(why) => return stop(&why),
            Judgement::Relay => {
                let relayed = json!({
                    "id": request.id, "method": request.method, "params": request.params,
                });
                let Some(answered) = host.ask(&relayed) else {
                    return stop("the hosted local-md plugin stopped");
                };
                println!("{answered}");
            }
        }
    }
    ExitCode::SUCCESS
}

/// What this destination does with one request before the store sees it.
enum Judgement {
    /// Carry it to the hosted plugin.
    Relay,
    /// Refuse it: the labels being written are not the labels the item carries.
    Refused(String),
    /// Stop: a message crossed this boundary that this source cannot act on, so it can
    /// neither apply its rule nor honestly relay past it.
    Unreadable(String),
}

/// Whether this destination refuses the write in `request`.
///
/// The rule is the shipped `github-projects` one: an update whose labels are not the
/// labels the destination item already carries is refused before anything is written.
/// A create has no item to disagree with, and a read is not a write.
fn judged(host: &mut Host, request: &Request) -> Judgement {
    let reading = match request.method.as_str() {
        "write_task" => "get_task",
        "write_project" => "get_project",
        _ => return Judgement::Relay,
    };
    let write: WriteParams = match serde_json::from_value(request.params.clone()) {
        Ok(write) => write,
        Err(error) => {
            return Judgement::Unreadable(format!("this write is not one it can read: {error}"))
        }
    };
    let ItemWrite { target, item } = write.write;
    let Some(target) = target else {
        return Judgement::Relay;
    };
    let Some(answered) = host.ask(&json!({
        "id": "strict-held",
        "method": reading,
        "params": {"id": target.0},
    })) else {
        return Judgement::Unreadable("the hosted local-md plugin stopped".to_owned());
    };
    // An answer this side cannot read is never read as "nothing is held there": that would
    // let a write past the rule this source exists to apply.
    let held: Held = match serde_json::from_value(answered) {
        Ok(held) => held,
        Err(error) => {
            return Judgement::Unreadable(format!(
                "the hosted local-md plugin answered {reading} for '{}' with something it \
                 cannot read: {error}",
                target.0
            ))
        }
    };
    // The store holds no such item, which is its own refusal to make rather than this
    // source's: there are no labels to disagree with.
    let Some(held) = held.result.item else {
        return Judgement::Relay;
    };
    if held.labels == item.labels {
        return Judgement::Relay;
    }
    Judgement::Refused(format!(
        "{DIFFER}: {} carries [{}] and the write carries [{}]",
        target.0,
        named(&held.labels),
        named(&item.labels),
    ))
}

fn named(labels: &[Label]) -> String {
    labels
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
