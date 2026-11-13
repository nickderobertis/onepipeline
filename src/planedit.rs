//! The plan-edit hook: every node a reply introduces or changes, offered — with the task
//! its next dispatch would be given — to the command the run's launch named, before anything
//! of the reply is applied or journalled as an edit.
//!
//! The hook is the engine's rather than a bus validator's because only the engine can
//! compose the task a node will be dispatched with: a validator on the channel sees an
//! envelope's bytes, and an `amend` there is a clause with no task around it. So it runs
//! where the edit is applied — in the live driver's validation of the envelope, or in the
//! replying process when nothing drives the run — and the caller hands it each node's task as
//! [`crate::composition`] composes it for dispatch, from that process's own dispatch inputs:
//! the reference block, the stack, a resumed attempt's notes and the boundary section.
//!
//! Exit `0` passes the node, `1` refuses the whole reply with the hook's stderr as the
//! reason, and anything else — another status, a signal, a command that will not start, or
//! the hook outliving its timeout, whose process tree is then ended — is *could not be
//! judged*, which refuses it too. The caller journals either refusal as `edit-rejected`
//! naming the hook and the node, and answers the sender as it answers any refused reply.

use std::io::{Read, Write};
use std::num::NonZeroU64;
use std::path::Path;
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::channel::Command;
use crate::error::{Error, Result};
use crate::filter::PlanEditHook;
use crate::graph::Graph;
use crate::ledger::{LaunchRecord, RunPaths};
use crate::plan::{Node, NodeKind};

/// The schema of the document a plan-edit hook reads on its stdin.
pub const SCHEMA_VERSION: u32 = 1;

/// How often a running hook is asked whether it has exited.
const POLL: Duration = Duration::from_millis(20);

/// The most of a hook's stderr this build reads; what a refusal carries is bounded again
/// where it becomes one.
const MAX_STDERR_BYTES: u64 = 64 * 1024;

/// How long a hook's stderr is waited for once the hook itself has ended.
const STDERR_GRACE: Duration = Duration::from_secs(2);

/// The keys of a `requeue`'s amendment that change what a node is judged against, and so
/// offer it to the hook; a requeue stating none of them changes nothing it reads.
const REQUEUE_KEYS: [&str; 3] = ["task", "steps", "kind"];

/// The node one command introduces or changes in what it is judged against, read from the
/// graph the whole reply leaves behind, or `None` for a command that changes no such node.
///
/// `add` and `retry` introduce one, `amend` moves the bar of one, and a `requeue` is one
/// only where its amendment states `task`, `steps` or `kind`. A `note` — and every op that
/// moves edges, parks, attests or reports — offers nothing.
fn offered<'a>(command: &Command, graph: &'a Graph) -> Option<&'a Node> {
    let id = match command {
        Command::Add { node } | Command::Retry { node, .. } => node.id.as_str(),
        Command::Amend { id, .. } => id.as_str(),
        Command::Requeue { id, amend } => {
            amend
                .as_ref()
                .filter(|amend| REQUEUE_KEYS.iter().any(|key| amend.contains_key(*key)))?;
            id.as_str()
        }
        _ => return None,
    };
    graph.get(id)
}

/// One node, as the hook reads it.
#[derive(Debug, Serialize)]
struct Offered<'a> {
    id: &'a str,
    kind: NodeKind,
    persona: Option<&'a str>,
    agent_graph: Option<&'a str>,
    repo: Option<&'a str>,
    deps: &'a [String],
}

/// The task a node will be given, as the hook reads it.
#[derive(Debug, Serialize)]
struct Task {
    title: String,
    body: String,
}

