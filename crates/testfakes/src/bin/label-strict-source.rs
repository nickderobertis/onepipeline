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

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

/// The refusal this destination makes, worded as the shipped one is.
const DIFFER: &str = "destination labels differ from the labels being written";

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

/// Answer one request without asking the store: a refusal, or a protocol complaint.
fn answer(id: &Value, error: &str, kind: &str) {
    let response = json!({"id": id, "error": {"kind": kind, "message": error}});
    println!("{response}");
}

fn main() {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let Some(Ok(first)) = lines.next() else {
        return;
    };
    let handshake: Value = match serde_json::from_str(&first) {
        Ok(value) => value,
        Err(error) => {
            answer(
                &json!("0"),
                &format!("unreadable handshake: {error}"),
                "config",
            );
            return;
        }
    };
    let id = handshake["id"].clone();
    let settings = &handshake["params"]["config"];
    let (Some(host), Some(root)) = (settings["host"].as_str(), settings["root"].as_str()) else {
        answer(
            &id,
            "this source's settings name a `host` executable and a `root` folder",
            "config",
        );
        return;
    };

    let spawned = Command::new(host)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            answer(&id, &format!("cannot run {host}: {error}"), "config");
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
            "protocol_version": handshake["params"]["protocol_version"],
            "engine": handshake["params"]["engine"],
            "source_name": handshake["params"]["source_name"],
            "config": {"kind": "local-md", "config": {"root": root}},
            "secrets": {},
        },
    });
    let Some(mut answered) = host.ask(&hosted) else {
        answer(
            &id,
            "the hosted local-md plugin did not start",
            "unavailable",
        );
        return;
    };
    answered["id"] = id;
    println!("{answered}");

    for line in lines {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = request["id"].clone();
        if let Some(refusal) = refused(&mut host, &request) {
            answer(&id, &refusal, "refused");
            continue;
        }
        let Some(answered) = host.ask(&request) else {
            answer(&id, "the hosted local-md plugin stopped", "unavailable");
            break;
        };
        println!("{answered}");
    }

    drop(host.input);
    let _ = host.child.wait();
}

/// Whether this destination refuses the write in `request`, and why.
///
/// The rule is the shipped `github-projects` one: an update whose labels are not the
/// labels the destination item already carries is refused before anything is written.
/// A create has no item to disagree with, and a read is not a write.
fn refused(host: &mut Host, request: &Value) -> Option<String> {
    let reading = match request["method"].as_str()? {
        "write_task" => "get_task",
        "write_project" => "get_project",
        _ => return None,
    };
    let write = &request["params"]["write"];
    let target = write["target"].as_str()?;
    let held = host.ask(&json!({
        "id": "strict-held",
        "method": reading,
        "params": {"id": target},
    }))?;
    let item = held["result"].get(reading.trim_start_matches("get_"))?;
    let held = item.get("labels")?;
    let writing = write["item"].get("labels")?;
    (held != writing)
        .then(|| format!("{DIFFER}: {target} carries {held} and the write carries {writing}"))
}
