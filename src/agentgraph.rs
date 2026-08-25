//! The `oneagentgraph` seam.
//!
//! Agent, harness, and model selection stay in that library, so every verb this
//! crate needs is one of its **library** entry points: [`oneagentgraph::run::start`]
//! for a graph, [`oneagentgraph::run::signal`] for a pacemaker reset,
//! [`oneagentgraph::control::interrupt`] for a live redirection, and
//! [`oneagentgraph::health::read`] for the provider block. Composition, not
//! reimplementation: nothing here decides a harness, a chain, or a model, and
//! envelopes cross the same serialized boundary as the former NDJSON relay.
//!
//! [`BINARY_ENV`] remains an explicit compatibility override, and it is
//! all-or-nothing: naming an executable sends *every* verb to it, so an operator
//! pinning an install never gets half a run from one build and half from
//! another. Detached launches still retain a process, because a library
//! scheduler thread cannot outlive the process that is about to exit — but the
//! process they retain is **this executable**, at [`DRIVE_VERB`], composing this
//! build's own `oneagentgraph`. So the override is the only way an installed
//! sibling is composed, and one build decides what a graph document may contain
//! whichever way a run was launched. See [`retained_command`].
//!
//! # What moving in-process changed, and what it did not
//!
//! **Isolation.** The process boundary did not disappear — it moved one layer
//! down, to where the risk is. What can wedge or burn is the *agent turn*, and
//! `oneagentgraph` spawns that as its own `oneharness` process either way. What
//! became a thread is the graph *scheduler*, and a scheduler that panics is
//! reported rather than fatal: [`oneagentgraph::run::Running::wait`] answers a
//! panicked thread with `InvalidConfig`, which [`exit_for`] turns into the same
//! exit code the process path carried, and [`GraphRun::wait`] settles the run on
//! it. A run that must be stopped is stopped through the sibling's own
//! [`cancel`](oneagentgraph::run::Running::cancel), which writes the run's stop
//! signal and reaps its member process trees — the same reap the `cancel` verb
//! performs.
//!
//! **The stream.** Envelopes arrive as they occur, and they arrive as the same
//! value the subprocess path relayed. Both are held by tests:
//! `a_relayed_envelope_is_the_same_whether_it_crossed_as_a_value_or_as_a_line`
//! for the content, and, for the timing,
//! `status_says_what_a_live_dispatch_is_doing_and_the_readout_advances` in
//! `tests/e2e/dispatch.rs`, which reads a dispatch's tool summary out of the
//! merged store twice while the node is still in flight.
//!
//! **Interrupts and exit codes.** The three answers stay three: a redirection
//! delivered, the *fact* that there was no controllable turn, and a lever that
//! broke. The library hands back a [`Delivery`](oneagentgraph::control::Delivery)
//! where the process path handed back an exit code, so [`interrupt`] applies the
//! CLI's own mapping — and publishes the `turn-interrupted` envelope the verb
//! publishes, through the sibling's own emitter.
//!
//! **Concurrency.** One thing in the sibling's library path is process-wide and
//! is therefore *no longer isolated between concurrent nodes*: a graph's `env:`
//! block is exported into the running process, and `ONEHARNESS_HARNESSES` is
//! removed from it. That is deliberate upstream — a two-party member is a thread
//! there, and the `oneharness run` it spawns has to inherit what the contract
//! promises it — and it was safe while one graph run was one process. This crate
//! dispatches several nodes at once, so it no longer is.
//! `a_graphs_env_block_is_exported_into_this_process_and_not_into_the_run_alone`
//! observes it. The shipped graphs declare no `env:` block, so nothing here
//! trips it today.
//!
//! From `oneagentgraph 0.2.18` that model covers a **single-sided** member too:
//! its turn is an `oneharness_core` library call, so the harness process
//! oneharness spawns inherits this process's environment rather than one
//! composed per member. [`export`] is what keeps [`Launch::env`] meaning the
//! same thing on both backends under it, and says why the pairs it carries are
//! safe to put there.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::event::{Envelope, Labels};
use crate::filter::EventFilter;

/// The environment variable naming the `oneagentgraph` executable.
pub const BINARY_ENV: &str = "ONEPIPELINE_ONEAGENTGRAPH_BIN";

/// The executable's name when the environment names none.
pub const DEFAULT_BINARY: &str = "oneagentgraph";

/// The verb this executable retains a detached launch's driver with.
///
/// Named once, because [`retained_command`] spells it into a command line and
/// [`crate::cli`] parses it back off one; the two spellings agreeing is what
/// makes a detached launch start at all.
pub const DRIVE_VERB: &str = "drive";

/// Where `oneagentgraph` keeps its runs.
///
/// Restated rather than imported, and that is a duplicated configuration
/// surface rather than a choice: the sibling declares this name — and the one
/// below — as a private `const` in its **binary**, so there is no library item
/// to name. A library entry point that took its environment as a parameter but
/// left the caller to spell the keys is the gap; `docs/contract-divergences.md`
/// records the surface that would close it.
const STATE_DIR_ENV: &str = "ONEAGENTGRAPH_STATE_DIR";

/// The `oneharness` executable a run — and an interrupt's delivery — drives.
const ONEHARNESS_BIN_ENV: &str = "ONEAGENTGRAPH_ONEHARNESS_BIN";

/// This process's environment, as the sibling's entry points take it.
///
/// Every library call below is handed one of these rather than left to read the
/// process's own: the sibling's surface is written so that a consumer holding
/// two runs on two installs can give each its own, and taking that parameter
/// from one place here keeps this crate's default — *this* process's
/// environment, which is what the subprocess path inherited — stated once.
fn process_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// Where the sibling keeps its run state, resolved exactly as its CLI resolves
/// it — `HOME` on every platform, because that is what the sibling reads.
fn state_dir(env: &BTreeMap<String, String>) -> PathBuf {
    env.get(STATE_DIR_ENV).map_or_else(
        || {
            env.get("HOME")
                .map_or_else(std::env::temp_dir, PathBuf::from)
                .join(".local/state/oneagentgraph/runs")
        },
        PathBuf::from,
    )
}

/// The `oneharness` executable the sibling drives, resolved as its CLI does.
fn oneharness_bin(env: &BTreeMap<String, String>) -> String {
    env.get(ONEHARNESS_BIN_ENV)
        .cloned()
        .unwrap_or_else(|| "oneharness".into())
}

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
/// mismatch into a run that merely looks uneventful. A line can only arrive
/// unreadable where one is *read* — the [`BINARY_ENV`] override's stdout, and
/// the serialized hop the library path's envelopes make — so this is what both
/// of those report through.
fn report_skipped(skipped: usize) {
    if skipped > 0 {
        eprintln!("onepipeline: skipped {skipped} oneagentgraph line(s) this build cannot read");
    }
}

/// The executable this process invokes.
pub fn binary() -> String {
    overriding_binary().unwrap_or_else(|| DEFAULT_BINARY.to_string())
}

/// Whether [`BINARY_ENV`] names an executable to compose instead of this build's.
///
/// One predicate, because two callers ask it: [`binary`] resolves the name and
/// [`retained_command`] chooses a whole launch shape from it. Asked as
/// "is the variable set", an empty or unreadable value sends the launch down the
/// override path and then resolves to the default executable anyway — half a run
/// from each answer, which is the skew the override exists to make deliberate.
fn overridden() -> bool {
    overriding_binary().is_some()
}

/// The executable [`BINARY_ENV`] names, when it names a usable one.
fn overriding_binary() -> Option<String> {
    std::env::var(BINARY_ENV)
        .ok()
        .filter(|value| !value.is_empty())
}

fn sibling(message: impl Into<String>) -> Error {
    Error::Sibling {
        tool: "oneagentgraph",
        message: message.into(),
    }
}

/// The exit code the sibling's own CLI carries one of its failures out on.
///
/// The subprocess path read a code and the library path is handed an `Error`,
/// so this is the CLI's rule applied here — including its fall-through, because
/// `oneagentgraph::error::Error` is `#[non_exhaustive]` and a variant added
/// later must still settle a run with the code the contract assigns it rather
/// than fail to compile.
fn exit_for(error: &oneagentgraph::error::Error) -> i32 {
    match error {
        oneagentgraph::error::Error::InvalidConfig(_) => oneagentgraph::error::EXIT_INVALID_CONFIG,
        _ => oneagentgraph::error::EXIT_MEMBER_FAILED,
    }
}

/// How a process of the sibling's ended, in words.
///
/// One phrasing for both ways a process can stop, so a caller reporting a
/// refusal never has to branch on which: a signal leaves no code, and reading
/// that absence as a code would report `exited 0` for a killed process.
fn ended(status: &std::process::ExitStatus) -> String {
    status.code().map_or_else(
        || "was ended by a signal".to_string(),
        |code| format!("exited {code}"),
    )
}

/// The envelope a line the graph wrote carries, when this build can read one.
///
/// What the startup handshake accepts as an announcement, so the bar is the
/// schema rather than "some JSON": a graph writes its refusal and its warnings
/// to the same place, and a newer build's envelope shape is a line this one
/// cannot read. Neither is a run saying it started.
///
/// The parsed value rather than a yes/no, because the announcement is also
/// where the graph's *own* run id arrives on the retained-process path — the
/// only place a detached launcher can learn it.
fn envelope_of(line: &str) -> Option<Envelope> {
    serde_json::from_str::<Envelope>(line.trim()).ok()
}

/// Whether a line the graph wrote is an envelope this build can read.
fn is_envelope(line: &str) -> bool {
    envelope_of(line).is_some()
}

