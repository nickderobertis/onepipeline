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
//! `write_task`, `set_task_status`, `update_task`, `set_project_metadata`:
//!
//! * `<key>.<method>.refuse` — every call of that method is answered with the source error
//!   the file holds, in the store's own shape (`{"kind": "rate-limited", ...}`), for as long
//!   as the file is there. Refused **by name** when it does not read as one: a fixture that
//!   failed nothing would hand a journey the real store's answer while it asserts a failure.
//! * `<key>.<method>@<id>.refuse` — the same, for the calls that name one item alone, where
//!   `<id>` is that item's native id as `fake::segment` spells a file name: one ticket of
//!   several refused and the rest written.
//! * `<key>.<method>.refuse.once` — the next call only, and the file is taken away.
//! * `<key>.<method>.refuse.after-first` — every call of that method but the **first** this
//!   source is handed in a command, which the real store answers; `end_command` starts the
//!   next command. So under a page size of one this is a read whose first page answered and
//!   whose next page failed: one answer beside one failure.
//! * `<key>.get_project.absent`, `<key>.get_task.absent` and `<key>.update_task.absent` — the
//!   read or the targeted update answers that the store holds no such item, with no failure
//!   beside it, for as long as the file is there: what a hosted destination answers for a
//!   board or an issue somebody deleted.
//! * `<key>.<method>.rendezvous` — the address this source meets the test at before it
//!   answers that method, held until the test lets go; written by `World::rendezvous`, read by
//!   `fake::meet`. The one way to make a store call **slow** rather than wrong.
//! * `<key>.initialize.rendezvous` — the handshake of every connection, held before it is
//!   answered: a source slow to start.
//! * `<key>.<method>.first.rendezvous` — the same, for the **first** call of that method this
//!   source is handed in a command and no later one; `end_command` starts the next command.
//!   Each plan read is a command, and so is each write-back attempt on the store the worker
//!   keeps, so this holds one call per attempt: the first task a copy writes, say, which is
//!   how a journey holds a copy without holding each of its writes in turn.
//! * `<key>.task.key` — the short handle every task this source answers `query_tasks` and
//!   `get_task` with carries, read verbatim, for as long as the file is there: what a hosted
//!   source with a handle of its own answers with, and the one field `local-md` never
//!   carries.
//! * `<key>.metering` — a `Metering` in the store's own shape that every request this source
//!   serves but `end_command` adds to its running total, which is what it answers `metering` with: a source
//!   that meters its own requests, as a hosted one does, so a copy's `spent` is the store's
//!   own figure rather than one a journey wrote into a report. Read when the source starts,
//!   because a source says in its handshake whether it meters. Every request it charges is
//!   also appended to `<key>.charged.jsonl` as `{"method": <method>, "spent": <Metering>}` —
//!   the meter's own account across every connection, which a running total dies with — so
//!   a journey sums what a whole run spent off what the source charged, not off a report.
//!
//! Every call is recorded into the directory's `invocations.jsonl` as
//! `{"tool": <key>, "args": [<method>, <the id it names, where it names one>]}`, the
//! handshake included as `initialize`, naming each credential the engine handed over with a
//! SHA-256 digest of its value — `NAME=<hex>` — never the value itself.

// llmlint: ignore-file[new_code_lands_in_a_project] `crates/testfakes` has no `project.json`
// of its own, like every double beside this file: its files are inputs of the
// `onepipeline-note-journeys` project (`noteJourneySource` lists `crates/**/*` in `nx.json`)
// and of the root project's `visualDocsSource`, and the crate is a member of the one Cargo
// workspace those projects build. It ships nowhere, and a project of its own would restate
// their targets over the same build unit.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use onepipeline_testfakes as fake;
use onepipeline_testfakes::hosted::Host;
use onetaskgraph_plugin_api::{Metering, SourceError, SourceName};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

// llmlint: ignore-block[boundary_inputs_validated] These shapes deliberately **ignore**
// members they do not know, because §2.1 of the plugin protocol requires it: that is how a
// later protocol version adds an optional field without a version bump. What this program
// acts on — its own settings, and each request's method — is required and typed; a request's
// parameters are relayed exactly as they arrived.

