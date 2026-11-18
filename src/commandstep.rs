//! `kind: command` nodes: a step of a plan that runs a command and settles on its exit
//! status.
//!
//! The one node kind besides an agent's and a person's. When it is eligible the engine runs
//! its `command` argv — no shell — from the launch directory, with the driver's environment
//! and the run's own coordinates on top: `ONEPIPELINE_RUN_ID`, `ONEPIPELINE_NODE_ID`,
//! `ONEPIPELINE_RUN_ROOT` and `ONEPIPELINE_PLAN`, and — for a run that belongs to a flow —
//! `ONEPIPELINE_FLOW` and `ONEPIPELINE_LAUNCHER_SESSION` as the launch recorded them, so a
//! run the command itself starts joins the same flow and is owned by the same session. Its
//! stdout and stderr are kept in `<run>/commands/<node>.log`.
//!
//! It is also told `ONEPIPELINE_STEP_OUTPUTS`, a path no earlier run of the command used, and
//! may write a JSON object whose values are strings there: what it publishes for a human node
//! after it to quote (`human_items::quote`). On exit 0 the engine records that object as the
//! `outputs` of its `command-exited`, and a file that is not such an object fails the step,
//! naming what it holds instead. An exit that is not 0 has failed already, and reads none.
//!
//! It settles `done` on exit 0, `failed` with outcome [`COMMAND_FAILED`] on any other exit —
//! the detail its exit status and a bounded tail of its stderr — and `failed` with
//! [`COMMAND_TIMED_OUT`] when it outlives its `timeout_seconds`, its process tree stopped.
//! Cancelled while it runs, its process tree is stopped and it settles `cancelled`. It never
//! dispatches an agent and opens no session. A driver that dies while it runs takes it
//! along, and the adopting driver runs it again from the start — a command must be safe to
//! repeat.
//!
//! The command runs on a thread of its own, as a dispatch does, and hands the loop — the
//! run's single writer — how it ended and its settlement.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command as Process, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::engine::{Message, Settled, Settlement};
use crate::error::{Error, Result};
use crate::executor::CancellationToken;
use crate::filter::Argv;
use crate::graph::NodeStatus;
use crate::ledger::{LaunchRecord, RunPaths};
use crate::payload::{CommandEnding, CommandExited, CommandStarted};
use crate::plan::Node;

/// The outcome a command that exited non-zero settles under.
pub(crate) const COMMAND_FAILED: &str = "command-failed";

/// The outcome a command that outlived its `timeout_seconds` settles under.
pub(crate) const COMMAND_TIMED_OUT: &str = "command-timed-out";

/// The variables a command is told the run's coordinates in.
pub(crate) const RUN_ID_ENV: &str = "ONEPIPELINE_RUN_ID";
/// The node the command runs for.
pub(crate) const NODE_ID_ENV: &str = "ONEPIPELINE_NODE_ID";
/// The run's own directory, absolute.
pub(crate) const RUN_ROOT_ENV: &str = "ONEPIPELINE_RUN_ROOT";
/// The run's qualified project, `<source>:<project>`.
pub(crate) const PLAN_ENV: &str = "ONEPIPELINE_PLAN";
/// The flow the run belongs to, where it belongs to one.
pub(crate) const FLOW_ENV: &str = "ONEPIPELINE_FLOW";
/// The session that launched the run, where the launch recorded one.
pub(crate) const LAUNCHER_SESSION_ENV: &str = "ONEPIPELINE_LAUNCHER_SESSION";
/// The file the command may publish its outputs to.
pub(crate) const STEP_OUTPUTS_ENV: &str = "ONEPIPELINE_STEP_OUTPUTS";

/// The most a step's outputs file may hold: each value is copied into the run's journal,
/// so one step cannot grow every later read of it without bound.
const MAX_OUTPUTS_BYTES: u64 = 64 * 1024;

/// How much of a command's stderr its failure's detail carries, from the end.
pub(crate) const STDERR_TAIL_BYTES: usize = 2048;

/// How often a running command is asked whether it has ended.
const POLL: Duration = Duration::from_millis(50);

/// How long a command's stderr is waited for once the command itself has ended: a
/// background process it left holding the pipe is not waited on past this.
const STDERR_GRACE: Duration = Duration::from_secs(2);

/// Where a command node's output is kept.
pub(crate) fn log_of(paths: &RunPaths, node: &str) -> PathBuf {
    crate::hooks::run_root(paths)
        .join("commands")
        .join(format!("{node}.log"))
}

/// Where this run of a command node's command may publish its outputs: a name no earlier
/// run used, so a file an earlier attempt left is never read as this one's.
fn outputs_of(paths: &RunPaths, node: &str) -> PathBuf {
    crate::hooks::run_root(paths).join("commands").join(format!(
        "{node}.outputs.{}.{}.json",
        std::process::id(),
        crate::sys::now_millis()
    ))
}

