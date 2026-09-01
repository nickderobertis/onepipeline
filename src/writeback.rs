//! Best-effort projection of the journal-owned graph into its onetaskgraph project.
//!
//! The reconcile loop remains the only author of graph state: it hands immutable folded
//! snapshots to this worker, and the worker only projects them. Store reads never feed back
//! into scheduling, and a failed or slow write is reported and retried off the engine thread.
//!
//! # Ownership: the write-back owns exactly what the plan document declares
//!
//! A projection is a *total replacement* of the destination item, so every field it does
//! not restate is a field it deletes. One rule decides each of them, and it is the plan
//! document: what a plan declares, this worker owns and overwrites; what a plan does not
//! model, it reads off the destination and writes back unchanged. The consequences are
//! enumerated here rather than rediscovered per field, so the next field a destination item
//! grows is decided by the rule instead of by whichever neighbour it was copied from.
//!
//! * **A task's title, body, status, dependency edges and engine metadata are declared** —
//!   by the node the plan holds and the graph the run folded — so the projection replaces
//!   them. That is the whole point of the projection.
//! * **A project's title is not declared.** A plan's `name` is reserved project metadata,
//!   never the board's own heading, so the destination's title is read and written back. In
//!   particular it is *not* the project's native identifier: on a store where those two
//!   coincide the difference is invisible, and on one where they do not, writing the
//!   identifier renames a person's board to a machine id.
//! * **A project's description is not declared**, so the destination's `content` is read and
//!   written back.
//! * **Labels are not modelled by a plan at all** — neither a project's nor a task's — so
//!   both are read off the destination and written back. A destination may refuse a write
//!   whose labels differ from the ones it holds, so a projection that dropped them would
//!   stop reaching the board the moment anybody labelled one of its items.
//! * **Metadata a plan does not name is not declared**, so the destination's own keys
//!   survive and only the reserved `onepipeline.*` keys this worker owns are rewritten.
//!
//! What the destination alone owns — its native id, URL and timestamps — is never written
//! by anybody, so it is neither replaced nor carried.
//!
//! # The status a node is projected under is the settlement's own word
//!
//! A settled run is read as the record of what happened, so the word on the board is the
//! word the settlement used: `done` for a node that is done, `failed` for a task failure,
//! `provider-failed` for the provider death that is not the work's fault, `cancelled` for a
//! cancel, `parked` for a planner's own idle, and `skipped` for a node a failed dependency
//! made unsafe. See [`ProjectedStatus`] for why that is a *name* rather than one of
//! onetaskgraph's seven normalised categories.
//!
//! Beside it, the **change that closed the node**: the commit its change landed at, or the
//! change request a person reads it in. A status alone cannot say which of those happened,
//! and a reader closing work on the status would close it on a change that reached nobody.
//! Both are absent for a node with no change of its own, which is most of them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::edits::Operation;
use crate::event::Source;
use crate::graph::{Landing, NodeStatus};
use crate::ledger::{LaunchRecord, RunPaths};
use crate::plan::Node;
use crate::projection::RunState;
use crate::taskgraph::{QualifiedId, BINARY_ENV};

const SHADOW_SOURCE: &str = "onepipeline-writeback";
/// The reserved key saying whether the change a node published reached its base.
///
/// Named once, and held against the document that records them by
/// [`tests::every_word_and_key_this_projection_writes_is_named_by_the_divergence`]:
/// `docs/contract.md` fixes a narrower vocabulary than this projection writes, so
/// what these three and the words below are reconciled against is
/// `docs/contract-divergences.md`, where that departure is recorded.
const LANDING_KEY: &str = "onepipeline.landing";
/// The reserved key naming the commit a landed change reached its base at.
const LANDING_COMMIT_KEY: &str = "onepipeline.landing_commit";
/// The reserved key naming where a person reads the change a node published.
const CHANGE_URL_KEY: &str = "onepipeline.change_url";
// Cross-platform runners have measured real sibling commands taking longer than ten seconds
// under suite-wide contention. This remains a backstop for an unreachable store, not a
// latency target: projection stays off the reconcile loop while the child runs.
const COMMAND_LIMIT: Duration = Duration::from_secs(60);
// The retry schedule, chosen from the refusal that produces it in practice: a hosted
// destination's rate limiter, which every further attempt extends. It starts where a fixed
// quarter-second schedule did, so a projection that fails once still lands unnoticeably
// fast, and quadruples so that five attempts reach the ceiling inside twenty seconds rather
// than spending a limiter's whole minutes-long window asking several times a minute. The
// ceiling is what GitHub's secondary limit — reported by no endpoint a caller can poll —
// documents as the wait to take: at least one minute. It is a ceiling and not a give-up,
// because a board nobody projects to stays wrong for the rest of the run.
const FIRST_RETRY_AFTER: Duration = Duration::from_millis(250);
const RETRY_GROWTH: u32 = 4;
const RETRY_CEILING: Duration = Duration::from_secs(60);
// Closeout never inherits the duration of a store command. A slow store may keep working in
// the worker, but it still cannot turn a completed graph into run settlement.
const CLOSEOUT_WAIT: Duration = Duration::from_millis(2_250);

#[derive(Clone, PartialEq)]
struct Snapshot {
    project: QualifiedId,
    dir: PathBuf,
    nodes: BTreeMap<String, Node>,
    statuses: BTreeMap<String, NodeStatus>,
    // llmlint: ignore-block[invalid_states_unrepresentable] the string-keyed maps below are
    // copies of `RunState`'s own fields, each of which records there why its value is the
    // plain string every identifier in this crate is: an outcome is the *harness's* open
    // vocabulary and a set declared here would refuse a classification that layer added, a
    // landing commit is checked where it enters by `vcs::landing_commit_of`, and a change
    // URL is the sibling's own and never minted here. Narrowing a copy of a field the crate
    // holds unnarrowed would put a type on this side of a boundary the other side does not
    // have.
    /// The named outcome each settled node carries, which is what tells a
    /// provider death from a task the agent failed. Both settle `failed`.
    outcomes: BTreeMap<String, String>,
    /// Whether each published node's change reached its base branch.
    landings: BTreeMap<String, Landing>,
    /// The commit each landed change reached its base at.
    landing_commits: BTreeMap<String, String>,
    /// Where a person reads the change a node published.
    change_urls: BTreeMap<String, String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    settlements: BTreeMap<String, Value>,
    project_metadata: BTreeMap<String, Value>,
}

/// One projection that failed, as the planner is told about it.
///
/// Carried out of the worker rather than raised there: the worker runs on a
/// thread of its own and the journal has one writer, so what it produces is this
/// record and the reconcile loop raises the surface.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Unprojected {
    /// The onetaskgraph project the projection could not reach.
    pub project: QualifiedId,
    /// The items it was carrying, by plan node id.
    // llmlint: ignore-block[invalid_states_unrepresentable] a node id is the plain string
    // every identifier in this crate is, for the reason `NodeResult::superseded_by` records:
    // these are the ids of the plan this run is executing, read straight off the snapshot
    // the worker was projecting, and the only thing done with them is naming them on a
    // surface.
    pub items: Vec<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// What the sibling, or this worker, said went wrong.
    pub reason: String,
}

#[derive(Default, PartialEq, Eq)]
enum WorkerState {
    #[default]
    Idle,
    Working,
}

/// Where the run holding this worker has got to.
///
/// One value rather than a flag beside the worker's own, because these are phases of one
/// run and it only ever moves forward through them: what a worker between snapshots and a
/// worker waiting out a retry interval each do next is decided by the same value.
#[derive(Default, PartialEq, Eq)]
enum RunPhase {
    #[default]
    Running,
    /// The graph has converged and the run is inside its bounded closeout window, which
    /// suspends the retry schedule: a terminal snapshot left sitting out a minute's
    /// interval inside a two-second window would never be projected at all.
    ClosingOut,
    Stopping,
}

#[derive(Default)]
struct Pending {
    latest: Option<Snapshot>,
    last_success: Option<Snapshot>,
    worker: WorkerState,
    phase: RunPhase,
    /// Projections that failed and have not yet been raised with the planner.
    ///
    /// One entry per outage rather than one per retry: the worker retries until
    /// the store returns, and a surface for every attempt would bury the first.
    unprojected: Vec<Unprojected>,
}

