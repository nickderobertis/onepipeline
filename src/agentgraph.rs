//! The `oneagentgraph` seam.
//!
//! Agent, harness, and model selection stay in that library, so this crate
//! reaches it the way any other caller does: through its CLI. Composition, not
//! reimplementation — nothing here decides a harness, a chain, or a model, and
//! the envelopes it produces are relayed into the merged stream exactly as it
//! emitted them.
//!
//! The binary is resolved from [`BINARY_ENV`] so an operator can point at a
//! specific build, and so a test can compose against a real executable standing
//! in for one.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::event::{Envelope, Labels};

/// The environment variable naming the `oneagentgraph` executable.
pub const BINARY_ENV: &str = "ONEPIPELINE_ONEAGENTGRAPH_BIN";

/// The executable's name when the environment names none.
pub const DEFAULT_BINARY: &str = "oneagentgraph";

/// The environment variable the dag-scope graph substitutes the run id into.
pub const RUN_ID_ENV: &str = "ONEPIPELINE_RUN_ID";

/// The member of the shipped dag-scope graph that paces planner updates.
pub const CHECK_IN_MEMBER: &str = "check-in";

/// The prefix every label this crate stamps on a sibling's run carries.
///
/// A run of this library is not a run of `oneagentgraph`: one plan's node is
/// dispatched as its own graph run, so both libraries have a `run_id` and the
/// two mean different things. `oneagentgraph` reserves the keys it stamps
/// itself — `run_id`, `member`, `persona` — and **refuses** a `--label` naming
/// one, which is a correct and general contract rather than anything to work
/// around: a consumer of the merged stream has to be able to tell the two
/// identities apart. So every key this crate sends is namespaced under a prefix
/// it owns, including the ones that do not collide today, because a label added
/// later must not be able to start colliding.
pub const LABEL_PREFIX: &str = "onepipeline.";

/// The backstop on the startup handshake — not the handshake itself.
///
/// The handshake below waits for an *answer*: the graph's first envelope, or its
/// exit. This bound only covers the third case, a process that gives neither,
/// and reaching it fails the launch rather than passing it. Nothing is ever
/// reported as started because a stopwatch ran out — that reading is the defect
/// this replaced, where a refusal a little slower than the window was announced
/// as a running driver.
pub const DEFAULT_STARTUP_TIMEOUT_SECONDS: u64 = 30;

/// The environment variable that moves the backstop above.
pub const STARTUP_TIMEOUT_ENV: &str = "ONEPIPELINE_STARTUP_TIMEOUT_SECONDS";

/// How long a launch waits for an answer before reporting that it got none.
///
/// An unusable value falls back to the default rather than to zero: a `0` would
/// make every launch time out before its graph could answer, which fails every
/// run rather than the one the operator was configuring.
fn startup_timeout() -> Duration {
    let seconds = std::env::var(STARTUP_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_STARTUP_TIMEOUT_SECONDS);
    Duration::from_secs(seconds)
}

/// How often a logged launch's output is re-read while it is waited on.
const LAUNCH_POLL: Duration = Duration::from_millis(10);

/// How much of a refused launch's own output is carried into the failure.
const EVIDENCE_CHARS: usize = crate::event::MAX_PAYLOAD_TEXT_BYTES / 4;

/// Say when this sibling's stream carried lines this build could not read.
///
/// Skipping them is right — a sibling emitting a kind this build does not know
/// must not stop the ones it does — but skipping them *quietly* turns a schema
/// mismatch into a run that merely looks uneventful. `oneagentgraph` is reached
/// as a process and read off its stdout, so its stream is the one place in this
/// crate where a line can still arrive unreadable.
fn report_skipped(skipped: usize) {
    if skipped > 0 {
        eprintln!("onepipeline: skipped {skipped} oneagentgraph line(s) this build cannot read");
    }
}

/// The executable this process invokes.
pub fn binary() -> String {
    std::env::var(BINARY_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BINARY.to_string())
}

fn sibling(message: impl Into<String>) -> Error {
    Error::Sibling {
        tool: "oneagentgraph",
        message: message.into(),
    }
}