/// The `oneagentgraph` run an announcement belongs to.
///
/// The sibling stamps its own run onto every envelope it emits, and this crate
/// has no second way to learn it: a run of this library is not a run of that
/// one. [`adopt_labels`] never overwrites a producer's own `run_id`, so the
/// value read here is the graph's rather than this crate's.
///
/// Through the sibling's own parser, so what comes back is a run id that
/// library would answer to rather than whatever string was on the line — an
/// announcement this build cannot read as one leaves no address at all, which
/// is the same answer as a graph that never announced itself.
fn announced_run(envelope: &Envelope) -> Option<GraphRunId> {
    GraphRunId::parse(envelope.labels.run_id.as_deref()?.trim()).ok()
}

/// The id `oneagentgraph` minted for one of its runs.
///
/// The sibling's own type, not a second copy of it: it is the value that
/// library's signals are addressed by, and it is what refuses a string that
/// would name a path outside its run store. Aliased here so the rest of this
/// crate can name it without naming the sibling's module path everywhere, and
/// so it is visibly *not* a `onepipeline` run id — the two are different runs
/// and confusing them is what left the pacemaker reset dead.
pub type GraphRunId = oneagentgraph::run::RunId;

/// One graph run id read back off this crate's own launch record.
///
/// The record is a file on disk that a later, unrelated process re-reads, so
/// this field is external input like any other: it arrives as a string and only
/// becomes an address by passing the sibling's parser. Both refusals are
/// phrased for the operator reading them off `next`'s stderr, because that is
/// the only place a pacemaker that could not be reset is reported.
pub fn recorded_graph_run(recorded: &str, run: &str) -> Result<GraphRunId> {
    let recorded = recorded.trim();
    if recorded.is_empty() {
        return Err(Error::Invalid(format!(
            "run '{run}' records no agent-graph run to address it by"
        )));
    }
    GraphRunId::parse(recorded)
        .map_err(|error| sibling(format!("run '{run}' records '{recorded}': {error}")))
}

/// Whether the graph run a launch record names has **stopped running**, as the
/// sibling's own ownership records say.
///
/// Two of them, because a graph killed outright never wrote the ending a graph
/// that settles does — so the `owner.lock` answers where `finished_ms` cannot.
/// Nothing here sweeps a process table, which knows nothing about whose work it
/// matched.
///
/// Anything this host cannot prove is `false`: a run reported unwatched invites
/// an operator to intervene, and doing that to a working observer is worse than
/// saying nothing.
pub fn graph_run_ended(recorded: &str, run: &str) -> bool {
    let root = state_dir(&process_env());
    recorded_graph_run(recorded, run)
        .ok()
        .and_then(|graph_run| {
            let record = oneagentgraph::history::show(&root, graph_run.as_str()).ok()?;
            Some(
                record.finished_ms.is_some()
                    || oneagentgraph::scratch::reclaimable(&root.join(&graph_run)).is_ok(),
            )
        })
        .unwrap_or(false)
}