/// The handshake, as far as this program reads it.
#[derive(Deserialize)]
struct Handshake {
    id: String,
    /// §1.2: the first request on a connection is `initialize`, and nothing else may be
    /// answered before it, so a first line naming anything else does not read.
    #[allow(
        dead_code,
        reason = "read only to refuse a first line that is not a handshake"
    )]
    method: Initialize,
    params: HandshakeParams,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Initialize {
    Initialize,
}

#[derive(Deserialize)]
struct HandshakeParams {
    protocol_version: u32,
    #[serde(default)]
    engine: Value,
    source_name: SourceName,
    config: Settings,
    /// The credentials the engine resolved for this source, by the variable each is named by.
    #[serde(default)]
    secrets: std::collections::BTreeMap<String, String>,
    /// The status categories the engine knows (§3.5), relayed to the hosted plugin verbatim.
    #[serde(default)]
    statuses: Option<Value>,
}

#[derive(Deserialize)]
struct Settings {
    root: PathBuf,
    script: PathBuf,
    #[serde(default = "default_key")]
    key: Key,
}

fn default_key() -> Key {
    Key("store".to_owned())
}

/// What a source's scenario files and recorded calls are named by: lower-case letters,
/// digits and hyphens, so a key can name no file outside the scenario directory and no
/// scenario of another source.
#[derive(Deserialize)]
#[serde(try_from = "String")]
struct Key(String);

impl TryFrom<String> for Key {
    type Error = String;

    fn try_from(key: String) -> Result<Self, Self::Error> {
        let named = !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        named
            .then_some(Self(key.clone()))
            .ok_or_else(|| format!("`key` {key:?} is not lower-case letters, digits and hyphens"))
    }
}

/// One request off the engine's wire, its parameters kept as they arrived.
#[derive(Deserialize)]
struct Request {
    id: String,
    method: Method,
    params: Value,
}

/// A request's method name: lower-case letters and underscores, which is every name the
/// plugin protocol gives one, so a name can form a scenario file name and never a path.
// llmlint: ignore[invalid_states_unrepresentable] a grammar rather than a closed set, on
// purpose: this source relays every method the hosted plugin serves, and a closed set would
// refuse one the protocol adds before the hosted plugin could answer it — the one failure a
// relay must not have. What is refused is what could not be a scenario file's name.
#[derive(Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[serde(try_from = "String")]
struct Method(String);

impl TryFrom<String> for Method {
    type Error = String;

    fn try_from(method: String) -> Result<Self, Self::Error> {
        let named =
            !method.is_empty() && method.chars().all(|c| c.is_ascii_lowercase() || c == '_');
        named
            .then_some(Self(method.clone()))
            .ok_or_else(|| format!("{method:?} is not a method name the plugin protocol gives"))
    }
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
    let settings = handshake.params.config;
    if !settings.script.is_dir() {
        return stop(&format!(
            "`script` {} is not a directory to read a scenario from",
            settings.script.display()
        ));
    }
    // Each credential as its name and a digest of its value, so a journey can hold the value
    // that arrived without this fixture writing a credential down.
    let handed: Vec<String> = handshake
        .params
        .secrets
        .iter()
        .map(|(name, value)| format!("{name}={:x}", Sha256::digest(value.as_bytes())))
        .collect();
    fake::record(
        &settings.script,
        &settings.key.0,
        &["initialize".to_owned(), handed.join(",")],
    );
    // A handshake held open is a source slow to start: every connection meets it here, before
    // anything is answered.
    if let Err(why) = held(&settings.script, &format!("{}.initialize", settings.key.0)) {
        return stop(&why);
    }
    let mut host = match Host::start() {
        Ok(host) => host,
        Err(why) => return stop(&why),
    };
    let meters = match metering(&settings.script.join(format!("{}.metering", settings.key.0))) {
        Ok(metered) => metered.is_some(),
        Err(why) => return stop(&why),
    };
    match host.initialize(
        handshake.id,
        handshake.params.protocol_version,
        handshake.params.engine,
        handshake.params.source_name.as_str(),
        &settings.root,
        handshake.params.statuses,
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
        handed: std::collections::BTreeSet::new(),
    };
    let ending = source.relay(&mut host, lines).and_then(|()| host.finish());
    match ending {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => stop(&why),
    }
}

