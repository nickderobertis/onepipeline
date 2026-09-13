//! The two commands a run fires once when it **ends**: a success hook when every
//! node settled `done`, and a failure hook when it ended any other way.
//!
//! `docs/contract.md`'s run-end hooks paragraph is the whole rule. What this file
//! adds is where each half of it lives: [`judge`] is the rule over a folded run,
//! [`at_let_go`] and [`at_stop`] are the only two moments it is asked, and `fire`
//! is the once-per-run marker, the spawn, the wait and the record of how the hook
//! ended. Nothing here writes a node status, a result or a settlement, which is
//! what keeps a hook from changing any of them.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::cli::DEFAULT_HOOK_TIMEOUT_SECONDS;
use crate::error::Result;
use crate::graph::NodeStatus;
use crate::journal::{self, Journal, PipelineKind};
use crate::ledger::{self, LaunchRecord, RunPaths};
use crate::projection::RunState;
use crate::sys;
use crate::views::{self, RunView};

/// The version of the document a hook reads on its stdin.
const DOCUMENT_VERSION: u32 = 1;

/// How many of a hook's last lines of output `results` repeats.
const RESULTS_OUTPUT_LINES: usize = 20;

/// The most of a hook's log `results` reads to find those lines.
///
/// A hook's output is external and unbounded, and a view reads its tail: what a
/// reader is shown is twenty lines, so what it reads for them is bounded too.
const MAX_TAIL_BYTES: u64 = 64 * 1024;

/// How often a wait on a hook looks again, and relays what it said since.
const POLL: Duration = Duration::from_millis(50);

/// The environment variable naming which hook is running.
const HOOK_ENV: &str = "ONEPIPELINE_HOOK";

/// The environment variable naming the run a hook fired for.
const RUN_ID_ENV: &str = "ONEPIPELINE_RUN_ID";

/// The environment variable naming that run's own directory, absolute.
const RUN_ROOT_ENV: &str = "ONEPIPELINE_RUN_ROOT";

/// The settlement a driver lets go at when the run is paused rather than ended.
///
/// The word `start` and `adopt` print on their settlement line for the same
/// state, which is what a withheld hook's record carries so the two read alike.
const PAUSED: &str = "awaiting-planner";

/// One of the two run-end hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Hook {
    /// Every node settled `done`.
    Success,
    /// The run ended any other way.
    Failure,
}

impl Hook {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }

    /// The hook a record of this crate's own names, when it names one.
    fn parse(word: &str) -> Option<Self> {
        [Self::Success, Self::Failure]
            .into_iter()
            .find(|hook| hook.as_str() == word)
    }

    /// The command a launch record names for this hook.
    fn command(self, record: &LaunchRecord) -> Option<&str> {
        match self {
            Self::Success => record.success_hook(),
            Self::Failure => record.failure_hook(),
        }
    }
}

impl std::fmt::Display for Hook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why the failure hook fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReasonKind {
    /// The graph holds a `failed` or `skipped` node.
    Nodes,
    /// The graph holds a node that is not `done`, none failed or skipped, and no
    /// decision is outstanding.
    Unfinished,
    /// `stop` established a clean teardown.
    Stopped,
}

/// One node that was not `done` when a hook was judged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Unsettled {
    id: String,
    status: &'static str,
    /// Written as `null` rather than omitted: the document states the field.
    outcome: Option<String>,
}

/// Why the failure hook fired, and every node not `done` when it was judged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Reason {
    kind: ReasonKind,
    nodes: Vec<Unsettled>,
}

/// The one document a hook reads on its stdin.
#[derive(Serialize)]
struct Document<'a> {
    version: u32,
    hook: Hook,
    run_id: &'a str,
    run_root: String,
    reason: Option<&'a Reason>,
}

/// How a hook ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Ending {
    Succeeded,
    Failed,
    CouldNotStart,
    TimedOut,
}

/// What a driver letting go of a run owes it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Judged {
    /// The run has ended, and this hook is the one it ended under.
    Fire(Hook, Option<Reason>),
    /// A decision is outstanding: the run is paused, not ended.
    Withhold,
    /// The run has not ended and is not paused on a decision either — a
    /// `complete-but-draft` node is waiting on a release.
    NotEnded,
}

/// Whether an attached driver repeats a hook's output on its own stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Relay {
    /// An attached driver: somebody is watching this process's stderr.
    Stderr,
    /// A detached driver, or a verb that is not a driver: the log is the record.
    Quiet,
}

