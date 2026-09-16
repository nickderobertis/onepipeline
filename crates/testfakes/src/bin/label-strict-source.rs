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
//! serves is served by the real `local-md` plugin — the one `onetaskgraph-local-md`
//! publishes — hosted **in this process** by `onetaskgraph_core::subprocess::serve`, which
//! is the same reference host the `onetaskgraph-source` program runs. The only thing it
//! adds is the refusal: a `write_task` or `write_project` onto an item this store already
//! holds is refused, without reaching the store, when the labels being written are not the
//! labels that item carries.
//!
//! **Hosted rather than spawned**, because `onetaskgraph-source` is a reference host no
//! release installs, and a test that hunts an uninstalled executable resolves whatever is
//! lying around. The crates behind the plugin ship at every release, so the host lives
//! here and this program is the only executable the journey resolves.
//!
//! Its `config:` block — which a `subprocess` source hands over verbatim as `settings:` —
//! names the one thing left to name:
//!
//! * `root` — the folder of Markdown the hosted `local-md` plugin serves.
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

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, Sender};

use onetaskgraph_core::subprocess::serve;
use serde::Deserialize;
use serde_json::{json, Map, Value};

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] The rule below belongs
// to `onetaskgraph`'s `github-projects` destination, which can only be reached through a
// credentialled GitHub board. That crate is now in this binary's graph — `onetaskgraph-core`
// registers every plugin — and it still publishes no constant for this sentence: the
// refusal is a literal inside its own write path, so there is nothing to generate from and
// no gate this offline tier could run against it. What
// is restated is one sentence of *behaviour*, quoted verbatim in the module doc above, and
// the journey that drives this program asserts the refusal's own wording rather than
// trusting it. The alternative is not a gate; it is having no destination that refuses.
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
    method: String,
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

/// This source's own `config:` block: the folder the hosted plugin serves.
///
/// What hosts that plugin is no longer a setting, because it is no longer a program: the
/// `local-md` plugin runs in this process, so the only thing left for an operator's
/// configuration to decide is the store.
#[derive(Deserialize)]
struct Settings {
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
///
// llmlint: ignore-block[invalid_states_unrepresentable] These three are the store's own
// strings and this program neither mints nor interprets one: it compares what the
// destination holds against what a write carries. Narrowing them would make a label the
// store legitimately holds refuse a comparison it should simply lose or win, which is the
// one thing a destination rule must not do. `NativeId` above is narrowed for the opposite
// reason: an empty one names no item, so a target that was blank would be compared against
// whatever the store answered for it.
#[derive(Deserialize, PartialEq, Eq)]
struct Label {
    id: String,
    name: String,
    #[serde(default)]
    color: Option<String>,
}

// llmlint: ignore-end[invalid_states_unrepresentable]

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

/// The writing half of an in-process pipe.
///
/// What a spawned host's `ChildStdin` was, without the host: bytes handed to the thread
/// serving the `local-md` plugin. A closed reading half is the same [`BrokenPipe`] a
/// stopped child gave this program, so the one place that reports a stopped plugin does
/// not have to learn a second way of being told.
///
/// [`BrokenPipe`]: std::io::ErrorKind::BrokenPipe
struct Writing(Sender<Vec<u8>>);

impl Write for Writing {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.send(bytes.to_vec()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "nothing is reading")
        })?;
        Ok(bytes.len())
    }

    /// Nothing is buffered on this side: every write is already with the reader.
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The reading half, which is where the framing the protocol needs comes from.
///
/// A `Read` rather than a channel of lines, because both sides of this pipe are handed to
/// code that does its own framing — [`serve`] on one end and [`Host::ask`] on the other —
/// and a reader that delivered whole messages would be a second framing for them to
/// disagree with.
struct Reading {
    arriving: Receiver<Vec<u8>>,
    held: Vec<u8>,
    taken: usize,
}

impl Reading {
    fn new(arriving: Receiver<Vec<u8>>) -> Self {
        Self {
            arriving,
            held: Vec::new(),
            taken: 0,
        }
    }
}

impl Read for Reading {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        // A write of no bytes is not the end of anything, so an empty one is waited past
        // rather than reported as the closed stream `Ok(0)` means.
        while self.taken == self.held.len() {
            let Ok(more) = self.arriving.recv() else {
                return Ok(0);
            };
            self.held = more;
            self.taken = 0;
        }
        let taking = (self.held.len() - self.taken).min(into.len());
        into[..taking].copy_from_slice(&self.held[self.taken..self.taken + taking]);
        self.taken += taking;
        Ok(taking)
    }
}

struct Host {
    input: Writing,
    output: BufReader<Reading>,
    served: std::thread::JoinHandle<std::io::Result<()>>,
}