struct Scripted {
    script: PathBuf,
    key: Key,
    /// What the requests served so far have spent, where the scenario meters them.
    spent: Metering,
    /// The methods this source has been handed at least once in this command, answered or not.
    handed: std::collections::BTreeSet<Method>,
}

impl Scripted {
    fn scenario(&self, name: &str) -> PathBuf {
        self.script.join(format!("{}.{name}", self.key.0))
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
                &self.key.0,
                &[request.method.0.clone(), named(&request.params)?],
            );
            let answer = self.answer(host, &request)?;
            println!("{answer}");
            // A command's end starts the next command's firsts: a store kept across commands
            // is asked each method's first call again in every command it runs.
            if request.method.0 == "end_command" {
                self.handed.clear();
            }
        }
        Ok(())
    }

    fn answer(&mut self, host: &mut Host, request: &Request) -> Result<Value, String> {
        let first = self.handed.insert(request.method.clone());
        let method = request.method.0.as_str();
        let mut holds = vec![format!("{}.{method}", self.key.0)];
        if first {
            holds.push(format!("{}.{method}.first", self.key.0));
        }
        // A hold that cannot be read is answered as the source's own `config` failure naming
        // the script, rather than by ending the connection: an engine reports a plugin's own
        // words only for a failed handshake, so a source that stopped here would fail the call
        // under no name at all.
        for hold in holds {
            if let Err(why) = held(&self.script, &hold) {
                return Ok(json!({"id": request.id, "error": {"kind": "config", "message": why}}));
            }
        }
        if !first {
            let later = self.scenario(&format!("{method}.refuse.after-first"));
            if let Some(error) = refusal(&later)? {
                return Ok(json!({"id": request.id, "error": error}));
            }
        }
        let once = self.scenario(&format!("{method}.refuse.once"));
        if let Some(error) = refusal(&once)? {
            std::fs::remove_file(&once)
                .map_err(|why| format!("cannot take {} away: {why}", once.display()))?;
            return Ok(json!({"id": request.id, "error": error}));
        }
        let item = named(&request.params)?;
        if !item.is_empty() {
            let one = self.scenario(&format!("{method}@{}.refuse", fake::segment(&item)));
            if let Some(error) = refusal(&one)? {
                return Ok(json!({"id": request.id, "error": error}));
            }
        }
        if let Some(error) = refusal(&self.scenario(&format!("{method}.refuse")))? {
            return Ok(json!({"id": request.id, "error": error}));
        }
        if present(&self.scenario(&format!("{method}.absent")))? {
            let held = match method {
                "get_project" => "project",
                "get_task" => "task",
                // §4.21: an update of a task the source does not hold answers a `null` outcome.
                "update_task" => "outcome",
                other => return Err(format!("`{other}` names no one item that could be absent")),
            };
            return Ok(json!({"id": request.id, "result": {held: null}}));
        }
        let metered = self.scenario("metering");
        if method == "metering" && metering(&metered)?.is_some() {
            return Ok(json!({"id": request.id, "result": {"metering": self.spent}}));
        }
        let relayed = json!({
            "id": request.id, "method": request.method.0, "params": request.params,
        });
        let mut answered = host.ask(&relayed)?;
        if let Some(key) = handle(&self.scenario("task.key"))? {
            keyed(method, &mut answered, &key);
        }
        // Ending a command clears what the source held for it and sends nothing to a hosted
        // destination, so, like `metering`, it is never charged.
        if answered.contains_key("result") && method != "end_command" {
            if let Some(each) = metering(&metered)? {
                self.spend(&each);
                let charged = json!({"method": method, "spent": each}).to_string();
                fake::append(&self.scenario("charged.jsonl"), &charged);
            }
        }
        Ok(Value::Object(answered))
    }

    /// Budgets are summed by name and unit together, as the engine sums a difference of them.
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