/// What `command-started` records for this node, running `argv`.
pub(crate) fn started(paths: &RunPaths, node: &Node, argv: &Argv) -> CommandStarted {
    CommandStarted {
        command: argv.clone(),
        log: log_of(paths, &node.id).display().to_string(),
        timeout_seconds: node.timeout_seconds,
    }
}

/// Run one command node's command on a thread of its own, and hand the loop how it
/// ended and the settlement that follows.
///
/// # Errors
///
/// A thread this host would not start; the node is then left for the loop to settle.
pub(crate) fn spawn(
    paths: &RunPaths,
    launch: &LaunchRecord,
    node: &Node,
    argv: &Argv,
    cancel: CancellationToken,
    tx: Sender<Message>,
) -> Result<()> {
    let paths = paths.clone();
    let launch = launch.clone();
    let node = node.clone();
    let argv = argv.clone();
    let named = node.id.clone();
    std::thread::Builder::new()
        .name(format!("command-{}", node.id))
        .spawn(move || {
            let begun = Instant::now();
            let (ending, settlement, outputs) = run(&paths, &launch, &node, &argv, &cancel);
            // The node the loop started, so it has an identity; a node without one is a
            // node the loader refused, and nothing is sent for it but its settlement.
            if let Some(named) = crate::graph::NodeRef::of(&node) {
                let _ = tx.send(Message::CommandExited(Box::new((
                    named,
                    CommandExited {
                        ending,
                        duration_ms: u64::try_from(begun.elapsed().as_millis()).unwrap_or(u64::MAX),
                        outputs,
                    },
                ))));
            }
            let (recorded, _) = std::sync::mpsc::channel();
            let _ = tx.send(Message::Settled(Box::new(Settled {
                settlement,
                recorded,
            })));
        })
        .map(|_| ())
        .map_err(|error| {
            Error::Invalid(format!(
                "node '{named}': no thread could be started to run its command: {error}"
            ))
        })
}

/// Run the command to its end, and say how it ended, what the node settles as, and what it
/// published.
fn run(
    paths: &RunPaths,
    launch: &LaunchRecord,
    node: &Node,
    argv: &Argv,
    cancel: &CancellationToken,
) -> (CommandEnding, Settlement, Option<Outputs>) {
    let outputs_path = outputs_of(paths, &node.id);
    let (ending, settlement) = run_to_end(paths, launch, node, argv, cancel, &outputs_path);
    if settlement.status != NodeStatus::Done {
        let _ = std::fs::remove_file(&outputs_path);
        return (ending, settlement, None);
    }
    let read = read_outputs(&outputs_path);
    let _ = std::fs::remove_file(&outputs_path);
    match read {
        Ok(outputs) => (ending, settlement, outputs),
        Err(why) => (
            ending,
            Settlement {
                detail: Some(format!(
                    "the command exited 0, and the file {STEP_OUTPUTS_ENV} named ({}) {why}",
                    outputs_path.display()
                )),
                ..Settlement::plain(&node.id, NodeStatus::Failed, Some(COMMAND_FAILED))
            },
            None,
        ),
    }
}

/// What a step published: a value for each name.
type Outputs = std::collections::BTreeMap<String, String>;

/// The outputs a command wrote to `path`: none where it wrote no file there.
///
/// # Errors
///
/// What the file holds instead of a JSON object whose values are strings, in words that
/// follow "the file … ".
fn read_outputs(path: &std::path::Path) -> std::result::Result<Option<Outputs>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not be read: {error}")),
    };
    // Read through the bound rather than checked beside it, so a process the command left
    // behind cannot grow the file between a size check and the read.
    let mut text = Vec::new();
    file.take(MAX_OUTPUTS_BYTES + 1)
        .read_to_end(&mut text)
        .map_err(|error| format!("could not be read: {error}"))?;
    if text.len() as u64 > MAX_OUTPUTS_BYTES {
        return Err(format!(
            "holds more than the {MAX_OUTPUTS_BYTES} bytes a step's outputs may"
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&text)
        .map_err(|error| format!("is not JSON ({error}); it must hold an object of strings"))?;
    let serde_json::Value::Object(object) = value else {
        return Err(format!(
            "holds a JSON {}, not an object whose values are strings",
            shape(&value)
        ));
    };
    object
        .into_iter()
        .map(|(name, value)| match value {
            serde_json::Value::String(text) => Ok((name, text)),
            other => Err(format!(
                "holds an object whose value for {name:?} is a JSON {}, not a string",
                shape(&other)
            )),
        })
        .collect::<std::result::Result<Outputs, String>>()
        .map(Some)
}

/// A JSON value's kind, as a sentence names it.
fn shape(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "list",
        serde_json::Value::Object(_) => "object",
    }
}

