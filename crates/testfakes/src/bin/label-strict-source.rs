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
//! **What is typed here and what is not** is the protocol's own division. The handshake
//! and the write are what this program *interprets*, so both are parsed into the shapes
//! below and a line that is not one of them is refused rather than guessed at. A request
//! of any other method is what it *relays*, and relaying is the whole of what it does with
//! one: its parameters stay the JSON they arrived as, because narrowing a message this
//! program only carries would make it refuse a method the hosted plugin serves and it does
//! not — which is the one failure a proxy must not have.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde::Deserialize;
use serde_json::{json, Value};

/// The refusal this destination makes, worded as the shipped one is.
const DIFFER: &str = "destination labels differ from the labels being written";

/// The `SourceError` kinds this program itself reports. The hosted plugin's own errors are
/// relayed whole and never re-spelled here.
#[derive(Clone, Copy)]
enum Kind {
    /// The configuration for this source is invalid.
    Config,
    /// This source understood the request and refused it.
    Refused,
    /// The hosted plugin could not be reached.
    Unavailable,
}

impl Kind {
    fn wire(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Refused => "refused",
            Self::Unavailable => "unavailable",
        }
    }
}

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
    host: String,
    root: String,
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
    /// The item at this source to update, or `null` to create one.
    target: Option<String>,
    item: WrittenItem,
}

#[derive(Deserialize)]
struct WrittenItem {
    labels: Vec<Value>,
}

/// The hosted plugin's answer to a `get_task` or `get_project`.
#[derive(Deserialize)]
struct Held {
    /// Absent where the plugin answered an error rather than an item, and `null` inside
    /// where it answered that the store holds no such item.
    result: Option<HeldItem>,
}

#[derive(Deserialize)]
struct HeldItem {
    #[serde(rename = "task", alias = "project")]
    item: Option<HeldLabels>,
}

#[derive(Deserialize)]
struct HeldLabels {
    labels: Vec<Value>,
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

/// Answer one request without asking the store: a refusal, or a complaint about the
/// configuration or the host.
fn answer(id: &Value, kind: Kind, message: &str) {
    let response = json!({"id": id, "error": {"kind": kind.wire(), "message": message}});
    println!("{response}");
}

fn main() {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let Some(Ok(first)) = lines.next() else {
        return;
    };
    // A handshake this shape cannot hold is refused rather than guessed at, and there is
    // no id to answer under but the one the line itself carries.
    let handshake: Handshake = match serde_json::from_str(&first) {
        Ok(value) => value,
        Err(error) => {
            let id = serde_json::from_str::<Value>(&first)
                .map_or_else(|_| json!("0"), |value| value["id"].clone());
            answer(&id, Kind::Config, &format!("unreadable handshake: {error}"));
            return;
        }
    };

    let spawned = Command::new(&handshake.params.config.host)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            answer(
                &handshake.id,
                Kind::Config,
                &format!("cannot run {}: {error}", handshake.params.config.host),
            );
            return;
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
            "config": {"kind": "local-md", "config": {"root": handshake.params.config.root}},
            "secrets": {},
        },
    });
    let Some(mut answered) = host.ask(&hosted) else {
        answer(
            &handshake.id,
            Kind::Unavailable,
            "the hosted local-md plugin did not start",
        );
        return;
    };
    answered["id"] = handshake.id;
    println!("{answered}");

    for line in lines {
        let Ok(line) = line else { break };
        // A line this side cannot read is a violation of the framing rather than a request
        // to answer: there is no id to answer under, and inventing one would be a second
        // response to somebody. Say so where diagnostics go, and close the connection.
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                eprintln!("label-strict-source: unreadable request: {error}");
                break;
            }
        };
        match refused(&mut host, &request) {
            Refusal::Yes(why) => answer(&request.id, Kind::Refused, &why),
            Refusal::Unreadable(why) => answer(&request.id, Kind::Config, &why),
            Refusal::No => {
                let relayed = json!({
                    "id": request.id, "method": request.method, "params": request.params,
                });
                let Some(answered) = host.ask(&relayed) else {
                    answer(
                        &request.id,
                        Kind::Unavailable,
                        "the hosted local-md plugin stopped",
                    );
                    break;
                };
                println!("{answered}");
            }
        }
    }

    drop(host.input);
    let _ = host.child.wait();
}

/// What this destination does with one request before the store sees it.
enum Refusal {
    /// Relay it.
    No,
    /// Refuse it: the labels being written are not the labels the item carries.
    Yes(String),
    /// Refuse it as a message this source cannot act on, naming what could not be read.
    Unreadable(String),
}

/// Whether this destination refuses the write in `request`.
///
/// The rule is the shipped `github-projects` one: an update whose labels are not the
/// labels the destination item already carries is refused before anything is written.
/// A create has no item to disagree with, and a read is not a write.
fn refused(host: &mut Host, request: &Request) -> Refusal {
    let reading = match request.method.as_str() {
        "write_task" => "get_task",
        "write_project" => "get_project",
        _ => return Refusal::No,
    };
    let write: WriteParams = match serde_json::from_value(request.params.clone()) {
        Ok(write) => write,
        Err(error) => {
            return Refusal::Unreadable(format!(
                "this write is not one this source can read: {error}"
            ))
        }
    };
    let ItemWrite { target, item } = write.write;
    let Some(target) = target else {
        return Refusal::No;
    };
    let held = host.ask(&json!({
        "id": "strict-held",
        "method": reading,
        "params": {"id": target},
    }));
    let held = held.and_then(|answered| serde_json::from_value::<Held>(answered).ok());
    // No item to disagree with — the store holds none, or would not say — is left to the
    // store, which owns the refusal for a target it does not hold.
    let Some(held) = held.and_then(|held| held.result).and_then(|held| held.item) else {
        return Refusal::No;
    };
    if held.labels == item.labels {
        return Refusal::No;
    }
    Refusal::Yes(format!(
        "{DIFFER}: {target} carries {} and the write carries {}",
        json!(held.labels),
        json!(item.labels),
    ))
}
