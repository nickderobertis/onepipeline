//! The journal workload: the release-built driver adopting a synthesized run at each
//! journal size of 1, 20, 80, 160 and 800 MB, recorded for the five budgets
//! `budgets.yaml` registers.
//!
//! Its own binary and its own Nx project (`tests/journal_workload/project.json`), run
//! only by that project's `workload` target (`just workload`), so no per-change
//! selection pays for an 800 MB journal; its `check` lints it and stops there.
//!
//! Everything a measured driver meets is the repository's own: the e2e harness's
//! [`World`], the real `oneagentgraph` linked into the build, and `crates/testfakes`'
//! `fake-claude` standing in for the members' model turns alone. The generator and
//! every run it writes are synthesized from a seed; nothing comes from a real journal.

mod generate;
#[path = "../e2e/harness.rs"]
mod harness;
mod measure;

use std::path::{Path, PathBuf};

use harness::{World, MONITOR_TASK};
use serde_json::{json, Value};

/// The run every generated directory holds.
pub const RUN: &str = "journal-workload-run";

/// The journal sizes measured, in MB of 1,000,000 bytes.
const SIZES_MB: [u64; 5] = [1, 20, 80, 160, 800];

const SEED: u64 = 20_261_019;

/// The release-built driver: `JOURNAL_WORKLOAD_BINARY`, which `just
/// _journal-workload-run` sets to the `onepipeline` it has just built `--release`.
fn binary() -> PathBuf {
    let binary = std::env::var_os("JOURNAL_WORKLOAD_BINARY")
        .map(PathBuf::from)
        .expect("JOURNAL_WORKLOAD_BINARY names the release-built driver; run `just workload`");
    assert!(binary.is_file(), "{} is not a file", binary.display());
    binary
}

/// Where the size's world is rooted: fixed per size, because a generated run names its
/// world's paths, so one root, one seed and one size write one set of bytes.
fn root(size_mb: u64) -> PathBuf {
    std::env::temp_dir().join(format!("journal-workload-{size_mb}mb"))
}

/// The observer graph: two observing members, as this host's monitor and check-in.
fn observer_graph(world: &World) -> PathBuf {
    let mut members = String::new();
    for member in ["monitor", "check-in"] {
        members.push_str(&format!(
            "  {member}:\n    kind: oneharness\n    oneharness_config: {}\n    task: {}\n",
            world.harness_config(member),
            serde_json::to_string(MONITOR_TASK).unwrap()
        ));
    }
    let graph = world.graphs().join("dag-scope.yaml");
    std::fs::write(
        &graph,
        format!(
            "version: {}\nname: dag-scope\nmembers:\n{members}\
             env:\n  ONEPIPELINE_RUN_ID: ${{ONEPIPELINE_RUN_ID}}\n  \
             ONEPIPELINE_RUNS_DIR: ${{ONEPIPELINE_RUNS_DIR}}\n",
            oneagentgraph::config::FIRST_TASK_TOKEN_VERSION
        ),
    )
    .unwrap();
    graph
}

/// A world at `root` with the graphs, the observer and the plan's project in its store,
/// and the paths a generated run's launch record names.
pub fn world_at(root: &Path, seed: u64) -> (World, generate::Paths, Value) {
    let world = World::at(root, "journal-workload")
        // A held turn outlasts every phase of a measurement: the double holds at most an hour.
        .with_env(harness::RENDEZVOUS_SECONDS_ENV, "3600")
        .with_env("ONEPIPELINE_LOOP_STATS", "1")
        .with_env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "3600");
    world.write_graphs();
    let dag_graph = observer_graph(&world);
    let mut rng = generate::Rng::new(seed);
    let plan = generate::plan(&mut rng, RUN);
    let store_project = world.plan(RUN, &plan);
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|host| host.trim().to_owned())
        .expect("the kernel names this host");
    assert!(!host.is_empty(), "the kernel names this host as nothing");
    let paths = generate::Paths {
        runs: world.runs.clone(),
        project: world.project.clone(),
        dag_graph,
        node_graph: world.graphs().join("node-scope.yaml"),
        store_project,
        session: world.session.clone(),
        host,
    };
    (world, paths, plan)
}

/// Every file of a directory, by its path inside it, with its bytes' sha256.
fn digests(dir: &Path) -> std::collections::BTreeMap<String, String> {
    use sha2::Digest;
    files_under(dir)
        .into_iter()
        .map(|path| {
            let name = path.strip_prefix(dir).unwrap().display().to_string();
            let digest = sha2::Sha256::digest(std::fs::read(&path).unwrap());
            (name, format!("{digest:x}"))
        })
        .collect()
}

/// The checkpoint a run onepipeline v0.75.0 drove leaves: `tests/golden/checkpoint-v13.json`
/// is that schema's checked-in shape, and the one source of it here.
const CHECKPOINT_V13: &str = include_str!("../golden/checkpoint-v13.json");

