//! Best-effort projection of the journal-owned graph into its onetaskgraph project.
//!
//! The reconcile loop remains the only author of graph state: it hands immutable folded
//! snapshots to this worker, and the worker only projects them. Store reads never feed back
//! into scheduling, and a failed or slow write is reported and retried off the engine thread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::edits::Operation;
use crate::event::Source;
use crate::graph::NodeStatus;
use crate::ledger::{LaunchRecord, RunPaths};
use crate::plan::Node;
use crate::projection::RunState;
use crate::taskgraph::{QualifiedId, BINARY_ENV};

const SHADOW_SOURCE: &str = "onepipeline-writeback";
const COMMAND_LIMIT: Duration = Duration::from_secs(2);
const RETRY_AFTER: Duration = Duration::from_millis(250);

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
        let _: QualifiedId = launch.project.parse().ok()?;
        let pending = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker_pending = Arc::clone(&pending);
        let run_dir = paths.dir.clone();
        let launch_dir = if launch.dir.as_os_str().is_empty() {
            std::env::current_dir().ok()?
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
    pub fn wait_briefly(&self) {
        let deadline = Instant::now() + Duration::from_millis(500);
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
    _run_dir: &Path,
    snapshot: &Snapshot,
) -> Result<(), String> {
    write_shadow(snapshot)?;
    let root = snapshot.dir.to_string_lossy().into_owned();
    let shadow_project = format!("{SHADOW_SOURCE}:{}", project_file(&snapshot.project));
    let mut child = Command::new(binary)
        .current_dir(launch_dir)
        .args([
            "project",
            "copy",
            &shadow_project,
            "--to",
            snapshot.project.source(),
            "--match-by",
            "onepipeline.id",
            "--json",
            "--set",
            &format!("sources.{SHADOW_SOURCE}.plugin=local-md"),
            "--set",
            &format!("sources.{SHADOW_SOURCE}.config.root={root}"),
        ])
        .env_remove(BINARY_ENV)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run {}: {error}", binary.display()))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|error| error.to_string())?;
                return Err(format!(
                    "copy exited {}: {}",
                    status
                        .code()
                        .map_or_else(|| "on a signal".into(), |c| c.to_string()),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Ok(None) if started.elapsed() < COMMAND_LIMIT => {
                std::thread::sleep(Duration::from_millis(25))
            }
            // llmlint: ignore-block[changed_behavior_has_e2e] The real local-md outage
            // journey proves failed copies are bounded, reported, retried, and cannot alter
            // execution. Reaching this exact time limit would require a wrapper that hangs
            // in place of the real onetaskgraph binary, which would mock the boundary the
            // acceptance journey is required to drive for real.
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("copy exceeded {} seconds", COMMAND_LIMIT.as_secs()));
            }
            // llmlint: ignore-end[changed_behavior_has_e2e]
            Err(error) => return Err(format!("cannot wait for copy: {error}")),
        }
    }
}

fn write_shadow(snapshot: &Snapshot) -> Result<(), String> {
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

fn category(status: NodeStatus) -> &'static str {
    match status {
        NodeStatus::Running => "in progress",
        NodeStatus::Done | NodeStatus::Failed => "done",
        NodeStatus::Parked | NodeStatus::Cancelled | NodeStatus::Skipped => "cancelled",
        NodeStatus::Pending | NodeStatus::Ready | NodeStatus::Waiting | NodeStatus::Blocked => {
            "todo"
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