/// Whether a line the graph wrote is an envelope this build can read.
///
/// What the startup handshake accepts as an announcement, so the bar is the
/// schema rather than "some JSON": a graph writes its refusal and its warnings
/// to the same place, and a newer build's envelope shape is a line this one
/// cannot read. Neither is a run saying it started.
fn is_envelope(line: &str) -> bool {
    serde_json::from_str::<Envelope>(line.trim()).is_ok()
}

/// Render the reserved label keys as the `k=v` pairs the CLI takes, each under
/// [`LABEL_PREFIX`].
pub fn label_args(labels: &Labels) -> Vec<String> {
    let mut args = Vec::new();
    let mut push = |key: &str, value: String| args.push(format!("{LABEL_PREFIX}{key}={value}"));
    if let Some(run) = &labels.run_id {
        push("run_id", run.clone());
    }
    if let Some(round) = labels.round {
        push("round", round.to_string());
    }
    if let Some(node) = &labels.node {
        push("node", node.clone());
    }
    if let Some(step) = &labels.step {
        push("step", step.clone());
    }
    if let Some(persona) = &labels.persona {
        push("persona", persona.clone());
    }
    args
}

/// Read this crate's own place in the run back off a relayed envelope.
///
/// The namespaced keys arrive in [`Labels::extra`], because that is where a key
/// the schema does not name lands. A view, a stall watch, and a per-node
/// evidence list all ask a relayed envelope which node it belongs to, so the
/// answer is put where every other envelope carries it.
///
/// This is an enricher, and enrichers never rewrite what is already there: a
/// key the producer stamped itself stands, and the namespaced copy stays in
/// `extra` beside it rather than being consumed. That is what keeps the two
/// `run_id`s — the graph run's and this run's — both readable on the one line.
///
/// It is also why a namespaced value this crate cannot read — a `round` that is
/// not a number — is left rather than reported: nothing is dropped, because the
/// value stays under its own key exactly as it arrived. The envelope's *own*
/// boundary is [`GraphRun::events`], which parses the line or skips it; a label
/// the schema does not name has already crossed it.
pub fn adopt_labels(labels: &mut Labels) {
    let stamped = |key: &str| {
        labels
            .extra
            .get(&format!("{LABEL_PREFIX}{key}"))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    let (run, round, node, step, persona) = (
        stamped("run_id"),
        stamped("round"),
        stamped("node"),
        stamped("step"),
        stamped("persona"),
    );
    labels.run_id = labels.run_id.take().or(run);
    labels.round = labels.round.or_else(|| round.and_then(|r| r.parse().ok()));
    labels.node = labels.node.take().or(node);
    labels.step = labels.step.take().or(step);
    labels.persona = labels.persona.take().or(persona);
}

/// One `oneagentgraph run`, started and streaming.
#[derive(Debug)]
pub struct GraphRun {
    child: Child,
    /// Where this launch's output went, and what reads it back.
    output: Output,
    /// Everything the handshake read on its way to an answer. These are the
    /// graph's own lines — its announcement, and anything it wrote before one —
    /// so they are put back at the head of the stream rather than spent on the
    /// handshake.
    started_with: Vec<String>,
}

/// Where a started graph's own stdout and stderr go.
///
/// Not a detail: a pipe is only a place to write if something holds its read
/// end. A launcher that starts a graph and then exits leaves the graph writing
/// into a pipe with no reader, and the graph dies on its first line — so a
/// launcher that will not stay to read says so here.
#[derive(Debug, Clone, Copy)]
pub enum GraphOutput<'a> {
    /// Piped, for a caller that stays and reads the envelopes.
    Relayed,
    /// Appended to a file, for a caller that is about to exit.
    Logged(&'a Path),
}

/// A started graph's output, from the reading side.
///
/// One value rather than a field per destination: a launch's output went to a
/// pipe or to a file, never to both and never to neither, and which one decides
/// where its announcement and its refusal are read from. Held as two `Option`s
/// those two questions could disagree — a launch with neither would be asked for
/// its answer in a file it does not have, and wait out the backstop for a
/// refusal already sitting on its pipe.
#[derive(Debug)]
enum Output {
    /// Piped here. The reader lives on this side rather than on the child
    /// because the handshake reads the first line off it and
    /// [`events`](GraphRun::events) reads the rest; `None` is that stream handed
    /// on, not a relayed launch that never had one.
    Relayed(Option<BufReader<std::process::ChildStdout>>),
    /// Appended to this file. It is the only place a refusal's message exists
    /// for a launch that logs, and it holds one launch's output and no other:
    /// the only caller that logs is `start --detach`, into a run directory
    /// minted for that launch.
    Logged(PathBuf),
}

impl GraphRun {
    /// Start a graph, with its envelopes going wherever `output` says.
    pub fn start(
        graph: &str,
        task: &str,
        dir: Option<&Path>,
        labels: &Labels,
        env: &[(String, String)],
        output: GraphOutput<'_>,
    ) -> Result<Self> {
        let mut command = Command::new(binary());
        command.arg("run").arg(graph);
        command.arg("--task").arg(task);
        command.arg("--output").arg("json");
        if let Some(dir) = dir {
            command.arg("--dir").arg(dir);
        }
        for label in label_args(labels) {
            command.arg("--label").arg(label);
        }
        for (key, value) in env {
            command.env(key, value);
        }
        command.stdin(Stdio::null());
        match output {
            GraphOutput::Relayed => {
                command.stdout(Stdio::piped()).stderr(Stdio::piped());
            }
            GraphOutput::Logged(path) => {
                // One file, opened twice: the two streams interleave the way
                // they would on a terminal, which is how they are read.
                let log = |path: &Path| {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .map_err(|e| {
                            sibling(format!(
                                "cannot open {} for the driver: {e}",
                                path.display()
                            ))
                        })
                };
                command.stdout(log(path)?).stderr(log(path)?);
            }
        }
        let mut child = command
            .spawn()
            .map_err(|e| sibling(format!("cannot start `{} run`: {e}", binary())))?;
        // The destination asked for is the destination recorded, so the two
        // cannot come apart: a relayed launch reads its pipe even on the host
        // where taking the handle back off the child somehow gave nothing, and
        // reports that silence as the launch failing to answer.
        let output = match output {
            GraphOutput::Relayed => Output::Relayed(child.stdout.take().map(BufReader::new)),
            GraphOutput::Logged(path) => Output::Logged(path.to_path_buf()),
        };
        Ok(Self {
            child,
            output,
            started_with: Vec::new(),
        })
    }

    /// Wait for the graph to say it started, and report it if it did not.
    ///
    /// Spawning proves a program was found, and nothing else. A graph that
    /// rejects its arguments — an unreadable config, a label it reserves — has
    /// exited before it drove anything, and a launcher that only asked whether
    /// the *spawn* worked answers with an exit 0 and the pid of a dead process.
    /// The run then sits with nothing driving it, and the message saying why is
    /// in a stream nobody read.
    ///
    /// So this waits for an **answer** rather than for a stopwatch: a graph
    /// announces itself with an envelope before it does any work, so the launch
    /// returns on whichever comes first — that envelope, or the process's exit —
    /// and, in the one case that is neither, when it has been silent for the
    /// [backstop](DEFAULT_STARTUP_TIMEOUT_SECONDS). A window a refusal merely
    /// has to outlast is what this replaced: it passed the launch on "still
    /// alive", which a graph delayed by scheduling or by its own startup work
    /// satisfies right up until it exits non-zero a moment later.
    ///
    /// **Whichever comes first**, and nothing after it. A graph that announced
    /// itself and then died has started, and its driver dying afterwards is what
    /// `DRIVER DEAD` and `adopt` are for; looking again after the answer would
    /// only make the same scenario land differently depending on which process
    /// the scheduler ran next.
    ///
    /// A graph that *succeeded* before answering is not a failure either. It ran
    /// whatever it was given and finished, which the caller reads from the
    /// stream and the ledger like any other settlement.
    pub fn confirm_started(&mut self) -> Result<()> {
        // On where the output went, not on whether a stream happens to be in
        // hand: those are the same question only as long as they agree.
        let piped = match &mut self.output {
            Output::Relayed(stdout) => stdout.take(),
            // Written to a file, so the answer is read from there.
            Output::Logged(_) => return self.await_logged_line(),
        };
        match piped {
            // Piped here, so the first line is the answer — and it is an
            // envelope the caller is owed, not a token to spend.
            Some(reader) => self.await_first_line(reader),
            // A pipe was asked for and there is nothing to read it with, so the
            // graph cannot answer through it. Its exit is the whole answer.
            None => self.settle_unstarted(),
        }
    }

    /// The handshake for a launch whose output is piped here.
    fn await_first_line(&mut self, reader: BufReader<std::process::ChildStdout>) -> Result<()> {
        // On a thread, because a read of a pipe blocks until the graph writes,
        // dies, or neither — and the third is exactly what this call must not
        // hang on. The reader is handed back with what was read, so the stream is
        // whole again whichever way the answer came.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name(format!("{}-handshake", binary()))
            .spawn(move || {
                let mut reader = reader;
                let mut read = Vec::new();
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Err(error) => break tx.send((Some(error), read, reader)),
                        // End of stream: it will say nothing more.
                        Ok(0) => break tx.send((None, read, reader)),
                        Ok(_) => {
                            let announced = is_envelope(&line);
                            read.push(line);
                            if announced {
                                break tx.send((None, read, reader));
                            }
                        }
                    }
                }
            })
            .map_err(|e| sibling(format!("cannot wait for `{} run` to start: {e}", binary())))?;

        match rx.recv_timeout(startup_timeout()) {
            // Everything read on the way to the answer is the caller's, whether
            // or not it was the answer: a line this build cannot parse is one
            // `events` reports as skipped, and a handshake that ate it would
            // hide the gap it leaves.
            Ok((error, read, reader)) => {
                let announced = read.last().is_some_and(|line| is_envelope(line));
                self.output = Output::Relayed(Some(reader));
                self.started_with = read;
                match error {
                    Some(error) => Err(sibling(format!(
                        "cannot read `{} run`'s first envelope: {error}",
                        binary()
                    ))),
                    // It announced itself, so it started.
                    None if announced => Ok(()),
                    // The stream ended without one, so its exit is the whole
                    // answer.
                    None => self.settle_unstarted(),
                }
            }
            Err(_) => self.gave_no_answer(),
        }
    }

    /// The handshake for a launch whose output goes to a file.
    ///
    /// The announcement is looked for first, so this reads the same way the
    /// piped side does: whichever answer the graph gave first is the answer. A
    /// graph that announced itself and then died started — the launch is what
    /// starts it, and a driver that dies afterwards is what `DRIVER DEAD` and
    /// `adopt` are for.
    fn await_logged_line(&mut self) -> Result<()> {
        let deadline = Instant::now() + startup_timeout();
        loop {
            if self.logged_an_envelope() {
                return Ok(());
            }
            match self.child.try_wait() {
                Err(error) => {
                    return Err(sibling(format!(
                        "cannot tell whether `{} run` started: {error}",
                        binary()
                    )))
                }
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => return self.refused(status),
                Ok(None) => {}
            }
            if Instant::now() >= deadline {
                return self.gave_no_answer();
            }
            std::thread::sleep(LAUNCH_POLL);
        }
    }

    /// Whether this launch has written a whole envelope into its log yet.
    ///
    /// A *complete* line — one the writer has terminated — that the envelope
    /// schema accepts. Both streams share the file, so this looks past whatever
    /// the graph said on stderr, and past a line from a build whose shape this
    /// one cannot read, rather than taking either for an announcement.
    fn logged_an_envelope(&self) -> bool {
        let Output::Logged(path) = &self.output else {
            return false;
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        text.split_inclusive('\n')
            .filter(|line| line.ends_with('\n'))
            .any(is_envelope)
    }

    /// The graph closed its stream without announcing itself: report whatever it
    /// exited with.
    fn settle_unstarted(&mut self) -> Result<()> {
        let deadline = Instant::now() + startup_timeout();
        loop {
            match self.child.try_wait() {
                Err(error) => {
                    return Err(sibling(format!(
                        "cannot tell whether `{} run` started: {error}",
                        binary()
                    )))
                }
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => return self.refused(status),
                Ok(None) if Instant::now() >= deadline => return self.gave_no_answer(),
                Ok(None) => std::thread::sleep(LAUNCH_POLL),
            }
        }
    }

    /// A launch the graph ended instead of driving.
    fn refused(&mut self, status: std::process::ExitStatus) -> Result<()> {
        let ended = status.code().map_or_else(
            || "was ended by a signal".to_string(),
            |code| format!("exited {code}"),
        );
        Err(sibling(format!(
            "`{} run` {ended} instead of driving the run: {}",
            binary(),
            self.evidence()
        )))
    }

    /// A launch that neither started nor ended. It is not left running: nothing
    /// would ever collect it, and the caller is being told it did not start.
    fn gave_no_answer(&mut self) -> Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        Err(sibling(format!(
            "`{} run` neither started nor exited within {}s, so nothing is driving the run: {}",
            binary(),
            startup_timeout().as_secs(),
            self.evidence()
        )))
    }

    /// What the graph itself said, from wherever its output went.
    ///
    /// The tail rather than the head: a refusal is the last thing a program
    /// writes, and a logged launch's file also holds whatever it managed to emit
    /// first.
    fn evidence(&mut self) -> String {
        let text = match &self.output {
            Output::Logged(path) => std::fs::read_to_string(path).unwrap_or_default(),
            Output::Relayed(_) => self
                .child
                .stderr
                .take()
                .map(|mut pipe| {
                    use std::io::Read;
                    let mut text = String::new();
                    let _ = pipe.read_to_string(&mut text);
                    text
                })
                .unwrap_or_default(),
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return "it said nothing".to_string();
        }
        let mut tail: Vec<char> = trimmed.chars().rev().take(EVIDENCE_CHARS).collect();
        tail.reverse();
        tail.into_iter().collect()
    }

    /// The envelopes it has produced, taken once.
    ///
    /// A line the envelope schema does not accept is skipped rather than ending
    /// the stream: a sibling emitting a kind this build does not know is not a
    /// reason to stop relaying the ones it does.
    ///
    /// The line the handshake read is put back at the head: it is the graph's
    /// own `graph-started`, and a launcher that swallowed it would leave the
    /// merged store without the event that says the driver began.
    pub fn events(&mut self) -> Box<dyn Iterator<Item = Result<Envelope>> + Send> {
        let piped = match &mut self.output {
            // Taken for good: the stream is the caller's from here.
            Output::Relayed(stdout) => stdout.take(),
            // A logged launch's envelopes are in its file, which stays named —
            // its refusal is still read from there.
            Output::Logged(_) => None,
        };
        let Some(stdout) = piped else {
            return Box::new(std::iter::empty());
        };
        let announced: Vec<_> = std::mem::take(&mut self.started_with);
        Box::new(
            announced
                .into_iter()
                .map(Ok)
                .chain(stdout.lines())
                .filter_map(|line| match line {
                    // A stream that broke is not a stream that ended. Read as the same
                    // thing, a relay stops mid-run and reports a clean finish, and the
                    // turns after the break are lost with nothing saying so.
                    Err(error) => Some(Err(sibling(format!(
                        "reading `{} run` output: {error}",
                        binary()
                    )))),
                    Ok(line) if line.trim().is_empty() => None,
                    Ok(line) => match serde_json::from_str::<Envelope>(&line) {
                        Ok(mut envelope) => {
                            adopt_labels(&mut envelope.labels);
                            Some(Ok(envelope))
                        }
                        Err(_) => {
                            report_skipped(1);
                            None
                        }
                    },
                }),
        )
    }

    /// Block until the graph settles, and report whether it succeeded.
    pub fn wait(&mut self) -> Result<Settled> {
        let status = self
            .child
            .wait()
            .map_err(|e| sibling(format!("waiting for `{} run`: {e}", binary())))?;
        let stderr = self
            .child
            .stderr
            .take()
            .map(|mut pipe| {
                use std::io::Read;
                let mut text = String::new();
                let _ = pipe.read_to_string(&mut text);
                text
            })
            .unwrap_or_default();
        Ok(Settled {
            code: status.code(),
            stderr,
        })
    }

    /// The started process's id, for the ledger's record of what is running.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Whether the graph process has ended, reaping it if it has.
    ///
    /// Reaping is the point. A child nobody waits on stays a zombie, and a
    /// zombie answers a liveness probe as alive — so an attach that never
    /// collected its driver would report a run as driven long after nothing
    /// was driving it.
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

