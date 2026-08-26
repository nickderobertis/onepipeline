//! Best-effort projection of the journal-owned graph into its onetaskgraph project.
//!
//! The reconcile loop remains the only author of graph state: it hands immutable folded
//! snapshots to this worker, and the worker only projects them. Store reads never feed back
//! into scheduling, and a failed or slow write is reported and retried off the engine thread.

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

#[derive(Default)]
struct Pending {
    latest: Option<Snapshot>,
    last_success: Option<Snapshot>,
    working: bool,
    stopped: bool,
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
            if pending.latest.as_ref() == Some(&snapshot)
                || pending.last_success.as_ref() == Some(&snapshot)
            {
                return;
            }
            pending.latest = Some(snapshot);
            ready.notify_one();
        }
    }

    /// Give the active worker a bounded closeout window for the terminal snapshot.
    // llmlint: ignore-block[changed_behavior_has_e2e] The real outage journey below this
    // boundary proves terminal settlement completes while the store is absent and recovery can
    // project it during this window. The remaining expiry behavior is solely
    // `Condvar::wait_timeout` returning at the private constant; holding a journey open past the
    // same timer would re-test the standard-library clock rather than another product boundary.
    pub fn wait_briefly(&self) {
        // Let one already-running real copy reach its own deadline before the process
        // exits. This keeps a completed run from racing a person's next store command,
        // while the hard command limit preserves write-back's latency boundary.
        let deadline = Instant::now() + CLOSEOUT_WAIT;
        let (lock, ready) = &*self.pending;
        let Ok(mut pending) = lock.lock() else { return };
        while (pending.latest.is_some() || pending.working) && Instant::now() < deadline {
            let wait = deadline.saturating_duration_since(Instant::now());
            let Ok((next, _)) = ready.wait_timeout(pending, wait) else {
                return;
            };
            pending = next;
        }
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
}

impl Drop for Writeback {
    fn drop(&mut self) {
        let (lock, ready) = &*self.pending;
        if let Ok(mut pending) = lock.lock() {
            pending.stopped = true;
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
            while state.latest.is_none() && !state.stopped {
                state = match ready.wait(state) {
                    Ok(state) => state,
                    Err(_) => return,
                };
            }
            if state.stopped && state.latest.is_none() {
                return;
            }
            state.working = true;
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
                    if state.stopped && state.latest.is_none() {
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
                if state.stopped {
                    return;
                }
                if state.latest.is_none() && !state.stopped {
                    state.latest = Some(snapshot);
                    ready.notify_one();
                }
            }
        }
        let (lock, ready) = &*pending;
        if let Ok(mut state) = lock.lock() {
            state.working = false;
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
    let origins = destination_origins(binary, launch_dir, run_dir, snapshot)?;
    // llmlint: ignore-block[changed_behavior_has_e2e] The real outage journey drives
    // destination write failure through onetaskgraph. Making this private, run-owned
    // shadow directory unwritable would instead require sabotaging the host filesystem,
    // outside the public run interface and unrelated to store availability.
    write_shadow(snapshot, &origins)?;
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

fn destination_origins(
    binary: &Path,
    launch_dir: &Path,
    run_dir: &Path,
    snapshot: &Snapshot,
) -> Result<BTreeMap<String, String>, String> {
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
            if origins
                .insert(node.to_owned(), task.id.as_str().to_owned())
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
struct DestinationTask {
    id: QualifiedId,
    item: DestinationTaskItem,
}

// llmlint: ignore-block[invalid_states_unrepresentable] This projection consumes only the
// qualified id and metadata below. The remaining required fields prove that the sibling
// returned its complete task wire shape, but constraining values that never enter graph state
// would duplicate onetaskgraph's contract and make a harmless representation change break
// best-effort write-back.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DestinationTaskItem {
    #[serde(rename = "id")]
    _id: Value,
    #[serde(rename = "title")]
    _title: Value,
    #[serde(rename = "content")]
    _content: Value,
    #[serde(rename = "status")]
    _status: Value,
    #[serde(rename = "labels")]
    _labels: Value,
    #[serde(rename = "project")]
    _project: Value,
    #[serde(rename = "url")]
    _url: Value,
    #[serde(rename = "created_at")]
    _created_at: Value,
    #[serde(rename = "updated_at")]
    _updated_at: Value,
    metadata: BTreeMap<String, Value>,
    #[serde(rename = "repositories")]
    _repositories: Value,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

fn write_shadow(snapshot: &Snapshot, origins: &BTreeMap<String, String>) -> Result<(), String> {
    let projects = snapshot.dir.join("projects");
    let tasks = snapshot
        .dir
        .join("tasks")
        .join(project_file(&snapshot.project));
    std::fs::create_dir_all(&projects).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&tasks).map_err(|e| e.to_string())?;
    let mut project_metadata = snapshot.project_metadata.clone();
    project_metadata.insert(
        "onetaskgraph.origin".into(),
        json!(snapshot.project.as_str()),
    );
    document(
        &projects.join(format!("{}.md", project_file(&snapshot.project))),
        &json!({
            "title": snapshot.project.native(), "metadata": project_metadata
        }),
        "",
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
        if let Some(origin) = origins.get(id) {
            metadata.insert("onetaskgraph.origin".into(), json!(origin));
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
    use super::TaskCategory;

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
}