/// Judge a run a driver has let go of, on the graph as it now stands.
///
/// In the contract's order, which is also the settlement line's: a complete graph
/// is success, an outstanding decision is a pause, a draft waiting on a release is
/// a run still going, and anything else not `done` is a failure — `nodes` where
/// something failed or was skipped, `unfinished` otherwise.
pub(crate) fn judge(state: &RunState, paths: &RunPaths) -> Judged {
    let statuses = state.statuses();
    // A graph with no nodes has not started, which is not an ending.
    if statuses.is_empty() {
        return Judged::NotEnded;
    }
    if statuses.values().all(|status| *status == NodeStatus::Done) {
        return Judged::Fire(Hook::Success, None);
    }
    if views::decision_outstanding(state, paths) {
        return Judged::Withhold;
    }
    if statuses
        .values()
        .any(|status| *status == NodeStatus::CompleteDraft)
    {
        return Judged::NotEnded;
    }
    let kind = if statuses
        .values()
        .any(|status| matches!(status, NodeStatus::Failed | NodeStatus::Skipped))
    {
        ReasonKind::Nodes
    } else {
        ReasonKind::Unfinished
    };
    Judged::Fire(
        Hook::Failure,
        Some(Reason {
            kind,
            nodes: unsettled(state, &statuses),
        }),
    )
}

/// Every node not `done`, in plan order, as a hook is handed it.
fn unsettled(state: &RunState, statuses: &BTreeMap<String, NodeStatus>) -> Vec<Unsettled> {
    state
        .graph
        .iter()
        .filter_map(|node| {
            let status = statuses
                .get(&node.id)
                .copied()
                .unwrap_or(NodeStatus::Pending);
            (status != NodeStatus::Done).then(|| Unsettled {
                id: node.id.clone(),
                status: status.as_str(),
                outcome: state.outcomes.get(&node.id).cloned(),
            })
        })
        .collect()
}

/// What a driver owes the run it has just let go of: the hook the run ended
/// under, a withheld record for a run paused on a decision, or nothing.
///
/// Called only once the ownership lock is released, so the hook runs while the
/// run reads as free — and never by a driver that left the run claimed.
pub(crate) fn at_let_go(paths: &RunPaths, relay: Relay) {
    let Some(view) = judged_view(paths) else {
        return;
    };
    match judge(&view.state, paths) {
        Judged::Fire(hook, reason) => fire(paths, &view.launch, hook, reason.as_ref(), relay),
        Judged::Withhold => withhold(paths),
        Judged::NotEnded => {}
    }
}

/// What a clean `stop` owes the run it stopped: the failure hook, as `stopped`.
///
/// The stop verb's own, after it journals `run-stopped`. It is not the run's
/// driver — the driver is what it ended — so it journals the way the stop itself
/// did, from outside the lock.
pub(crate) fn at_stop(paths: &RunPaths) {
    let Some(view) = judged_view(paths) else {
        return;
    };
    let statuses = view.state.statuses();
    let reason = Reason {
        kind: ReasonKind::Stopped,
        nodes: unsettled(&view.state, &statuses),
    };
    fire(
        paths,
        &view.launch,
        Hook::Failure,
        Some(&reason),
        Relay::Quiet,
    );
}

/// The run, read once, where its record names a hook at all.
///
/// A run whose record names none is a launch that did exactly what launches did
/// before hooks existed, so nothing is read, judged or journaled for it.
fn judged_view(paths: &RunPaths) -> Option<RunView> {
    let record: LaunchRecord = ledger::read_json_opt(&paths.launch())?;
    if record.success_hook().is_none() && record.failure_hook().is_none() {
        return None;
    }
    match RunView::open(paths) {
        Ok(view) => Some(view),
        Err(error) => {
            eprintln!(
                "onepipeline: whether run '{}' fires a run-end hook could not be judged: {error}",
                paths.run
            );
            None
        }
    }
}

/// Record that a driver let go of a paused run, and say so.
fn withhold(paths: &RunPaths) {
    // A run that has already fired its hook is past every judgement, a pause
    // included.
    if fired(paths) {
        return;
    }
    eprintln!(
        "onepipeline: run '{}' is paused on a decision ({PAUSED}), so no run-end hook fired; \
         the driver that adopts it once the decision is answered fires the hook the run then \
         reaches",
        paths.run
    );
    if let Err(error) = Journal::open(paths).emit(
        PipelineKind::RunHookWithheld,
        journal::labels(&paths.run, None),
        journal::payload(&[("settlement", json!(PAUSED))]),
    ) {
        eprintln!(
            "onepipeline: the withheld run-end hook of run '{}' could not be recorded: {error}",
            paths.run
        );
    }
}