/// Render the reserved label keys as the `k=v` pairs the CLI takes, each under
/// [`LABEL_PREFIX`].
///
/// `round` is not among them and never is: execution is continuous, so nothing
/// this crate writes stamps one.
pub fn label_args(labels: &Labels) -> Vec<String> {
    let mut args = Vec::new();
    let mut push = |key: &str, value: String| args.push(format!("{LABEL_PREFIX}{key}={value}"));
    if let Some(run) = &labels.run_id {
        push("run_id", run.clone());
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
/// It is also why a namespaced key this crate no longer reads — the retired
/// `round`, which an older build's envelopes carry — is left rather than
/// reported: nothing is dropped, because the value stays under its own key
/// exactly as it arrived. The envelope's *own* boundary is
/// [`GraphRun::events`], which parses the line or skips it; a label the schema
/// does not name has already crossed it.
pub fn adopt_labels(labels: &mut Labels) {
    let stamped = |key: &str| {
        labels
            .extra
            .get(&format!("{LABEL_PREFIX}{key}"))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    let (run, node, step, persona) = (
        stamped("run_id"),
        stamped("node"),
        stamped("step"),
        stamped("persona"),
    );
    labels.run_id = labels.run_id.take().or(run);
    labels.node = labels.node.take().or(node);
    labels.step = labels.step.take().or(step);
    labels.persona = labels.persona.take().or(persona);
}

/// One envelope the sibling handed over, as this crate's own.
///
/// The library gives a typed [`oneagentgraph::event::Envelope`] where the
/// subprocess path gave a line of that type's own NDJSON. Both cross the same
/// boundary — the sibling's `Serialize` — so an envelope relayed either way is
/// the same value, and the crossing is kept rather than skipped for exactly
/// that reason: a direct field-by-field copy would be a second reading of a
/// schema the sibling owns, and would silently drop the first field it added.
/// `a_relayed_envelope_is_the_same_whether_it_crossed_as_a_value_or_as_a_line`
/// holds the two to each other.
fn relayed(envelope: oneagentgraph::event::Envelope) -> Result<Envelope> {
    serde_json::to_value(envelope)
        .map_err(|error| sibling(format!("serializing graph event: {error}")))
        .and_then(|value| {
            serde_json::from_value::<Envelope>(value)
                .map_err(|error| sibling(format!("reading graph event: {error}")))
        })
        .map(|mut envelope| {
            adopt_labels(&mut envelope.labels);
            envelope
        })
}

/// One `oneagentgraph run`, started and streaming.
#[derive(Debug)]
pub struct GraphRun {
    backend: GraphBackend,
}

#[derive(Debug)]
enum GraphBackend {
    Library(LibraryGraphRun),
    Process(ProcessGraphRun),
}

#[derive(Debug)]
struct LibraryGraphRun {
    events: Option<mpsc::Receiver<Result<Envelope>>>,
    settled: mpsc::Receiver<Result<Settled>>,
    cancel: mpsc::Sender<()>,
    /// The graph run's own id, as the sibling minted it.
    run_id: GraphRunId,
    exited: Arc<AtomicBool>,
}

/// The retained process implementation for detached launches. A scheduler
/// thread cannot outlive the embedding process, so the SDK cannot implement a
/// launch whose caller deliberately exits immediately afterward.
#[derive(Debug)]
struct ProcessGraphRun {
    /// The graph process, shared because the relayed stream asks it a question
    /// of its own: whether the process this launch started is over, which is what
    /// ends that stream when the pipe does not. Every other holder takes the lock
    /// for one call, and the stream never blocks on it — see [`over`].
    child: Arc<Mutex<Child>>,
    /// The graph process's id, kept beside the process rather than read off it.
    ///
    /// [`cancel`](GraphRun::cancel) takes `&self` and is called from the engine's
    /// drain while other holders have the lock, so the one thing a teardown needs
    /// is the one thing that must never wait for it.
    pid: u32,
    /// Where this launch's output went, and what reads it back.
    output: Output,
    /// Everything the handshake read on its way to an answer. These are the
    /// graph's own lines — its announcement, and anything it wrote before one —
    /// so they are put back at the head of the stream rather than spent on the
    /// handshake.
    started_with: Vec<String>,
    /// The graph run's own id, read off its announcement.
    run_id: Option<GraphRunId>,
}

/// How long the relayed stream goes quiet before it asks whether the graph
/// process is still there.
///
/// It is asked *only* on a silence, so a launch whose pipe closes the moment its
/// graph exits — every launch on a host that leaves nothing holding it — ends on
/// the pipe as it always did and pays nothing for this. What the interval bounds
/// is the other case: how long after the graph is gone a stream held open by
/// something else waits before it ends. Long enough that a burst still in the
/// pipe is delivered before the silence is read as one, and short enough that a
/// node's settlement is not held on an interval's convenience.
const RELAY_POLL: Duration = Duration::from_millis(250);

/// How long a reader of a graph's stderr waits for the drain to reach the end of
/// the pipe before taking what has arrived.
///
/// Every caller asks after the process has exited, so what is left is whatever
/// it flushed on its way out — already in the pipe, and read by the drain as soon
/// as that thread is scheduled. The bound is what makes this a read rather than a
/// wait: a pipe something else is holding open never reaches its end, and a
/// launch's message is not worth hanging a run on.
const SAID_PATIENCE: Duration = Duration::from_secs(2);

/// How often [`Said::settled`] looks again while it waits out that bound.
const SAID_POLL: Duration = Duration::from_millis(10);

/// What a launch has said on its stderr, drained off the pipe as it arrives.
///
/// A pipe is read to its end, and its end is every writing handle closed — not
/// the process this launch started exiting. Anything that inherited that handle
/// holds the stream open after the graph is gone, and on Windows that is a wider
/// set than the graph's own children: a console process is given a `conhost` and
/// a `.bat` a `cmd`, and either can outlive what it was started for. So the
/// stream is drained on a thread of its own and read here as a snapshot, rather
/// than read to its end by whoever is asking what the graph said.
#[derive(Debug, Clone)]
struct Said {
    /// Everything the drain has read so far, as it was read.
    bytes: Arc<Mutex<Vec<u8>>>,
    /// Set once the pipe reached its end, so a reader can tell a stream that has
    /// finished from one that has merely said nothing yet.
    ended: Arc<AtomicBool>,
}

impl Said {
    /// Start draining `pipe` on a thread of its own.
    ///
    /// `None` is a host that gave a piped launch no handle back — the same
    /// silence [`Output::Relayed`] records for the other stream — and it is a
    /// drain with nothing to read rather than a second state to carry.
    ///
    /// Nothing here branches on whether the thread started. A [`Drained`] goes
    /// with the closure either way, and going is what marks the drain finished,
    /// so a host that would not start the thread takes the path every launch
    /// takes rather than one of its own.
    fn draining(pipe: Option<std::process::ChildStderr>) -> Self {
        let said = Self {
            bytes: Arc::new(Mutex::new(Vec::new())),
            ended: Arc::new(AtomicBool::new(false)),
        };
        let drain = Drained(said.clone());
        let _ = std::thread::Builder::new()
            .name(format!("{}-stderr", binary()))
            .spawn(move || {
                use std::io::Read;
                let mut buffer = [0u8; 4096];
                if let Some(mut pipe) = pipe {
                    loop {
                        match pipe.read(&mut buffer) {
                            // The end of the pipe, or a host that will not say
                            // more about it. Either way there is nothing further.
                            Ok(0) => break,
                            Ok(read) => held(&drain.0.bytes).extend_from_slice(&buffer[..read]),
                            // A read the signal handling interrupted read nothing
                            // and is not the stream ending; every other failure is.
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(_) => break,
                        }
                    }
                }
            });
        said
    }

    /// Everything the graph said, once the drain has finished or the patience has
    /// run out.
    ///
    /// Decoded here rather than as it arrives: a read ends wherever the pipe
    /// filled, which is not where a character does, so converting each chunk
    /// would put a replacement character in the middle of every message that
    /// crossed one.
    fn settled(&self) -> String {
        let deadline = Instant::now() + SAID_PATIENCE;
        while !self.ended.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(SAID_POLL);
        }
        String::from_utf8_lossy(&held(&self.bytes)).into_owned()
    }
}

/// A drain that marks itself finished when it goes, however it went.
///
/// The pipe reaching its end and a thread this host would not start are the same
/// answer to the one question a reader of [`Said`] has — nothing more is coming —
/// and putting that answer on `Drop` is what makes them one path instead of two,
/// only one of which any run reaches. The thread body ends and this drops with
/// it; the thread never starts and the closure holding this is dropped instead.
/// Every launch takes it, so nothing here is a branch a journey cannot reach.
struct Drained(Said);

impl Drop for Drained {
    fn drop(&mut self) {
        self.0.ended.store(true, Ordering::Release);
    }
}

/// A lock taken past a holder that panicked while it had it.
///
/// Nothing guarded here has an invariant a panic could break — one is a buffer
/// being appended to and the other a `Child` — so a poisoned lock is a thread
/// that died, not state to refuse. Refusing would turn a panic in a relay into a
/// run that cannot say what its graph said.
fn held<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the graph process is over, without ever waiting to find out.
///
/// `try_lock`, because the only other holders are this launch's own `wait` and
/// its handshake — and a relay that blocked on `wait` would be waiting on
/// precisely the answer it is asking for. A lock it could not take is *not
/// known to be over*, which is the same safe direction an unanswerable liveness
/// question takes everywhere else in this crate.
fn over(child: &Mutex<Child>) -> bool {
    child
        .try_lock()
        .is_ok_and(|mut child| matches!(child.try_wait(), Ok(Some(_))))
}

/// The lines a relayed launch wrote, as a stream that ends when the **graph
/// process** ends.
///
/// The pipe's own end is not that question. A pipe reaches its end when every
/// handle that may write to it is closed, and a process that inherited one holds
/// it open long after the graph that was given it has exited — so a relay that
/// read to the end waited on processes this run never started, and the node whose
/// dispatch it was never settled. Reading on a thread of its own is what
/// separates the two: what has arrived is yielded as it arrives, and a silence is
/// the moment to ask whether there is still anything that could write.
///
/// Nothing is lost by ending there. What the graph wrote before it exited is in
/// the pipe and read before the first silence; what could arrive afterwards is
/// another process's output on a stream this run has finished with.
fn relayed_lines(
    reader: BufReader<std::process::ChildStdout>,
    child: Arc<Mutex<Child>>,
) -> impl Iterator<Item = std::io::Result<String>> + Send {
    let (lines, arriving) = mpsc::channel();
    // Not waited on, and not branched on: the channel closing is what says the
    // stream ended, and the sender goes with the closure whether the thread ran
    // it or a host refused to start it. So a thread that never started ends the
    // stream down the same arm every launch ends down — a relay that reported
    // what the graph said and then finished — rather than down one of its own.
    let _ = std::thread::Builder::new()
        .name(format!("{}-relay", binary()))
        .spawn(move || {
            for line in reader.lines() {
                if lines.send(line).is_err() {
                    return;
                }
            }
        });
    std::iter::from_fn(move || loop {
        match arriving.recv_timeout(RELAY_POLL) {
            Ok(line) => return Some(line),
            // The pipe reached its end, which is the stream ending as it always
            // did on a host that leaves nothing holding it.
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            Err(mpsc::RecvTimeoutError::Timeout) if over(&child) => return None,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    })
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

/// One graph launch, as the two backends both receive it.
///
/// A value rather than eight positional arguments: the two backends and the
/// retained-process command line all take exactly this, and a launch that
/// reached one of them missing a field the others got is the drift this removes.
/// The source filter in particular has three ways to arrive — a library
/// `Request`, this build's own `drive`, and an overridden sibling's
/// `--event-filter` — and all three read it from here.
// llmlint: ignore-block[invalid_states_unrepresentable] `graph` is the resolved
// reference this struct's own callers already held as a string, gathered rather than
// retyped: the same launch-recorded, `resolve_graph`-checked value `src/lifecycle.rs`
// carries under the same reasoning, and the same one `retained_command` has to spell onto
// an argv either way. `oneagentgraph::config::ConfigRef` is transparent over exactly this
// string and adds no invariant, and one of the two callers here holds a `String` off the
// launch record rather than a `ConfigRef` — so taking one would mint the sibling's type
// at this seam only to unwrap it again two functions down.
#[derive(Debug, Clone, Copy)]
pub struct Launch<'a> {
    /// The agent-graph config to run.
    pub graph: &'a str,
    /// The task prose every member without its own is given.
    pub task: &'a str,
    /// The directory the graph's members work in.
    pub dir: &'a Path,
    /// The labels stamped on every envelope this launch relays.
    pub labels: &'a Labels,
    /// Environment exported to the launch, beyond this process's own.
    pub env: &'a [(String, String)],
    /// Opaque graph-config overrides, applied in order.
    pub sets: &'a [String],
    /// What this launch may relay onto the run's merged store, or everything.
    ///
    /// The run's own say over a source it does not itself produce: filtering
    /// belongs to the library that owns the stream, so what a launch is told not
    /// to emit is never emitted rather than read and dropped here.
    pub filter: Option<&'a EventFilter>,
    /// Where the started graph's own output goes.
    pub output: GraphOutput<'a>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

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
    /// Piped here, and read on this side: both streams belong to the arm that
    /// has them, so a logged launch cannot be holding a drain of a pipe it was
    /// never given.
    Relayed {
        /// The reader lives on this side rather than on the child because the
        /// handshake reads the first line off it and
        /// [`events`](GraphRun::events) reads the rest; `None` is that stream
        /// handed on, not a relayed launch that never had one.
        stdout: Option<BufReader<std::process::ChildStdout>>,
        /// This launch's stderr, drained as it arrives rather than read when
        /// somebody asks — see [`Said`].
        stderr: Said,
    },
    /// Appended to this file. It is the only place a refusal's message exists
    /// for a launch that logs, and it holds one launch's output and no other:
    /// the only caller that logs is `start --detach`, into a run directory
    /// minted for that launch.
    Logged(PathBuf),
}

/// The command a retained launch runs its graph with: *this executable*, asked
/// to [`drive`] the graph.
///
/// Self-exec rather than resolving `oneagentgraph` by name, which would compose
/// whatever the host installed — a second parser that can refuse what the
/// attached path accepts. [`BINARY_ENV`] is the explicit, all-or-nothing
/// override, and the only way an installed sibling is composed instead.
fn retained_command(
    graph: &str,
    task: &str,
    dir: &Path,
    labels: &[String],
    sets: &[String],
    filter: Option<&EventFilter>,
) -> Result<Command> {
    let mut command = match overridden() {
        true => {
            let mut command = Command::new(binary());
            command.arg("run").arg(graph);
            // The override's own CLI has to be told which of its renderings to
            // emit; this build's `drive` has only the one.
            command.arg("--output").arg("json");
            command
        }
        false => {
            let mut command = Command::new(std::env::current_exe().map_err(|e| {
                sibling(format!(
                    "cannot find this executable to retain a driver: {e}"
                ))
            })?);
            command.arg(DRIVE_VERB).arg(graph);
            command
        }
    };
    command.arg("--task").arg(task);
    command.arg("--dir").arg(dir);
    for label in labels {
        command.arg("--label").arg(label);
    }
    for value in sets {
        command.arg("--set").arg(value);
    }
    // Spelled the same to both, because both parse it the same way: this build's
    // own `drive` and the sibling's `run` each read a `--event-filter` that
    // starts with `{` as the document itself. Rendered rather than passed as the
    // spec an operator typed, so a launch never re-reads a file that may have
    // changed — or vanished — since the launch that validated it.
    if let Some(filter) = filter {
        command.arg("--event-filter").arg(
            serde_json::to_string(filter)
                .map_err(|error| sibling(format!("rendering the event filter: {error}")))?,
        );
    }
    Ok(command)
}

/// This crate's filter, as the sibling's own type.
///
/// Through the wire shape rather than field by field, because the wire shape is
/// what the grammar is: the two types are the same document by contract, and
/// crossing at their `Serialize`/`Deserialize` is what makes a field one of them
/// grows and the other has not a refusal here rather than a value silently
/// dropped on the way through. The sibling's own reader is also the one that
/// decides whether it will accept the spec, which is the answer that matters —
/// this is the value it is about to filter with.
fn sibling_filter(filter: &EventFilter) -> Result<oneagentgraph::event::EventFilter> {
    let document = serde_json::to_string(filter)
        .map_err(|error| sibling(format!("rendering the event filter: {error}")))?;
    serde_json::from_str(&document)
        .map_err(|error| sibling(format!("`oneagentgraph` refused the event filter: {error}")))
}

impl ProcessGraphRun {
    /// Start a graph, with its envelopes going wherever `output` says.
    pub fn start(launch: &Launch<'_>) -> Result<Self> {
        let output = launch.output;
        let mut command = retained_command(
            launch.graph,
            launch.task,
            launch.dir,
            &label_args(launch.labels),
            launch.sets,
            launch.filter,
        )?;
        for (key, value) in launch.env {
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
        let pid = child.id();
        // The destination asked for is the destination recorded, so the two
        // cannot come apart: a relayed launch reads its pipe even on the host
        // where taking the handle back off the child somehow gave nothing, and
        // reports that silence as the launch failing to answer.
        //
        // The stderr drain starts here rather than reading when somebody asks:
        // the reader of a pipe waits for every writing handle to close, and what
        // this launch is owed is what *its* process said.
        let output = match output {
            GraphOutput::Relayed => Output::Relayed {
                stdout: child.stdout.take().map(BufReader::new),
                stderr: Said::draining(child.stderr.take()),
            },
            GraphOutput::Logged(path) => Output::Logged(path.to_path_buf()),
        };
        Ok(Self {
            child: Arc::new(Mutex::new(child)),
            pid,
            output,
            started_with: Vec::new(),
            run_id: None,
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
            Output::Relayed { stdout, .. } => stdout.take(),
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
                let announcement = read.last().and_then(|line| envelope_of(line));
                let announced = announcement.is_some();
                self.run_id = announcement.as_ref().and_then(announced_run);
                // Put back rather than reassigned: this arm is the one the
                // reader came off a moment ago, and its drain of the other
                // stream has been running since the launch.
                if let Output::Relayed { stdout, .. } = &mut self.output {
                    *stdout = Some(reader);
                }
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
            if let Some(announcement) = self.logged_envelope() {
                self.run_id = announced_run(&announcement);
                return Ok(());
            }
            let waited = held(&self.child).try_wait();
            match waited {
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

    /// The first whole envelope this launch has written into its log, if any.
    ///
    /// A *complete* line — one the writer has terminated — that the envelope
    /// schema accepts. Both streams share the file, so this looks past whatever
    /// the graph said on stderr, and past a line from a build whose shape this
    /// one cannot read, rather than taking either for an announcement.
    fn logged_envelope(&self) -> Option<Envelope> {
        let Output::Logged(path) = &self.output else {
            return None;
        };
        let text = std::fs::read_to_string(path).ok()?;
        text.split_inclusive('\n')
            .filter(|line| line.ends_with('\n'))
            .find_map(envelope_of)
    }

    /// The graph closed its stream without announcing itself: report whatever it
    /// exited with.
    fn settle_unstarted(&mut self) -> Result<()> {
        let deadline = Instant::now() + startup_timeout();
        loop {
            let waited = held(&self.child).try_wait();
            match waited {
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
        Err(sibling(format!(
            "`{} run` {} instead of driving the run: {}",
            binary(),
            ended(&status),
            self.evidence()
        )))
    }

    /// A launch that neither started nor ended. It is not left running: nothing
    /// would ever collect it, and the caller is being told it did not start.
    fn gave_no_answer(&mut self) -> Result<()> {
        {
            let mut child = held(&self.child);
            let _ = child.kill();
            let _ = child.wait();
        }
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
            Output::Relayed { .. } => self.said(),
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
            Output::Relayed { stdout, .. } => stdout.take(),
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
                .chain(relayed_lines(stdout, Arc::clone(&self.child)))
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
        let status = held(&self.child)
            .wait()
            .map_err(|e| sibling(format!("waiting for `{} run`: {e}", binary())))?;
        Ok(Settled {
            code: status.code(),
            stderr: self.said(),
        })
    }

    /// Everything this launch said on its stderr, as the drain has it.
    ///
    /// A logged launch has no drain — its stderr is the file — and its own
    /// reader of that file is [`evidence`](Self::evidence).
    fn said(&self) -> String {
        match &self.output {
            Output::Relayed { stderr, .. } => stderr.settled(),
            Output::Logged(_) => String::new(),
        }
    }

    /// The started process's id, for the ledger's record of what is running.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The graph run's own id, once its announcement has been read.
    pub fn run_id(&self) -> Option<&GraphRunId> {
        self.run_id.as_ref()
    }

    /// Whether the graph process has ended, reaping it if it has.
    ///
    /// Reaping is the point. A child nobody waits on stays a zombie, and a
    /// zombie answers a liveness probe as alive — so an attach that never
    /// collected its driver would report a run as driven long after nothing
    /// was driving it.
    pub fn has_exited(&mut self) -> bool {
        matches!(held(&self.child).try_wait(), Ok(Some(_)))
    }
}

/// Put a launch's own pairs on **this** process, so the library path exports
/// what [`Launch::env`] promises.
///
/// The subprocess path sets them on the command it spawns, and every member of
/// that graph inherits them. The library path had the same effect until
/// `oneagentgraph 0.2.18` composed a member's environment per launch; from there
/// a member's turn is an `oneharness_core` call whose harness child inherits
/// *the hosting process's* environment, and the map handed to
/// [`oneagentgraph::run::start`] reaches only the `${VAR}` expansion of the
/// graph's own `env:` block. A pair set nowhere else therefore reached nothing —
/// silently, and on one backend of two. That is the split
/// [`GraphRun::start`]'s `dir` exists to prevent, so it is closed the same way:
/// both backends give a graph's members one answer.
///
/// Process-wide, which is what the module's **Concurrency** note is about, and
/// what makes it safe here is *which* pairs these are: a launch carries the
/// run's own id and where its ledger lives, both constant for the life of a
/// driver, and one driver drives one run. A per-node value must never come
/// through here.
fn export(env: &[(String, String)]) {
    for (key, value) in env {
        // A pair already holding what it is about to be given is left alone. The
        // caller that made this per-dispatch is `crate::executor`, and a driver's
        // several concurrent dispatches all carry the same run id — so writing it
        // once and then reading it is the difference between one process-wide
        // mutation per driver and one per node.
        if std::env::var(key).is_ok_and(|held| &held == value) {
            continue;
        }
        std::env::set_var(key, value);
    }
}

impl GraphRun {
    /// Start a graph through the sibling library. Detached launches retain the
    /// process boundary because a library scheduler thread cannot survive this
    /// process exiting.
    ///
    /// `dir` is required: the two backends have no common default for an absent
    /// one — the CLI's is `.`, resolved against whatever process spawns the
    /// graph, and the library's would be this process's own — so a caller that
    /// said nothing would send a different `--cwd` to the harness depending on
    /// which backend it happened to take. Naming it is the only way a run gets
    /// one answer.
    pub fn start(launch: &Launch<'_>) -> Result<Self> {
        if matches!(launch.output, GraphOutput::Logged(_)) || std::env::var_os(BINARY_ENV).is_some()
        {
            return ProcessGraphRun::start(launch).map(|run| Self {
                backend: GraphBackend::Process(run),
            });
        }
        Self::in_library(
            launch.graph,
            launch.task,
            launch.dir,
            &label_args(launch.labels),
            launch.env,
            launch.sets,
            launch.filter,
        )
    }

    /// Start a graph through the sibling library, in this process.
    ///
    /// Takes the labels already rendered as the `k=v` pairs the sibling parses,
    /// because [`drive`] receives them that way: it is the retained process, and
    /// what reached it came off a command line. [`start`](Self::start) renders
    /// its own with [`label_args`], so both callers hand the sibling's parser
    /// the same spelling.
    fn in_library(
        graph: &str,
        task: &str,
        dir: &Path,
        labels: &[String],
        env: &[(String, String)],
        sets: &[String],
        filter: Option<&EventFilter>,
    ) -> Result<Self> {
        let mut run_env = process_env();
        run_env.extend(env.iter().cloned());
        export(env);
        let labels = labels
            .iter()
            .map(|label| {
                oneagentgraph::run::parse_label(label).map_err(|error| sibling(error.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;
        let overrides = sets
            .iter()
            .map(|value| {
                oneagentgraph::run::parse_set(value).map_err(|error| sibling(error.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;
        let state_dir = state_dir(&run_env);
        let request = oneagentgraph::run::Request {
            graph: oneagentgraph::config::ConfigRef(graph.to_string()),
            task: Some(task.to_string()),
            dir: dir.to_path_buf(),
            labels,
            overrides,
            filter: filter.map(sibling_filter).transpose()?,
            state_dir,
            oneharness_bin: oneharness_bin(&run_env),
        };
        let running = oneagentgraph::run::start(&request, &run_env)
            .map_err(|error| sibling(error.to_string()))?;
        let run_id = running.started().run_id.clone();
        let (events_tx, events_rx) = mpsc::channel();
        let (settled_tx, settled_rx) = mpsc::channel();
        let (cancel_tx, cancel_rx) = mpsc::channel();
        let exited = Arc::new(AtomicBool::new(false));
        let thread_exited = Arc::clone(&exited);
        std::thread::Builder::new()
            .name("oneagentgraph-relay".into())
            .spawn(move || {
                loop {
                    if cancel_rx.try_recv().is_ok() {
                        let _ = running.cancel();
                    }
                    match running.recv_timeout(Duration::from_millis(10)) {
                        Ok(Some(envelope)) => {
                            if events_tx.send(relayed(envelope)).is_err() {
                                let _ = running.cancel();
                                break;
                            }
                        }
                        // Nothing yet. Round again — which is also what makes
                        // the cancel above reachable while the graph is quiet,
                        // so the poll is the design rather than a busy wait.
                        //
                        // The sibling reports a timeout as `Ok(None)` and keeps
                        // `Err` for a channel that is finished, so the second
                        // arm is what a newer build could start answering with.
                        // Rounding again is the safe reading of both: a relay
                        // that panicked here would take a live run down over a
                        // wait that had simply expired.
                        Ok(None) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                let settled = Ok(match running.wait() {
                    Ok(code) => Settled {
                        code: Some(code),
                        stderr: String::new(),
                    },
                    // A refusal, given the code the *process* path would have
                    // carried for it. The sibling's CLI turns its `Error` into an
                    // exit code and the library hands the `Error` over instead,
                    // so a caller of both has to apply that rule itself or the
                    // two paths settle the same graph differently.
                    Err(error) => Settled {
                        code: Some(exit_for(&error)),
                        stderr: error.to_string(),
                    },
                });
                thread_exited.store(true, Ordering::Release);
                let _ = settled_tx.send(settled);
            })
            .map_err(|error| sibling(format!("cannot start graph relay: {error}")))?;
        Ok(Self {
            backend: GraphBackend::Library(LibraryGraphRun {
                events: Some(events_rx),
                settled: settled_rx,
                cancel: cancel_tx,
                run_id,
                exited,
            }),
        })
    }

    pub fn confirm_started(&mut self) -> Result<()> {
        match &mut self.backend {
            GraphBackend::Library(_) => Ok(()),
            GraphBackend::Process(run) => run.confirm_started(),
        }
    }

    pub fn events(&mut self) -> Box<dyn Iterator<Item = Result<Envelope>> + Send> {
        match &mut self.backend {
            GraphBackend::Library(run) => run.events.take().map_or_else(
                || {
                    Box::new(std::iter::empty())
                        as Box<dyn Iterator<Item = Result<Envelope>> + Send>
                },
                |events| Box::new(events.into_iter()),
            ),
            GraphBackend::Process(run) => run.events(),
        }
    }

    pub fn wait(&mut self) -> Result<Settled> {
        match &mut self.backend {
            GraphBackend::Library(run) => run
                .settled
                .recv()
                .map_err(|error| sibling(format!("waiting for graph run: {error}")))?,
            GraphBackend::Process(run) => run.wait(),
        }
    }

    /// The `oneagentgraph` run id this launch minted, whichever way it ran.
    ///
    /// **Not this crate's run id**, and that distinction is the whole reason
    /// this exists: the sibling addresses a run's signals — a pacemaker reset
    /// among them — by the id it minted, and a caller that handed it a
    /// `onepipeline` run id would be naming a run the sibling has never heard
    /// of. The library backend is told at startup; the retained-process backend
    /// reads it off the announcement its handshake already waits for, so a
    /// detached launch knows it too.
    ///
    /// `None` only where the graph started without announcing itself — a run
    /// that succeeded before it said anything — which is the same case that
    /// leaves nothing to address.
    pub fn run_id(&self) -> Option<&GraphRunId> {
        match &self.backend {
            GraphBackend::Library(run) => Some(&run.run_id),
            GraphBackend::Process(run) => run.run_id(),
        }
    }

    /// The process this graph run's work is happening in, where it is one this
    /// crate started.
    ///
    /// `None` for the library backend, and that is not "no process": the graph
    /// is running *in this one*, and a caller recording where a run's work is
    /// records its own pid for it. The distinction is which process a teardown
    /// would have to aim at, and only the caller knows whether it is willing to
    /// name itself.
    pub fn process(&self) -> Option<u32> {
        match &self.backend {
            GraphBackend::Library(_) => None,
            GraphBackend::Process(run) => Some(run.pid()),
        }
    }

    /// Whether the graph has ended, reaping a retained process if it has.
    ///
    /// Reaping is the point for the process backend. A child nobody waits on
    /// stays a zombie, and a zombie answers a liveness probe as alive.
    pub fn has_exited(&mut self) -> bool {
        match &mut self.backend {
            GraphBackend::Library(run) => run.exited.load(Ordering::Acquire),
            GraphBackend::Process(run) => run.has_exited(),
        }
    }

    /// Stop the graph, whichever way it is running.
    ///
    /// Both backends, and that is the whole point of the name: a caller asking
    /// a run to stop must not get silence because of how the run happens to be
    /// reached. The library backend hands the ask to the relay, which calls the
    /// sibling's own [`cancel`](oneagentgraph::run::Running::cancel) — the same
    /// stop signal and process-tree reap the `cancel` verb performs. The
    /// process backend has no signal file to write, so it is the child that is
    /// taken down, and its descendants with it: the harness the graph started
    /// is one of them, and stopping only the parent would leave it holding the
    /// workspace.
    ///
    /// Best-effort and non-blocking in both, because a cancel is a caller
    /// changing its mind rather than an operation whose failure it can act on.
    /// [`wait`](Self::wait) is what reports how the run actually ended.
    pub fn cancel(&self) {
        match &self.backend {
            GraphBackend::Library(run) => {
                let _ = run.cancel.send(());
            }
            GraphBackend::Process(run) => {
                // A cancel is best-effort by contract — the caller has changed
                // its mind rather than asked a question — so how far the
                // teardown reached is not reported back here. `wait` is what
                // says how the run actually ended.
                let _ = crate::sys::stop(run.pid(), crate::sys::Stop::Now);
            }
        }
    }
}

/// Run a graph in **this** process, streaming its envelopes as NDJSON on stdout.
///
/// The retained half of [`retained_command`]. What it writes is the same NDJSON
/// `oneagentgraph run --output json` writes, because both cross the sibling's
/// own `Serialize`, so a launcher's handshake reads either the same way.
///
/// Each line is flushed as it is written: a launcher is waiting on the
/// announcement, and a buffered one arrives after the launch has given up on it.
///
/// A refusal is left to the caller — printed by the binary with every other one,
/// into the launch log the launcher reads its evidence from.
pub fn drive(
    graph: &str,
    task: &str,
    dir: &Path,
    labels: &[String],
    sets: &[String],
    filter: Option<&str>,
) -> Result<i32> {
    use std::io::Write;

    // Read here rather than in the launcher that spelled it: this is a process
    // boundary, so the spec arrives as text and this is where it becomes a value
    // again — refused, with the offending matcher named, before a graph starts.
    let filter = filter.map(EventFilter::read).transpose()?;
    let mut run = GraphRun::in_library(graph, task, dir, labels, &[], sets, filter.as_ref())?;
    let mut out = std::io::stdout();
    for envelope in run.events() {
        let envelope = envelope?;
        let line = serde_json::to_string(&envelope)
            .map_err(|error| sibling(format!("rendering graph event: {error}")))?;
        writeln!(out, "{line}")
            .map_err(|error| sibling(format!("relaying graph event: {error}")))?;
        out.flush()
            .map_err(|error| sibling(format!("relaying graph event: {error}")))?;
    }
    let settled = run.wait()?;
    let said = settled.stderr.trim();
    if !said.is_empty() {
        eprintln!("{said}");
    }
    // The graph's own code, so a launcher reading this process's exit reads the
    // sibling's answer rather than a second opinion about it. A run ended by a
    // signal carries none, and the sibling's own CLI reports that as the
    // member-failed code.
    Ok(settled
        .code
        .unwrap_or(oneagentgraph::error::EXIT_MEMBER_FAILED))
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
///
/// [`oneagentgraph::run::signal`] is the same implementation the `reset-timer`
/// verb runs, so which member names are addressable and where the run watches
/// are decided once, in the sibling, rather than twice.
pub fn reset_timer(run: &GraphRunId, member: &str) -> Result<()> {
    if std::env::var_os(BINARY_ENV).is_some() {
        return reset_timer_by_process(run, member);
    }
    let member_name = oneagentgraph::run::MemberName::parse(member)
        .map_err(|error| sibling(format!("reset-timer {run} {member}: {error}")))?;
    oneagentgraph::run::signal(
        &state_dir(&process_env()),
        run,
        &member_name,
        oneagentgraph::run::Signal::Reset,
    )
    .map_err(|error| sibling(format!("reset-timer {run} {member}: {error}")))
}

/// The same reset, through an executable an operator named at [`BINARY_ENV`].
fn reset_timer_by_process(run: &GraphRunId, member: &str) -> Result<()> {
    let output = Command::new(binary())
        .arg("reset-timer")
        .arg(run.as_str())
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
    if std::env::var_os(BINARY_ENV).is_some() {
        return interrupt_by_process(address, input);
    }
    let env = process_env();
    let addressed = oneagentgraph::run::RunId::parse(address.run())
        .map_err(|error| error.to_string())
        .and_then(|run_id| {
            oneagentgraph::run::MemberName::parse(address.member())
                .map_err(|error| error.to_string())
                .map(|member| (run_id, member))
        });
    let (run_id, member) = match addressed {
        Ok(addressed) => addressed,
        // Not an address the sibling can act on, so nothing was delivered and
        // no lever was pulled — the same answer, and the same silence on the
        // stream, that the verb's own argument refusal gave.
        Err(reason) => {
            return Interrupt {
                outcome: Interrupted::Failed(format!(
                    "`oneagentgraph interrupt {} {}` was refused: {reason}",
                    address.run(),
                    address.member()
                )),
                events: Vec::new(),
            }
        }
    };
    let delivered = oneagentgraph::control::interrupt(
        &state_dir(&env),
        &run_id,
        &member,
        Some(input),
        &oneharness_bin(&env),
    );
    // llmlint: ignore-block[changed_behavior_has_e2e] three of these five answers cannot
    // be reached offline, and the reason is the sibling's rather than a gap in the suite.
    // `Delivered`, `Failed`, and `Invalid` are all what `control::deliver` came back with,
    // and it is only ever called for a member whose scratch holds an *open* turn — which
    // `oneagentgraph` writes from `judge.rs` alone, for a `kind: onejudge` member. The
    // graphs these journeys run declare `kind: oneharness` members, deliberately: the
    // two-party kind runs its conversation against a provider this repository has no
    // offline stand-in for, which is divergence 21. So a real dispatch here reaches
    // `NoTurn`, and `a_note_delivered_through_the_real_sibling_records_what_its_lever_answered`
    // is the journey that drives it, envelope and all. The other two arms — an address the
    // sibling cannot parse, and a run it cannot find — are driven directly by this module's
    // own tests, which is the only entry point either has.
    let (outcome, reason) = match delivered {
        Ok(oneagentgraph::control::Delivery::Delivered) => (Interrupted::Delivered, None),
        Ok(oneagentgraph::control::Delivery::NoTurn(reason)) => {
            (Interrupted::NoTurn(reason.clone()), Some(reason))
        }
        Ok(oneagentgraph::control::Delivery::Failed(reason)) => (
            Interrupted::Failed(format!(
                "`oneagentgraph interrupt {} {}` could not deliver: {reason}",
                address.run(),
                address.member()
            )),
            Some(reason),
        ),
        // A redirection the sibling would not take, and a run or member it
        // cannot address, are both arguments this caller got wrong: the verb
        // refuses them *before* any event claims a lever was pulled, so neither
        // publishes one here either.
        Ok(oneagentgraph::control::Delivery::Invalid(reason)) => {
            return Interrupt {
                outcome: Interrupted::Failed(format!("--input: {reason}")),
                events: Vec::new(),
            }
        }
        Err(error) => {
            return Interrupt {
                outcome: Interrupted::Failed(format!(
                    "`oneagentgraph interrupt {} {}` was refused: {error}",
                    address.run(),
                    address.member()
                )),
                events: Vec::new(),
            }
        }
    }; // llmlint: ignore-end[changed_behavior_has_e2e]
    Interrupt {
        outcome,
        events: published(&run_id, address.member(), input.len() as u64, reason),
    }
}

/// The `turn-interrupted` envelope this interrupt is owed, for the merged store.
///
/// The library call hands back the [`Delivery`](oneagentgraph::control::Delivery)
/// and nothing else, deliberately: the verb's other two halves are an exit code
/// and an envelope on *a process's* stdout, and a library caller has neither.
/// So this crate publishes the envelope — but through the sibling's own
/// [`Emitter`](oneagentgraph::event::Emitter) and its own
/// [`TurnInterrupted`](oneagentgraph::event::TurnInterrupted) payload, into a
/// buffer instead of onto stdout. The bytes are the ones the CLI wrote, because
/// the code that writes them is the same; a hand-rolled JSON object here would
/// be a second producer of a shape the sibling owns, and the first field it
/// added or renamed would land only on the process path.
fn published(
    run_id: &oneagentgraph::run::RunId,
    member: &str,
    input_bytes: u64,
    reason: Option<String>,
) -> Vec<Envelope> {
    let sink = Captured::new();
    let emitter = oneagentgraph::event::Emitter::new(
        // The verb's own stream id: an envelope's `stream` is a unique id per
        // producing process, and this process is the one producing it.
        format!("{run_id}-interrupt-{}", std::process::id()),
        Box::new(sink.clone()),
    )
    .with_labels(oneagentgraph::event::Labels {
        run_id: Some(run_id.to_string()),
        member: Some(member.to_string()),
        ..oneagentgraph::event::Labels::default()
    });
    let payload = oneagentgraph::event::TurnInterrupted {
        member: member.to_string(),
        delivered: reason.is_none(),
        input_bytes,
        reason,
    };
    emitter.emit(
        oneagentgraph::event::EventKind::TurnInterrupted,
        match serde_json::to_value(&payload) {
            Ok(serde_json::Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        },
    );
    read_envelopes(&sink.written())
}

/// A sink that keeps what was written to it.
///
/// [`Emitter`](oneagentgraph::event::Emitter) takes ownership of its sink and
/// hands nothing back, so the way to read what it wrote is to write into
/// something shared. One line is ever emitted through it, and it is read after
/// the emit returns.
#[derive(Debug, Clone)]
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

impl Captured {
    fn new() -> Self {
        Self(Arc::new(std::sync::Mutex::new(Vec::new())))
    }

    /// What has been written to it so far, as text.
    fn written(&self) -> String {
        self.0.lock().map_or_else(
            |held| String::from_utf8_lossy(&held.into_inner()).into_owned(),
            |held| String::from_utf8_lossy(&held).into_owned(),
        )
    }
}

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut held) = self.0.lock() {
            held.extend_from_slice(bytes);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Read a verb's NDJSON into envelopes, saying how many lines were skipped.
///
/// A line this build cannot read is skipped rather than ending the read — a
/// sibling emitting a shape this build does not know must not stop the ones it
/// does — but never *quietly*: the same rule, and the same report, as the
/// relayed run stream.
fn read_envelopes(text: &str) -> Vec<Envelope> {
    let mut skipped = 0;
    let envelopes: Vec<Envelope> = text
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
    envelopes
}

/// The same interrupt, through an executable an operator named at
/// [`BINARY_ENV`].
fn interrupt_by_process(address: &TurnAddress, input: &str) -> Interrupt {
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
    let events = read_envelopes(&String::from_utf8_lossy(&output.stdout));
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
    // Three answers, and everything that is not one of the first two is the
    // third: a delivery that broke is one outcome however the process ended, so
    // a signal and a non-zero exit take the same arm and differ only in the
    // words [`ended`] gives them.
    let outcome = match output.status.code() {
        Some(oneagentgraph::error::EXIT_SUCCESS) => Interrupted::Delivered,
        Some(oneagentgraph::error::EXIT_NO_CONTROLLABLE_TURN) => Interrupted::NoTurn(reason()),
        _ => Interrupted::Failed(format!(
            "`{} interrupt {} {}` {}: {}",
            binary(),
            address.run(),
            address.member(),
            ended(&output.status),
            reason()
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
    if std::env::var_os(BINARY_ENV).is_some() {
        let output = Command::new(binary())
            .arg("health")
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return (!text.is_empty()).then_some(text);
    }
    oneagentgraph::health::read()
        .ok()
        .and_then(|report| serde_json::to_string_pretty(&report).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state directory holding one real `oneagentgraph` run record.
    ///
    /// The sibling's **own** [`Record`](oneagentgraph::run::Record), serialized
    /// by the sibling's own `serde` impl into the file name the sibling names,
    /// so what the calls below read back is the on-disk contract they read in
    /// production rather than a shape restated here. A record this crate
    /// hand-wrote as JSON would keep passing against a sibling that had renamed
    /// a field — the exact drift the subprocess doubles used to hide.
    ///
    /// The variable is set rather than passed because these entry points read
    /// the process's environment, which is what a run of the binary gives them;
    /// nextest runs each test in its own process, so it reaches nothing else.
    fn state_dir_holding(run: &str, members: &[&str]) -> PathBuf {
        static NTH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "op-graphstate-{}-{}",
            std::process::id(),
            NTH.fetch_add(1, Ordering::SeqCst)
        ));
        let run_id = oneagentgraph::run::RunId::parse(run).expect("a run id the sibling accepts");
        let dir = root.join(run_id.as_str());
        std::fs::create_dir_all(&dir).expect("a run directory");
        let record = oneagentgraph::run::Record {
            schema_version: oneagentgraph::run::RECORD_SCHEMA_VERSION,
            run_id,
            graph: "node-scope.yaml".into(),
            name: "node-scope".into(),
            started_ms: 1_786_304_152_340,
            finished_ms: None,
            exit_code: None,
            members: std::collections::BTreeMap::new(),
            declared_members: members.iter().map(|m| (*m).to_string()).collect(),
            refs: Vec::new(),
            events_path: dir
                .join(oneagentgraph::run::EVENTS_FILE)
                .display()
                .to_string(),
        };
        std::fs::write(
            dir.join(oneagentgraph::run::RECORD_FILE),
            serde_json::to_string(&record).expect("the sibling's record serialises"),
        )
        .expect("the run record is written");
        std::env::set_var(STATE_DIR_ENV, &root);
        std::env::remove_var(BINARY_ENV);
        root
    }

    /// A graph's `env:` block lands in **this** process, which is what stops
    /// concurrent dispatches from being isolated from one another.
    ///
    /// This is the answer to "what in the sibling's library path is
    /// process-wide", and it is observed rather than argued: the graph below
    /// declares one variable and removes nothing, and after the launch this
    /// process is carrying it.
    ///
    /// The mechanism is `oneagentgraph::run::run` calling its private `export`,
    /// which does `std::env::remove_var` and `std::env::set_var` on the running
    /// process — correct while a graph run *was* a process, and load-bearing
    /// even now, because a two-party member is a thread here and the
    /// `oneharness run` it spawns has to inherit what the contract promises it.
    /// One process per graph made that safe. One thread per graph does not:
    /// this crate dispatches several nodes at once, so two concurrent runs each
    /// write the other's members' environment, and `ONEHARNESS_HARNESSES` is
    /// cleared out from under whichever run did not ask for that. It is also a
    /// data race — `set_var` is why Rust 2024 made it `unsafe`.
    ///
    /// The shipped graphs declare no `env:` block, so nothing in this
    /// repository trips it today; a graph that adds one would. Held as a test
    /// rather than as a note so the day upstream confines it, this fails and
    /// says so.
    #[test]
    fn a_graphs_env_block_is_exported_into_this_process_and_not_into_the_run_alone() {
        let root = state_dir_holding("node-scope-1786304152340-30", &["worker"]);
        let probe = "ONEPIPELINE_GRAPH_ENV_PROBE";
        std::env::remove_var(probe);
        // A harness that is not there, so the member fails immediately: the
        // export happens before anything launches, which is exactly the point.
        std::env::set_var(ONEHARNESS_BIN_ENV, "oneharness-that-is-not-installed");
        std::fs::write(
            root.join("oneharness.toml"),
            "run_mode = \"fallback\"\nharnesses = [\"claude-code\"]\n",
        )
        .expect("the harness config is written");
        let graph = root.join("exports.yaml");
        std::fs::write(
            &graph,
            format!(
                "version: 1\nname: exports\nenv:\n  {probe}: \"from the graph\"\nmembers:\n  \
                 worker:\n    kind: oneharness\n    oneharness_config: ./oneharness.toml\n"
            ),
        )
        .expect("the graph config is written");

        // Whether it *ran* is not the claim — the member cannot, by
        // construction. The claim is what the launch did to this process.
        let started = GraphRun::start(&Launch {
            graph: &graph.to_string_lossy(),
            task: "## What\nNothing.\n\n## Why\nThe export is the subject.\n\n## Acceptance \
                   criteria\n- None.",
            dir: &root,
            labels: &Labels::default(),
            env: &[],
            sets: &[],
            filter: None,
            output: GraphOutput::Relayed,
        });
        if let Ok(mut run) = started {
            run.cancel();
            let _ = run.wait();
        }

        assert_eq!(
            std::env::var(probe).ok().as_deref(),
            Some("from the graph"),
            "the graph's env block did not reach this process — if upstream has confined it to \
             the run, this test has done its job and the concurrency note above is stale"
        );
        std::env::remove_var(probe);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The linked `oneagentgraph` produces the session conversation this crate
    /// relays.
    ///
    /// The floor it holds, and why it is carried by `Cargo.lock` rather than by
    /// the requirement, are with the pin in `Cargo.toml`.
    ///
    /// What is not obvious here is the **spelling**: both halves are written in
    /// items the *older* resolution also has — [`Emitter`], [`Labels`], and
    /// [`EventKind`]'s own deserializer — and the label key is the literal
    /// string rather than that library's `SESSION_LABEL` constant. 0.3.3's new
    /// vocabulary would make this a *compile* error below the floor, and a
    /// compile error names a missing symbol rather than a stale lock.
    ///
    /// [`Emitter`]: oneagentgraph::event::Emitter
    /// [`EventKind`]: oneagentgraph::event::EventKind
    #[test]
    fn the_linked_oneagentgraph_produces_the_session_conversation_this_crate_relays() {
        let run_id =
            oneagentgraph::run::RunId::parse("node-scope-1786304152340-19").expect("a run id");
        let [envelope] = &published(&run_id, "worker", 12, None)[..] else {
            panic!("an interrupt publishes exactly one envelope");
        };
        let conversation = format!("{}.worker", envelope.stream);
        assert_eq!(
            envelope
                .labels
                .extra
                .get("session")
                .and_then(serde_json::Value::as_str),
            Some(conversation.as_str()),
            "the linked oneagentgraph stamps no `session` on a turn it names: the \
             session-conversation producer ships in 0.3.3, and `Cargo.toml` requires the \
             newest release, which is above that floor — so `Cargo.lock` is behind the \
             manifest too and `cargo update -p oneagentgraph` is the whole of the fix"
        );
        assert!(
            serde_json::from_value::<oneagentgraph::event::EventKind>(serde_json::Value::String(
                "oneharness-session".to_string()
            ))
            .is_ok(),
            "the linked oneagentgraph does not know the `oneharness-session` kind, so no run \
             this engine drives can say where an agent's conversation was written down: that \
             event ships in 0.3.3 and `Cargo.lock` predates it — `cargo update -p \
             oneagentgraph`"
        );
    }

    /// The linked `oneagentgraph` produces the **whole** turn a transcript is
    /// rendered from, and not an outline of one.
    ///
    /// The second floor carried by `Cargo.lock`; what it holds is with the pin.
    ///
    /// The **spelling** is this test's own concern, as it is the sibling
    /// above's: every assertion is written in an item the older resolution also
    /// has — [`EventKind`]'s deserializer, and the [`TurnActivity`] and
    /// [`TurnCompleted`] payload types, which existed there with fewer fields
    /// and `deny_unknown_fields` over them. `turn-started` is the one field set
    /// that cannot be, because its payload type is new at the floor;
    /// `tests/e2e/turns.rs` holds that one against a real dispatch instead.
    ///
    /// [`EventKind`]: oneagentgraph::event::EventKind
    /// [`TurnActivity`]: oneagentgraph::event::TurnActivity
    /// [`TurnCompleted`]: oneagentgraph::event::TurnCompleted
    #[test]
    fn the_linked_oneagentgraph_produces_the_whole_turn_this_crate_relays() {
        /// What every assertion here has to say, because it is the only thing
        /// that fixes any of them.
        const MOVE_THE_LOCK: &str = "`Cargo.toml` requires the newest release, which is \
             above this floor, so a resolution that fails here is behind the manifest too and \
             `cargo update -p oneagentgraph` is the whole of the fix; `just engines-current` \
             names it without running the suite";

        assert!(
            serde_json::from_value::<oneagentgraph::event::EventKind>(serde_json::Value::String(
                "turn-message".to_string()
            ))
            .is_ok(),
            "the linked oneagentgraph does not know the `turn-message` kind, so no dispatch \
             this engine drives relays a word any party said while it was saying it: that kind \
             ships in 0.3.6 and the resolution predates it. {MOVE_THE_LOCK}"
        );
        // The observation half of an activity: a `tool_result` names no tool
        // because it answers one already named, carries what came back, and
        // joins to its call by id. Below the floor `name` is a bare `String`
        // and the other three are unknown fields, so this is refused there.
        serde_json::from_value::<oneagentgraph::event::TurnActivity>(serde_json::json!({
            "kind": "tool_result",
            "name": null,
            "detail": "",
            "output": "ok",
            "tool_call_id": "toolu_1",
            "index": 1,
        }))
        .unwrap_or_else(|error| {
            panic!(
                "the linked oneagentgraph has no reading of the observation that answered a \
                 tool call, so a relayed turn carries what the agent asked for and never what \
                 came back: {error}. {MOVE_THE_LOCK}"
            )
        });
        // And a turn's close is **one turn's** close: which turn, whose, over
        // what interval, and what that turn alone consumed. Below the floor the
        // payload is a lone `usage` whose figures are spelled differently, so
        // every field named here is unknown there.
        serde_json::from_value::<oneagentgraph::event::TurnCompleted>(serde_json::json!({
            "turn": 1,
            "role": "assistant",
            "usage": {"input_tokens": 1, "output_tokens": 1, "cost_usd": 0.0},
            "started_at": "2026-08-21T09:15:02.847Z",
            "finished_at": "2026-08-21T09:15:04.912Z",
        }))
        .unwrap_or_else(|error| {
            panic!(
                "the linked oneagentgraph does not close a turn on that turn's own account, so \
                 a run this engine drives cannot say which turn spent what: {error}. \
                 {MOVE_THE_LOCK}"
            )
        });
    }

    /// The linked `oneagentgraph` holds the `engineer` bar to what a dispatch
    /// can settle inside its own run.
    ///
    /// `engineer` is the role [`crate::executor`] names on `members.worker` for
    /// an ordinary implementation node, so the bar its judge reviews against is
    /// the linked library's file rather than anything this crate ships. Until
    /// 0.3.5 that bar refused "done" until the behaviour was *proven end to
    /// end* — a demand no dispatch can meet now that no gate runs inside the
    /// publication and verification is the merge path's, so every node would
    /// fail its review. The requirement is above that floor, so a resolution
    /// below it is behind the manifest too and moving the lock is the fix.
    ///
    /// Read through [`merge`] rather than off the YAML, because the merged
    /// config is what a judge is handed; a bar that arrived some other way is
    /// not the one under review. Both halves are asserted — that the demand is
    /// gone, and that what replaced it still reviews the change the member
    /// produced — so a resolution that softened the bar rather than narrowing
    /// it fails here too.
    ///
    /// [`merge`]: oneagentgraph::persona::merge
    #[test]
    fn the_linked_oneagentgraph_holds_the_engineer_bar_to_what_a_dispatch_can_prove() {
        let document = oneagentgraph::persona::shipped("engineer")
            .expect("the linked oneagentgraph ships the role this crate dispatches under");
        let persona = oneagentgraph::persona::Persona::parse(document, "engineer")
            .expect("the shipped engineer role loads");
        // An empty base, so what the merge leaves under `user:` is the shipped
        // role's own bar rather than an operator's layered under it.
        let effective = oneagentgraph::persona::merge("{}\n", "an empty base config", &persona)
            .expect("the shipped engineer role layers onto a base config");
        // A bar is a wrapped block scalar, so match on its words and not its
        // line breaks.
        let stance = effective["user"]["persona"]
            .as_str()
            .expect("the engineer role hands its judge a stance")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        assert!(
            !stance.contains("proven end to end"),
            "the linked oneagentgraph's `engineer` bar still refuses to accept work until it is \
             proven end to end, which no dispatch can satisfy from inside its own run: the \
             correction ships in 0.3.5, and `Cargo.toml` requires the newest release, which is \
             above that floor — so `Cargo.lock` is behind the manifest too and \
             `cargo update -p oneagentgraph` is the whole of the fix:\n{stance}"
        );
        for demand in [
            "the task's acceptance criteria are met",
            "proven at the level this run can reach",
            "no regression is introduced in what it touched",
        ] {
            assert!(
                stance.contains(demand),
                "the linked oneagentgraph's `engineer` bar no longer demands {demand:?}, so \
                 narrowing what it may ask for has softened what it must:\n{stance}"
            );
        }
    }

    /// An envelope is the same value whichever way it crossed.
    ///
    /// This is the *content* half of the streaming promise: the subprocess path
    /// read a line of the sibling's NDJSON and the library path is handed the
    /// typed envelope that line is written from, so a relay that had drifted
    /// would put a different value into the merged store depending on which
    /// path a run took. Held by sending one envelope both ways and comparing —
    /// the `serde` round-trip is the *same* boundary in both, which is the
    /// property being kept rather than an implementation detail.
    ///
    /// The *timing* half is
    /// `status_says_what_a_live_dispatch_is_doing_and_the_readout_advances` in
    /// `tests/e2e/dispatch.rs`, which runs on this path: it reads a live
    /// dispatch's tool summary out of the merged store while the node is still
    /// in flight, and then reads it again — advanced — while it still is. An
    /// envelope buffered to the end of the turn would fail both readings.
    #[test]
    fn a_relayed_envelope_is_the_same_whether_it_crossed_as_a_value_or_as_a_line() {
        let mut labels = oneagentgraph::event::Labels {
            run_id: Some("node-scope-1786304152340-19".into()),
            member: Some("worker".into()),
            ..oneagentgraph::event::Labels::default()
        };
        labels
            .extra
            .insert("onepipeline.node".into(), "build".into());
        labels
            .extra
            .insert("onepipeline.step".into(), "implement".into());
        let produced = oneagentgraph::event::Envelope {
            v: 1,
            ts: "2026-08-13T09:15:00.123Z".into(),
            stream: "node-scope-1786304152340-19".into(),
            seq: 7,
            source: oneagentgraph::event::Source::Agentgraph,
            kind: oneagentgraph::event::EventKind::TurnActivity,
            labels,
            payload: serde_json::Map::new(),
            artifacts: Vec::new(),
        };

        // The line the subprocess path read, off the sibling's own serializer.
        let line = serde_json::to_string(&produced).expect("the sibling's envelope serialises");
        let [off_the_wire] = &read_envelopes(&line)[..] else {
            panic!("the sibling's own NDJSON did not read back as one envelope: {line}");
        };
        let mut off_the_wire = off_the_wire.clone();
        adopt_labels(&mut off_the_wire.labels);

        let in_process = relayed(produced).expect("the library path relays it");

        assert_eq!(
            in_process, off_the_wire,
            "the same envelope reaches the merged stream differently depending on which path \
             relayed it"
        );
        // Not a vacuous comparison: the enrichment both paths apply really ran.
        assert_eq!(in_process.labels.node.as_deref(), Some("build"));
        assert_eq!(in_process.labels.step.as_deref(), Some("implement"));
    }

    /// A reset reaches the run's own signal directory, under the name the
    /// sibling watches for.
    ///
    /// The file rather than an `Ok(())`: `signal` answers success for a write it
    /// made, so asserting only on the return would pass against a call that wrote
    /// somewhere the run never looks — which is the whole failure mode the
    /// sibling grew `declared_members` to close.
    #[test]
    fn a_reset_leaves_the_signal_the_run_watches_for() {
        let root = state_dir_holding("node-scope-1786304152340-19", &[CHECK_IN_MEMBER]);
        let graph_run = recorded_graph_run("node-scope-1786304152340-19", "demo")
            .expect("the sibling accepts its own run id");
        reset_timer(&graph_run, CHECK_IN_MEMBER)
            .expect("the sibling accepts a reset for a member it declared");
        assert!(
            root.join("node-scope-1786304152340-19")
                .join(oneagentgraph::run::SIGNAL_DIR)
                .join(format!("{CHECK_IN_MEMBER}.reset"))
                .is_file(),
            "the reset left no signal where the run watches for one"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A recorded value that is not an address leaves the observer reported as
    /// watching.
    ///
    /// The record is a file a later process re-reads, so this is the interfered-with
    /// launch record `recorded_graph_run` exists for — and the direction is the
    /// point: this verdict is what tells an operator a run is executing unwatched,
    /// and saying that because a *field* was unreadable would send somebody to
    /// relaunch a graph that is working. What the sibling's record says, in both
    /// directions, is driven through the views themselves by
    /// `dispatch::a_run_whose_observer_graph_is_watching_and_then_is_not_reads_as_each`
    /// and `views::an_observer_this_host_cannot_ask_about_is_never_reported_dead`.
    #[test]
    fn a_recorded_value_that_is_not_an_address_leaves_the_observer_watching() {
        let root = state_dir_holding("dag-scope-1786304152340-19", &["monitor"]);
        for recorded in ["   ", "../elsewhere"] {
            assert!(
                !graph_run_ended(recorded, "demo"),
                "'{recorded}' was read as a graph run that had ended"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A graph run read back off a launch record only becomes an address by
    /// passing the sibling's own parser.
    ///
    /// The record is a file a later process re-reads, so this field is a
    /// stranger's string: one that would name a path outside the sibling's run
    /// store must not reach a call that joins it onto that store, and an absent
    /// one is a different answer from a malformed one — the first is a record
    /// from a build that had no such field, the second is a record that has been
    /// interfered with.
    #[test]
    fn a_recorded_graph_run_is_an_address_only_if_the_sibling_would_answer_to_it() {
        assert_eq!(
            recorded_graph_run("node-scope-1786304152340-19", "demo")
                .expect("the sibling's own alphabet")
                .as_str(),
            "node-scope-1786304152340-19"
        );
        let absent = recorded_graph_run("   ", "demo").expect_err("no address at all");
        assert!(
            absent.to_string().contains("records no agent-graph run"),
            "{absent} does not say the record named none"
        );
        for interfered in ["../elsewhere", "Node-Scope-1", "a/b"] {
            let refused = recorded_graph_run(interfered, "demo")
                .expect_err("a string the sibling would not answer to");
            assert!(
                refused.to_string().contains(interfered),
                "{refused} does not name the value it refused"
            );
        }
    }

    /// A member the run never declared is refused rather than answered with a
    /// signal file nothing will read.
    #[test]
    fn a_reset_for_a_member_the_run_never_declared_is_refused() {
        let root = state_dir_holding("node-scope-1786304152340-20", &["worker"]);
        let graph_run = recorded_graph_run("node-scope-1786304152340-20", "demo")
            .expect("the sibling accepts its own run id");
        let refused = reset_timer(&graph_run, CHECK_IN_MEMBER)
            .expect_err("a member the run does not have is not resettable");
        assert!(
            refused.to_string().contains(CHECK_IN_MEMBER),
            "{refused} does not name the member that could not be reset"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An interrupt against a run the sibling cannot find is a delivery that
    /// *failed*, and it publishes nothing.
    ///
    /// The silence is the assertion: the verb refuses an address before any
    /// event claims a lever was pulled, so a `turn-interrupted` here would put
    /// an interrupt into the merged store that never happened.
    #[test]
    fn an_interrupt_against_a_run_that_is_not_there_is_a_failed_delivery_that_publishes_nothing() {
        let root = state_dir_holding("node-scope-1786304152340-21", &["worker"]);
        let interrupt = interrupt(
            &TurnAddress::of("node-scope-1786304152340-99", "worker").expect("an address"),
            "the fixture moved",
        );
        assert!(
            matches!(&interrupt.outcome, Interrupted::Failed(reason)
                if reason.contains("node-scope-1786304152340-99")),
            "{:?} does not name the run that could not be reached",
            interrupt.outcome
        );
        assert!(
            interrupt.events.is_empty(),
            "a delivery that was never addressed published an event anyway"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A member with no controllable turn answers `NoTurn` — and still publishes
    /// the envelope, because "the lever was pulled and nothing happened" is
    /// exactly what the merged store has to carry.
    ///
    /// The envelope is the sibling's, emitted through the sibling's own emitter,
    /// so this checks the fields the contract names rather than a shape this
    /// crate composed.
    #[test]
    fn an_interrupt_with_no_turn_to_reach_still_publishes_what_the_lever_did() {
        let root = state_dir_holding("node-scope-1786304152340-22", &["worker"]);
        let interrupt = interrupt(
            &TurnAddress::of("node-scope-1786304152340-22", "worker").expect("an address"),
            "the fixture moved",
        );
        let Interrupted::NoTurn(reason) = &interrupt.outcome else {
            panic!(
                "{:?} is not the no-controllable-turn answer a member with no lever gives",
                interrupt.outcome
            );
        };
        assert!(!reason.is_empty(), "the answer carried no reason");
        let [published] = &interrupt.events[..] else {
            panic!(
                "an interrupt published {} envelopes, not the one the contract names",
                interrupt.events.len()
            );
        };
        assert_eq!(published.kind.0, "turn-interrupted");
        assert_eq!(published.payload["member"], serde_json::json!("worker"));
        assert_eq!(published.payload["delivered"], serde_json::json!(false));
        assert_eq!(
            published.payload["input_bytes"],
            serde_json::json!("the fixture moved".len())
        );
        assert_eq!(
            published.payload["reason"],
            serde_json::json!(reason),
            "the envelope's reason and the answer's are the same fact"
        );
        assert_eq!(
            published.labels.run_id.as_deref(),
            Some("node-scope-1786304152340-22"),
            "the envelope does not say which run's lever was pulled"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

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
            // Set, and deliberately not rendered: the key is retired, so a
            // value that reached this type from an older record must not be
            // sent on as a label the contract no longer names.
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
            node: Some("build".into()),
            step: Some("implement".into()),
            persona: Some("engineer".into()),
            ..Labels::default()
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
        assert_eq!(labels.node.as_deref(), Some("build"));
        assert_eq!(labels.step.as_deref(), Some("implement"));
        assert_eq!(labels.persona.as_deref(), Some("engineer"));
        assert_eq!(
            labels.extra["onepipeline.run_id"], "demo",
            "the namespaced copy is what tells the two runs apart"
        );
        // The retired key is neither read nor dropped: an older build's
        // envelopes carry it, and it stays exactly where it arrived.
        assert_eq!(labels.round, None, "a retired label was adopted");
        assert_eq!(labels.extra["onepipeline.round"], "2");
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
