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
//! **Hosted rather than spawned**, for the reason `onepipeline_testfakes::hosted` gives:
//! this program is the only executable the journey resolves.
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

use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;

use onepipeline_testfakes::hosted::{Host, ANSWERED};
use serde::Deserialize;
use serde_json::{json, Value};

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
    id: String,
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
    /// The status categories the engine knows (§3.5), relayed to the hosted plugin verbatim.
    #[serde(default)]
    statuses: Option<Value>,
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

    match host.initialize(
        handshake.id,
        handshake.params.protocol_version,
        handshake.params.engine,
        &handshake.params.source_name,
        &handshake.params.config.root,
        handshake.params.statuses,
    ) {
        Ok(answered) => println!("{answered}"),
        Err(why) => return stop(&why),
    }

    let ending = relay(&mut host, lines).and_then(|()| host.finish());
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
