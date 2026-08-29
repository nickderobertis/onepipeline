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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::edits::Operation;
use crate::event::Source;
use crate::graph::NodeStatus;
use crate::ledger::{LaunchRecord, RunPaths};
use crate::plan::Node;
use crate::projection::RunState;
use crate::taskgraph::{QualifiedId, BINARY_ENV};

const SHADOW_SOURCE: &str = "onepipeline-writeback";
// Cross-platform runners have measured real sibling commands taking longer than ten seconds
// under suite-wide contention. This remains a backstop for an unreachable store, not a
// latency target: projection stays off the reconcile loop while the child runs.
const COMMAND_LIMIT: Duration = Duration::from_secs(60);
const RETRY_AFTER: Duration = Duration::from_millis(250);
// Closeout never inherits the duration of a store command. A slow store may keep working in
// the worker, but it still cannot turn a completed graph into run settlement.
const CLOSEOUT_WAIT: Duration = Duration::from_millis(2_250);

#[derive(Clone, PartialEq)]
struct Snapshot {
    project: QualifiedId,
    dir: PathBuf,
    nodes: BTreeMap<String, Node>,
    statuses: BTreeMap<String, NodeStatus>,
    settlements: BTreeMap<String, Value>,
    project_metadata: BTreeMap<String, Value>,
}

#[derive(Default, PartialEq, Eq)]
enum WorkerState {
    #[default]
    Idle,
    Working,
    StopRequested,
}

#[derive(Default)]
struct Pending {
    latest: Option<Snapshot>,
    last_success: Option<Snapshot>,
    worker: WorkerState,
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