/// Run the command to its end, and say how it ended and what the node settles as.
fn run_to_end(
    paths: &RunPaths,
    launch: &LaunchRecord,
    node: &Node,
    argv: &Argv,
    cancel: &CancellationToken,
    outputs: &std::path::Path,
) -> (CommandEnding, Settlement) {
    let ran = run_program(
        paths,
        launch,
        &node.id,
        argv,
        &log_of(paths, &node.id),
        node.timeout_seconds,
        cancel,
        Some(outputs),
    );
    let failed = |detail: String| Settlement {
        detail: Some(detail),
        ..Settlement::plain(&node.id, NodeStatus::Failed, Some(COMMAND_FAILED))
    };
    match ran.ended {
        Ended::NotStarted(why) => (
            CommandEnding::NotStarted,
            failed(format!("the command could not be started: {why}")),
        ),
        Ended::Unwaitable(why) => (
            CommandEnding::Exited { exit_code: None },
            failed(with_tail(
                format!("the command could not be waited for: {why}"),
                &ran.tail,
            )),
        ),
        Ended::Exited { exit_code: Some(0) } => (
            CommandEnding::Exited { exit_code: Some(0) },
            Settlement::plain(&node.id, NodeStatus::Done, None),
        ),
        Ended::Exited { exit_code } => (
            CommandEnding::Exited { exit_code },
            failed(with_tail(
                match exit_code {
                    Some(code) => format!("the command exited with status {code}"),
                    None => "the command was ended by a signal".to_owned(),
                },
                &ran.tail,
            )),
        ),
        Ended::TimedOut => (
            CommandEnding::TimedOut,
            Settlement {
                detail: Some(with_tail(
                    format!(
                        "the command was still running after its {}-second timeout, so its \
                         process tree was stopped",
                        node.timeout_seconds.map_or(0, std::num::NonZeroU64::get)
                    ),
                    &ran.tail,
                )),
                ..Settlement::plain(&node.id, NodeStatus::Failed, Some(COMMAND_TIMED_OUT))
            },
        ),
        Ended::Cancelled => (
            CommandEnding::Cancelled,
            Settlement {
                detail: Some(
                    "the node was cancelled while its command ran, so its process tree was \
                     stopped"
                        .to_owned(),
                ),
                ..Settlement::plain(&node.id, NodeStatus::Cancelled, None)
            },
        ),
    }
}

/// How a program the engine ran for a node — a command node's command, or a guard — ended.
pub(crate) enum Ended {
    /// It exited, with its status where the host reported one: zero is success, and none
    /// is a signal ending it.
    Exited {
        /// Its exit status; absent where a signal ended it.
        exit_code: Option<i32>,
    },
    /// It outlived its deadline, and its process tree was stopped.
    TimedOut,
    /// Its node was cancelled while it ran, and its process tree was stopped.
    Cancelled,
    /// It could not be started at all, and why.
    NotStarted(String),
    /// It started and could not be waited for, and why; its process tree was stopped.
    Unwaitable(String),
}

/// What [`run_program`] answers: how the program ended, and the end of what it said on
/// stderr, at most [`STDERR_TAIL_BYTES`].
pub(crate) struct Ran {
    /// How it ended.
    pub(crate) ended: Ended,
    /// The last of its stderr, as far as it was read.
    pub(crate) tail: Vec<u8>,
}

