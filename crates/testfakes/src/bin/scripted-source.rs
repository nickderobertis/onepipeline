//! A **real** `onetaskgraph` source that can be told to fail the way a hosted one does.
//!
//! Every read and every write it serves is served by the real `local-md` plugin, hosted in
//! this process by `onetaskgraph_core::subprocess::serve` over the store's own stdio plugin
//! protocol — `onepipeline_testfakes::hosted` says why hosted rather than spawned. What it
//! adds is what an offline store cannot be made to do: answer one call with the source error
//! a hosted destination answers with — a refusal, a rate limit, a source that could not be
//! reached — hold one call open until the journey lets it go, and say what each call
//! spent. The engine under test links the real store and reaches this source exactly as it
//! reaches any `subprocess` source an operator configures, so what it classifies is the
//! store's own typed failure, arrived at the store's own boundary.
//!
//! Its `config:` block — which a `subprocess` source hands over verbatim as `settings:` —
//! names:
//!
//! * `root` — the folder of Markdown the hosted `local-md` plugin serves.
//! * `script` — the directory its scenario is read out of and its calls are recorded into:
//!   the directory every double in this crate reads, since a `subprocess` source is started
//!   with an empty environment and cannot find it any other way.
//! * `key` — what its scenario files and its recorded calls are named by, so two sources in
//!   one journey are scripted apart. `store` where it names none.
//!
//! Scripted from that directory, where `<method>` is the plugin protocol's own method name —
//! `get_project`, `query_tasks`, `get_task`, `task_dependencies`, `write_project`,
//! `write_task`, `set_task_status`:
//!
//! * `<key>.<method>.refuse` — every call of that method is answered with the source error
//!   the file holds, in the store's own shape (`{"kind": "rate-limited", ...}`), for as long
//!   as the file is there. Refused **by name** when it does not read as one: a fixture that
//!   failed nothing would hand a journey the real store's answer while it asserts a failure.
//! * `<key>.<method>@<id>.refuse` — the same, for the calls that name one item alone, where
//!   `<id>` is that item's native id as `fake::segment` spells a file name: one ticket of
//!   several refused and the rest written.
//! * `<key>.<method>.refuse.once` — the next call only, and the file is taken away.
//! * `<key>.get_project.absent` and `<key>.get_task.absent` — the read answers that the
//!   store holds no such item, with no failure beside it, for as long as the file is there:
//!   what a hosted destination answers for a board or an issue somebody deleted.
//! * `<key>.<method>.rendezvous` — the address this source meets the test at before it
//!   answers that method, held until the test lets go; written by `World::rendezvous`, read by
//!   `fake::meet`. The one way to make a store call **slow** rather than wrong.
//! * `<key>.<method>.first.rendezvous` — the same, for the **first** call of that method this
//!   source is handed and no later one. The engine starts a source for each command it runs —
//!   each plan read, each write-back attempt — so this holds one call per attempt: the first
//!   task a copy writes, say, which is how a journey holds a copy without holding each of its
//!   writes in turn.
//! * `<key>.metering` — a `Metering` in the store's own shape that every request this source
//!   serves adds to its running total, which is what it answers `metering` with: a source
//!   that meters its own requests, as a hosted one does, so a copy's `spent` is the store's
//!   own figure rather than one a journey wrote into a report. Read when the source starts,
//!   because a source says in its handshake whether it meters.
//!
//! Every call is recorded into the directory's `invocations.jsonl` as
//! `{"tool": <key>, "args": [<method>, <the id it names, where it names one>]}`, the
//! handshake included as `initialize`, naming the credentials the engine handed over — their
//! names, never their values.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use onepipeline_testfakes as fake;
use onepipeline_testfakes::hosted::Host;
use onetaskgraph_plugin_api::{Metering, SourceError};
use serde::Deserialize;
use serde_json::{json, Value};

// llmlint: ignore-block[boundary_inputs_validated] These shapes deliberately **ignore**
// members they do not know, because §2.1 of the plugin protocol requires it: that is how a
// later protocol version adds an optional field without a version bump. What this program
// acts on — its own settings, and each request's method — is required and typed; a request's
// parameters are relayed exactly as they arrived.

/// The handshake, as far as this program reads it.
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
    /// The credentials the engine resolved for this source, by the variable each is named by.
    #[serde(default)]
    secrets: std::collections::BTreeMap<String, String>,
}

/// This source's own `config:` block.
#[derive(Deserialize)]
struct Settings {
    root: PathBuf,
    script: PathBuf,
    #[serde(default = "default_key")]
    key: String,
}

fn default_key() -> String {
    "store".to_owned()
}

/// One request off the engine's wire, its parameters kept as they arrived.
#[derive(Deserialize)]
struct Request {
    id: Value,
    method: String,
    params: Value,
}

// llmlint: ignore-end[boundary_inputs_validated]