/// Fire one hook, once per run: mark it, run it, and record how it ended.
fn fire(
    paths: &RunPaths,
    record: &LaunchRecord,
    hook: Hook,
    reason: Option<&Reason>,
    relay: Relay,
) {
    // An ending whose hook the record does not name fires nothing, and marks
    // nothing — so a hook it does name is still reachable later.
    let Some(command) = hook.command(record) else {
        return;
    };
    match mark(paths, hook, command, reason) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            eprintln!(
                "onepipeline: the {hook} hook of run '{}' was not fired, because its firing \
                 could not be recorded: {error}",
                paths.run
            );
            return;
        }
    }
    let log = log_path(paths, hook);
    let ran = run(paths, record, hook, command, reason, &log, relay);
    if let Err(error) = Journal::open(paths).emit(
        PipelineKind::RunHookFinished,
        journal::labels(&paths.run, None),
        journal::payload(&[
            ("hook", json!(hook)),
            ("exit", json!(ran.exit)),
            ("ending", json!(ran.ending)),
            ("log", json!(log.to_string_lossy())),
        ]),
    ) {
        eprintln!(
            "onepipeline: how the {hook} hook of run '{}' ended could not be recorded: {error}",
            paths.run
        );
    }
}

/// Journal `run-hook-fired`, unless the run already carries one.
///
/// The check and the append are one section, under the gate a driver lets go of
/// the run under: a `stop` and a driver that let go on another host are two
/// processes that may both judge the run, and exactly one of them fires.
fn mark(paths: &RunPaths, hook: Hook, command: &str, reason: Option<&Reason>) -> Result<bool> {
    let handover = ledger::Handover::hold(paths)?;
    let marked = if fired(paths) {
        Ok(false)
    } else {
        Journal::open(paths)
            .emit(
                PipelineKind::RunHookFired,
                journal::labels(&paths.run, None),
                journal::payload(&[
                    ("hook", json!(hook)),
                    ("command", json!(command)),
                    ("reason", json!(reason)),
                ]),
            )
            .map(|()| true)
    };
    drop(handover);
    marked
}

/// Whether the run's journal carries the marker a firing leaves.
fn fired(paths: &RunPaths) -> bool {
    journal::read(&paths.journal())
        .iter()
        .any(|event| PipelineKind::from_wire(&event.kind) == Some(PipelineKind::RunHookFired))
}

/// Where a hook's output is kept: `hooks/<hook>.log` under the run's own
/// directory, absolute.
fn log_path(paths: &RunPaths, hook: Hook) -> PathBuf {
    run_root(paths).join("hooks").join(format!("{hook}.log"))
}

/// The run's own directory, absolute, as a hook is told it.
fn run_root(paths: &RunPaths) -> PathBuf {
    std::path::absolute(&paths.dir).unwrap_or_else(|_| paths.dir.clone())
}

/// How a hook's process ended.
struct Ran {
    exit: Option<i32>,
    ending: Ending,
}