impl Pending {
    fn queue(&mut self, snapshot: Snapshot) -> bool {
        if self.latest.as_ref() == Some(&snapshot) {
            return false;
        }
        if self.worker == WorkerState::Idle
            && self.latest.is_none()
            && self.last_success.as_ref() == Some(&snapshot)
        {
            return false;
        }
        self.latest = Some(snapshot);
        true
    }
}

/// A non-blocking handle owned by the one reconcile loop.
pub struct Writeback {
    pending: Arc<(Mutex<Pending>, Condvar)>,
}

impl Writeback {
    pub fn start(binary: PathBuf, paths: &RunPaths, launch: &LaunchRecord) -> Option<Self> {
        let pending = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker_pending = Arc::clone(&pending);
        let run_dir = paths.dir.clone();
        let launch_dir = if launch.dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            launch.dir.clone()
        };
        // llmlint: ignore-block[changed_behavior_has_e2e] A host refusing one thread while
        // continuing to run this process is resource exhaustion no real CLI journey can
        // arrange at this boundary. Every reachable worker failure is covered against the
        // real sibling and real store; this compatibility edge deliberately disables only
        // the projection and leaves the run unchanged.
        std::thread::Builder::new()
            .name(format!("writeback-{}", paths.run))
            .spawn(move || worker(binary, launch_dir, run_dir, worker_pending))
            .ok()?;
        // llmlint: ignore-end[changed_behavior_has_e2e]
        let writer = Self { pending };
        // The project is retained in each snapshot rather than in the worker so a malformed
        // old launch record disables projection without weakening LaunchRecord's compatibility.
        Some(writer)
    }

    /// Replace any queued projection with the newest journal fold.
    pub fn publish(&self, paths: &RunPaths, launch: &LaunchRecord, state: &RunState) {
        let Ok(project) = launch.project.parse() else {
            return;
        };
        let snapshot = Snapshot {
            project,
            dir: paths.dir.join("writeback"),
            nodes: all_nodes(paths, state),
            statuses: state.statuses(),
            outcomes: state.outcomes.clone(),
            landings: state.landings.clone(),
            landing_commits: state.landing_commits.clone(),
            change_urls: state.change_urls.clone(),
            settlements: settlements(paths),
            project_metadata: state
                .plan
                .as_ref()
                .map(|plan| {
                    let mut metadata = BTreeMap::from([
                        (
                            "onepipeline.schema_version".into(),
                            json!(plan.schema_version),
                        ),
                        ("onepipeline.concurrency".into(), json!(plan.concurrency)),
                    ]);
                    if let Some(goal) = &plan.goal {
                        metadata.insert("onepipeline.goal".into(), json!(goal));
                    }
                    if let Some(name) = &plan.name {
                        metadata.insert("onepipeline.name".into(), json!(name));
                    }
                    metadata
                })
                .unwrap_or_default(),
        };
        let (lock, ready) = &*self.pending;
        if let Ok(mut pending) = lock.lock() {
            if pending.queue(snapshot) {
                ready.notify_one();
            }
        }
    }

    /// Take the projections that failed since this was last asked, clearing them.
    ///
    /// Draining rather than reading: each failure is the planner's to hear once,
    /// and the caller raises it. A worker that recovers does not withdraw one —
    /// what it says is that the board was behind, which stays true.
    pub fn take_unprojected(&self) -> Vec<Unprojected> {
        let (lock, _) = &*self.pending;
        lock.lock()
            .map(|mut pending| std::mem::take(&mut pending.unprojected))
            .unwrap_or_default()
    }

    /// Give the active worker a bounded closeout window for the terminal snapshot.
    pub fn wait_briefly(&self) {
        // Let one already-running real copy reach its own deadline before the process
        // exits. This keeps a completed run from racing a person's next store command,
        // while the hard command limit preserves write-back's latency boundary.
        let deadline = Instant::now() + CLOSEOUT_WAIT;
        let (lock, ready) = &*self.pending;
        let Ok(mut pending) = lock.lock() else { return };
        pending.phase = RunPhase::ClosingOut;
        ready.notify_all();
        while (pending.latest.is_some() || pending.worker == WorkerState::Working)
            && Instant::now() < deadline
        {
            let wait = deadline.saturating_duration_since(Instant::now());
            let Ok((next, _)) = ready.wait_timeout(pending, wait) else {
                return;
            };
            pending = next;
        }
    }
}

impl Drop for Writeback {
    fn drop(&mut self) {
        let (lock, ready) = &*self.pending;
        if let Ok(mut pending) = lock.lock() {
            pending.phase = RunPhase::Stopping;
            // Every waiter, because two of them wait on this one condition: the worker
            // idle between snapshots, and the worker waiting out a retry interval that
            // grows into the tens of seconds. Waking one of the two would leave whichever
            // it was not sitting on a stop it has already been told about.
            ready.notify_all();
        }
        // Deliberately no join: a store process is outside the run's failure and latency
        // boundary, and waiting for it here would turn write-back into run settlement.
    }
}

fn worker(
    binary: PathBuf,
    launch_dir: PathBuf,
    run_dir: PathBuf,
    pending: Arc<(Mutex<Pending>, Condvar)>,
) {
    // The consecutive failures of the streak in progress, and the whole of what this worker
    // remembers about one: zero is a projection that is landing.
    let mut failures: u32 = 0;
    loop {
        let snapshot = {
            let (lock, ready) = &*pending;
            let mut state = match lock.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            while state.latest.is_none() && state.phase != RunPhase::Stopping {
                state = match ready.wait(state) {
                    Ok(state) => state,
                    Err(_) => return,
                };
            }
            if state.phase == RunPhase::Stopping && state.latest.is_none() {
                return;
            }
            state.worker = WorkerState::Working;
            state
                .latest
                .take()
                .expect("the worker was woken by a snapshot")
        };
        match project(&binary, &launch_dir, &run_dir, &snapshot) {
            Ok(()) => {
                if failures > 0 {
                    eprintln!(
                        "onetaskgraph write-back recovered for '{}'",
                        snapshot.project
                    );
                }
                failures = 0;
                let (lock, _) = &*pending;
                if let Ok(mut state) = lock.lock() {
                    state.last_success = Some(snapshot.clone());
                    if state.phase == RunPhase::Stopping && state.latest.is_none() {
                        return;
                    }
                }
            }
            Err(error) => {
                let first = failures == 0;
                failures = failures.saturating_add(1);
                if first {
                    // The one line on the driver's stderr that ever says a projection is in
                    // trouble, so it says what an operator's next question is: whether to
                    // expect another attempt in a moment or in a minute.
                    eprintln!(
                        "onetaskgraph write-back failed for '{}': {error}; retrying, spacing \
                         further attempts out to {} seconds apart while it keeps failing",
                        snapshot.project,
                        RETRY_CEILING.as_secs()
                    );
                }
                if !should_retry_after(&pending, retry_after(failures)) {
                    return;
                }
                let (lock, ready) = &*pending;
                let Ok(mut state) = lock.lock() else { return };
                if first {
                    state.unprojected.push(Unprojected {
                        project: snapshot.project.clone(),
                        items: snapshot.nodes.keys().cloned().collect(),
                        reason: error,
                    });
                }
                if state.phase == RunPhase::Stopping {
                    return;
                }
                if state.latest.is_none() {
                    state.latest = Some(snapshot);
                    ready.notify_one();
                }
            }
        }
        let (lock, ready) = &*pending;
        if let Ok(mut state) = lock.lock() {
            state.worker = WorkerState::Idle;
            ready.notify_all();
        }
    }
}

/// How long to wait before retrying the `failures`-th consecutive failure of a streak.
///
/// `failures` counts the failures of the streak in progress, the first being 1, so the
/// interval is [`FIRST_RETRY_AFTER`] after an isolated failure however long an earlier outage
/// lasted, and never longer than [`RETRY_CEILING`] however long this one does.
fn retry_after(failures: u32) -> Duration {
    FIRST_RETRY_AFTER
        .saturating_mul(RETRY_GROWTH.saturating_pow(failures.saturating_sub(1)))
        .min(RETRY_CEILING)
}