fn stop(why: &str) -> ExitCode {
    eprintln!("scripted-source: {why}");
    ExitCode::FAILURE
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
    // answered before it.
    if handshake.method != "initialize" {
        return stop(&format!(
            "the first request is `initialize`, not `{}`",
            handshake.method
        ));
    }
    let settings = handshake.params.config;
    let handed: Vec<&str> = handshake
        .params
        .secrets
        .keys()
        .map(String::as_str)
        .collect();
    fake::record(
        &settings.script,
        &settings.key,
        &["initialize".to_owned(), handed.join(",")],
    );
    let mut host = match Host::start() {
        Ok(host) => host,
        Err(why) => return stop(&why),
    };
    let meters = settings
        .script
        .join(format!("{}.metering", settings.key))
        .is_file();
    match host.initialize(
        handshake.id,
        handshake.params.protocol_version,
        handshake.params.engine,
        &handshake.params.source_name,
        &settings.root,
    ) {
        // A source that meters says so in its handshake (§3.4), or the engine never asks it.
        Ok(mut answered) => {
            if meters {
                if let Some(result) = answered.get_mut("result").and_then(Value::as_object_mut) {
                    result.insert("meters".to_owned(), Value::Bool(true));
                }
            }
            println!("{answered}");
        }
        Err(why) => return stop(&why),
    }
    let mut source = Scripted {
        script: settings.script,
        key: settings.key,
        spent: Metering::default(),
        answered: std::collections::BTreeSet::new(),
    };
    let ending = source.relay(&mut host, lines).and_then(|()| host.finish());
    match ending {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => stop(&why),
    }
}

struct Scripted {
    script: PathBuf,
    key: String,
    /// What the writes served so far have spent, where the scenario meters them.
    spent: Metering,
    /// The methods this source has been handed at least once.
    answered: std::collections::BTreeSet<String>,
}

impl Scripted {
    fn scenario(&self, name: &str) -> PathBuf {
        self.script.join(format!("{}.{name}", self.key))
    }

    fn relay(
        &mut self,
        host: &mut Host,
        lines: impl Iterator<Item = std::io::Result<String>>,
    ) -> Result<(), String> {
        for line in lines {
            let Ok(line) = line else {
                return Err("standard input could not be read".to_owned());
            };
            // A line this side cannot read is a violation of the framing rather than a
            // request to answer: there is no id to answer under.
            let request: Request = serde_json::from_str(&line)
                .map_err(|error| format!("unreadable request: {error}"))?;
            fake::record(
                &self.script,
                &self.key,
                &[request.method.clone(), named(&request.params)],
            );
            let answer = self.answer(host, &request)?;
            println!("{answer}");
        }
        Ok(())
    }

    fn answer(&mut self, host: &mut Host, request: &Request) -> Result<Value, String> {
        let method = request.method.as_str();
        let first = self.answered.insert(method.to_owned());
        let mut holds = vec![format!("{}.{method}", self.key)];
        if first {
            holds.push(format!("{}.{method}.first", self.key));
        }
        for hold in holds {
            let script = fake::rendezvous_script(&self.script, &hold);
            match std::fs::read_to_string(&script) {
                Ok(address) => fake::meet(address.trim()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                // A hold that is there and cannot be read is never read as no hold: a journey
                // timing a store that has not answered would be handed one that answered at
                // once. So this source stops, which the engine reports as its own failure.
                Err(error) => {
                    return Err(format!(
                        "`{}` could not be read: {error}",
                        script
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    ))
                }
            }
        }
        let once = self.scenario(&format!("{method}.refuse.once"));
        if let Some(error) = refusal(&once)? {
            std::fs::remove_file(&once)
                .map_err(|why| format!("cannot take {} away: {why}", once.display()))?;
            return Ok(json!({"id": request.id, "error": error}));
        }
        let item = named(&request.params);
        if !item.is_empty() {
            let one = self.scenario(&format!("{method}@{}.refuse", fake::segment(&item)));
            if let Some(error) = refusal(&one)? {
                return Ok(json!({"id": request.id, "error": error}));
            }
        }
        if let Some(error) = refusal(&self.scenario(&format!("{method}.refuse")))? {
            return Ok(json!({"id": request.id, "error": error}));
        }
        if self.scenario(&format!("{method}.absent")).is_file() {
            let held = match method {
                "get_project" => "project",
                "get_task" => "task",
                other => return Err(format!("`{other}` reads no one item that could be absent")),
            };
            return Ok(json!({"id": request.id, "result": {held: null}}));
        }
        let metered = self.scenario("metering");
        if method == "metering" && metered.is_file() {
            return Ok(json!({"id": request.id, "result": {"metering": self.spent}}));
        }
        let relayed = json!({
            "id": request.id, "method": request.method, "params": request.params,
        });
        let answered = host.ask(&relayed)?;
        if answered.contains_key("result") {
            if let Some(each) = metering(&metered)? {
                self.spend(&each);
            }
        }
        Ok(Value::Object(answered))
    }

    /// Add one request's cost to what this source has spent.
    fn spend(&mut self, each: &Metering) {
        self.spent.requests += each.requests;
        for budget in &each.budgets {
            match self
                .spent
                .budgets
                .iter_mut()
                .find(|held| held.budget == budget.budget && held.unit == budget.unit)
            {
                Some(held) => {
                    held.measured += budget.measured;
                    held.modelled += budget.modelled;
                }
                None => self.spent.budgets.push(budget.clone()),
            }
        }
    }
}

/// The id one request names — a read's `id`, or a write's `target` — or nothing.
fn named(params: &Value) -> String {
    params
        .get("id")
        .or_else(|| params.get("write").and_then(|write| write.get("target")))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The source error a scenario file scripts, read through the store's own type, or `None`
/// where no file is there.
fn refusal(path: &Path) -> Result<Option<SourceError>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| {
            format!(
                "{} is not a source error in the store's own shape: {error}",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// What each request spends, where the scenario meters them.
fn metering(path: &Path) -> Result<Option<Metering>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| {
            format!(
                "{} is not a metering in the store's own shape: {error}",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}