/// Run `argv` for node `node` to its end — no shell, from the launch directory, with the
/// driver's environment and the run's coordinates on top — appending its stdout and stderr
/// to `log_path`, stopping its process tree at `timeout_seconds` or when `cancel` is
/// cancelled, and answer how it ended with the end of its stderr. `outputs` is the file a
/// command step is told it may publish to, and `None` for a guard, which publishes nothing.
///
/// The one runner a command node's command and a guard share, so the two are started,
/// told the run's coordinates, logged, bounded and stopped alike.
pub(crate) fn run_program(
    paths: &RunPaths,
    launch: &LaunchRecord,
    node: &str,
    argv: &Argv,
    log_path: &std::path::Path,
    timeout_seconds: Option<std::num::NonZeroU64>,
    cancel: &CancellationToken,
    outputs: Option<&std::path::Path>,
) -> Ran {
    let not_started = |why: String| Ran {
        ended: Ended::NotStarted(why),
        tail: Vec::new(),
    };
    let log = log_path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            let mut options = std::fs::OpenOptions::new();
            options.create(true);
            // Windows grants an append-only handle no `FILE_WRITE_DATA`, and a program of the
            // Cygwin family — git for Windows' `bash` among them — refuses every write to such
            // a stdout as a bad descriptor. So there it is opened for writing and placed at its
            // end, which keeps what an earlier run of the program wrote, as appending does.
            #[cfg(windows)]
            options.write(true);
            #[cfg(not(windows))]
            options.append(true);
            let mut log = options.open(log_path)?;
            std::io::Seek::seek(&mut log, std::io::SeekFrom::End(0))?;
            Ok(log)
        });
    let log = match log {
        Ok(log) => log,
        // llmlint: ignore-block[changed_behavior_has_e2e] a run directory the driver can write
        // its journal into and not a log beside it is a host condition — a full disk, a
        // permission changed under a live run — that no verb or plan produces; staging it means
        // editing the run's own layout, which proves the fixture. What it must do is settle the
        // node `not-started` as an unstartable program does, which `command_step.rs`'s endings
        // journey and `guard.rs`'s failures journey drive.
        Err(error) => {
            return not_started(format!(
                "its log {} could not be opened: {error}",
                log_path.display()
            ))
        } // llmlint: ignore-end[changed_behavior_has_e2e]
    };
    let stdout = match log.try_clone() {
        Ok(stdout) => stdout,
        Err(error) => return not_started(format!("its log could not be shared: {error}")),
    };
    let mut spawning = Process::new(argv.program_from(&launch.dir));
    spawning
        .args(argv.arguments())
        .env(RUN_ID_ENV, &paths.run)
        .env(NODE_ID_ENV, node)
        .env(RUN_ROOT_ENV, crate::hooks::run_root(paths))
        .env(PLAN_ENV, &launch.project)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped());
    if let Some(outputs) = outputs {
        spawning.env(STEP_OUTPUTS_ENV, outputs);
    }
    // The flow and the session as the launch recorded them: a run this program starts
    // reads both to join the flow and be owned by the session that launched this one.
    if !launch.flow.is_empty() {
        spawning.env(FLOW_ENV, &launch.flow);
        if !launch.session.is_empty() {
            spawning.env(LAUNCHER_SESSION_ENV, &launch.session);
        }
    }
    // A record from before the launch directory was recorded names none, and the program
    // starts where the driver is.
    if !launch.dir.as_os_str().is_empty() {
        spawning.current_dir(&launch.dir);
    }
    // A process the program leaves behind must not hold this one's own streams open behind
    // it: an attached launch's caller reads them to their end. The engine's own spawns name
    // every stream they hand on, so nothing here is meant to inherit them.
    crate::sys::disown_standard_handles();
    let mut child = match spawning.spawn() {
        Ok(child) => child,
        Err(error) => return not_started(error.to_string()),
    };

    // Read on a thread of its own, so a program that fills the pipe cannot block on a reader
    // that is waiting for it to exit — and into a buffer this thread shares, so what it has
    // read is still there when a process the program left behind holds the pipe open past
    // the grace this side waits.
    let tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let (drained_tx, drained) = std::sync::mpsc::channel::<()>();
    let mut stderr = child.stderr.take();
    let mut log_for_stderr = log;
    let kept = std::sync::Arc::clone(&tail);
    let _ = std::thread::Builder::new()
        .name(format!("program-{node}-stderr"))
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            if let Some(stderr) = stderr.as_mut() {
                while let Ok(read) = stderr.read(&mut buffer) {
                    if read == 0 {
                        break;
                    }
                    let _ = std::io::Write::write_all(&mut log_for_stderr, &buffer[..read]);
                    let mut tail = kept
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    tail.extend_from_slice(&buffer[..read]);
                    if tail.len() > STDERR_TAIL_BYTES {
                        let over = tail.len() - STDERR_TAIL_BYTES;
                        tail.drain(..over);
                    }
                }
            }
            let _ = drained_tx.send(());
        });
    let said = || {
        let _ = drained.recv_timeout(STDERR_GRACE);
        tail.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    };

    let deadline = timeout_seconds.and_then(crate::hooks::deadline_after);
    let stopped = |child: &mut std::process::Child| {
        let _ = crate::sys::stop(child.id(), crate::sys::Stop::Now);
        let _ = child.kill();
        let _ = child.wait();
    };
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break Ended::Exited {
                    exit_code: status.code(),
                }
            }
            Ok(None) if cancel.is_cancelled() => {
                stopped(&mut child);
                break Ended::Cancelled;
            }
            Ok(None) if deadline.is_some_and(|deadline| Instant::now() >= deadline) => {
                stopped(&mut child);
                break Ended::TimedOut;
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => {
                stopped(&mut child);
                break Ended::Unwaitable(error.to_string());
            }
        }
    };
    Ran {
        ended,
        tail: said(),
    }
}

/// A detail with the program's stderr tail after it, where it said anything.
pub(crate) fn with_tail(detail: String, tail: &[u8]) -> String {
    let said = String::from_utf8_lossy(tail);
    let said = said.trim();
    if said.is_empty() {
        return detail;
    }
    format!("{detail}; stderr (last {STDERR_TAIL_BYTES} bytes at most):\n{said}")
}