/// The id one request names — a read's `id`, or a write's `target` — or nothing, for a
/// request that names none or a write that creates, which the protocol spells as a `null`
/// target. Any other id that is there and is not a non-empty string — a read's `null` `id`
/// included — is refused rather than read as none: it would be recorded against no item and
/// match no scenario a journey scripted for it.
fn named(params: &Value) -> Result<String, String> {
    let named = match params.get("id") {
        Some(id) => Some(id),
        None => match params.get("write").and_then(|write| write.get("target")) {
            None | Some(Value::Null) => None,
            target => target,
        },
    };
    match named {
        None => Ok(String::new()),
        Some(Value::String(id)) if !id.is_empty() => Ok(id.clone()),
        Some(other) => Err(format!(
            "a request names its item as {other}, which is not an id"
        )),
    }
}

/// Meet the test at the rendezvous `hold` names, where one is scripted.
///
/// A hold that is there and cannot be read is never read as no hold: a journey timing a store
/// that has not answered would be handed one that answered at once. So this source stops,
/// which the engine reports as its own failure.
fn held(script: &Path, hold: &str) -> Result<(), String> {
    let path = fake::rendezvous_script(script, hold);
    match std::fs::read_to_string(&path) {
        Ok(address) => {
            fake::meet(address.trim());
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "`{}` could not be read: {error}",
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        )),
    }
}

/// Whether a scenario file that says only by being there is there. One that cannot be read
/// as a file is refused rather than read as absent: a fixture that scripted nothing would
/// hand a journey the real store's answer while it asserts the scripted one.
fn present(path: &Path) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(found) if found.is_file() => Ok(true),
        Ok(_) => Err(format!("{} is not a scenario file", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
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

/// The handle a scenario file gives every task, read verbatim — a blank one included, which
/// is how a source carries none — or `None` where no file is there.
fn handle(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(key) => Ok(Some(key)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// Give every task an answer to `method` carries `key`: a page's items for `query_tasks`, the
/// one task for `get_task`, and nothing for any other method or for a refusal.
fn keyed(method: &str, answered: &mut serde_json::Map<String, Value>, key: &str) {
    let Some(result) = answered.get_mut("result") else {
        return;
    };
    let tasks: Vec<&mut Value> = match method {
        "query_tasks" => result
            .get_mut("items")
            .and_then(Value::as_array_mut)
            .map(|items| items.iter_mut().collect())
            .unwrap_or_default(),
        "get_task" => result.get_mut("task").into_iter().collect(),
        _ => Vec::new(),
    };
    for task in tasks {
        if let Some(task) = task.as_object_mut() {
            task.insert("key".to_owned(), Value::String(key.to_owned()));
        }
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

#[cfg(test)]
mod tests {
    use super::named;
    use serde_json::json;

    /// A read names its item by `id`, an update by its write's `target`, and a create by a
    /// `null` target, which the protocol writes for every create.
    #[test]
    fn a_request_names_the_item_its_protocol_spelling_names() {
        assert_eq!(named(&json!({"id": "board"})), Ok("board".to_owned()));
        assert_eq!(
            named(&json!({"write": {"target": "work"}})),
            Ok("work".to_owned())
        );
        assert_eq!(
            named(&json!({"write": {"target": null}})),
            Ok(String::new())
        );
        assert_eq!(named(&json!({"project": "board"})), Ok(String::new()));
    }

    /// A read's `null` id is no create, so it is refused rather than read as naming nothing.
    #[test]
    fn an_id_that_is_there_and_is_not_one_is_refused() {
        for params in [
            json!({"id": null}),
            json!({"id": ""}),
            json!({"id": 7}),
            json!({"write": {"target": ""}}),
        ] {
            assert!(
                named(&params).is_err(),
                "{params} was read as an id or as none"
            );
        }
    }
}