/// How a graph run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    /// Its exit code, or `None` when a signal ended it.
    pub code: Option<i32>,
    /// What it wrote to stderr, for the failure's own evidence.
    pub stderr: String,
}

impl Settled {
    /// Whether the graph completed successfully.
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }
}

/// Restart a resettable schedule's clock.
///
/// This is the whole pacemaker-reset contract: a surface a planner actually
/// read is what restarts the check-in clock, so a run that is already reporting
/// does not also get a pacemaker surface.
pub fn reset_timer(run: &str, member: &str) -> Result<()> {
    let output = Command::new(binary())
        .arg("reset-timer")
        .arg(run)
        .arg(member)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| sibling(format!("cannot start `{} reset-timer`: {e}", binary())))?;
    if output.status.success() {
        return Ok(());
    }
    Err(sibling(format!(
        "reset-timer {run} {member} exited {}: {}",
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// Where one member's in-flight turn is addressed.
///
/// Read off the sibling's own relayed envelopes rather than derived: the graph
/// run's id and the member within it are labels `oneagentgraph` stamps, and this
/// crate has no second way to know either. A node whose dispatch has not
/// produced one yet has no address, which is the same answer as a node with no
/// turn to reach.
///
/// The fields are private and [`of`](Self::of) is the only way to build one, so
/// an address that exists is one the verb can act on: a blank run id or a member
/// name that would name a path outside the run is not an address this type can
/// hold. The member is judged by the **sibling's own** predicate, because that
/// is the rule the verb applies and a second copy of it here would drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnAddress {
    /// The `oneagentgraph` run — not this crate's, which is a different run.
    run: String,
    /// The member within it whose turn is in flight.
    member: String,
}

impl TurnAddress {
    /// One address, or `None` when what was read is not one.
    pub fn of(run: &str, member: &str) -> Option<Self> {
        let (run, member) = (run.trim(), member.trim());
        (!run.is_empty() && oneagentgraph::config::is_member_name(member)).then(|| Self {
            run: run.to_string(),
            member: member.to_string(),
        })
    }

    /// The graph run this turn belongs to.
    pub fn run(&self) -> &str {
        &self.run
    }

    /// The member within it.
    pub fn member(&self) -> &str {
        &self.member
    }
}

/// What one `oneagentgraph interrupt` answered.
///
/// The three outcomes are not interchangeable, which is the whole reason that
/// verb has an exit code of its own for the middle one: a redirection that
/// landed, a **fact** that there was no controllable turn to land it in, and a
/// lever that was pulled and broke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interrupted {
    /// The running turn took the redirection.
    Delivered,
    /// There was no controllable turn in flight, and this is which reason
    /// applied. Not an error: the member may be between turns, already settled,
    /// or running on a harness with no out-of-band control at all.
    NoTurn(String),
    /// The delivery was attempted and failed, or was one the sibling refused.
    Failed(String),
}

/// The envelopes an `interrupt` produced, and what it answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Interrupt {
    /// Delivered, no turn, or failed.
    pub outcome: Interrupted,
    /// The `turn-interrupted` envelope the verb published, for the merged
    /// store. It is emitted for every interrupt, delivered or not, so "the lever
    /// was pulled and nothing happened" reaches the run's own record.
    pub events: Vec<Envelope>,
}