impl Host {
    /// Start the real `local-md` plugin **in this process**, behind the protocol.
    ///
    /// [`serve`] is `onetaskgraph`'s own reference host — the one the `onetaskgraph-source`
    /// program runs — so what answers here is the same code a spawned host would have run,
    /// over the same wire, reached through a pipe that never leaves this process.
    ///
    /// On a thread of its own because the protocol is a conversation: this side writes a
    /// request and then blocks reading its response, and a host sharing the thread would
    /// never get to answer. The plugin's own futures are runtime-agnostic — nothing under
    /// `local-md` is a socket or a timer — so a current-thread runtime is the whole of what
    /// hosting one costs.
    ///
    /// A host that cannot be started is reported rather than panicked on, for the same
    /// reason a host that could not be spawned was: it is a plugin that never came up, and
    /// §1 has the engine tell its caller that.
    fn start() -> Result<Self, String> {
        let (asking, asked) = mpsc::channel();
        let (answering, answered) = mpsc::channel();
        let served = std::thread::Builder::new()
            .name("local-md".to_owned())
            .spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .build()?
                    .block_on(serve(
                        BufReader::new(Reading::new(asked)),
                        Writing(answering),
                    ))
            })
            .map_err(|error| format!("cannot start the hosted local-md plugin: {error}"))?;
        Ok(Self {
            input: Writing(asking),
            output: BufReader::new(Reading::new(answered)),
            served,
        })
    }

    /// One request out and its response back, checked against §2's envelope: an object, on
    /// the id that was asked, carrying exactly one of `result` and `error`.
    ///
    /// Strictly sequential — this proxy never has more than one request outstanding — so a
    /// response on any other id is a violation rather than a message to hold on to.
    fn ask(&mut self, request: &Value) -> Result<Map<String, Value>, String> {
        writeln!(self.input, "{request}").map_err(|error| stopped(&error))?;
        self.input.flush().map_err(|error| stopped(&error))?;
        let mut line = String::new();
        match self.output.read_line(&mut line) {
            Ok(0) => return Err("the hosted local-md plugin stopped".to_owned()),
            Err(error) => return Err(stopped(&error)),
            Ok(_) => {}
        }
        let answered: Value =
            serde_json::from_str(&line).map_err(|error| format!("{ANSWERED}: {error}"))?;
        let Value::Object(answered) = answered else {
            return Err(format!("{ANSWERED}: it is not a response object"));
        };
        if answered.get("id") != request.get("id") {
            return Err(format!("{ANSWERED}: it answers an id nothing asked"));
        }
        if answered.contains_key("result") == answered.contains_key("error") {
            return Err(format!(
                "{ANSWERED}: a response carries one of `result` and `error`"
            ));
        }
        Ok(answered)
    }
}

/// What every complaint about the hosted plugin's own answers opens with.
const ANSWERED: &str = "the hosted local-md plugin answered something it cannot read";

fn stopped(error: &std::io::Error) -> String {
    format!("the hosted local-md plugin stopped: {error}")
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

fn refuse(id: &Value, message: &str) {
    let response = json!({"id": id, "error": {"kind": "refused", "message": message}});
    println!("{response}");
}

fn main() -> ExitCode {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let Some(Ok(first)) = lines.next() else {
        return stop("standard input ended before the handshake");
    };
    let handshake: Handshake = match serde_json::from_str(&first) {
        Ok(value) => value,
        Err(error) => return stop(&format!("unreadable handshake: {error}")),
    };
    // §1.2: the first request on a connection is `initialize`, and nothing else may be
    // answered before it. Anything else here is a peer this program is not talking to.
    if handshake.method != "initialize" {
        return stop(&format!(
            "the first request is `initialize`, not `{}`",
            handshake.method
        ));
    }

    let mut host = match Host::start() {
        Ok(host) => host,
        Err(why) => return stop(&why),
    };

    // The hosted plugin's own handshake, in the version the engine asked for and under the
    // name the engine gave this source, so what it reports is the real `local-md` source's
    // capabilities rather than a second opinion about them. The kind is that plugin's own
    // `KIND` rather than a string spelled here: a registry name copied into a fixture is
    // one that goes on naming a plugin after the release renamed it.
    let hosted = json!({
        "id": "strict-initialize",
        "method": "initialize",
        "params": {
            "protocol_version": handshake.params.protocol_version,
            "engine": handshake.params.engine,
            "source_name": handshake.params.source_name,
            "config": {
                "kind": onetaskgraph_local_md::KIND,
                "config": {"root": handshake.params.config.root},
            },
            "secrets": {},
        },
    });
    let mut answered = match host.ask(&hosted) {
        Ok(answered) => answered,
        Err(why) => return stop(&why),
    };
    answered.insert("id".to_owned(), handshake.id);
    println!("{}", Value::Object(answered));

    let ending = relay(&mut host, lines);
    drop(host.input);
    // The hosted plugin is where every read and write actually happened, so its own ending
    // is this source's: a host that died reporting something must not be reported as a
    // clean close by the code that carried its answers. Closing the writing half above is
    // what ends it — the plugin reads until its input does, exactly as it would behind a
    // pipe to another process.
    let ending = ending.and_then(|()| match host.served.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(stopped(&error)),
        Err(_) => Err("the hosted local-md plugin panicked".to_owned()),
    });
    match ending {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => stop(&why),
    }
}

fn relay(
    host: &mut Host,
    lines: impl Iterator<Item = std::io::Result<String>>,
) -> Result<(), String> {
    for line in lines {
        let Ok(line) = line else {
            return Err("standard input could not be read".to_owned());
        };
        // A line this side cannot read is a violation of the framing rather than a request
        // to answer: there is no id to answer under, and inventing one would be a second
        // response to somebody.
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => return Err(format!("unreadable request: {error}")),
        };
        match judged(host, &request) {
            Judgement::Refused(why) => refuse(&request.id, &why),
            Judgement::Unreadable(why) => return Err(why),
            Judgement::Relay => {
                let relayed = json!({
                    "id": request.id, "method": request.method, "params": request.params,
                });
                println!("{}", Value::Object(host.ask(&relayed)?));
            }
        }
    }
    Ok(())
}

enum Judgement {
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
    let answered = host.ask(&json!({
        "id": "strict-held",
        "method": reading,
        "params": {"id": target.0},
    }));
    // An answer this side cannot read is never read as "nothing is held there": that would
    // let a write past the rule this source exists to apply.
    let held: Held = match answered.and_then(|answered| {
        serde_json::from_value(Value::Object(answered))
            .map_err(|error| format!("{ANSWERED}: {reading} for '{}' answered {error}", target.0))
    }) {
        Ok(held) => held,
        Err(why) => return Judgement::Unreadable(why),
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

// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

fn named(labels: &[Label]) -> String {
    labels
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