/// The one document a hook reads on its stdin for one node.
// llmlint: ignore-block[invalid_states_unrepresentable] `op` is `channel::op_of`'s own word
// for the command, produced rather than accepted, exactly as the envelope reviewer's
// `ChangedNode::op` is; an enum here would be a second vocabulary for the same four words.
#[derive(Debug, Serialize)]
struct Document<'a> {
    schema_version: u32,
    run: &'a str,
    plan: &'a str,
    op: &'static str,
    node: Offered<'a>,
    task: Task,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl<'a> Document<'a> {
    fn of(run: &'a str, plan: &'a str, op: &'static str, node: &'a Node, body: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            run,
            plan,
            op,
            node: Offered {
                id: &node.id,
                kind: node.kind,
                persona: node.persona.as_deref(),
                agent_graph: node.agent_graph.as_ref().map(|graph| graph.0.as_str()),
                repo: node.repo.as_deref(),
                deps: &node.deps,
            },
            task: Task {
                title: node
                    .title
                    .clone()
                    .or_else(|| node.task_record.as_ref().map(|record| record.title.clone()))
                    .unwrap_or_default(),
                body,
            },
        }
    }
}

/// What a hook said about one node.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// Exit 0.
    Pass,
    /// Exit 1, with what it said on stderr.
    Refuse(String),
    /// Anything else, and why.
    Unjudged(String),
}

/// Offer every node a reply's commands introduce or change to the plan-edit hook the run's
/// launch named, once per node, in the order the commands name them — and refuse the whole
/// reply at the first node the hook refuses or could not judge.
///
/// `edited` is the graph the whole reply leaves behind, and `compose` answers the task a
/// node of it would be given by its next dispatch — the caller's, because only the process
/// applying the edit holds what that dispatch is composed from. A launch naming no hook runs
/// nothing and composes nothing.
///
/// # Errors
///
/// The refusal, as [`Error::Refused`] naming the hook and the node, beside the position of the
/// command that named that node; the caller journals it there.
pub fn judge(
    paths: &RunPaths,
    launch: &LaunchRecord,
    commands: &[Command],
    edited: &Graph,
    compose: &mut dyn FnMut(&Node) -> Result<String>,
) -> std::result::Result<(), (usize, Error)> {
    let Some(hook) = launch.plan_edit_hook.as_ref() else {
        return Ok(());
    };
    let mut seen = std::collections::BTreeSet::new();
    for (index, command) in commands.iter().enumerate() {
        let Some(node) = offered(command, edited) else {
            continue;
        };
        if !seen.insert(node.id.clone()) {
            continue;
        }
        let op = crate::channel::op_of(command);
        let body = compose(node).map_err(|error| (index, error))?;
        let document =
            serde_json::to_vec(&Document::of(&paths.run, &launch.project, op, node, body))
                .map_err(|error| {
                    (
                        index,
                        Error::Invalid(format!("node '{}' does not serialize: {error}", node.id)),
                    )
                })?;
        let named = hook.command().spelled();
        let refusal = match run(hook, &launch.dir, &document) {
            Verdict::Pass => continue,
            Verdict::Refuse(said) => format!(
                "{op}: the plan-edit hook `{named}` refused node '{}', so none of this reply's \
                 edits were applied: {}",
                node.id,
                words(&said, "it refused and said nothing on stderr")
            ),
            Verdict::Unjudged(why) => format!(
                "{op}: the plan-edit hook `{named}` could not judge node '{}' — {why} — so it \
                 was not judged and none of this reply's edits were applied",
                node.id
            ),
        };
        return Err((index, Error::Refused(refusal)));
    }
    Ok(())
}

/// What a hook said, as a refusal carries it: bounded, on one line, and never blank.
fn words(said: &str, otherwise: &str) -> String {
    let mut end = said.len().min(crate::event::MAX_PAYLOAD_TEXT_BYTES);
    while !said.is_char_boundary(end) {
        end -= 1;
    }
    let said = crate::views::one_line(&said[..end]).trim().to_owned();
    if said.is_empty() {
        otherwise.to_owned()
    } else {
        said
    }
}