/// Spawn one hook, hand it its document, and wait for it — for up to the run's
/// hook timeout, and then end its process tree.
///
/// Its stdout and stderr both go straight to the log file rather than through a
/// pipe this process reads, so a hook that leaves something running behind it
/// holding those streams cannot keep this wait open past the hook itself.
fn run(
    paths: &RunPaths,
    record: &LaunchRecord,
    hook: Hook,
    command: &str,
    reason: Option<&Reason>,
    log: &Path,
    relay: Relay,
) -> Ran {
    let could_not_start = Ran {
        exit: None,
        ending: Ending::CouldNotStart,
    };
    let opened = log
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::File::create(log));
    let output = match opened {
        Ok(output) => output,
        Err(error) => {
            eprintln!(
                "onepipeline: the {hook} hook of run '{}' could not be started, because its log \
                 {} could not be opened: {error}",
                paths.run,
                log.display()
            );
            return could_not_start;
        }
    };
    let document = Document {
        version: DOCUMENT_VERSION,
        hook,
        run_id: &paths.run,
        run_root: run_root(paths).to_string_lossy().into_owned(),
        reason,
    };
    let mut spawning = std::process::Command::new(command);
    // The launch directory. A record from before the field existed names none, and
    // the hook starts where this process is, which is what that launch did.
    if !record.dir.as_os_str().is_empty() {
        spawning.current_dir(&record.dir);
    }
    spawning
        .env(HOOK_ENV, hook.as_str())
        .env(RUN_ID_ENV, &paths.run)
        .env(RUN_ROOT_ENV, run_root(paths))
        .stdin(Stdio::piped());
    // The run's owner, whichever process fired it: a follow-up a hook launches
    // belongs to whoever this run belongs to. A record naming nobody leaves both
    // unset rather than handing on this process's own.
    if record.owned_by(&record.session) {
        spawning
            .env(sys::LAUNCHER_ENV, &record.launcher)
            .env(sys::LAUNCHER_SESSION_ENV, &record.session);
    } else {
        spawning
            .env_remove(sys::LAUNCHER_ENV)
            .env_remove(sys::LAUNCHER_SESSION_ENV);
    }
    let spawned = serde_json::to_string(&document)
        .map_err(std::io::Error::other)
        .and_then(|document| {
            spawning
                .stdout(output.try_clone()?)
                .stderr(output.try_clone()?);
            Ok((spawning.spawn()?, document))
        });
    let (mut child, document) = match spawned {
        Ok(spawned) => spawned,
        Err(error) => {
            // The log is what a reader opens to find out why, so it says.
            let _ = writeln!(
                &output,
                "onepipeline: the {hook} hook '{command}' could not be started: {error}"
            );
            return could_not_start;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        // On a thread of its own, so a hook that never reads its stdin cannot hold
        // this wait on a full pipe: the write ends when the hook does.
        std::thread::spawn(move || {
            let _ = stdin.write_all(document.as_bytes());
        });
    }

    let deadline = deadline_after(record.hook_timeout());
    let mut relayed = 0;
    let ran = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break Ran {
                    exit: status.code(),
                    ending: if status.success() {
                        Ending::Succeeded
                    } else {
                        Ending::Failed
                    },
                }
            }
            Ok(None) if deadline.is_none_or(|deadline| Instant::now() < deadline) => {}
            // Past the timeout — or a child this process can no longer ask about,
            // which is waited out the same way rather than left running unrecorded.
            waited => {
                let _ = sys::stop(child.id(), sys::Stop::Now);
                let _ = child.kill();
                let _ = child.wait();
                break Ran {
                    exit: None,
                    ending: if waited.is_ok() {
                        Ending::TimedOut
                    } else {
                        Ending::Failed
                    },
                };
            }
        }
        if relay == Relay::Stderr {
            relayed = relay_from(log, relayed);
        }
        std::thread::sleep(POLL);
    };
    if relay == Relay::Stderr {
        relay_from(log, relayed);
    }
    ran
}

/// When a hook started now has outlived its timeout, or `None` where that is
/// further off than this host's clock can count to.
///
/// A timeout is any positive whole number of seconds, so one can name an instant
/// no `Instant` holds. That is a timeout no hook can outlive, and it is waited
/// without a bound rather than panicking on a value the launch accepted.
fn deadline_after(timeout: NonZeroU64) -> Option<Instant> {
    Instant::now().checked_add(Duration::from_secs(timeout.get()))
}

/// Repeat what a hook's log gained since `from` on this process's stderr, and
/// answer how far that got.
fn relay_from(log: &Path, from: u64) -> u64 {
    let Ok(mut file) = std::fs::File::open(log) else {
        return from;
    };
    let mut said = Vec::new();
    if file.seek(SeekFrom::Start(from)).is_err() || file.read_to_end(&mut said).is_err() {
        return from;
    }
    let _ = std::io::stderr().write_all(&said);
    from + said.len() as u64
}