/// Ask a member's in-flight turn to do something else.
///
/// The whole mechanism is `oneagentgraph`'s: which harnesses have a lever, where
/// the socket is, and what a turn does with a redirection are all decided there,
/// and nothing about them is rebuilt here. This composes that verb and reads its
/// exit code — including [`EXIT_NO_CONTROLLABLE_TURN`], which is a fact rather
/// than a failure and is what the `auto` fall-through and the `live` refusal are
/// both made of.
///
/// [`EXIT_NO_CONTROLLABLE_TURN`]: oneagentgraph::error::EXIT_NO_CONTROLLABLE_TURN
pub fn interrupt(address: &TurnAddress, input: &str) -> Interrupt {
    let output = Command::new(binary())
        .arg("interrupt")
        .arg(address.run())
        .arg(address.member())
        .arg("--input")
        .arg(input)
        .stdin(Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        // llmlint: ignore-block[changed_behavior_has_e2e] no invocation a user can type
        // reaches this arm. The executable is resolved from one variable the whole run
        // inherits, and a run whose `oneagentgraph` is missing fails at its launch — the
        // journey would be over before a `context` edit could be submitted to it. The unit
        // test below drives this function directly, which is the only entry point that
        // exists for it; `tests/e2e/context_delivery.rs` drives every arm a run can reach,
        // including a delivery the sibling attempted and failed.
        Err(error) => {
            return Interrupt {
                outcome: Interrupted::Failed(format!(
                    "cannot start `{} interrupt`: {error}",
                    binary()
                )),
                events: Vec::new(),
            }
        } // llmlint: ignore-end[changed_behavior_has_e2e]
    };
    // A line this build cannot read is skipped rather than ending the read — a
    // sibling emitting a shape this build does not know must not stop the ones
    // it does — but never *quietly*: the same rule, and the same report, as the
    // relayed run stream a few lines up.
    let mut skipped = 0;
    let events: Vec<Envelope> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| match serde_json::from_str::<Envelope>(line.trim()) {
            Ok(envelope) => Some(envelope),
            Err(_) => {
                skipped += 1;
                None
            }
        })
        .collect();
    report_skipped(skipped);
    // The published event's own words first, because that is where the verb puts
    // the reason a delivery did not land; its exit code says which kind of
    // answer it is.
    let reason = || {
        events
            .iter()
            .rev()
            .find_map(|event| event.payload.get("reason").and_then(|v| v.as_str()))
            .map(str::to_string)
            .unwrap_or_else(|| {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if stderr.is_empty() {
                    "it said nothing".to_string()
                } else {
                    stderr
                }
            })
    };
    let outcome = match output.status.code() {
        Some(oneagentgraph::error::EXIT_SUCCESS) => Interrupted::Delivered,
        Some(oneagentgraph::error::EXIT_NO_CONTROLLABLE_TURN) => Interrupted::NoTurn(reason()),
        Some(code) => Interrupted::Failed(format!(
            "`{} interrupt {} {}` exited {code}: {}",
            binary(),
            address.run(),
            address.member(),
            reason()
        )),
        None => Interrupted::Failed(format!(
            "`{} interrupt {} {}` was ended by a signal",
            binary(),
            address.run(),
            address.member()
        )),
    };
    Interrupt { outcome, events }
}

