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

/// How much of a command's stderr its failure's detail carries, from the end.
const STDERR_TAIL_BYTES: usize = 2048;

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

/// What `command-started` records for this node.
pub(crate) fn started(paths: &RunPaths, node: &Node) -> CommandStarted {
    CommandStarted {
        command: node.command.clone(),
        log: log_of(paths, &node.id).display().to_string(),
        timeout_seconds: node.timeout_seconds.map(std::num::NonZeroU64::get),
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
    cancel: CancellationToken,
    tx: Sender<Message>,
) -> Result<()> {
    let paths = paths.clone();
    let launch = launch.clone();
    let node = node.clone();
    let named = node.id.clone();
    std::thread::Builder::new()
        .name(format!("command-{}", node.id))
        .spawn(move || {
            let begun = Instant::now();
            let (ending, exit_code, settlement) = run(&paths, &launch, &node, &cancel);
            let _ = tx.send(Message::CommandExited(Box::new((
                node.id.clone(),
                CommandExited {
                    ending,
                    exit_code,
                    duration_ms: u64::try_from(begun.elapsed().as_millis()).unwrap_or(u64::MAX),
                },
            ))));
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

/// Run the command to its end, and say how it ended and what the node settles as.
fn run(
    paths: &RunPaths,
    launch: &LaunchRecord,
    node: &Node,
    cancel: &CancellationToken,
) -> (CommandEnding, Option<i32>, Settlement) {
    let not_started = |why: String| {
        (
            CommandEnding::NotStarted,
            None,
            Settlement {
                detail: Some(format!("the command could not be started: {why}")),
                ..Settlement::plain(&node.id, NodeStatus::Failed, Some(COMMAND_FAILED))
            },
        )
    };
    let Some((program, arguments)) = node.command.split_first() else {
        return not_started("it names no program".to_owned());
    };
    let log_path = log_of(paths, &node.id);
    let log = log_path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
        });
    let log = match log {
        Ok(log) => log,
        Err(error) => {
            return not_started(format!(
                "its log {} could not be opened: {error}",
                log_path.display()
            ))
        }
    };
    let stdout = match log.try_clone() {
        Ok(stdout) => stdout,
        Err(error) => return not_started(format!("its log could not be shared: {error}")),
    };
    let mut spawning = Process::new(program);
    spawning
        .args(arguments)
        .env(RUN_ID_ENV, &paths.run)
        .env(NODE_ID_ENV, &node.id)
        .env(RUN_ROOT_ENV, crate::hooks::run_root(paths))
        .env(PLAN_ENV, &launch.project)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped());
    // The flow and the session as the launch recorded them: a run this command starts
    // reads both to join the flow and be owned by the session that launched this one.
    if !launch.flow.is_empty() {
        spawning.env(FLOW_ENV, &launch.flow);
        if !launch.session.is_empty() {
            spawning.env(LAUNCHER_SESSION_ENV, &launch.session);
        }
    }
    // A record from before the launch directory was recorded names none, and the command
    // starts where the driver is.
    if !launch.dir.as_os_str().is_empty() {
        spawning.current_dir(&launch.dir);
    }
    let mut child = match spawning.spawn() {
        Ok(child) => child,
        Err(error) => return not_started(error.to_string()),
    };

    // Read on a thread of its own, so a command that fills the pipe cannot block on a reader
    // that is waiting for it to exit.
    let (tail_tx, tail_rx) = std::sync::mpsc::channel();
    let mut stderr = child.stderr.take();
    let mut log_for_stderr = log;
    let _ = std::thread::Builder::new()
        .name(format!("command-{}-stderr", node.id))
        .spawn(move || {
            let mut tail: Vec<u8> = Vec::new();
            let mut buffer = [0_u8; 8192];
            if let Some(stderr) = stderr.as_mut() {
                while let Ok(read) = stderr.read(&mut buffer) {
                    if read == 0 {
                        break;
                    }
                    let _ = std::io::Write::write_all(&mut log_for_stderr, &buffer[..read]);
                    tail.extend_from_slice(&buffer[..read]);
                    if tail.len() > STDERR_TAIL_BYTES {
                        tail.drain(..tail.len() - STDERR_TAIL_BYTES);
                    }
                }
            }
            let _ = tail_tx.send(tail);
        });

    let deadline = node.timeout_seconds.and_then(crate::hooks::deadline_after);
    let stopped = |child: &mut std::process::Child| {
        let _ = crate::sys::stop(child.id(), crate::sys::Stop::Now);
        let _ = child.kill();
        let _ = child.wait();
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if cancel.is_cancelled() => {
                stopped(&mut child);
                break Err(CommandEnding::Cancelled);
            }
            Ok(None) if deadline.is_some_and(|deadline| Instant::now() >= deadline) => {
                stopped(&mut child);
                break Err(CommandEnding::TimedOut);
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => {
                stopped(&mut child);
                let tail = tail_rx.recv_timeout(STDERR_GRACE).unwrap_or_default();
                return (
                    CommandEnding::Exited,
                    None,
                    Settlement {
                        detail: Some(with_tail(
                            format!("the command could not be waited for: {error}"),
                            &tail,
                        )),
                        ..Settlement::plain(&node.id, NodeStatus::Failed, Some(COMMAND_FAILED))
                    },
                );
            }
        }
    };
    let tail = tail_rx.recv_timeout(STDERR_GRACE).unwrap_or_default();
    match status {
        Ok(status) if status.success() => (
            CommandEnding::Exited,
            status.code(),
            Settlement::plain(&node.id, NodeStatus::Done, None),
        ),
        Ok(status) => (
            CommandEnding::Exited,
            status.code(),
            Settlement {
                detail: Some(with_tail(
                    match status.code() {
                        Some(code) => format!("the command exited with status {code}"),
                        None => "the command was ended by a signal".to_owned(),
                    },
                    &tail,
                )),
                ..Settlement::plain(&node.id, NodeStatus::Failed, Some(COMMAND_FAILED))
            },
        ),
        Err(CommandEnding::TimedOut) => (
            CommandEnding::TimedOut,
            None,
            Settlement {
                detail: Some(with_tail(
                    format!(
                        "the command was still running after its {}-second timeout, so its \
                         process tree was stopped",
                        node.timeout_seconds.map_or(0, std::num::NonZeroU64::get)
                    ),
                    &tail,
                )),
                ..Settlement::plain(&node.id, NodeStatus::Failed, Some(COMMAND_TIMED_OUT))
            },
        ),
        Err(ending) => (
            ending,
            None,
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

/// A detail with the command's stderr tail after it, where it said anything.
fn with_tail(detail: String, tail: &[u8]) -> String {
    let said = String::from_utf8_lossy(tail);
    let said = said.trim();
    if said.is_empty() {
        return detail;
    }
    format!("{detail}; stderr (last {STDERR_TAIL_BYTES} bytes at most):\n{said}")
}