/// What `results` says about a run's hooks: each that fired — which, why, how it
/// ended, its exit, its log, and the tail of its output — and each let-go that
/// withheld one.
pub(crate) fn results_lines(view: &RunView) -> String {
    let mut out = String::new();
    for (at, event) in view.events.iter().enumerate() {
        match PipelineKind::from_wire(&event.kind) {
            Some(PipelineKind::RunHookFired) => {
                let Some(hook) = event
                    .payload
                    .get("hook")
                    .and_then(Value::as_str)
                    .and_then(Hook::parse)
                else {
                    continue;
                };
                let reason = event
                    .payload
                    .get("reason")
                    .and_then(|reason| reason.get("kind"))
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                // The log is derived from the run rather than taken from a record,
                // so a view opens only this run's own storage.
                let log = log_path(&view.paths, hook);
                let finished = view.events[at + 1..].iter().find(|later| {
                    PipelineKind::from_wire(&later.kind) == Some(PipelineKind::RunHookFinished)
                        && later.payload.get("hook").and_then(Value::as_str) == Some(hook.as_str())
                });
                let ended = match finished {
                    Some(finished) => format!(
                        "ending: {}; exit: {}",
                        views::one_line(
                            finished
                                .payload
                                .get("ending")
                                .and_then(Value::as_str)
                                .unwrap_or("unrecorded")
                        ),
                        finished
                            .payload
                            .get("exit")
                            .and_then(Value::as_i64)
                            .map_or_else(|| "none".to_string(), |code| code.to_string())
                    ),
                    None => "still running".to_string(),
                };
                out.push_str(&format!(
                    "  {hook} hook fired — reason: {}; {ended}; log: {}\n",
                    views::one_line(reason),
                    log.display()
                ));
                for line in tail(&log) {
                    out.push_str(&format!("      output: {}\n", views::one_line(&line)));
                }
            }
            Some(PipelineKind::RunHookWithheld) => out.push_str(&format!(
                "  run-end hook withheld — the run is paused on a decision ({}), so no hook \
                 fired\n",
                views::one_line(
                    event
                        .payload
                        .get("settlement")
                        .and_then(Value::as_str)
                        .unwrap_or(PAUSED)
                )
            )),
            _ => {}
        }
    }
    out
}

/// The last lines of a hook's log, read from no further back than
/// [`MAX_TAIL_BYTES`].
fn tail(log: &Path) -> Vec<String> {
    let Ok(mut file) = std::fs::File::open(log) else {
        return Vec::new();
    };
    let length = file.metadata().map_or(0, |about| about.len());
    let start = length.saturating_sub(MAX_TAIL_BYTES);
    let mut said = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut said).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&said);
    let mut lines: Vec<&str> = text.lines().collect();
    // A read that began mid-file began mid-line, and half a line is not one.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let from = lines.len().saturating_sub(RESULTS_OUTPUT_LINES);
    lines[from..]
        .iter()
        .map(|line| (*line).to_string())
        .collect()
}

/// The hook command a launch names, resolved from its two rungs.
///
/// The **presence** of a rung decides which one answers, as it does for the node
/// validator: the flag beats the launch config even when what it names is blank,
/// and a blank command is this launch saying it has none rather than a
/// fall-through to the rung below. Not resolved against the launch directory — a
/// command may as legitimately be a name on `PATH` as a path, and it runs in that
/// directory anyway.
pub(crate) fn named(flag: Option<&str>, config: Option<&str>) -> Option<String> {
    flag.or(config)
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_string)
}