/// The provider-health block a view reports, sourced from `oneagentgraph
/// health`.
///
/// A health probe that cannot run is silence rather than a failure: a view whose
/// provider block is missing still reports everything else it knows.
pub fn health() -> Option<String> {
    let output = Command::new(binary())
        .arg("health")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_binary_comes_from_the_environment_or_falls_back() {
        // The variable is read per call rather than cached, so a test harness
        // and an operator both reach the executable they named.
        assert_eq!(
            std::env::var(BINARY_ENV)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| DEFAULT_BINARY.to_string()),
            binary()
        );
    }

    #[test]
    fn only_the_reserved_labels_the_contract_names_are_rendered_and_each_is_namespaced() {
        let labels = Labels {
            run_id: Some("demo".into()),
            round: Some(2),
            node: Some("build".into()),
            step: Some("implement".into()),
            persona: Some("engineer".into()),
            extra: serde_json::Map::new(),
        };
        assert_eq!(
            label_args(&labels),
            vec![
                "onepipeline.run_id=demo",
                "onepipeline.round=2",
                "onepipeline.node=build",
                "onepipeline.step=implement",
                "onepipeline.persona=engineer",
            ]
        );
        assert!(label_args(&Labels::default()).is_empty());
    }

    /// The sibling's own rule, driven through the sibling's own parser — not a
    /// second copy of the list it reserves. A key this crate starts sending
    /// fails here rather than at the launch it would have refused.
    #[test]
    fn every_label_this_crate_sends_is_one_oneagentgraph_accepts() {
        let labels = Labels {
            run_id: Some("demo".into()),
            round: Some(2),
            node: Some("build".into()),
            step: Some("implement".into()),
            persona: Some("engineer".into()),
            extra: serde_json::Map::new(),
        };
        for arg in label_args(&labels) {
            let parsed = oneagentgraph::run::parse_label(&arg)
                .unwrap_or_else(|error| panic!("oneagentgraph refuses `--label {arg}`: {error}"));
            assert!(
                parsed.key().starts_with(LABEL_PREFIX),
                "{} escaped the namespace",
                parsed.key()
            );
        }
    }

    #[test]
    fn a_relayed_envelopes_namespaced_labels_are_adopted_without_rewriting_the_producers() {
        let mut labels = Labels {
            // The graph run's own identity, which is not this run's.
            run_id: Some("node-scope-1786304152340-19".into()),
            ..Labels::default()
        };
        for (key, value) in [
            ("onepipeline.run_id", "demo"),
            ("onepipeline.round", "2"),
            ("onepipeline.node", "build"),
            ("onepipeline.step", "implement"),
            ("onepipeline.persona", "engineer"),
        ] {
            labels.extra.insert(key.into(), value.into());
        }
        adopt_labels(&mut labels);

        assert_eq!(
            labels.run_id.as_deref(),
            Some("node-scope-1786304152340-19"),
            "the graph run's own id was overwritten"
        );
        assert_eq!(labels.round, Some(2));
        assert_eq!(labels.node.as_deref(), Some("build"));
        assert_eq!(labels.step.as_deref(), Some("implement"));
        assert_eq!(labels.persona.as_deref(), Some("engineer"));
        assert_eq!(
            labels.extra["onepipeline.run_id"], "demo",
            "the namespaced copy is what tells the two runs apart"
        );
    }

    #[test]
    fn a_relayed_envelope_stamped_with_nothing_of_this_crates_is_left_as_it_arrived() {
        let mut labels = Labels {
            run_id: Some("elsewhere".into()),
            ..Labels::default()
        };
        labels.extra.insert("member".into(), "worker".into());
        let untouched = labels.clone();
        adopt_labels(&mut labels);
        assert_eq!(labels, untouched);
    }

    /// A sibling that is not installed is a delivery that *failed*, not a turn
    /// that was found to be absent. The two are different exit codes on the
    /// verb and different answers to the planner: one defers the note under
    /// `auto`, and this one refuses it.
    ///
    /// The seam is named rather than left to `PATH` — `oneagentgraph` is a
    /// published CLI, so a host that has it installed would otherwise decide
    /// this assertion. nextest runs each test in its own process, so the
    /// variable reaches nothing else.
    #[test]
    fn an_oneagentgraph_that_cannot_be_started_is_a_failed_delivery() {
        std::env::set_var(BINARY_ENV, "oneagentgraph-that-is-not-installed");
        let interrupt = interrupt(
            &TurnAddress {
                run: "node-scope-1".into(),
                member: "worker".into(),
            },
            "the fixture moved",
        );
        assert!(
            matches!(&interrupt.outcome, Interrupted::Failed(reason)
                if reason.contains("oneagentgraph-that-is-not-installed")),
            "{:?} does not name the binary that could not be started",
            interrupt.outcome
        );
        assert!(
            interrupt.events.is_empty(),
            "a delivery nothing ran produced envelopes"
        );
    }

    #[test]
    fn a_settled_run_reports_only_a_zero_exit_as_success() {
        assert!(Settled {
            code: Some(0),
            stderr: String::new()
        }
        .succeeded());
        assert!(!Settled {
            code: Some(1),
            stderr: String::new()
        }
        .succeeded());
        assert!(!Settled {
            code: None,
            stderr: String::new()
        }
        .succeeded());
    }
}