/// Leave the run the schema-13 `checkpoint.json` v0.75.0 would have, folded over its
/// generated journal.
///
/// The fold is the build under test's own: its `status` folds the journal and writes its
/// checkpoint at its own schema. That document is then cut to the keys the schema-13
/// golden holds, at the top and in its state, and relabelled 13, so it holds no key that
/// schema lacks. Whatever else `status` left
/// in the run's directory is removed, so the run is in the shape that release leaves.
pub fn checkpoint(world: &World, binary: &Path) {
    let dir = world.run_file(RUN, "");
    let before: std::collections::BTreeSet<PathBuf> = files_under(&dir).into_iter().collect();
    let output = world
        .agentgraph_cmd_on(binary, &["status", RUN])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "status: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = world.run_file(RUN, "checkpoint.json");
    let written: Value = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|error| panic!("status wrote no checkpoint: {error}")),
    )
    .expect("the build's checkpoint is JSON");
    for file in files_under(&dir) {
        if file != path && !before.contains(&file) {
            std::fs::remove_file(&file).unwrap();
        }
    }
    let golden: Value = serde_json::from_str(CHECKPOINT_V13).unwrap();
    let keys = |doc: &Value| -> std::collections::BTreeSet<String> {
        doc.as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect()
    };
    // A key the build left out is one it holds empty, which every schema leaves out.
    let cut = |doc: &Value,
               keep: &std::collections::BTreeSet<String>|
     -> serde_json::Map<String, Value> {
        doc.as_object()
            .expect("an object")
            .iter()
            .filter(|(key, _)| keep.contains(*key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    };
    let mut frozen = cut(&written, &keys(&golden));
    frozen.insert(
        "state".into(),
        Value::Object(cut(&written["state"], &keys(&golden["state"]))),
    );
    frozen.insert("schema_version".into(), golden["schema_version"].clone());
    let frozen = Value::Object(frozen);
    assert!(keys(&frozen).is_subset(&keys(&golden)));
    assert!(keys(&frozen["state"]).is_subset(&keys(&golden["state"])));
    std::fs::write(&path, serde_json::to_vec(&frozen).unwrap()).unwrap();
}

/// Every file under `dir`, at any depth.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// One size twice from one seed into one world writes byte-identical files: the
/// generator is a pure function of its seed and the world's paths, so a size measured
/// twice at one root on one host is the same run both times.
#[test]
fn one_seed_generates_one_run() {
    let root = root(1);
    let mut runs = Vec::new();
    for _ in 0..2 {
        let (world, paths, plan) = world_at(&root, SEED);
        let generated = generate::generate(&paths, RUN, &plan, 1_000_000, SEED);
        runs.push(digests(&generated.dir));
        drop(world);
    }
    assert!(runs[0].contains_key("events.jsonl"), "{:#?}", runs[0]);
    assert_eq!(runs[0], runs[1], "one seed wrote two different runs");
}

/// The journey: every size measured in turn, one at a time, and the recording the five
/// budgets read written to `target/budget-records/journal-workload.json`.
#[test]
fn journal_workload() {
    let binary = binary();
    // `JOURNAL_WORKLOAD_SIZES` (`1,20`) measures only those of the workload's sizes, for
    // a trial, and its recording goes beside the one the budgets read rather than over it.
    let trial: Option<Vec<u64>> = std::env::var("JOURNAL_WORKLOAD_SIZES").ok().map(|sizes| {
        sizes
            .split(',')
            .map(|size| {
                let size_mb = size.trim().parse().expect("a size in whole MB");
                assert!(
                    SIZES_MB.contains(&size_mb),
                    "{size_mb} MB is not one of the workload's sizes, {SIZES_MB:?}"
                );
                size_mb
            })
            .collect()
    });
    let mut sizes = Vec::new();
    for size_mb in trial.clone().unwrap_or(SIZES_MB.to_vec()) {
        let record = measure::measure(&binary, &root(size_mb), size_mb, SEED);
        println!("journal-workload {size_mb} MB: {record}");
        sizes.push(record);
    }
    let rev = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .current_dir(harness::repo_file("."))
        .output()
        .expect("git runs");
    let commit = String::from_utf8_lossy(&rev.stdout).trim().to_owned();
    assert!(
        rev.status.success() && commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "git named no commit: {}",
        String::from_utf8_lossy(&rev.stderr)
    );
    let recording = json!({
        "commit": commit,
        "seed": SEED,
        "sizes": sizes,
    });
    let out = harness::repo_file(if trial.is_some() {
        "target/budget-records/journal-workload-trial.json"
    } else {
        "target/budget-records/journal-workload.json"
    });
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, serde_json::to_string_pretty(&recording).unwrap()).unwrap();
}
