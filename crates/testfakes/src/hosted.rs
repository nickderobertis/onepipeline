//! The real `local-md` plugin, hosted **in this process** behind `onetaskgraph`'s own
//! stdio plugin protocol, for the two sources in this crate that stand in front of it:
//! `label-strict-source`, which adds one destination's rule, and `scripted-source`, which
//! adds the failures a hosted destination has and an offline one cannot be made to.
//!
//! [`serve`] is `onetaskgraph`'s own reference host — the one the `onetaskgraph-source`
//! program runs — so what answers is the same code a spawned host would have run, over the
//! same wire, reached through a pipe that never leaves this process. **Hosted rather than
//! spawned**, because `onetaskgraph-source` is a reference host no release installs, and a
//! test that hunts an uninstalled executable resolves whatever is lying around. The crates
//! behind the plugin ship at every release, so the host lives here and each source program
//! is the only executable a journey resolves.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc::{self, Receiver, Sender};

use onetaskgraph_core::subprocess::serve;
use serde_json::{Map, Value};

/// The writing half of an in-process pipe.
///
/// What a spawned host's `ChildStdin` was, without the host: bytes handed to the thread
/// serving the `local-md` plugin. A closed reading half is the same [`BrokenPipe`] a
/// stopped child gave, so the one place that reports a stopped plugin does not have to
/// learn a second way of being told.
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

/// The hosted `local-md` plugin, and the two ends of the pipe it is reached through.
pub struct Host {
    input: Writing,
    output: BufReader<Reading>,
    served: std::thread::JoinHandle<std::io::Result<()>>,
}

impl Host {
    /// Start the real `local-md` plugin **in this process**, behind the protocol.
    ///
    /// On a thread of its own because the protocol is a conversation: this side writes a
    /// request and then blocks reading its response, and a host sharing the thread would
    /// never get to answer. The plugin's own futures are runtime-agnostic — nothing under
    /// `local-md` is a socket or a timer — so a current-thread runtime is the whole of what
    /// hosting one costs.
    ///
    /// A host that cannot be started is reported rather than panicked on: it is a plugin
    /// that never came up, and §1 has the engine tell its caller that.
    pub fn start() -> Result<Self, String> {
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

    /// The hosted plugin's own handshake, for the source `handshake` began: in the version
    /// the engine asked for and under the name the engine gave that source, over the folder
    /// `root` names, answered on the engine's own id.
    ///
    /// What it reports is the real `local-md` source's capabilities rather than a second
    /// opinion about them. The kind is that plugin's own `KIND` rather than a string spelled
    /// here: a registry name copied into a fixture is one that goes on naming a plugin after
    /// the release renamed it.
    pub fn initialize(
        &mut self,
        id: Value,
        protocol_version: u32,
        engine: Value,
        source_name: &str,
        root: &std::path::Path,
    ) -> Result<Value, String> {
        let hosted = serde_json::json!({
            "id": "hosted-initialize",
            "method": "initialize",
            "params": {
                "protocol_version": protocol_version,
                "engine": engine,
                "source_name": source_name,
                "config": {
                    "kind": onetaskgraph_local_md::KIND,
                    "config": {"root": root},
                },
                "secrets": {},
            },
        });
        let mut answered = self.ask(&hosted)?;
        answered.insert("id".to_owned(), id);
        Ok(Value::Object(answered))
    }

    /// One request out and its response back, checked against §2's envelope: an object, on
    /// the id that was asked, carrying exactly one of `result` and `error`.
    ///
    /// Strictly sequential — a source in front of this host never has more than one request
    /// outstanding — so a response on any other id is a violation rather than a message to
    /// hold on to.
    pub fn ask(&mut self, request: &Value) -> Result<Map<String, Value>, String> {
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

    /// Close the plugin's input and wait for it to end, answering how it ended.
    ///
    /// The hosted plugin is where every read and write actually happened, so its own ending
    /// is the source's: a host that died reporting something must not be reported as a clean
    /// close by the code that carried its answers. Closing the writing half is what ends it —
    /// the plugin reads until its input does, exactly as it would behind a pipe to another
    /// process.
    pub fn finish(self) -> Result<(), String> {
        drop(self.input);
        match self.served.join() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(stopped(&error)),
            Err(_) => Err("the hosted local-md plugin panicked".to_owned()),
        }
    }
}

/// What every complaint about the hosted plugin's own answers opens with.
pub const ANSWERED: &str = "the hosted local-md plugin answered something it cannot read";

fn stopped(error: &std::io::Error) -> String {
    format!("the hosted local-md plugin stopped: {error}")
}