    /// Give the active worker a bounded closeout window for the terminal snapshot.
    pub fn wait_briefly(&self) {
        // Let one already-running real copy reach its own deadline before the process
        // exits. This keeps a completed run from racing a person's next store command,
        // while the hard command limit preserves write-back's latency boundary.
        let deadline = Instant::now() + CLOSEOUT_WAIT;
        let (lock, ready) = &*self.pending;
        let Ok(mut pending) = lock.lock() else { return };
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
            pending.worker = WorkerState::StopRequested;
            ready.notify_one();
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
    let mut failing = false;
    loop {
        let snapshot = {
            let (lock, ready) = &*pending;
            let mut state = match lock.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            while state.latest.is_none() && state.worker != WorkerState::StopRequested {
                state = match ready.wait(state) {
                    Ok(state) => state,
                    Err(_) => return,
                };
            }
            if state.worker == WorkerState::StopRequested && state.latest.is_none() {
                return;
            }
            if state.worker == WorkerState::Idle {
                state.worker = WorkerState::Working;
            }
            state
                .latest
                .take()
                .expect("the worker was woken by a snapshot")
        };
        match project(&binary, &launch_dir, &run_dir, &snapshot) {
            Ok(()) => {
                if failing {
                    eprintln!(
                        "onetaskgraph write-back recovered for '{}'",
                        snapshot.project
                    );
                }
                failing = false;
                let (lock, _) = &*pending;
                if let Ok(mut state) = lock.lock() {
                    state.last_success = Some(snapshot.clone());
                    if state.worker == WorkerState::StopRequested && state.latest.is_none() {
                        return;
                    }
                }
            }
            Err(error) => {
                if !failing {
                    eprintln!(
                        "onetaskgraph write-back failed for '{}': {error}; retrying",
                        snapshot.project
                    );
                }
                failing = true;
                std::thread::sleep(RETRY_AFTER);
                let (lock, ready) = &*pending;
                let Ok(mut state) = lock.lock() else { return };
                if state.worker == WorkerState::StopRequested {
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
            if state.worker == WorkerState::Working {
                state.worker = WorkerState::Idle;
            }
            ready.notify_all();
        }
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
    let _ = (response.next, response.plan);
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
        let _ = response.plan;
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskPage {
    items: Vec<DestinationTask>,
    next: Option<String>,
    plan: Value,
    errors: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectPage {
    items: Vec<DestinationProject>,
    next: Option<String>,
    plan: Value,
    errors: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DestinationProject {
    id: QualifiedId,
    item: DestinationProjectItem,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DestinationProjectItem {
    /// Not declared by a plan, so it is preserved rather than replaced.
    title: String,
    /// Not declared by a plan, so it is preserved rather than replaced.
    content: Option<String>,
    /// Not modelled by a plan at all, so they are preserved rather than dropped.
    labels: Vec<DestinationLabel>,
    /// Only the reserved keys this worker owns are rewritten; the rest are preserved.
    metadata: BTreeMap<String, Value>,
    // llmlint: ignore-block[invalid_states_unrepresentable] These fields enumerate the
    // compiled sibling's complete, deny-unknown machine response but are not inputs this
    // projection interprets. onetaskgraph owns and validates its native id, URL, timestamps,
    // and repository identities, and a project's status is not a plan's to state.
    #[serde(rename = "id")]
    _id: String,
    #[serde(rename = "status")]
    _status: DestinationStatus,
    #[serde(rename = "url")]
    _url: Option<String>,
    #[serde(rename = "created_at")]
    _created_at: Option<String>,
    #[serde(rename = "updated_at")]
    _updated_at: Option<String>,
    #[serde(rename = "repositories")]
    _repositories: Vec<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DestinationTask {
    id: QualifiedId,
    item: DestinationTaskItem,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DestinationTaskItem {
    /// Not modelled by a plan at all, so they are preserved rather than dropped.
    labels: Vec<DestinationLabel>,
    /// Read for `onepipeline.id`, which is how a destination task names its plan node.
    metadata: BTreeMap<String, Value>,
    // llmlint: ignore-block[invalid_states_unrepresentable] A task's title, body, status,
    // project and repositories are declared by the plan, so the projection replaces them
    // and never reads the destination's; the remaining fields are onetaskgraph's own. They
    // are enumerated because the sibling's machine response denies unknown fields.
    #[serde(rename = "id")]
    _id: String,
    #[serde(rename = "title")]
    _title: String,
    #[serde(rename = "content")]
    _content: Option<String>,
    #[serde(rename = "status")]
    _status: DestinationStatus,
    #[serde(rename = "project")]
    _project: Option<String>,
    #[serde(rename = "url")]
    _url: Option<String>,
    #[serde(rename = "created_at")]
    _created_at: Option<String>,
    #[serde(rename = "updated_at")]
    _updated_at: Option<String>,
    #[serde(rename = "repositories")]
    _repositories: Vec<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DestinationStatus {
    #[serde(rename = "category")]
    _category: DestinationStatusCategory,
    #[serde(rename = "name")]
    _name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum DestinationStatusCategory {
    Backlog,
    Todo,
    InProgress,
    Done,
    Cancelled,
    Unknown,
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
#[serde(deny_unknown_fields)]
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
        front.insert("status".into(), json!(category(status)));
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

#[derive(Serialize)]
enum TaskCategory {
    #[serde(rename = "in progress")]
    InProgress,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "todo")]
    Todo,
}

fn category(status: NodeStatus) -> TaskCategory {
    match status {
        NodeStatus::Running => TaskCategory::InProgress,
        NodeStatus::Done | NodeStatus::Failed => TaskCategory::Done,
        NodeStatus::Parked | NodeStatus::Cancelled | NodeStatus::Skipped => TaskCategory::Cancelled,
        NodeStatus::Pending | NodeStatus::Ready | NodeStatus::Waiting | NodeStatus::Blocked => {
            TaskCategory::Todo
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
        write_shadow, DestinationProjectItem, Origin, Pending, Snapshot, TaskCategory, WorkerState,
    };
    use crate::graph::NodeStatus;
    use crate::plan::Node;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn snapshot(status: NodeStatus) -> Snapshot {
        Snapshot {
            project: "plans:deduplication".parse().expect("a qualified project"),
            dir: PathBuf::from("writeback"),
            nodes: BTreeMap::new(),
            statuses: BTreeMap::from([("node".to_owned(), status)]),
            settlements: BTreeMap::new(),
            project_metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn returning_to_the_last_success_supersedes_a_different_pending_snapshot() {
        let first = snapshot(NodeStatus::Pending);
        let superseded = snapshot(NodeStatus::Running);
        let mut pending = Pending {
            latest: Some(superseded),
            last_success: Some(first.clone()),
            worker: WorkerState::Working,
        };

        assert!(pending.queue(first.clone()));
        assert!(pending.latest.as_ref() == Some(&first));
    }

    #[test]
    fn task_categories_remain_named_by_the_approved_contract() {
        let contract = include_str!("../docs/contract.md");
        for category in [
            TaskCategory::Todo,
            TaskCategory::InProgress,
            TaskCategory::Done,
            TaskCategory::Cancelled,
        ] {
            let native = serde_json::to_value(category)
                .expect("a task category serializes")
                .as_str()
                .expect("a task category is a string")
                .replace(' ', "-");
            assert!(
                contract.contains(&format!("`{native}`")),
                "docs/contract.md no longer names the projected status category `{native}`"
            );
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

    /// The destination project a projection is written against.
    ///
    /// Built out of the sibling's own machine response rather than by naming fields, so
    /// the shape this projection reads is the shape `project show --json` answers in.
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
            "metadata": {"authored.note": "keep this value"},
            "repositories": [],
        }))
        .expect("the sibling's own project response")
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

    fn projection(dir: &Path, status: NodeStatus) -> Snapshot {
        let node: Node = serde_json::from_value(json!({
            "id": "build",
            "title": "feat: build it",
            "task": "## What\nBuild it.",
            "persona": "engineer",
            "deps": ["design"],
        }))
        .expect("a plan node");
        Snapshot {
            project: "plans:board".parse().expect("a qualified project"),
            dir: dir.to_path_buf(),
            nodes: BTreeMap::from([("build".to_owned(), node)]),
            statuses: BTreeMap::from([("build".to_owned(), status)]),
            settlements: BTreeMap::new(),
            project_metadata: BTreeMap::from([("onepipeline.concurrency".into(), json!(4))]),
        }
    }

    fn documents(dir: &Path) -> (PathBuf, PathBuf) {
        let project = super::project_file(&"plans:board".parse().expect("a qualified project"));
        (
            dir.join("projects").join(format!("{project}.md")),
            dir.join("tasks")
                .join(&project)
                .join(format!("{}.md", super::task_file("build"))),
        )
    }

    /// The rule's second consequence: a project's title is not a plan's to state, so the
    /// destination's own title is what the projection writes back.
    ///
    /// A store whose native identifier *is* its title cannot tell this apart from writing
    /// the identifier, so the destination here is one where the two differ.
    #[test]
    fn a_projection_writes_back_the_destination_projects_own_title() {
        let dir = scratch("project-title");
        let snapshot = projection(&dir, NodeStatus::Done);
        write_shadow(
            &snapshot,
            &BTreeMap::new(),
            &destination("A person's own board", &[]),
        )
        .expect("the shadow project is written");

        let (front, body) = written(&documents(&dir).0);
        assert_eq!(
            front["title"], "A person's own board",
            "the projection renamed the destination project"
        );
        assert_ne!(
            front["title"],
            json!(snapshot.project.native()),
            "the projection wrote the project's native identifier as its title"
        );
        assert_eq!(
            body, "A person's own description.",
            "the projection replaced the destination's description"
        );
        assert_eq!(
            front["metadata"]["authored.note"], "keep this value",
            "the projection dropped metadata no plan declares"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rule's fourth consequence, for a project: labels are not modelled by a plan, so
    /// the destination's own are carried through whole rather than dropped.
    #[test]
    fn a_projection_carries_the_destination_projects_labels_through_whole() {
        let dir = scratch("project-labels");
        write_shadow(
            &projection(&dir, NodeStatus::Done),
            &BTreeMap::new(),
            &destination("A person's own board", &["planning", "q3"]),
        )
        .expect("the shadow project is written");

        let (front, _) = written(&documents(&dir).0);
        assert_eq!(
            front["labels"],
            json!([
                {"id": "planning", "name": "planning", "color": null},
                {"id": "q3", "name": "q3", "color": null},
            ]),
            "the projection dropped the destination project's labels"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same consequence for a task, read off the destination task the node projects
    /// onto — and nothing at all for a node the destination has no task for yet.
    #[test]
    fn a_projection_carries_a_destination_tasks_labels_through_and_invents_none() {
        let dir = scratch("task-labels");
        write_shadow(
            &projection(&dir, NodeStatus::Done),
            &BTreeMap::from([(
                "build".to_owned(),
                Origin {
                    id: "plans:board/002-build".parse().expect("a qualified task"),
                    labels: serde_json::from_value(json!([
                        {"id": "needs-review", "name": "needs-review", "color": "d73a4a"}
                    ]))
                    .expect("the sibling's own labels"),
                },
            )]),
            &destination("A person's own board", &[]),
        )
        .expect("the shadow project is written");
        let (front, _) = written(&documents(&dir).1);
        assert_eq!(
            front["labels"],
            json!([{"id": "needs-review", "name": "needs-review", "color": "d73a4a"}]),
            "the projection dropped the destination task's labels"
        );
        assert_eq!(
            front["metadata"]["onetaskgraph.origin"], "plans:board/002-build",
            "the projection lost the destination task it writes onto"
        );

        write_shadow(
            &projection(&dir, NodeStatus::Done),
            &BTreeMap::new(),
            &destination("A person's own board", &[]),
        )
        .expect("the shadow project is written for a node with no destination task");
        let (front, _) = written(&documents(&dir).1);
        assert_eq!(
            front["labels"],
            json!([]),
            "the projection invented labels for a task the destination does not hold"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rule's first consequence: everything a plan *does* declare is replaced, which is
    /// what the projection is for. Held beside the preservation tests so neither can be
    /// satisfied by writing less.
    #[test]
    fn a_projection_replaces_every_field_the_plan_declares() {
        let dir = scratch("declared");
        write_shadow(
            &projection(&dir, NodeStatus::Running),
            &BTreeMap::new(),
            &destination("A person's own board", &[]),
        )
        .expect("the shadow project is written");

        let (front, body) = written(&documents(&dir).1);
        assert_eq!(front["title"], "feat: build it");
        assert_eq!(body, "## What\nBuild it.");
        assert_eq!(front["status"], "in progress");
        assert_eq!(
            front["depends_on"],
            json!([super::task_file("design")]),
            "the projection lost the plan's dependency edge"
        );
        assert_eq!(front["metadata"]["onepipeline.id"], "build");
        assert_eq!(front["metadata"]["onepipeline.persona"], "engineer");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