/// Wait out at most one retry interval, answering whether to attempt again at all.
///
/// `false` is [`RunPhase::Stopping`] and the caller returns on it; `true` is the wait
/// having been served, or [`RunPhase::ClosingOut`]. Both phases are read on entry as well
/// as on every wake, so one the run reached while this worker was projecting is honoured
/// rather than missed. A snapshot published meanwhile deliberately does *not* shorten the
/// wait: what the interval spaces is the destination's refusal, and a run that keeps
/// folding new graph state would otherwise retry as fast as it publishes.
fn should_retry_after(pending: &(Mutex<Pending>, Condvar), interval: Duration) -> bool {
    let (lock, ready) = pending;
    let Ok(mut state) = lock.lock() else {
        return false;
    };
    let due = Instant::now() + interval;
    loop {
        match state.phase {
            RunPhase::Stopping => return false,
            RunPhase::ClosingOut => return true,
            RunPhase::Running => {}
        }
        let left = due.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        let Ok((next, _)) = ready.wait_timeout(state, left) else {
            return false;
        };
        state = next;
    }
}

fn project(
    binary: &Path,
    launch_dir: &Path,
    run_dir: &Path,
    snapshot: &Snapshot,
) -> Result<(), String> {
    let destination_project = destination_project(binary, launch_dir, run_dir, snapshot)?;
    let origins = destination_origins(binary, launch_dir, run_dir, snapshot)?;
    // llmlint: ignore-block[changed_behavior_has_e2e] The real outage journey drives
    // destination write failure through onetaskgraph. Making this private, run-owned
    // shadow directory unwritable would instead require sabotaging the host filesystem,
    // outside the public run interface and unrelated to store availability.
    write_shadow(snapshot, &origins, &destination_project)?;
    // llmlint: ignore-end[changed_behavior_has_e2e]
    let root = snapshot.dir.to_string_lossy().into_owned();
    let shadow_project = format!("{SHADOW_SOURCE}:{}", project_file(&snapshot.project));
    let args = [
        "project",
        "copy",
        &shadow_project,
        "--to",
        snapshot.project.source(),
        "--json",
        "--set",
        &format!("sources.{SHADOW_SOURCE}.plugin=local-md"),
        "--set",
        &format!("sources.{SHADOW_SOURCE}.config.root={root}"),
    ];
    let output = bounded_output(binary, launch_dir, run_dir, "project-copy", &args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "copy exited {}: {}",
            exit(&output.status),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn destination_project(
    binary: &Path,
    launch_dir: &Path,
    run_dir: &Path,
    snapshot: &Snapshot,
) -> Result<DestinationProjectItem, String> {
    let args = ["project", "show", snapshot.project.as_str(), "--json"];
    let output = bounded_output(binary, launch_dir, run_dir, "project-show", &args)?;
    if !output.status.success() {
        return Err(format!(
            "project show exited {}: {}",
            exit(&output.status),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // llmlint: ignore-block[changed_behavior_has_e2e] These refusals defend the compiled
    // sibling's machine contract. Producing malformed JSON, partial results, no project,
    // or a different/duplicate project here requires replacing the real onetaskgraph
    // executable with a scripted mock; the real-store journey drives the successful read,
    // total-replacement copy, and preservation of present and absent content end to end.
    let response: ProjectPage =
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
    if !response.errors.is_empty() {
        return Err("project show returned partial results".to_owned());
    }
    let mut items = response.items.into_iter();
    let project = items
        .next()
        .ok_or_else(|| format!("project '{}' was not found", snapshot.project))?;
    if items.next().is_some() || project.id != snapshot.project {
        return Err(format!(
            "project show returned the wrong project for '{}'",
            snapshot.project
        ));
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
    Ok(project.item)
}

/// What the destination already holds for one plan node.
///
/// The id is what a projection writes back onto; the labels are what it carries forward
/// unchanged, because no plan models them. The id keeps the type it was read through:
/// every id the store answers with crossed [`QualifiedId`]'s boundary, and narrowing it to
/// a `String` here would let an unqualified one be written back.
struct Origin {
    id: QualifiedId,
    labels: Vec<DestinationLabel>,
}

fn destination_origins(
    binary: &Path,
    launch_dir: &Path,
    run_dir: &Path,
    snapshot: &Snapshot,
) -> Result<BTreeMap<String, Origin>, String> {
    let mut origins = BTreeMap::new();
    let mut page: Option<String> = None;
    let mut cursors = BTreeSet::new();
    loop {
        let mut args = vec![
            "task".to_owned(),
            "list".to_owned(),
            "--project".to_owned(),
            snapshot.project.as_str().to_owned(),
            "--limit".to_owned(),
            "10000".to_owned(),
            "--json".to_owned(),
        ];
        if let Some(token) = &page {
            args.extend(["--page".to_owned(), token.clone()]);
        }
        // llmlint: ignore-block[changed_behavior_has_e2e] The real unavailable-store
        // journey proves this asynchronous read cannot affect or delay reconciliation.
        // Making the real sibling hang requires host-level process suspension, not an
        // input exposed by either CLI, and substituting a hanging script would mock the
        // exact executable boundary the journey is required to drive.
        let output = bounded_output(binary, launch_dir, run_dir, "task-list", &args)?;
        // llmlint: ignore-end[changed_behavior_has_e2e]
        if !output.status.success() {
            return Err(format!(
                "task list exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        // llmlint: ignore-block[changed_behavior_has_e2e] These refusals defend the
        // compiled sibling's machine contract. Producing malformed JSON, partial errors,
        // invalid qualified ids, missing node ids, or duplicate node ids here requires
        // replacing the real onetaskgraph executable with a scripted mock; real-store
        // success and outage/recovery are driven end to end instead.
        let response: TaskPage =
            serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
        if !response.errors.is_empty() {
            return Err("task list returned partial results".to_owned());
        }
        for task in response.items {
            let node = task
                .item
                .metadata
                .get("onepipeline.id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| format!("task '{}' has no onepipeline.id", task.id.as_str()))?;
            let node = node.to_owned();
            if origins
                .insert(
                    node.clone(),
                    Origin {
                        id: task.id,
                        labels: task.item.labels,
                    },
                )
                .is_some()
            {
                return Err(format!("project has more than one task for node '{node}'"));
            }
        }
        let Some(next) = response.next else { break };
        if next.is_empty() {
            return Err("task list returned an empty next-page cursor".to_owned());
        }
        if !cursors.insert(next.clone()) {
            return Err("task list repeated a next-page cursor".to_owned());
        }
        page = Some(next);
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
    Ok(origins)
}

struct Output {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Run one sibling command without letting either its duration or output pipes hold the worker.
fn bounded_output<S: AsRef<std::ffi::OsStr>>(
    binary: &Path,
    launch_dir: &Path,
    run_dir: &Path,
    name: &str,
    args: &[S],
) -> Result<Output, String> {
    let stdout = run_dir.join(format!("writeback-{name}.stdout"));
    let stderr = run_dir.join(format!("writeback-{name}.stderr"));
    let stdout_file = std::fs::File::create(&stdout).map_err(|error| error.to_string())?;
    let stderr_file = std::fs::File::create(&stderr).map_err(|error| error.to_string())?;
    // llmlint: ignore-block[changed_behavior_has_e2e] Resolution and version checking
    // already exercise the real executable. Inducing spawn or wait syscall failure requires
    // replacing the executable or sabotaging the host; the real-store journey covers the
    // actionable command refusal, retry, and recovery behavior.
    let mut child = Command::new(binary)
        .current_dir(launch_dir)
        .args(args)
        .env_remove(BINARY_ENV)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|error| format!("cannot run {}: {error}", binary.display()))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < COMMAND_LIMIT => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{name} exceeded {} seconds",
                    COMMAND_LIMIT.as_secs()
                ));
            }
            Err(error) => return Err(format!("cannot wait for {name}: {error}")),
        }
    };
    // llmlint: ignore-end[changed_behavior_has_e2e]
    Ok(Output {
        status,
        stdout: std::fs::read(stdout).map_err(|error| error.to_string())?,
        stderr: std::fs::read(stderr).map_err(|error| error.to_string())?,
    })
}

fn exit(status: &ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "on a signal".into(), |code| code.to_string())
}

// The plan store's own `--json` answers, as far as this projection reads them.
//
// **None of the types below deny unknown fields, deliberately.** Each one describes a
// response `onetaskgraph` composes, and that program adds fields to its own answers in
// patch releases — `location` on a project item arrived in 0.2.14. A consumer that mirrors
// a producer's shape under `deny_unknown_fields` turns every additive change upstream into
// a breaking change downstream, and this projection is best-effort: the break surfaces as
// one line on the driver's own log while the board silently stops being written, which is
// the worst shape the failure could take. The opposite rule still holds for a document
// this crate *authors* and reads back — a plan file, an executor-rules file, a reply
// envelope — and those keep `deny_unknown_fields`. The only document this module authors
// is the shadow project `write_shadow` builds, and nothing reads that back through a type.
//
// Tolerating growth is not tolerating anything. Every field kept below is one the
// projection consumes, and each is still required and still typed, so a response that
// stops carrying one — or carries it as something else — is refused by name. What is no
// longer enumerated is what the projection never read: the store's native ids, URLs,
// timestamps, repositories and normalised status were listed only because
// `deny_unknown_fields` demanded a complete mirror of a shape this crate does not own, and
// the mirror is the defect rather than the check.

/// One page of the store's answer to `task list`.
#[derive(Deserialize)]
struct TaskPage {
    items: Vec<DestinationTask>,
    next: Option<String>,
    errors: Vec<Value>,
}

/// The store's answer to `project show`.
#[derive(Deserialize)]
struct ProjectPage {
    items: Vec<DestinationProject>,
    errors: Vec<Value>,
}

#[derive(Deserialize)]
struct DestinationProject {
    id: QualifiedId,
    item: DestinationProjectItem,
}

#[derive(Deserialize)]
struct DestinationProjectItem {
    /// Not declared by a plan, so it is preserved rather than replaced.
    title: String,
    /// Not declared by a plan, so it is preserved rather than replaced.
    content: Option<String>,
    /// Not modelled by a plan at all, so they are preserved rather than dropped.
    labels: Vec<DestinationLabel>,
    /// Only the reserved keys this worker owns are rewritten; the rest are preserved.
    metadata: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct DestinationTask {
    id: QualifiedId,
    item: DestinationTaskItem,
}

#[derive(Deserialize)]
struct DestinationTaskItem {
    /// Not modelled by a plan at all, so they are preserved rather than dropped.
    labels: Vec<DestinationLabel>,
    /// Read for `onepipeline.id`, which is how a destination task names its plan node.
    metadata: BTreeMap<String, Value>,
}

/// One label a destination item carries, read back and written unchanged.
///
/// Serialized as well as deserialized: a preserved label is written into the shadow
/// document whole, so the destination's own label id and colour survive the round trip
/// rather than being reduced to a name the store would have to re-resolve.
// llmlint: ignore-block[invalid_states_unrepresentable] These three are onetaskgraph's own
// strings, and this projection neither mints nor interprets one — it reads a label off the
// destination and writes the same label back. Narrowing them here would turn a label the
// store legitimately holds into a projection failure, which is the defect this type exists
// to fix rather than a stricter form of the fix.
#[derive(Clone, Deserialize, Serialize)]
struct DestinationLabel {
    id: String,
    name: String,
    color: Option<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// Build the shadow project a `project copy` then projects onto the destination.
///
/// Every field written here is decided by the module's ownership rule: a plan-declared
/// field is restated from the snapshot, and a field no plan models is carried over from
/// the destination item this was read against.
fn write_shadow(
    snapshot: &Snapshot,
    origins: &BTreeMap<String, Origin>,
    destination_project: &DestinationProjectItem,
) -> Result<(), String> {
    let projects = snapshot.dir.join("projects");
    let tasks = snapshot
        .dir
        .join("tasks")
        .join(project_file(&snapshot.project));
    std::fs::create_dir_all(&projects).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&tasks).map_err(|e| e.to_string())?;
    let mut project_metadata = destination_project.metadata.clone();
    for (key, value) in &snapshot.project_metadata {
        if project_metadata.contains_key(key) {
            project_metadata.insert(key.clone(), value.clone());
        }
    }
    project_metadata.insert(
        "onetaskgraph.origin".into(),
        json!(snapshot.project.as_str()),
    );
    document(
        &projects.join(format!("{}.md", project_file(&snapshot.project))),
        &json!({
            "title": destination_project.title,
            "labels": destination_project.labels,
            "metadata": project_metadata
        }),
        destination_project.content.as_deref().unwrap_or_default(),
    )?;
    for (id, node) in &snapshot.nodes {
        let mut wire = serde_json::to_value(node)
            .map_err(|e| e.to_string())?
            .as_object()
            .cloned()
            .ok_or_else(|| "node did not serialize as a mapping".to_owned())?;
        let title = wire
            .remove("title")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let content = wire
            .remove("task")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let deps = wire
            .remove("deps")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let repo = wire
            .remove("repo")
            .and_then(|v| v.as_str().map(str::to_owned));
        wire.remove("id");
        let mut metadata = Map::new();
        metadata.insert("onepipeline.id".into(), json!(id));
        let origin = origins.get(id);
        if let Some(origin) = origin {
            metadata.insert("onetaskgraph.origin".into(), json!(origin.id.as_str()));
        }
        for (key, value) in wire {
            metadata.insert(format!("onepipeline.{key}"), value);
        }
        if let Some(settlement) = snapshot.settlements.get(id) {
            metadata.insert(crate::taskgraph::SETTLEMENT_KEY.into(), settlement.clone());
        }
        // What closed the node, beside the word that says it closed. Each is
        // written only where the run *observed* one, so a node with no change of
        // its own — a direct agent node, a human action, a branch its base
        // already carried — carries none of these keys at all rather than an
        // empty value a reader would have to interpret.
        if let Some(landing) = snapshot.landings.get(id) {
            metadata.insert(LANDING_KEY.into(), json!(landing.as_str()));
        }
        if let Some(commit) = snapshot.landing_commits.get(id) {
            metadata.insert(LANDING_COMMIT_KEY.into(), json!(commit));
        }
        if let Some(url) = snapshot.change_urls.get(id) {
            metadata.insert(CHANGE_URL_KEY.into(), json!(url));
        }
        let local_deps: Vec<String> = deps
            .iter()
            .filter_map(Value::as_str)
            .filter(|dep| !crate::graph::is_cross_dag(dep))
            .map(task_file)
            .collect();
        let cross: Vec<String> = deps
            .iter()
            .filter_map(Value::as_str)
            .filter(|dep| crate::graph::is_cross_dag(dep))
            .map(str::to_owned)
            .collect();
        if !cross.is_empty() {
            metadata.insert("onepipeline.deps".into(), json!(cross));
        }
        let status = snapshot
            .statuses
            .get(id)
            .copied()
            .unwrap_or(NodeStatus::Cancelled);
        let mut front = Map::new();
        front.insert("title".into(), json!(title));
        front.insert("project".into(), json!(project_file(&snapshot.project)));
        front.insert(
            "status".into(),
            json!(projected(
                status,
                snapshot.outcomes.get(id).map(String::as_str)
            )),
        );
        // A node the plan has just added has no destination item yet, so there is nothing
        // to preserve and the created task starts with none.
        front.insert(
            "labels".into(),
            json!(origin.map(|origin| origin.labels.as_slice()).unwrap_or(&[])),
        );
        front.insert("depends_on".into(), json!(local_deps));
        front.insert("metadata".into(), Value::Object(metadata));
        if let Some(repo) = repo {
            if repo.starts_with("github.com/") {
                front.insert("repositories".into(), json!([repo]));
            } else if let Some(Value::Object(metadata)) = front.get_mut("metadata") {
                metadata.insert("onepipeline.repo".into(), json!(repo));
            }
        }
        document(
            &tasks.join(format!("{}.md", task_file(id))),
            &Value::Object(front),
            &content,
        )?;
    }
    Ok(())
}

fn document(path: &Path, front: &Value, body: &str) -> Result<(), String> {
    let yaml = serde_norway::to_string(front).map_err(|e| e.to_string())?;
    std::fs::write(path, format!("---\n{yaml}---\n{body}")).map_err(|e| e.to_string())
}

/// The word one node's state is written onto its destination item's status.
///
/// A onetaskgraph status is a **name** and a normalised **category**, and this is
/// the name — four of these are a category's own word and the rest are words that
/// vocabulary has none of. `docs/contract-divergences.md` records what that costs
/// and why it is the cheaper of the two.
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
enum ProjectedStatus {
    #[serde(rename = "in progress")]
    InProgress,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "provider-failed")]
    ProviderFailed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "parked")]
    Parked,
    #[serde(rename = "skipped")]
    Skipped,
    #[serde(rename = "todo")]
    Todo,
}

/// Mirror one node's settlement onto the word its destination item reads under.
///
/// The outcome is read for one distinction alone, and it is the one no status
/// carries: a dispatch its provider killed and a task its agent failed both
/// settle [`NodeStatus::Failed`], and a reader who cannot tell them apart goes
/// looking for what the work got wrong when nothing was wrong with the work.
fn projected(status: NodeStatus, outcome: Option<&str>) -> ProjectedStatus {
    match status {
        // The one node here projected under a word that is not its own: a draft
        // has not settled, so `done` and `cancelled` would both be false, and
        // the run is still on it.
        NodeStatus::Running | NodeStatus::CompleteDraft => ProjectedStatus::InProgress,
        NodeStatus::Done => ProjectedStatus::Done,
        NodeStatus::Failed if outcome == Some(crate::engine::PROVIDER_FAILED) => {
            ProjectedStatus::ProviderFailed
        }
        NodeStatus::Failed => ProjectedStatus::Failed,
        NodeStatus::Cancelled => ProjectedStatus::Cancelled,
        NodeStatus::Parked => ProjectedStatus::Parked,
        NodeStatus::Skipped => ProjectedStatus::Skipped,
        NodeStatus::Pending | NodeStatus::Ready | NodeStatus::Waiting | NodeStatus::Blocked => {
            ProjectedStatus::Todo
        }
    }
}

fn all_nodes(paths: &RunPaths, state: &RunState) -> BTreeMap<String, Node> {
    let mut nodes: BTreeMap<String, Node> = state
        .plan
        .as_ref()
        .into_iter()
        .flat_map(|plan| plan.tasks.iter())
        .map(|node| (node.id.clone(), node.clone()))
        .collect();
    for event in crate::journal::read(&paths.journal()) {
        if event.source != Source::Pipeline
            || event.kind.0 != crate::journal::PipelineKind::EditCommitted.as_str()
        {
            continue;
        }
        let Some(operations) = event
            .payload
            .get("operations")
            .and_then(|v| serde_json::from_value::<Vec<Operation>>(v.clone()).ok())
        else {
            continue;
        };
        for operation in operations {
            if let Operation::NodeAdded { node, .. } = operation {
                nodes.insert(node.id.clone(), *node);
            }
        }
    }
    for node in state.graph.iter() {
        nodes.insert(node.id.clone(), node.clone());
    }
    nodes
}

fn settlements(paths: &RunPaths) -> BTreeMap<String, Value> {
    let mut found = BTreeMap::new();
    for event in crate::journal::read(&paths.journal()) {
        if event.source == Source::Pipeline
            && event.kind.0 == crate::journal::PipelineKind::NodeSettled.as_str()
        {
            if let Some(node) = event.labels.node.as_ref() {
                found.insert(node.clone(), Value::Object(event.payload));
            }
        }
    }
    found
}

fn project_file(project: &QualifiedId) -> String {
    encoded(project.as_str())
}
fn task_file(id: &str) -> String {
    encoded(id)
}
fn encoded(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        projected, write_shadow, DestinationLabel, DestinationProjectItem, Landing, Origin,
        Pending, ProjectedStatus, Snapshot, WorkerState, Writeback, CHANGE_URL_KEY,
        LANDING_COMMIT_KEY, LANDING_KEY,
    };
    use crate::graph::NodeStatus;
    use crate::ledger::{LaunchRecord, RunPaths};
    use crate::plan::Node;
    use crate::projection::RunState;
    use serde_json::{json, Map, Value};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn snapshot(status: NodeStatus) -> Snapshot {
        Fixture::new("queue").snapshot_with(|snapshot| {
            snapshot.project = "plans:deduplication".parse().expect("a qualified project");
            snapshot.dir = PathBuf::from("writeback");
            snapshot.statuses = BTreeMap::from([("node".to_owned(), status)]);
        })
    }

    #[test]
    fn returning_to_the_last_success_supersedes_a_different_pending_snapshot() {
        let first = snapshot(NodeStatus::Pending);
        let superseded = snapshot(NodeStatus::Running);
        let mut pending = Pending {
            latest: Some(superseded),
            last_success: Some(first.clone()),
            worker: WorkerState::Working,
            phase: super::RunPhase::Running,
            unprojected: Vec::new(),
        };

        assert!(pending.queue(first.clone()));
        assert!(pending.latest.as_ref() == Some(&first));
    }

    /// A run's own end, arriving while the worker is waiting out a retry interval, is
    /// honoured then rather than once the interval nobody is waiting for has run out.
    ///
    /// In this process on purpose. The stop an operator types signals the whole process
    /// tree, so the journey beside this one — `a_stop_during_a_long_retry_interval_is_...`
    /// in `tests/e2e/store.rs` — cannot tell a worker that left because it was told from
    /// one the kill took with it whatever it was doing. Proving the worker's own half means
    /// watching it *after* its stop is requested, and only something holding this process
    /// open across that request can.
    ///
    /// What it watches is real: the thread [`Writeback::start`] spawns, the schedule that
    /// thread keeps, and its own reference to the state it shares — which it lets go of
    /// when, and only when, `worker` returns. The destination refuses at the same
    /// subprocess boundary a rate-limited one does, because the binary the run names is not
    /// installed, which is a refusal this test can arrange without a store at all.
    #[test]
    fn a_stop_reaches_a_worker_that_is_waiting_out_a_retry_interval() {
        let dir = scratch("stop-mid-wait");
        let paths = RunPaths {
            run: "stopmidwait".to_owned(),
            dir: dir.clone(),
        };
        let launch = a_launch(&paths);
        let writeback =
            Writeback::start(dir.join("onetaskgraph-nobody-installed"), &paths, &launch)
                .expect("a write-back worker");
        writeback.publish(&paths, &launch, &RunState::default());

        // Four attempts in, the interval the worker is now waiting out is longer than every
        // one before it — so the last one this test actually watched is a lower bound on
        // what a stop would have to sit through if it were not honoured.
        let waited = intervals_between_attempts(&paths, 4);
        let outstanding = *waited.last().expect("an interval between attempts");
        assert!(
            outstanding >= Duration::from_secs(2),
            "the streak is not deep enough for a stop to have anything to wait out: {waited:?}"
        );

        let shared = Arc::clone(&writeback.pending);
        let asked = Instant::now();
        drop(writeback);
        while Arc::strong_count(&shared) > 1 {
            assert!(
                asked.elapsed() < outstanding,
                "the worker was still running {:?} after its stop was requested, with an \
                 interval of at least {outstanding:?} outstanding",
                asked.elapsed()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !attempt_capture(&paths).exists(),
            "the worker asked the destination again on its way out"
        );
    }

    /// The wall-clock intervals between the worker's next `count` attempts.
    ///
    /// An attempt is counted where the destination's side of the boundary is: the capture
    /// file the worker creates for the store command it is about to run. This takes each
    /// one away again, so the next one appearing is the next attempt rather than the same
    /// file read twice — which a modification time this filesystem may round would not tell
    /// apart.
    fn intervals_between_attempts(paths: &RunPaths, count: usize) -> Vec<Duration> {
        let capture = attempt_capture(paths);
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut at: Vec<Instant> = Vec::new();
        while at.len() < count {
            assert!(
                Instant::now() < deadline,
                "the worker made {} attempts, not the {count} this test reads its intervals \
                 off",
                at.len()
            );
            if capture.exists() {
                at.push(Instant::now());
                std::fs::remove_file(&capture).expect("the capture file is taken away");
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        at.windows(2).map(|pair| pair[1] - pair[0]).collect()
    }

    fn attempt_capture(paths: &RunPaths) -> PathBuf {
        paths.dir.join("writeback-project-show.stderr")
    }

    /// A launch record naming a project to project into, and nothing else this worker reads.
    fn a_launch(paths: &RunPaths) -> LaunchRecord {
        serde_json::from_value(json!({
            "run_id": paths.run,
            "project": "plans:stop-mid-wait",
            "dir": paths.dir,
            "launcher": "test",
            "session": "test",
            "pid": crate::sys::pid(),
            "host": "test",
            "started": "linux-proc-stat:1",
            "started_at": "2026-09-01T00:00:00Z",
            "heartbeat_interval": 1_800,
        }))
        .expect("a launch record")
    }

    /// Every word this projection writes, in the one arrangement that states them:
    /// the list, and an exhaustive match over it, so a ninth status stops this
    /// compiling until it is named here too.
    fn every_projected_word() -> Vec<String> {
        [
            ProjectedStatus::Todo,
            ProjectedStatus::InProgress,
            ProjectedStatus::Done,
            ProjectedStatus::Failed,
            ProjectedStatus::ProviderFailed,
            ProjectedStatus::Cancelled,
            ProjectedStatus::Parked,
            ProjectedStatus::Skipped,
        ]
        .into_iter()
        .inspect(|status| match status {
            ProjectedStatus::Todo
            | ProjectedStatus::InProgress
            | ProjectedStatus::Done
            | ProjectedStatus::Failed
            | ProjectedStatus::ProviderFailed
            | ProjectedStatus::Cancelled
            | ProjectedStatus::Parked
            | ProjectedStatus::Skipped => {}
        })
        .map(word)
        .collect()
    }

    /// The projection writes a vocabulary wider than the approved contract fixes,
    /// and three reserved keys beside the settlement it already carried. Both are
    /// recorded as a divergence, and this is the gate that keeps the record and
    /// the code from parting: a word or a key the document does not name is one
    /// nobody proposed.
    #[test]
    fn every_word_and_key_this_projection_writes_is_named_by_the_divergence() {
        let divergence = include_str!("../docs/contract-divergences.md");
        let words = every_projected_word();
        for word in &words {
            assert!(
                divergence.contains(&format!("`{word}`")),
                "docs/contract-divergences.md does not name the projected status `{word}`"
            );
        }
        let keys = [LANDING_KEY, LANDING_COMMIT_KEY, CHANGE_URL_KEY];
        for key in keys {
            assert!(
                divergence.contains(&format!("`{key}`")),
                "docs/contract-divergences.md does not name the reserved key `{key}`"
            );
        }
        // And how many of each, because naming them all is not the same as
        // counting them: a document that lists eight words and then says six has
        // one sentence a reader trusts and one that is wrong.
        for stated in [
            format!("{} words where the contract names 4", words.len()),
            format!("the {} reserved keys beside the settlement", keys.len()),
        ] {
            assert!(
                divergence.contains(&stated),
                "docs/contract-divergences.md does not say \"{stated}\""
            );
        }
    }

    /// The four words that *are* onetaskgraph's own normalised categories stay
    /// the words the approved contract names them with.
    ///
    /// The other four are deliberately not here: they are names that vocabulary
    /// has none of, which `docs/contract-divergences.md` records as a divergence
    /// rather than the contract naming them.
    #[test]
    fn the_projected_categories_remain_named_by_the_approved_contract() {
        let contract = include_str!("../docs/contract.md");
        for status in [
            ProjectedStatus::Todo,
            ProjectedStatus::InProgress,
            ProjectedStatus::Done,
            ProjectedStatus::Cancelled,
        ] {
            let native = word(status).replace(' ', "-");
            assert!(
                contract.contains(&format!("`{native}`")),
                "docs/contract.md no longer names the projected status category `{native}`"
            );
        }
    }

    /// Each settlement is projected under a word of its own.
    ///
    /// The defect this replaces put `done` on a node whose work was thrown away
    /// — the same word a merged node got, with only a nested `outcome` to
    /// disagree — and made a parked node indistinguishable from a cancelled one.
    /// So the assertion is that no two of these share a word, which is a
    /// property a mapping that collapsed any pair could not have.
    #[test]
    fn every_settlement_is_projected_under_a_word_of_its_own() {
        let settlements = [
            ("done", NodeStatus::Done, None),
            ("a task the agent failed", NodeStatus::Failed, None),
            (
                "a dispatch its provider killed",
                NodeStatus::Failed,
                Some(crate::engine::PROVIDER_FAILED),
            ),
            ("cancelled", NodeStatus::Cancelled, None),
            ("parked", NodeStatus::Parked, None),
        ];
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for (settlement, status, outcome) in settlements {
            let projected = word(projected(status, outcome));
            if let Some(shared) = seen.insert(projected.clone(), settlement) {
                panic!(
                    "'{settlement}' and '{shared}' are both projected as `{projected}`, so a \
                     board cannot tell them apart"
                );
            }
        }
        assert_eq!(
            seen.keys().cloned().collect::<Vec<_>>(),
            ["cancelled", "done", "failed", "parked", "provider-failed"],
        );
    }

    /// The word one projected status is written as, taken through the
    /// serializer that writes it rather than restated beside it.
    fn word(status: ProjectedStatus) -> String {
        match serde_json::to_value(status).expect("a projected status serializes") {
            Value::String(word) => word,
            other => panic!("a projected status is a string, not {other}"),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "onepipeline-writeback-{name}-{}",
            crate::sys::pid()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// The reserved keys this worker owns and overwrites on a projected item.
    ///
    /// Everything else on a destination item is the complement — what the
    /// projection has to carry through unchanged — and [`preserved`] is that
    /// complement read off either side of the write.
    fn owned(key: &str) -> bool {
        key.starts_with("onepipeline.") || key.starts_with("onetaskgraph.")
    }

    /// Everything on a projected document that the writer does **not** own.
    ///
    /// The complement of what it declares, taken as one value rather than field
    /// by field: "did the new thing arrive?" and "did anything else leave?" are
    /// different questions, and only an assertion shaped like this one can fail
    /// on the second.
    fn preserved(front: &Value, body: &str) -> Value {
        let metadata: Map<String, Value> = front["metadata"]
            .as_object()
            .expect("a projected document carries metadata")
            .iter()
            .filter(|(key, _)| !owned(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        json!({
            "title": front["title"],
            "labels": front["labels"],
            "metadata": metadata,
            "body": body,
        })
    }

    /// The same complement, read off the destination the projection was written
    /// against.
    fn preserved_of(destination: &DestinationProjectItem) -> Value {
        let metadata: Map<String, Value> = destination
            .metadata
            .iter()
            .filter(|(key, _)| !owned(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        json!({
            "title": destination.title,
            "labels": destination.labels,
            "metadata": metadata,
            "body": destination.content.clone().unwrap_or_default(),
        })
    }

    fn written(path: &Path) -> (Value, String) {
        let document = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("{} was not written: {error}", path.display()));
        let (front, body) = document
            .strip_prefix("---\n")
            .expect("a projected document opens its front matter")
            .split_once("---\n")
            .expect("a projected document closes its front matter");
        (
            serde_norway::from_str(front).expect("the front matter is YAML"),
            body.to_owned(),
        )
    }

    /// One store fixture: a destination project, the snapshot projected onto it,
    /// and what that destination already holds for each node.
    ///
    /// Held together rather than assembled per test because the properties that
    /// make it able to fail are properties of the three *together* — see
    /// [`undiscriminating`].
    struct Fixture {
        name: &'static str,
        dir: PathBuf,
        snapshot: Snapshot,
        origins: BTreeMap<String, Origin>,
        destination: DestinationProjectItem,
    }

    impl Fixture {
        /// Two nodes, of which exactly one has a destination task; a project
        /// whose title is not the identifier the store holds it under; and
        /// authored labels, content and metadata on both sides.
        fn new(name: &'static str) -> Self {
            let dir = scratch(name);
            let node = |id: &str, deps: &[&str]| -> Node {
                serde_json::from_value(json!({
                    "id": id,
                    "title": format!("feat: {id} it"),
                    "task": format!("## What\n{id} it."),
                    "persona": "engineer",
                    "deps": deps,
                }))
                .expect("a plan node")
            };
            Self {
                name,
                snapshot: Snapshot {
                    project: "plans:board".parse().expect("a qualified project"),
                    dir: dir.clone(),
                    nodes: BTreeMap::from([
                        ("build".to_owned(), node("build", &["design"])),
                        ("design".to_owned(), node("design", &[])),
                    ]),
                    statuses: BTreeMap::from([
                        ("build".to_owned(), NodeStatus::Done),
                        ("design".to_owned(), NodeStatus::Done),
                    ]),
                    outcomes: BTreeMap::new(),
                    landings: BTreeMap::new(),
                    landing_commits: BTreeMap::new(),
                    change_urls: BTreeMap::new(),
                    settlements: BTreeMap::new(),
                    project_metadata: BTreeMap::from([(
                        "onepipeline.concurrency".into(),
                        json!(4),
                    )]),
                },
                // Only `build`. A node the plan has just added has no destination
                // task at all, and holding both kinds in one fixture is what makes
                // "carried through" and "invented none" separable answers.
                origins: BTreeMap::from([(
                    "build".to_owned(),
                    Origin {
                        id: "plans:board/002-build".parse().expect("a qualified task"),
                        labels: labels(&[("needs-review", Some("d73a4a"))]),
                    },
                )]),
                destination: destination("A person's own board", &["planning", "q3"]),
                dir,
            }
        }

        /// The same fixture with one field of the snapshot restated.
        fn snapshot_with(mut self, edit: impl FnOnce(&mut Snapshot)) -> Snapshot {
            edit(&mut self.snapshot);
            self.snapshot.clone()
        }

        /// Project it, refusing a fixture that could not tell a right answer from
        /// a wrong one.
        fn project(&self) {
            if let Some(missing) =
                undiscriminating(self.name, &self.destination, &self.snapshot, &self.origins)
            {
                panic!("{missing}");
            }
            write_shadow(&self.snapshot, &self.origins, &self.destination)
                .expect("the shadow project is written");
        }

        /// The project document and one node's task document, as written.
        fn project_document(&self) -> (Value, String) {
            written(&self.dir.join("projects").join(format!(
                "{}.md",
                super::project_file(&self.snapshot.project)
            )))
        }

        fn task_document(&self, node: &str) -> (Value, String) {
            written(
                &self
                    .dir
                    .join("tasks")
                    .join(super::project_file(&self.snapshot.project))
                    .join(format!("{}.md", super::task_file(node))),
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Why a fixture standing in for a store could not fail, or `None` when it
    /// can.
    ///
    /// Two rules, enforced where the fixture is built rather than trusted to be
    /// remembered per test, because each of them is a defect that shipped
    /// through a worker, a judge, a monitor and a manager. Under a store whose
    /// project's native id *is* its title, writing the identifier as the title
    /// was byte-identical to preserving it; under a fixture holding one of
    /// everything the code filters on, an ignored filter and an honoured one
    /// produced the same bytes.
    fn undiscriminating(
        fixture: &str,
        destination: &DestinationProjectItem,
        snapshot: &Snapshot,
        origins: &BTreeMap<String, Origin>,
    ) -> Option<String> {
        let missing = if snapshot.project.native() == destination.title {
            "its destination project's identifier is its own title, so writing the \
             identifier and preserving the title are the same bytes"
        } else if snapshot.nodes.len() < 2 {
            "it holds one node, so a projection that wrote the wrong one is \
             indistinguishable from one that wrote the right one"
        } else if !snapshot.nodes.keys().any(|id| origins.contains_key(id))
            || !snapshot.nodes.keys().any(|id| !origins.contains_key(id))
        {
            "every node has a destination task or none does, so a projection that \
             ignored what the destination already holds cannot be told from one that \
             read it"
        } else {
            return None;
        };
        Some(format!("fixture '{fixture}': {missing}"))
    }

    /// The rule the fixtures are built to, held against fixtures that break it.
    ///
    /// A check that passed a one-project, id-equals-title fixture would be the
    /// thing it exists to prevent, so each degenerate shape is stated here and
    /// the check has to name it.
    #[test]
    fn a_fixture_that_could_not_tell_a_right_answer_from_a_wrong_one_is_refused() {
        let sound = Fixture::new("discrimination");
        assert_eq!(
            undiscriminating(
                sound.name,
                &sound.destination,
                &sound.snapshot,
                &sound.origins
            ),
            None,
            "the fixture every test here is built from does not meet its own rule"
        );

        let named = |missing: Option<String>, property: &str| {
            let said = missing.unwrap_or_else(|| {
                panic!("a fixture whose {property} could prove nothing was accepted")
            });
            assert!(
                said.contains("fixture 'degenerate'"),
                "the refusal does not name the fixture: {said}"
            );
            assert!(
                said.contains(property),
                "the refusal does not name the missing property: {said}"
            );
        };

        // A project whose title is the store's own identifier for it.
        named(
            undiscriminating(
                "degenerate",
                &destination("board", &[]),
                &sound.snapshot,
                &sound.origins,
            ),
            "identifier is its own title",
        );
        // One node, so an ignored project or node filter is invisible.
        let one = Fixture::new("one-node").snapshot_with(|snapshot| {
            snapshot.nodes.retain(|id, _| id == "build");
        });
        named(
            undiscriminating("degenerate", &sound.destination, &one, &sound.origins),
            "holds one node",
        );
        // Every node already known to the destination, so a projection that never
        // read it looks the same.
        named(
            undiscriminating(
                "degenerate",
                &sound.destination,
                &sound.snapshot,
                &BTreeMap::new(),
            ),
            "every node has a destination task or none does",
        );
    }

    /// The destination project a projection is written against.
    ///
    /// Built out of the sibling's own machine response rather than by naming
    /// fields, so the shape this projection reads is the shape `project show
    /// --json` answers in.
    fn destination(title: &str, labels: &[&str]) -> DestinationProjectItem {
        serde_json::from_value(json!({
            "id": "board",
            "title": title,
            "content": "A person's own description.",
            "status": {"category": "backlog", "name": "backlog"},
            "labels": labels
                .iter()
                .map(|name| json!({"id": name, "name": name, "color": null}))
                .collect::<Vec<_>>(),
            "url": null,
            "created_at": null,
            "updated_at": null,
            "metadata": {"authored.note": "keep this value", "authored.owner": "a person"},
            "repositories": [],
        }))
        .expect("the sibling's own project response")
    }

    fn labels(named: &[(&str, Option<&str>)]) -> Vec<DestinationLabel> {
        serde_json::from_value(json!(named
            .iter()
            .map(|(name, color)| json!({"id": name, "name": name, "color": color}))
            .collect::<Vec<_>>()))
        .expect("the sibling's own labels")
    }

    /// The rule's first consequence: everything a plan declares is replaced,
    /// which is what the projection is for.
    #[test]
    fn a_projection_replaces_every_field_the_plan_declares() {
        let mut fixture = Fixture::new("declared");
        fixture
            .snapshot
            .statuses
            .insert("build".to_owned(), NodeStatus::Running);
        fixture.project();

        let (front, body) = fixture.task_document("build");
        assert_eq!(front["title"], "feat: build it");
        assert_eq!(body, "## What\nbuild it.");
        assert_eq!(front["status"], "in progress");
        assert_eq!(
            front["depends_on"],
            json!([super::task_file("design")]),
            "the projection lost the plan's dependency edge"
        );
        assert_eq!(front["metadata"]["onepipeline.id"], "build");
        assert_eq!(front["metadata"]["onepipeline.persona"], "engineer");
        // And the node the destination has no task for is written too, under its
        // own title rather than the other node's.
        let (design, _) = fixture.task_document("design");
        assert_eq!(design["title"], "feat: design it");
        assert_eq!(design["metadata"]["onepipeline.id"], "design");
    }

    /// The rule's second, third and fourth consequences at once, as one
    /// complement: the projection is a total replacement of the destination
    /// item, so everything it does not own has to come back unchanged.
    ///
    /// Asserted as the whole complement rather than field by field, and held
    /// against a destination missing one of those fields: an assertion that
    /// could not fail on a deletion is the defect this replaces.
    #[test]
    fn a_projection_carries_through_everything_the_plan_does_not_declare() {
        let fixture = Fixture::new("preserved");
        fixture.project();

        let (front, body) = fixture.project_document();
        assert_eq!(
            preserved(&front, &body),
            preserved_of(&fixture.destination),
            "the projection changed something the plan does not declare"
        );
        assert_ne!(
            front["title"],
            json!(fixture.snapshot.project.native()),
            "the projection wrote the project's native identifier as its title"
        );
        assert_eq!(
            front["metadata"]["onetaskgraph.origin"],
            json!(fixture.snapshot.project.as_str()),
            "the projection lost the destination project it writes onto"
        );

        // The same assertion, against a destination one unowned field lighter.
        // It has to fail, or it was never asking whether anything left.
        let mut deleted = destination("A person's own board", &["planning", "q3"]);
        deleted.metadata.remove("authored.note");
        assert_ne!(
            preserved(&front, &body),
            preserved_of(&deleted),
            "the complement assertion cannot fail on a deleted field, so it proves nothing"
        );
        let mut unlabelled = destination("A person's own board", &["planning"]);
        unlabelled.metadata = fixture.destination.metadata.clone();
        assert_ne!(
            preserved(&front, &body),
            preserved_of(&unlabelled),
            "the complement assertion cannot fail on a dropped label, so it proves nothing"
        );
    }

    /// The same consequence for a task: labels are read off the destination task
    /// the node projects onto, and a node the destination has no task for gets
    /// none invented for it.
    #[test]
    fn a_projection_carries_a_destination_tasks_labels_through_and_invents_none() {
        let fixture = Fixture::new("task-labels");
        fixture.project();

        let (build, _) = fixture.task_document("build");
        assert_eq!(
            build["labels"],
            json!([{"id": "needs-review", "name": "needs-review", "color": "d73a4a"}]),
            "the projection dropped the destination task's labels"
        );
        assert_eq!(
            build["metadata"]["onetaskgraph.origin"], "plans:board/002-build",
            "the projection lost the destination task it writes onto"
        );

        let (design, _) = fixture.task_document("design");
        assert_eq!(
            design["labels"],
            json!([]),
            "the projection invented labels for a task the destination does not hold"
        );
        assert_eq!(
            design["metadata"].get("onetaskgraph.origin"),
            None,
            "the projection claimed a destination task for a node the destination has none for"
        );
    }

    /// A settled node's item says which settlement closed it, and what the
    /// change that closed it was.
    ///
    /// The two halves are one fact: a status alone cannot say whether the work
    /// reached anybody, and a reader closing work on the word `done` would close
    /// it on a change request nobody has merged.
    #[test]
    fn a_settled_node_carries_the_change_that_closed_it_and_a_node_without_one_carries_none() {
        let mut fixture = Fixture::new("landing");
        fixture
            .snapshot
            .landings
            .insert("build".to_owned(), Landing::Landed);
        fixture
            .snapshot
            .landing_commits
            .insert("build".to_owned(), "d3adb33f".to_owned());
        fixture.snapshot.change_urls.insert(
            "build".to_owned(),
            "https://example.invalid/pull/7".to_owned(),
        );
        fixture.project();

        let (build, _) = fixture.task_document("build");
        assert_eq!(build["status"], "done", "a node that landed is not closed");
        assert_eq!(build["metadata"]["onepipeline.landing"], "landed");
        assert_eq!(build["metadata"]["onepipeline.landing_commit"], "d3adb33f");
        assert_eq!(
            build["metadata"]["onepipeline.change_url"],
            "https://example.invalid/pull/7"
        );

        // A node with no change of its own claims none, rather than claiming one
        // with an empty value a reader would have to interpret.
        let (design, _) = fixture.task_document("design");
        for absent in [
            "onepipeline.landing",
            "onepipeline.landing_commit",
            "onepipeline.change_url",
        ] {
            assert_eq!(
                design["metadata"].get(absent),
                None,
                "a node with no change of its own was recorded as having {absent}"
            );
        }
    }

    /// Each settlement reaches the destination document under its own word, and
    /// the settlement itself is beside it.
    #[test]
    fn each_settlement_reaches_the_document_under_its_own_word() {
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for (settlement, status, outcome) in [
            ("done", NodeStatus::Done, None),
            ("failed", NodeStatus::Failed, None),
            (
                "provider-failed",
                NodeStatus::Failed,
                Some(crate::engine::PROVIDER_FAILED),
            ),
            ("cancelled", NodeStatus::Cancelled, None),
            ("parked", NodeStatus::Parked, None),
        ] {
            let mut fixture = Fixture::new("settlement-words");
            fixture.snapshot.statuses.insert("build".to_owned(), status);
            if let Some(outcome) = outcome {
                fixture
                    .snapshot
                    .outcomes
                    .insert("build".to_owned(), outcome.to_owned());
            }
            fixture.snapshot.settlements.insert(
                "build".to_owned(),
                json!({"status": status.as_str(), "outcome": outcome}),
            );
            fixture.project();

            let (build, _) = fixture.task_document("build");
            let word = build["status"]
                .as_str()
                .expect("a projected status is a string")
                .to_owned();
            assert_eq!(
                build["metadata"][crate::taskgraph::SETTLEMENT_KEY]["status"],
                json!(status.as_str()),
                "the settlement did not reach the item beside its word"
            );
            if let Some(shared) = seen.insert(word.clone(), settlement) {
                panic!("'{settlement}' and '{shared}' both reached the board as `{word}`");
            }
        }
        assert_eq!(seen.len(), 5, "two settlements shared a word: {seen:?}");
    }

    /// A draft-complete node reaches the board as `in progress`, and why it is
    /// still open is beside the word.
    ///
    /// It is the one node projected under a word that is not its settlement's,
    /// because it has not settled: its work is finished and the change it
    /// published cannot land until the release it awaits happens, so the run is
    /// still on it. `done` would tell a reader the change landed and `cancelled`
    /// would tell them nobody is coming back to it, so both halves are held —
    /// the word it takes, and the two it must not share with the settlements
    /// that do mean those things.
    #[test]
    fn a_draft_complete_node_reaches_the_board_as_in_progress_and_says_why() {
        let mut fixture = Fixture::new("draft-complete");
        fixture
            .snapshot
            .statuses
            .insert("build".to_owned(), NodeStatus::CompleteDraft);
        fixture
            .snapshot
            .outcomes
            .insert("build".to_owned(), crate::vcs::DRAFTED.to_owned());
        fixture
            .snapshot
            .landings
            .insert("build".to_owned(), Landing::Unlanded);
        fixture.snapshot.change_urls.insert(
            "build".to_owned(),
            "https://example.invalid/pull/9".to_owned(),
        );
        fixture.snapshot.settlements.insert(
            "build".to_owned(),
            json!({
                "status": NodeStatus::CompleteDraft.as_str(),
                "outcome": crate::vcs::DRAFTED,
                "detail": "awaiting the crate release of github.com/owner/engine",
            }),
        );
        fixture.project();

        let (build, _) = fixture.task_document("build");
        assert_eq!(
            build["status"], "in progress",
            "a node whose change cannot land yet did not reach the board as work still in hand"
        );
        // The two words it must not take, read off the projection that writes
        // them rather than restated beside it: a draft is neither closed nor
        // abandoned.
        for settled in [NodeStatus::Done, NodeStatus::Cancelled] {
            assert_ne!(
                build["status"].as_str(),
                Some(word(projected(settled, None)).as_str()),
                "a draft-complete node took the board word `{}` settles under",
                settled.as_str()
            );
        }

        // Why it is still open, beside the word that cannot say it. The
        // settlement's own reason is on the reserved key, and the draft a person
        // opens to read it is beside that.
        assert_eq!(
            build["metadata"][crate::taskgraph::SETTLEMENT_KEY]["detail"],
            json!("awaiting the crate release of github.com/owner/engine"),
            "the draft reached the board with nothing on it saying why it is still open"
        );
        assert_eq!(
            build["metadata"]["onepipeline.change_url"],
            "https://example.invalid/pull/9"
        );
        assert_eq!(build["metadata"]["onepipeline.landing"], "unlanded");
    }
}