/// Run the hook over one document: from the launch directory, the document on its stdin,
/// its stderr kept, and its process tree ended when it outlives its timeout.
fn run(hook: &PlanEditHook, dir: &Path, document: &[u8]) -> Verdict {
    let mut spawning = Process::new(hook.command().program_from(dir));
    spawning
        .args(hook.arguments())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // A record from before the launch directory was recorded names none, and the hook
    // starts where this process is.
    if !dir.as_os_str().is_empty() {
        spawning.current_dir(dir);
    }
    // A process the hook leaves behind must not hold this one's own streams open behind it:
    // the sender of a reply reads them to their end. See [`crate::sys::disown_standard_handles`].
    crate::sys::disown_standard_handles();
    let mut child = match spawning.spawn() {
        Ok(child) => child,
        Err(error) => return Verdict::Unjudged(format!("it could not be started: {error}")),
    };
    // On a thread, so a hook that reads nothing cannot block this process on a full pipe.
    let mut stdin = child.stdin.take();
    let owned = document.to_vec();
    let _ = std::thread::Builder::new()
        .name("plan-edit-hook-stdin".to_owned())
        .spawn(move || {
            if let Some(stdin) = stdin.as_mut() {
                // A hook that judged without reading all of it is answering, not failing.
                let _ = stdin.write_all(&owned);
            }
        });
    // Into a buffer this thread shares, so what the hook said is still read when a process it
    // left behind holds the pipe open past the grace this side waits.
    let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let (drained_tx, drained) = std::sync::mpsc::channel::<()>();
    let mut stderr = child.stderr.take();
    let kept = std::sync::Arc::clone(&said);
    let _ = std::thread::Builder::new()
        .name("plan-edit-hook-stderr".to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            if let Some(stderr) = stderr.as_mut() {
                while let Ok(read) = stderr.read(&mut buffer) {
                    if read == 0 {
                        break;
                    }
                    let mut said = kept
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let room = usize::try_from(MAX_STDERR_BYTES)
                        .unwrap_or(usize::MAX)
                        .saturating_sub(said.len());
                    said.extend_from_slice(&buffer[..read.min(room)]);
                }
            }
            let _ = drained_tx.send(());
        });
    let timeout: NonZeroU64 = hook.timeout_seconds();
    let deadline = crate::hooks::deadline_after(timeout);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if deadline.is_none_or(|deadline| Instant::now() < deadline) => {
                std::thread::sleep(POLL);
            }
            waited => {
                let _ = crate::sys::stop(child.id(), crate::sys::Stop::Now);
                let _ = child.kill();
                let _ = child.wait();
                return Verdict::Unjudged(match waited {
                    Ok(_) => format!(
                        "it was still running after its {timeout}-second timeout, so its \
                         process tree was ended"
                    ),
                    Err(error) => format!("it could not be waited for: {error}"),
                });
            }
        }
    };
    let _ = drained.recv_timeout(STDERR_GRACE);
    let said = String::from_utf8_lossy(
        &said
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_owned();
    match status.code() {
        Some(0) => Verdict::Pass,
        Some(1) => Verdict::Refuse(said),
        Some(code) => Verdict::Unjudged(format!(
            "it exited {code}{}",
            match words(&said, "") {
                said if said.is_empty() => String::new(),
                said => format!(" saying: {said}"),
            }
        )),
        None => Verdict::Unjudged("it was ended by a signal before it answered".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, task: &str) -> Node {
        Node {
            id: id.into(),
            persona: Some("engineer".into()),
            task: Some(task.into()),
            ..Node::default()
        }
    }

    fn graph_of(nodes: Vec<Node>) -> Graph {
        let mut graph = Graph::default();
        for node in nodes {
            graph.insert(node);
        }
        graph
    }

    #[test]
    fn only_the_ops_that_change_what_a_node_is_judged_against_offer_it() {
        let graph = graph_of(vec![node("build", "## Acceptance criteria\n- It builds.")]);
        let requeue = |keys: &[&str]| Command::Requeue {
            id: "build".into(),
            amend: Some(
                keys.iter()
                    .map(|key| ((*key).to_owned(), serde_json::json!("x")))
                    .collect(),
            ),
        };
        for key in REQUEUE_KEYS {
            assert!(offered(&requeue(&[key]), &graph).is_some(), "{key}");
        }
        assert!(offered(&requeue(&["max_turns"]), &graph).is_none());
        assert!(offered(
            &Command::Requeue {
                id: "build".into(),
                amend: None
            },
            &graph
        )
        .is_none());
    }
}