/// The refusal for a hook timeout of zero, by the spelling that carried it.
///
/// One sentence for the flag and the launch-config key, as the write-back's
/// budget has one: a timeout of zero ends every hook before it has begun, so a
/// launch that wrote it asked for something it would not get.
pub(crate) fn refused_zero_timeout(spelling: &str) -> String {
    format!(
        "{spelling} names a hook timeout of zero seconds, which ends every hook before it has \
         begun — give it a positive whole number of seconds, or leave it out to take \
         {DEFAULT_HOOK_TIMEOUT_SECONDS} seconds"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Graph;
    use crate::plan::Node;
    use crate::projection::Recorded;

    fn scratch(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("onepipeline-hooks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch root");
        root
    }

    /// A run whose graph holds these nodes, each recorded at its status, in order.
    fn holding(nodes: &[(&str, NodeStatus)]) -> RunState {
        let mut graph = Graph::with_concurrency(4);
        for (id, _) in nodes {
            graph.insert(Node {
                id: (*id).to_string(),
                persona: Some("engineer".into()),
                task: Some("## What\ndo it".into()),
                ..Node::default()
            });
        }
        RunState {
            graph,
            recorded: nodes
                .iter()
                .map(|(id, status)| ((*id).to_string(), Recorded::At(*status)))
                .collect(),
            ..RunState::default()
        }
    }

    /// A draft waiting on a release has not ended, whatever else the graph holds —
    /// and a graph with no nodes has not begun.
    ///
    /// The one arm of the rule no journey can reach: a driver never lets go of a
    /// run holding such a draft, because the draft is a node that can still move.
    /// It is held here so a hook cannot come to fire over one if that ever changes.
    #[test]
    fn a_draft_waiting_on_a_release_has_not_ended_and_an_empty_graph_has_not_begun() {
        let root = scratch("not-ended");
        let paths = RunPaths::under(&root, "demo");
        for beside in [NodeStatus::Done, NodeStatus::Failed, NodeStatus::Parked] {
            assert_eq!(
                judge(
                    &holding(&[("build", beside), ("lift", NodeStatus::CompleteDraft)]),
                    &paths
                ),
                Judged::NotEnded,
                "a draft beside a {} node was judged an ending",
                beside.as_str()
            );
        }
        assert_eq!(judge(&RunState::default(), &paths), Judged::NotEnded);
        // And the draft landing is what ends it.
        assert_eq!(
            judge(
                &holding(&[("build", NodeStatus::Done), ("lift", NodeStatus::Done)]),
                &paths
            ),
            Judged::Fire(Hook::Success, None)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The driver a firing names is read off the stream this crate stamps, and a
    /// stream in any other form names none — so it can never be taken for the
    /// driver a launch record claims.
    #[test]
    fn a_firing_names_its_driver_only_by_the_stream_this_crate_stamps() {
        use crate::projection::DriverClaim;
        let host = sys::hostname();
        let stamped = DriverClaim::of_stream(&format!("{host}-{}", sys::pid()))
            .expect("the stream a journal here writes names its driver");
        assert_eq!(stamped.host, host);
        assert!(stamped.is(Some(&host), std::num::NonZeroU32::new(sys::pid())));
        assert!(!stamped.is(None, std::num::NonZeroU32::new(sys::pid())));
        assert!(!stamped.is(Some(&host), None));
        // A host carrying its own hyphens still splits at the last one.
        let hyphenated = DriverClaim::of_stream("build-host-01-4242").expect("a claim");
        assert_eq!(hyphenated.host, "build-host-01");
        assert_eq!(hyphenated.pid.get(), 4242);
        for malformed in ["", "4242", "-4242", "host-", "host-0", "host-pid", "host"] {
            assert_eq!(
                DriverClaim::of_stream(malformed),
                None,
                "`{malformed}` was read as naming a driver"
            );
        }
        // And a document — a checkpoint or a summary — cannot put back what the
        // stream refuses: a claim naming no host is refused where it is read.
        let written = serde_json::to_value(&hyphenated).expect("a claim serialises");
        assert_eq!(
            serde_json::from_value::<DriverClaim>(written).expect("it reads back"),
            hyphenated
        );
        for refused in [
            json!({"host": "", "pid": 4242}),
            json!({"host": "h", "pid": 0}),
            json!({"host": "h", "pid": 1, "stream": "h-1"}),
        ] {
            assert!(
                serde_json::from_value::<DriverClaim>(refused.clone()).is_err(),
                "{refused} was read as a driver claim"
            );
        }
    }

    /// A timeout too large for this host's clock is no deadline rather than a panic,
    /// and the shipped one is an instant still to come.
    #[test]
    fn a_timeout_past_what_the_clock_can_count_to_is_no_deadline_rather_than_a_panic() {
        assert_eq!(deadline_after(NonZeroU64::MAX), None);
        assert!(deadline_after(DEFAULT_HOOK_TIMEOUT_SECONDS)
            .is_some_and(|deadline| deadline > Instant::now()));
    }

    /// `results` reads a hook's log from no further back than its bound, and never
    /// repeats the half line a read begun mid-file starts on.
    #[test]
    fn the_tail_of_a_long_log_is_its_last_whole_lines() {
        let root = scratch("tail");
        let log = root.join("failure.log");
        let written: String = (1..=10_000).map(|n| format!("said line {n}\n")).collect();
        assert!(written.len() as u64 > MAX_TAIL_BYTES);
        std::fs::write(&log, &written).expect("the log is written");
        let tail = tail(&log);
        assert_eq!(tail.len(), RESULTS_OUTPUT_LINES);
        assert_eq!(tail.first().map(String::as_str), Some("said line 9981"));
        assert_eq!(tail.last().map(String::as_str), Some("said line 10000"));
        assert!(super::tail(&root.join("absent.log")).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
