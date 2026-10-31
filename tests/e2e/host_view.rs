//! `onepipeline host` over a runs root shaped like a real host's.
//!
//! The view answers *what is running on this host right now*, and it is asked by
//! people and by scripts alike, so these journeys hold two things at once: that
//! every live dispatch is still proved — by its registry entry's pid and start
//! token — and that a run the view can establish has ended costs it close to
//! nothing however large its journal. The second is observed rather than
//! inferred: every ended run's journal is made unreadable to the view's process,
//! and the kernel is asked (through `strace`) which journals the process opened.
//!
//! The roots are written by `tests/e2e/host_view_fixture.mjs`, the one generator
//! these journeys and the `host-view-time` budget share: the budget's
//! `scripts/host-view-measure.mjs` times the release binary CI's `install` job
//! builds over the same root, and these journeys read it with this suite's own
//! build, under `strace`.
//!
//! Linux only: the live dispatches are real processes whose registry entries
//! carry their procfs start tokens, and the observation is `strace`'s.

// llmlint: ignore-file[e2e_not_mocked] the runs root is generated rather than recorded by a
// driver, because what is under test is the view's read of a root a host accumulates over
// weeks, which no journey can live through. Every byte of it is in shapes the engine itself
// writes — the journals are a recorded run's own records — and the view under test is the
// real compiled binary, read the way an operator reads it.

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] measured: the module's ten
// journeys take about five minutes on a loaded host (290 s at load1 about 17), nearly all of it three cold reads of the
// generated 100-run root's ~260 MB of journals by this suite's debug build — one by the
// journey over the root, two by the budget measurement's own journey (168 s run alone at load1
// about 17), which runs `scripts/host-view-measure.mjs` whole, once to its record and once to
// a refused trace. They exercise
// `hostview`, `checkpoint`, `summary`, `views`, `sys` and `freespace` through the compiled
// binary, and read the recorded run under `tests/recorded/` through the generator, so any
// change under `src/` can move what they hold; the narrowest edge they can honestly sit behind
// is the crate's, which is this target's. The declaration in `main.rs` says the same.

#![cfg(target_os = "linux")]

use onepipeline_testfakes::profile::process::{Child, Command, Stdio};
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Value};

use crate::harness::{now_millis, World};

/// The generator every journey here and `scripts/host-view-measure.mjs` share.
const GENERATOR: &str = "tests/e2e/host_view_fixture.mjs";
/// The host name every generated record and every read carries, as the generator
/// writes it (its `HOST`), so a launch record's driver and a registry entry's pid
/// are this host's to prove.
const HOST: &str = "host-view-fixture";
const MIB: u64 = 1 << 20;
/// The variable asking `onepipeline host` to append its phase breakdown to a file.
const PHASES_ENV: &str = "ONEPIPELINE_HOST_PHASES";
/// The phases a steady read over a fixture records, by the names `src/hostview.rs`,
/// `src/lib.rs` and `src/driver.rs` give them, and the quantity it counts: held
/// here so a phase renamed there fails these journeys rather than dropping out of
/// the readings.
const STEADY_PHASES: [&str; 9] = [
    "event-registry",
    "list",
    "launch-and-summary",
    "registry",
    "pid-proofs",
    "decide-ended",
    "live-fold",
    "free-space",
    "render",
];
/// Where the journey over the 100-run fixture leaves this build's readings: the
/// gate's evidence beside the budget, which `scripts/host-view-measure.mjs` takes
/// of the release binary in CI's `install` job.
const DEBUG_RECORD: &str = "target/budget-records/host-view-debug.json";

/// A process's start token as this host's procfs spells it, the way the engine
/// records one — asked of `/proc` here rather than of the crate under test.
fn start_token(pid: u32) -> String {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("a live process");
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .expect("a stat line")
        .1
        .split_whitespace()
        .collect();
    format!("linux-proc-stat:{}", fields[19])
}

/// Where each read's verdict for a run is expected to land.
#[derive(Default)]
struct Expected {
    /// Every `(run, node)` a held row.
    held: BTreeSet<(String, String)>,
    /// Every `(run, node)` an `UNPROVEN` row.
    unproven: BTreeSet<(String, String)>,
    /// Every `(run, node)` gone because its registry entry is stale.
    stale: BTreeSet<(String, String)>,
    /// Every `(run, node)` gone because its registry-less run was stopped.
    stopped: BTreeSet<(String, String)>,
    /// Every run root refused, by name.
    skipped: BTreeSet<String>,
}

/// One generated runs root, the manifest the generator answered for it, and the
/// processes its live dispatches are.
struct Fixture {
    runs: PathBuf,
    expected: Expected,
    /// The runs that hold a live dispatch, whose journals the view folds.
    live_runs: BTreeSet<String>,
    /// Every run whose journal a steady read may not open.
    ended: Vec<String>,
    sparse_run: String,
    /// The generator's manifest, as a file its next verb reads.
    manifest: PathBuf,
    children: Vec<Child>,
}

/// The generated runs go with the fixture, before the world that holds them is
/// dropped: nothing drives them, so the world's teardown — a `stop` of every run it
/// finds, which folds each one — has nothing of theirs to stop.
impl Drop for Fixture {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.runs);
    }
}

/// One verb of the generator, run in `world`'s environment — which names its
/// runs root and this host's name — answering what it printed.
fn generator(world: &World, args: &[&str]) -> String {
    let script = crate::harness::repo_file(GENERATOR);
    let mut argv = vec![script.to_str().expect("a UTF-8 path")];
    argv.extend_from_slice(args);
    let output = world
        .cmd_on(Path::new("node"), &argv)
        .output()
        .expect("node runs the fixture generator");
    assert!(
        output.status.success(),
        "`{GENERATOR} {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8")
}

/// Dead fixture entries share one reaped process; live entries still name the
/// caller's real processes. Observe execve so this holds the generator's cost,
/// and exercise a missing executable followed by recovery in the same process.
#[test]
fn the_generator_reaps_once_and_does_not_cache_a_failed_spawn() {
    let world = World::new("fixture-reaped-pid");
    let module = crate::harness::repo_file(GENERATOR);
    let script = r#"
        import {pathToFileURL} from 'node:url';
        const {reapedPid} = await import(pathToFileURL(process.argv[2]));
        console.log(JSON.stringify({node: process.execPath, pids: [reapedPid(), reapedPid(), reapedPid()]}));
    "#;
    let trace = world.root.join("generator.strace");
    let output = Command::new("strace")
        .args(["-f", "-e", "trace=execve", "-o"])
        .arg(&trace)
        .args(["node", "--input-type=module", "-e", script])
        .arg("fixture-probe")
        .arg(&module)
        .output()
        .expect("the generator runs under strace");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let held: Value = serde_json::from_slice(&output.stdout).expect("the reaped pids");
    let pids = held["pids"].as_array().expect("three pids");
    assert!(pids[0].as_u64().is_some_and(|pid| pid > 0));
    assert!(pids.iter().all(|pid| pid == &pids[0]), "{held}");
    let trace = std::fs::read_to_string(trace).expect("the exec trace");
    assert_eq!(
        trace
            .lines()
            .filter(|line| line.contains("/true\",") && line.ends_with("= 0"))
            .count(),
        1,
        "{trace}"
    );

    let recovery = r#"
        import {pathToFileURL} from 'node:url';
        import assert from 'node:assert/strict';
        const {reapedPid} = await import(pathToFileURL(process.argv[2]));
        const path = process.env.PATH;
        process.env.PATH = '';
        assert.throws(() => reapedPid(), /could not reap/);
        process.env.PATH = path;
        assert.ok(reapedPid() > 0);
    "#;
    let recovered = Command::new(held["node"].as_str().expect("node's executable"))
        .args(["--input-type=module", "-e", recovery])
        .arg("fixture-probe")
        .arg(module)
        .output()
        .expect("the generator retries after PATH is restored");
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
}

/// Copies retain the CLI's records, but their live entries belong to the new
/// caller and their journals can be changed without touching the shared source.
#[test]
fn shared_fixtures_rebind_live_processes_and_keep_independent_journals() {
    let world = World::new("host-fixture-copies");
    let mut fixture = Fixture::empty(&world);
    let (old_pid, old_token) = fixture.sleeper();
    let (new_pid, new_token) = fixture.sleeper();
    let started = now_millis();
    for (run, nodes) in [
        ("live-0", vec!["plan", "build"]),
        ("live-1", vec!["plan", "review"]),
        ("live-2", vec!["plan"]),
    ] {
        let mut spec = json!({
            "run": run, "started_ms": started, "nodes": &nodes[1..],
            "registry": nodes.iter().map(|node| json!({"node": node, "pid": old_pid, "token": old_token, "recorded_ms": started})).collect::<Vec<_>>()
        });
        if run != "live-2" {
            spec["driver"] = json!({"pid": old_pid, "token": old_token});
        }
        fixture.run(&world, &spec);
    }
    fixture.run(
        &world,
        &json!({"run": "sparse", "started_ms": started, "settled": true, "registry": []}),
    );
    let sparse = fixture.runs.join("sparse/events.jsonl");
    let prefix = std::fs::metadata(&sparse).unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&sparse)
        .unwrap()
        .set_len(8 * 1024 * 1024)
        .unwrap();
    let cache = world.root.join("template");
    std::fs::create_dir(&cache).unwrap();
    std::fs::rename(&fixture.runs, cache.join("runs")).unwrap();
    write(
        &cache.join("manifest.json"),
        &json!({
            "expected": {"held": [], "unproven": [], "stale": [], "stopped": [], "skipped": []},
            "live_runs": ["live-0", "live-1", "live-2"], "ended": ["sparse"], "settled": ["sparse"],
            "sparse_run": "sparse", "sparse_record_bytes": prefix
        }),
    );
    let process = json!({"pid": new_pid, "token": new_token});
    let processes = world.root.join("processes.json");
    write(
        &processes,
        &json!({"drivers": [process, process], "entries": [[process, process], [process, process], [process]]}),
    );
    let copied = world.root.join("copy");
    generator(
        &world,
        &["clone", path(&copied), path(&processes), path(&cache)],
    );
    for run in ["live-0", "live-1", "live-2"] {
        let source = cache.join("runs").join(run);
        let target = copied.join(run);
        if run != "live-2" {
            assert_eq!(read_json(&source.join("launch.json"))["pid"], old_pid);
            assert_eq!(read_json(&target.join("launch.json"))["pid"], new_pid);
        } else {
            assert_eq!(
                read_json(&source.join("launch.json")),
                read_json(&target.join("launch.json"))
            );
        }
        let entries: Vec<_> = std::fs::read_dir(target.join("dispatches"))
            .unwrap()
            .collect();
        assert_eq!(entries.len(), if run == "live-2" { 1 } else { 2 });
        for entry in entries {
            let entry = read_json(&entry.unwrap().path());
            assert_eq!(entry["pid"], new_pid);
            assert_eq!(entry["started"], new_token);
        }
        assert_eq!(
            std::fs::read(source.join("events.jsonl")).unwrap(),
            std::fs::read(target.join("events.jsonl")).unwrap()
        );
    }
    assert_eq!(
        std::fs::metadata(copied.join("sparse/events.jsonl"))
            .unwrap()
            .len(),
        8 * 1024 * 1024
    );
    let source = cache.join("runs/live-0/events.jsonl");
    let bytes = std::fs::read(&source).unwrap();
    std::fs::write(copied.join("live-0/events.jsonl"), "the copy changes").unwrap();
    assert_eq!(
        std::fs::read(&source).unwrap(),
        bytes,
        "a clone cannot mutate its template"
    );

    let refused_clone = |name: &str| {
        Command::new("node")
            .arg(crate::harness::repo_file(GENERATOR))
            .arg("clone")
            .arg(world.root.join(name))
            .arg(&processes)
            .arg(&cache)
            .output()
            .unwrap()
    };
    let launch = cache.join("runs/live-0/launch.json");
    let original = read_json(&launch);
    let dead_launch = cache.join("runs/live-2/launch.json");
    let original_dead_launch = read_json(&dead_launch);
    let plan = cache.join("runs/live-2/plan.json");
    let original_plan = read_json(&plan);
    let dispatch = cache
        .join("runs/live-0/dispatches")
        .join(format!("{old_pid}-0.json"));
    let original_dispatch = read_json(&dispatch);
    let mut unknown = original.clone();
    unknown["typo"] = json!(true);
    let mut unknown_dead = original_dead_launch.clone();
    unknown_dead["typo"] = json!(true);
    let mut unknown_plan = original_plan.clone();
    unknown_plan["typo"] = json!(true);
    let mut wrong_type = original_dispatch.clone();
    wrong_type["pid"] = json!([]);
    let mut wrong_time = original_dispatch.clone();
    wrong_time["dispatched_at"] = json!("yesterday");
    for (nth, (file, bad, message)) in [
        (&launch, unknown, "cached launch has unknown field typo"),
        (
            &dead_launch,
            unknown_dead,
            "cached launch has unknown field typo",
        ),
        (&plan, unknown_plan, "cached plan has unknown field typo"),
        (
            &dispatch,
            wrong_type,
            "cached dispatch.pid has the wrong type",
        ),
        (
            &dispatch,
            wrong_time,
            "cached dispatch.dispatched_at must be a timestamp",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        write(file, &bad);
        let refused = refused_clone(&format!("bad-{nth}"));
        assert_eq!(refused.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
        write(&launch, &original);
        write(&dead_launch, &original_dead_launch);
        write(&plan, &original_plan);
        write(&dispatch, &original_dispatch);
    }
    let manifest = cache.join("manifest.json");
    let original_manifest = read_json(&manifest);
    let mut fixed = original.clone();
    fixed["host"] = json!("another host");
    let mut invalid_pid = original.clone();
    invalid_pid["pid"] = json!(-1);
    let mut fractional_pid = original.clone();
    fractional_pid["pid"] = json!(1.5);
    let mut empty_plan = original_plan.clone();
    empty_plan["tasks"] = json!([]);
    let mut non_array_plan = original_plan.clone();
    non_array_plan["tasks"] = json!(null);
    let mut invalid_task = read_json(&cache.join("runs/live-0/plan.json"));
    invalid_task["tasks"][1]["id"] = json!("../outside");
    let mut wrong_shape = original_manifest.clone();
    wrong_shape["live_runs"] = json!(["live-0", "live-1"]);
    let extra_plan = cache.join("runs/live-0/plan.json");
    for (nth, (file, bad, message)) in [
        (&launch, json!(null), "cached launch must be an object"),
        (&launch, json!([]), "cached launch must be an object"),
        (&launch, json!({}), "cached launch is missing"),
        (&launch, fixed, "cached launch.host differs"),
        (&launch, invalid_pid, "cached launch must be {pid, token}"),
        (
            &launch,
            fractional_pid,
            "cached launch must be {pid, token}",
        ),
        (
            &plan,
            empty_plan,
            "cached plan.tasks must be a nonempty array",
        ),
        (
            &plan,
            non_array_plan,
            "cached plan.tasks must be a nonempty array",
        ),
        (
            &extra_plan,
            invalid_task,
            "cached plan task.id must be a run name",
        ),
        (
            &manifest,
            wrong_shape,
            "the template's live runs differ from the process shape",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let valid = read_json(file);
        write(file, &bad);
        let refused = refused_clone(&format!("invalid-metadata-{nth}"));
        assert_eq!(refused.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
        write(file, &valid);
    }
    let registry = cache.join("runs/live-0/dispatches");
    for (nth, replacement) in [
        None,
        Some("duplicate-1.json"),
        Some("gap-9.json"),
        Some("malformed.json"),
    ]
    .into_iter()
    .enumerate()
    {
        let moved = registry.join(format!("{old_pid}-0.json"));
        // Remove an index, duplicate the remaining index, leave a gap, or
        // replace its spelling: all must fail before a registry is rebound.
        let saved = std::fs::read(&moved).unwrap();
        std::fs::remove_file(&moved).unwrap();
        if let Some(name) = replacement {
            std::fs::write(registry.join(name), &saved).unwrap();
        }
        let refused = refused_clone(&format!("invalid-registry-{nth}"));
        assert_eq!(refused.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&refused.stderr)
                .contains("the template's registry for live-0 differs from the process shape"),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
        if let Some(name) = replacement {
            std::fs::remove_file(registry.join(name)).unwrap();
        }
        std::fs::write(moved, saved).unwrap();
    }
    std::fs::write(cache.join("runs/sparse/events.jsonl"), "").unwrap();
    let refused = refused_clone("refused");
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains("the sparse fixture's recorded prefix is truncated"));
}

fn fixture_cache(world: &World) -> PathBuf {
    match std::env::var("NEXTEST_RUN_ID") {
        Ok(run) => {
            assert!(
                !run.is_empty()
                    && run
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
            );
            crate::harness::binary()
                .parent()
                .unwrap()
                .join("host-fixtures")
                .join(run)
        }
        Err(_) => world.root.join("host-fixture-template"),
    }
}

/// Compare the raw CLI fixture with its first copy, including sparse prefixes
/// and file lengths. No host read has yet written summaries or checkpoints.
fn same_fixture(template: &Path, copied: &Path, sparse: &Path, sparse_prefix: u64) {
    use std::io::Read;
    let names = |root| {
        std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>()
    };
    let entries = names(template);
    assert_eq!(entries, names(copied), "{}", template.display());
    for name in entries {
        let source = template.join(&name);
        let destination = copied.join(&name);
        if source.is_dir() {
            same_fixture(&source, &destination, sparse, sparse_prefix);
        } else {
            let length = std::fs::metadata(&source).unwrap().len();
            assert_eq!(
                length,
                std::fs::metadata(&destination).unwrap().len(),
                "{}",
                source.display()
            );
            let prefix = if destination == sparse {
                sparse_prefix
            } else {
                length
            };
            let bytes = |path| {
                let mut bytes = Vec::new();
                std::fs::File::open(path)
                    .unwrap()
                    .take(prefix)
                    .read_to_end(&mut bytes)
                    .unwrap();
                bytes
            };
            assert_eq!(bytes(&source), bytes(&destination), "{}", source.display());
        }
    }
}

fn pairs(value: &Value) -> BTreeSet<(String, String)> {
    value
        .as_array()
        .expect("pairs")
        .iter()
        .map(|pair| {
            (
                pair[0].as_str().expect("a run").to_owned(),
                pair[1].as_str().expect("a node").to_owned(),
            )
        })
        .collect()
}

fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("names")
        .iter()
        .map(|name| name.as_str().expect("a name").to_owned())
        .collect()
}

impl Fixture {
    /// An empty root, built run by run with [`Fixture::run`].
    fn empty(world: &World) -> Self {
        std::fs::create_dir_all(&world.runs).expect("a runs root");
        Self {
            runs: world.runs.clone(),
            expected: Expected::default(),
            live_runs: BTreeSet::new(),
            ended: Vec::new(),
            sparse_run: String::new(),
            manifest: world.root.join("manifest.json"),
            children: Vec::new(),
        }
    }

    /// The generator's 100-run fixture, its live dispatches this test's own
    /// processes: two live drivers, and five dispatches across three runs, the
    /// third of which the generator gives a driver that has died.
    fn generate(world: &World) -> Self {
        let mut fixture = Self::empty(world);
        let mut process = || {
            let (pid, token) = fixture.sleeper();
            json!({"pid": pid, "token": token})
        };
        let processes = json!({
            "drivers": [process(), process()],
            "entries": [[process(), process()], [process(), process()], [process()]],
        });
        let file = world.root.join("processes.json");
        write(&file, &processes);
        let cache = fixture_cache(world);
        let mut generated = false;
        crate::harness::build_once(&cache, || {
            let template = cache.join("runs");
            let _ = std::fs::remove_dir_all(&template);
            let manifest = generator(world, &["generate", path(&template), path(&file)]);
            std::fs::write(cache.join("manifest.json"), manifest)
                .expect("the raw fixture's manifest");
            generated = true;
        });
        let manifest = generator(
            world,
            &["clone", path(&fixture.runs), path(&file), path(&cache)],
        );
        if generated {
            // The first copy uses the very processes the CLI-generated template
            // names, so every byte must agree before later copies rebind them.
            let parsed: Value = serde_json::from_str(&manifest).unwrap();
            same_fixture(
                &cache.join("runs"),
                &fixture.runs,
                &fixture
                    .runs
                    .join(parsed["sparse_run"].as_str().unwrap())
                    .join("events.jsonl"),
                parsed["sparse_record_bytes"].as_u64().unwrap(),
            );
        }
        fixture.adopt(&manifest);
        fixture
    }

    /// Take the generator's manifest as this fixture's.
    fn adopt(&mut self, manifest: &str) {
        std::fs::write(&self.manifest, manifest).expect("the manifest");
        let manifest: Value = serde_json::from_str(manifest).expect("a manifest");
        let expected = &manifest["expected"];
        self.expected = Expected {
            held: pairs(&expected["held"]),
            unproven: pairs(&expected["unproven"]),
            stale: pairs(&expected["stale"]),
            stopped: pairs(&expected["stopped"]),
            skipped: names(&expected["skipped"]).into_iter().collect(),
        };
        self.live_runs = names(&manifest["live_runs"]).into_iter().collect();
        self.ended = names(&manifest["ended"]);
        self.sparse_run = manifest["sparse_run"].as_str().expect("a run").to_owned();
    }

    /// Grow the root with ended history to `total` run roots.
    fn grow(&mut self, world: &World, total: usize) {
        let manifest = generator(
            world,
            &[
                "grow",
                path(&self.runs),
                path(&self.manifest),
                &total.to_string(),
            ],
        );
        self.adopt(&manifest);
    }

    /// The cold state: no ended run's derived document, every ended journal
    /// readable but the sparse one, which no state makes readable.
    fn cold(&self, world: &World) {
        generator(world, &["cold", path(&self.runs), path(&self.manifest)]);
    }

    /// The sparse run's summary as its writer keeps it, written by a read of
    /// `program`.
    fn stamp_sparse(&self, world: &World, program: &Path) {
        generator(
            world,
            &[
                "stamp-sparse",
                path(&self.runs),
                path(&self.manifest),
                path(program),
            ],
        );
    }

    /// One run, shaped as `spec` says — see the generator's `run` verb.
    fn run(&self, world: &World, spec: &Value) {
        let file = world.root.join("run.json");
        write(&file, spec);
        generator(world, &["run", path(&self.runs), path(&file)]);
    }

    /// Append `ending` to `run`'s journal — see the generator's `append` verb.
    fn append(&self, world: &World, run: &str, ending: &Value) {
        let file = world.root.join("append.json");
        write(&file, &json!({"run": run, "ending": ending}));
        generator(world, &["append", path(&self.runs), path(&file)]);
    }

    /// A real process standing in for a dispatch or a driver, and its start token.
    fn sleeper(&mut self) -> (u32, String) {
        let child = Command::new("sleep")
            .arg("86400")
            .stdin(Stdio::null())
            .spawn()
            .expect("a process to hold a dispatch");
        let pid = child.id();
        self.children.push(child);
        (pid, start_token(pid))
    }

    fn roots(&self) -> usize {
        std::fs::read_dir(&self.runs).expect("the root").count()
    }

    fn journal(&self, run: &str) -> PathBuf {
        self.runs.join(run).join("events.jsonl")
    }

    /// Make each of `runs`' journals readable, or not, to the view's process.
    fn readable(&self, runs: &[String], readable: bool) {
        for run in runs {
            let mode = if readable { 0o644 } else { 0o000 };
            let _ =
                std::fs::set_permissions(self.journal(run), std::fs::Permissions::from_mode(mode));
        }
    }
}

fn path(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

fn write(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_string_pretty(value).expect("JSON")).expect("a write");
}

/// The seal `src/checkpoint.rs`'s `Coverage::sealed_with` takes over a checkpoint
/// document: FNV-1a over its counts, its placement and its streams, then over the
/// state and the tally as they serialize.
///
/// A reader that has proved a run ended leaves a checkpoint covering its whole
/// journal, and a journey that stands in for that reader over a journal grown past
/// the bound has to seal the coverage it stamps, since the build refuses a
/// document whose claims do not seal to its digest. Callers check it against a
/// digest the build wrote first, so a seal that moves fails here by name.
fn sealed(document: &Value) -> Value {
    const PRIME: u128 = (1 << 88) | 0x13b;
    let digested = |from: u128, bytes: &[u8]| {
        bytes.iter().fold(from, |digest, byte| {
            (digest ^ u128::from(*byte)).wrapping_mul(PRIME)
        })
    };
    let coverage = &document["coverage"];
    let count = |field: &str| coverage[field].as_u64().expect("a count").to_le_bytes();
    let mut seal = digested(0x6c62_272e_07bb_0142_62b8_2175_6295_c58d, &count("bytes"));
    seal = digested(seal, &count("records"));
    if let Some(at) = coverage.get("at") {
        for field in ["ts", "stream"] {
            seal = digested(seal, at[field].as_str().expect("a placement").as_bytes());
        }
    }
    for (stream, reached) in coverage
        .get("streams")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        seal = digested(seal, stream.as_bytes());
        seal = digested(seal, &reached.as_u64().expect("a count").to_le_bytes());
    }
    for part in ["state", "tally"] {
        seal = digested(seal, &serde_json::to_vec(&document[part]).expect("JSON"));
    }
    json!(format!("{seal:032x}"))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// One real invocation of the binary, kept whole.
struct Invocation {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    wall_ms: f64,
    spawned_unix_ns: u128,
}

impl Invocation {
    fn reading(&self) -> Value {
        assert_eq!(
            self.code,
            Some(0),
            "`host --json` exited {:?}: {}",
            self.code,
            self.stderr
        );
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("`host --json` printed no reading ({e}): {}", self.stdout))
    }
}

fn unix_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_nanos()
}

/// Run `command` to its exit, timing it from spawn to reap.
fn invoke(mut command: Command) -> Invocation {
    let spawned_unix_ns = unix_ns();
    let began = Instant::now();
    let output = command.output().expect("the binary runs");
    Invocation {
        wall_ms: began.elapsed().as_secs_f64() * 1_000.0,
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        spawned_unix_ns,
    }
}

/// What the kernel says one traced read did with the runs' journals.
struct Traced {
    reading: Value,
    /// Bytes read from each run's journal the process opened, by run.
    opened: BTreeMap<String, u64>,
    /// The runs whose journal the process tried and failed to open.
    refused: BTreeSet<String>,
    /// Its wall clock, tracer included.
    wall_ms: f64,
    /// Its phase breakdown, where one was asked for.
    phases: Option<Value>,
}

/// `host --json` under `strace`, which records every open — and, with `reads`,
/// every read — of a journal.
///
/// It **refuses** rather than passes where the tracer will not run: the claim is
/// what the process did, and an observation nobody made is not an observation of
/// nothing.
fn traced(
    world: &World,
    program: &Path,
    into: &Path,
    reads: bool,
    phases: Option<&Path>,
) -> Traced {
    let mut inner = world.cmd_on(program, &["host", "--json"]);
    if let Some(phases) = phases {
        inner.env(PHASES_ENV, phases);
    }
    let calls = if reads {
        "trace=openat,open,read,pread64"
    } else {
        "trace=openat,open"
    };
    let mut command = Command::new("strace");
    // `--seccomp-bpf` stops the process only at the calls traced, so a read traced
    // for its opens alone runs close to its untraced speed.
    command
        .args([
            "-f",
            "--seccomp-bpf",
            "-qq",
            "-y",
            "-e",
            calls,
            "-e",
            "signal=none",
            "-o",
        ])
        .arg(into)
        .arg(inner.get_program())
        .args(inner.get_args())
        .stdin(Stdio::null());
    for (key, value) in inner.get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let invocation = invoke(command);
    let reading = invocation.reading();
    let trace = std::fs::read_to_string(into).unwrap_or_else(|error| {
        panic!(
            "this journey's claim is what the process opened, and the tracer left no trace \
             ({error}): install strace, or run the suite where ptrace is permitted"
        )
    });
    assert!(
        trace.contains("openat("),
        "the tracer recorded no open: {trace}"
    );
    let run_of = |path: &str| -> Option<String> {
        let dir = path.strip_suffix("/events.jsonl")?;
        Path::new(dir)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    let mut opened = BTreeMap::new();
    let mut refused = BTreeSet::new();
    for line in trace.lines() {
        // The thread's id is padded to five columns (`%-5u`), so a pid under 10000 is
        // followed by more than one space; a line with no id is not a call this reads,
        // and skipping it would read as a call never made.
        let (thread, body) = line.split_once(' ').unwrap_or_default();
        assert!(
            !thread.is_empty() && thread.bytes().all(|byte| byte.is_ascii_digit()),
            "a traced line names no thread: {line}"
        );
        let body = body.trim_start();
        let result = body
            .rsplit_once("= ")
            .map_or("", |(_, result)| result.trim());
        if body.starts_with("openat(") || body.starts_with("open(") {
            let Some(run) = body.split('"').nth(1).and_then(run_of) else {
                continue;
            };
            if result.starts_with('-') {
                refused.insert(run);
            } else {
                opened.entry(run).or_insert(0);
            }
        } else if body.starts_with("read(") || body.starts_with("pread64(") {
            let Some(run) = body
                .split_once('<')
                .and_then(|(_, rest)| rest.split_once('>'))
                .and_then(|(path, _)| run_of(path))
            else {
                continue;
            };
            *opened.entry(run).or_insert(0) += result.parse::<u64>().unwrap_or(0);
        }
    }
    Traced {
        phases: phases.and_then(|phases| phases_of(phases, &invocation)),
        wall_ms: invocation.wall_ms,
        reading,
        opened,
        refused,
    }
}

/// The verdicts of one reading, keyed so two readings compare.
#[derive(Debug, PartialEq, Eq)]
struct Verdicts {
    rows: Vec<(String, String, String)>,
    gone: BTreeSet<(String, String)>,
    skipped: BTreeSet<String>,
}

fn verdicts(reading: &Value) -> Verdicts {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    Verdicts {
        rows: reading["dispatches"]
            .as_array()
            .expect("dispatches")
            .iter()
            .map(|row| (text(&row["run"]), text(&row["node"]), text(&row["proof"])))
            .collect(),
        gone: reading["gone"]
            .as_array()
            .expect("gone")
            .iter()
            .map(|gone| (text(&gone["run"]), text(&gone["node"])))
            .collect(),
        skipped: reading["skipped"]
            .as_array()
            .expect("skipped")
            .iter()
            .map(|skipped| {
                Path::new(skipped["path"].as_str().expect("a path"))
                    .file_name()
                    .expect("a name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect(),
    }
}

/// Hold one `--json` reading to the fixture's expected answer.
fn holds(expected: &Expected, reading: &Value, what: &str) {
    assert_eq!(reading["schema_version"], 1, "{what}: {reading}");
    assert_eq!(reading["host"], HOST, "{what}");
    assert_eq!(reading["root_error"], Value::Null, "{what}");
    assert!(
        !reading["free_space"]
            .as_array()
            .expect("free space")
            .is_empty(),
        "{what}: no free space"
    );
    let mut rows: BTreeMap<(String, String), &Value> = BTreeMap::new();
    for row in reading["dispatches"].as_array().expect("dispatches") {
        let key = (
            row["run"].as_str().expect("a run").to_owned(),
            row["node"].as_str().expect("a node").to_owned(),
        );
        assert!(
            rows.insert(key.clone(), row).is_none(),
            "{what}: {key:?} twice"
        );
    }
    for key in &expected.held {
        let row = rows
            .get(key)
            .unwrap_or_else(|| panic!("{what}: live {key:?} is no row"));
        assert_eq!(row["proof"], "held", "{what}: live {key:?}: {row}");
        assert_eq!(row["unproven"], Value::Null, "{what}: {row}");
    }
    for key in &expected.unproven {
        let row = rows
            .get(key)
            .unwrap_or_else(|| panic!("{what}: {key:?} is no row"));
        assert_eq!(row["proof"], "unproven", "{what}: {row}");
        assert!(
            row["unproven"].as_str().is_some_and(|why| !why.is_empty()),
            "{what}: {key:?} is UNPROVEN with no reason: {row}"
        );
    }
    let expected_rows: BTreeSet<_> = expected.held.union(&expected.unproven).cloned().collect();
    assert_eq!(
        rows.keys().cloned().collect::<BTreeSet<_>>(),
        expected_rows,
        "{what}: rows the fixture does not expect"
    );
    let gone: BTreeMap<(String, String), String> = reading["gone"]
        .as_array()
        .expect("gone")
        .iter()
        .map(|gone| {
            (
                (
                    gone["run"].as_str().expect("a run").to_owned(),
                    gone["node"].as_str().expect("a node").to_owned(),
                ),
                gone["reason"].as_str().expect("a reason").to_owned(),
            )
        })
        .collect();
    let expected_gone: BTreeSet<_> = expected.stale.union(&expected.stopped).cloned().collect();
    assert_eq!(
        gone.keys().cloned().collect::<BTreeSet<_>>(),
        expected_gone,
        "{what}: gone"
    );
    for key in &expected.stale {
        assert!(
            gone[key].contains("is gone"),
            "{what}: {key:?}: {}",
            gone[key]
        );
    }
    for key in &expected.stopped {
        let reason = &gone[key];
        assert!(
            reason.contains("its run was stopped") && reason.contains("its workers signalled"),
            "{what}: {key:?}: {reason}"
        );
        assert!(!reason.contains("watched"), "{what}: {reason}");
    }
    assert_eq!(
        verdicts(reading).skipped,
        expected.skipped,
        "{what}: skipped"
    );
}

/// The text form is the same reading rendered: a row per dispatch in the same
/// order, each gone dispatch named, and the refused roots counted.
fn renders(text: &str, reading: &Value, what: &str) {
    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0;
    for row in reading["dispatches"].as_array().expect("dispatches") {
        let (run, node) = (
            row["run"].as_str().expect("run"),
            row["node"].as_str().expect("node"),
        );
        let found = lines[at..]
            .iter()
            .position(|line| {
                let words: Vec<&str> = line.split_whitespace().collect();
                words.first() == Some(&run) && words.get(1) == Some(&node)
            })
            .unwrap_or_else(|| {
                panic!("{what}: the text has no row for {run}/{node} in order:\n{text}")
            });
        let line = lines[at + found];
        assert_eq!(
            line.contains("UNPROVEN"),
            row["proof"] == "unproven",
            "{what}: {line}"
        );
        at += found + 1;
    }
    for gone in reading["gone"].as_array().expect("gone") {
        let named = format!(
            "{}/{}",
            gone["run"].as_str().expect("run"),
            gone["node"].as_str().expect("node")
        );
        assert!(
            text.contains(&named),
            "{what}: the text does not name {named} gone:\n{text}"
        );
    }
    let refused = reading["skipped"].as_array().expect("skipped").len();
    if refused > 0 {
        assert!(
            text.contains(&format!("{refused} run root")),
            "{what}: the text does not count {refused} refused roots:\n{text}"
        );
    }
}

fn median(walls: &[f64]) -> f64 {
    let mut sorted = walls.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

/// One state of the root, read `repeats` times at `host --json` (each timed and
/// its answer held), once as text, and once traced.
struct StateRead {
    walls_ms: Vec<f64>,
    phases: Option<Value>,
    traced: Traced,
    verdicts: Verdicts,
    load1: f64,
}

fn load1() -> f64 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|text| {
            text.split_whitespace()
                .next()
                .and_then(|load| load.parse().ok())
        })
        .unwrap_or(f64::NAN)
}

fn read_state(
    world: &World,
    program: &Path,
    fixture: &Fixture,
    label: &str,
    repeats: usize,
) -> StateRead {
    let phases = world.root.join(format!("{label}.phases.jsonl"));
    let mut walls_ms = Vec::new();
    let mut last = None;
    let load = load1();
    for n in 1..=repeats {
        let mut command = world.cmd_on(program, &["host", "--json"]);
        command.env(PHASES_ENV, &phases);
        let invocation = invoke(command);
        holds(
            &fixture.expected,
            &invocation.reading(),
            &format!("{label} read {n}"),
        );
        walls_ms.push(invocation.wall_ms);
        last = Some(invocation);
    }
    let last = last.expect("a read");
    let text = invoke(world.cmd_on(program, &["host"]));
    assert_eq!(text.code, Some(0), "{label}: text form: {}", text.stderr);
    let reading = last.reading();
    renders(&text.stdout, &reading, label);
    let traced = traced(
        world,
        program,
        &world.root.join(format!("{label}.strace")),
        true,
        None,
    );
    holds(
        &fixture.expected,
        &traced.reading,
        &format!("{label} traced read"),
    );
    assert!(
        !traced.opened.contains_key(&fixture.sparse_run),
        "{label}: the 3.6 GiB journal was opened"
    );
    let phases = phases_of(&phases, &last);
    recorded(&phases, &STEADY_PHASES, "live-journal-bytes", label);
    StateRead {
        walls_ms,
        phases,
        verdicts: verdicts(&reading),
        traced,
        load1: load,
    }
}

/// The last phase line a read appended to `phases`, with the process's start and
/// exit — what came before the command's work and after it — measured against the
/// invocation's own spawn and wall clock.
fn phases_of(phases: &Path, invocation: &Invocation) -> Option<Value> {
    let lines = std::fs::read_to_string(phases).ok()?;
    let mut line: Value = serde_json::from_str(lines.lines().last()?).ok()?;
    let marks = &line["marks_unix_ns"];
    let ns = |key: &str| marks[key].as_u64().map(u128::from);
    if let (Some(run), Some(flushed)) = (ns("run"), ns("flushed")) {
        let exited = invocation.spawned_unix_ns + (invocation.wall_ms * 1e6) as u128;
        line["process_start_ms"] =
            json!(run.saturating_sub(invocation.spawned_unix_ns) as f64 / 1e6);
        line["exit_ms"] = json!(exited.saturating_sub(flushed) as f64 / 1e6);
    }
    Some(line)
}

/// Fail a read whose phase line is missing, or lacks a phase or the quantity it
/// was expected to record.
fn recorded(phases: &Option<Value>, names: &[&str], quantity: &str, label: &str) {
    let line = phases
        .as_ref()
        .unwrap_or_else(|| panic!("{label}: the read recorded no phase line"));
    for name in names {
        assert!(
            line["phases"][name]["wall_ms"].is_number(),
            "{label}: the read recorded no `{name}` phase: {line}"
        );
    }
    assert!(
        line["quantities"][quantity].is_u64(),
        "{label}: the read counted no `{quantity}`: {line}"
    );
    // Each figure is a measurement rather than a placeholder: none negative, and
    // the whole read on a CPU for some of its time.
    for (name, phase) in line["phases"].as_object().expect("phases") {
        for figure in ["wall_ms", "cpu_ms", "runq_ms", "blocked_ms"] {
            assert!(
                phase[figure].as_f64().is_some_and(|ms| ms >= 0.0),
                "{label}: `{name}` {figure}: {phase}"
            );
        }
        assert!(
            phase["read_bytes"].is_u64() && phase["count"].as_u64() >= Some(1),
            "{label}: `{name}`: {phase}"
        );
    }
    assert!(
        line["cpu_ms"].as_f64().is_some_and(|ms| ms > 0.0),
        "{label}: {line}"
    );
    // A fold reads at least the journal bytes it counts.
    let folded = if quantity == "live-journal-bytes" {
        "live-fold"
    } else {
        "cold-fold"
    };
    assert!(
        line["phases"][folded]["read_bytes"].as_u64() >= line["quantities"][quantity].as_u64(),
        "{label}: `{folded}` read fewer bytes than the journals it folded: {line}"
    );
    assert!(
        line["process_start_ms"].is_number() && line["exit_ms"].is_number(),
        "{label}: the read marked no start or end of its work: {line}"
    );
}

/// Fail a read that opened any ended run's journal, even one it read nothing
/// from, or that tried to; answer with the runs whose journals it did open.
fn opened_no_ended_journal(ended: &[String], traced: &Traced, label: &str) -> BTreeSet<String> {
    let ended: BTreeSet<&String> = ended.iter().collect();
    let opened: Vec<&String> = traced
        .opened
        .keys()
        .filter(|run| ended.contains(run))
        .collect();
    assert!(
        opened.is_empty(),
        "{label}: a steady read opened ended runs' journals: {opened:?} ({:?})",
        traced.opened
    );
    let tried: Vec<&String> = traced
        .refused
        .iter()
        .filter(|run| ended.contains(run))
        .collect();
    assert!(
        tried.is_empty(),
        "{label}: a steady read tried to open {tried:?}"
    );
    traced.opened.keys().cloned().collect()
}

/// The cold read: no ended run's derived document, every ended journal readable
/// but the 3.6 GiB one, which stays unreadable. Traced for its opens alone, so the
/// journey sees that the sparse journal's verdict came without the read even
/// trying to open it.
fn cold_read(world: &World, program: &Path, fixture: &Fixture) -> Traced {
    // llmlint: ignore[tests_mirror_real_usage] the cold state is the one the task states as every ended run's `summary.json` and `checkpoint.json` deleted, which is how a root an older engine left, or never derived, looks to this reader; no interface of this crate removes derived documents, and the read that follows is the real binary's.
    fixture.cold(world);
    let phases = world.root.join("cold.phases.jsonl");
    let cold = traced(
        world,
        program,
        &world.root.join("cold.strace"),
        false,
        Some(&phases),
    );
    holds(&fixture.expected, &cold.reading, "the cold read");
    recorded(
        &cold.phases,
        &["cold-fold"],
        "cold-journal-bytes",
        "the cold read",
    );
    assert!(
        !cold.opened.contains_key(&fixture.sparse_run)
            && !cold.refused.contains(&fixture.sparse_run),
        "the cold read reached for the 3.6 GiB journal"
    );
    cold
}

/// `onepipeline host` over the generator's 100-run fixture, through this suite's
/// own build, with the kernel asked what every read opened.
///
/// The root is generated as a host's root is before any reader has been through
/// it: cold, with no derived document beside any journal. One read answers it —
/// every ended journal readable but the 3.6 GiB one, which no state makes readable
/// — and leaves it steady, as a host reaches steady state. The next read, with
/// every ended journal made unreadable, answers identically and opens none of
/// them. Then the sparse run's summary is stamped to its 3.6 GiB file, standing in
/// for the journal writer that keeps it current, and five single reads at that
/// steady state are each held to the answer, with the text form rendering the same
/// reading.
///
/// What a read costs does not grow with ended history: the root then grows to 400
/// and 800 run roots holding the same live set, and at each size the reads open no
/// ended journal, open the same runs — the live ones — and give the same rows and
/// proved-gone dispatches.
///
/// The `host-view-time` budget times the release binary CI's `install` job builds
/// (`scripts/host-view-measure.mjs`, over this same generator). What this build's
/// reads take is recorded here as the gate's evidence beside it, never as the
/// verdict.
#[test]
fn the_host_view_answers_a_host_sized_root_steady_cold_and_steady_again() {
    let world = World::new("host-view").with_env("HOSTNAME", HOST);
    let program = crate::harness::binary();
    // llmlint: ignore[tests_mirror_real_usage] the task requires a generated 100-run root shaped like this host's — its live, stale and registry-less runs and a 3.6 GiB sparse journal — which no sequence of real launches reproduces at that size or cost; `host_view_fixture.mjs`'s head says how it is built from the recorded run, and every read of it is the real binary's.
    let mut fixture = Fixture::generate(&world);

    let cold = cold_read(&world, &program, &fixture);
    let cold_verdicts = verdicts(&cold.reading);

    fixture.readable(&fixture.ended, false);
    let after_cold = traced(
        &world,
        &program,
        &world.root.join("after-cold.strace"),
        true,
        None,
    );
    holds(
        &fixture.expected,
        &after_cold.reading,
        "the read after the cold one",
    );
    assert_eq!(
        verdicts(&after_cold.reading),
        cold_verdicts,
        "the read after the cold one"
    );
    opened_no_ended_journal(&fixture.ended, &after_cold, "the read after the cold one");

    // llmlint: ignore[tests_mirror_real_usage] the 3.6 GiB journal is a sparse file, so no writer appended its records; its summary is written by this build's own read over the records alone and then stamped to the grown file, standing in for the journal writer that keeps a real one current — the generator's head says why the root is generated.
    fixture.stamp_sparse(&world, &program);
    let mut sizes = Vec::new();
    for total in [100, 400, 800] {
        if total > 100 {
            // llmlint: ignore[tests_mirror_real_usage] the task's scaling fixtures add ended, settled runs drawn from the 100-root fixture's own, each a hardlinked journal with its summary and checkpoint rewritten to its run id, to keep 800 roots' disk small; the reads over them are the real binary's.
            fixture.grow(&world, total);
        }
        assert_eq!(fixture.roots(), total);
        fixture.readable(&fixture.ended, false);
        let label = format!("steady-{total}");
        let read = read_state(&world, &program, &fixture, &label, 5);
        let opened = opened_no_ended_journal(&fixture.ended, &read.traced, &label);
        assert_eq!(
            opened, fixture.live_runs,
            "{label}: a steady read folds exactly the runs holding live work"
        );
        assert_eq!(read.verdicts.rows, cold_verdicts.rows, "{label}: rows");
        assert_eq!(read.verdicts.gone, cold_verdicts.gone, "{label}: gone");
        sizes.push(json!({
            "run_roots": total,
            "reads_ms": read.walls_ms,
            "median_ms": median(&read.walls_ms),
            "load1": read.load1,
            "ended_journal_bytes_opened": fixture
                .ended
                .iter()
                .filter_map(|run| read.traced.opened.get(run))
                .sum::<u64>(),
            "runs_opened": opened,
            "phases": read.phases,
        }));
    }

    let readings = json!({
        "build": "the e2e tier's own (debug, and coverage-instrumented under `just check`)",
        "cold_read_ms": cold.wall_ms,
        "sizes": sizes,
    });
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(DEBUG_RECORD);
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("the record's directory");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&readings).expect("JSON"),
    )
    .expect("the record");
    eprintln!("host_view: this build's steady reads, by root size: {sizes:?}");
}

/// The keys version 1 of the reading states, at each level, every one present.
fn has_the_version_1_keys(reading: &Value) {
    let keys = |value: &Value| -> BTreeSet<String> {
        value
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect()
    };
    let set =
        |keys: &[&str]| -> BTreeSet<String> { keys.iter().map(|k| (*k).to_owned()).collect() };
    assert_eq!(
        keys(reading),
        set(&[
            "schema_version",
            "host",
            "root",
            "root_error",
            "free_space",
            "dispatches",
            "gone",
            "skipped"
        ])
    );
    for line in reading["free_space"].as_array().expect("free space") {
        assert_eq!(
            keys(line),
            set(&["roots", "free_bytes", "total_bytes", "error"])
        );
        assert_eq!(
            line["error"].is_null(),
            !(line["free_bytes"].is_null() && line["total_bytes"].is_null()),
            "{line}"
        );
        for root in line["roots"].as_array().expect("roots") {
            assert_eq!(keys(root), set(&["role", "path", "measured_at"]));
            assert!(["runs-root", "workspaces"].contains(&root["role"].as_str().expect("a role")));
        }
    }
    for row in reading["dispatches"].as_array().expect("dispatches") {
        assert_eq!(
            keys(row),
            set(&["run", "node", "launcher", "age_ms", "proof", "unproven"])
        );
        assert_eq!(row["unproven"].is_null(), row["proof"] == "held", "{row}");
    }
    for gone in reading["gone"].as_array().expect("gone") {
        assert_eq!(keys(gone), set(&["run", "node", "reason"]));
    }
    for skipped in reading["skipped"].as_array().expect("skipped") {
        assert_eq!(keys(skipped), set(&["path", "reason"]));
    }
}

/// The text and `--json` forms of one read, each held to the other.
fn both_forms(world: &World, label: &str) -> (Value, String) {
    let json = invoke(world.cmd(&["host", "--json"]));
    let reading = json.reading();
    assert_eq!(
        json.stdout.trim_end().lines().count(),
        1,
        "one line: {}",
        json.stdout
    );
    has_the_version_1_keys(&reading);
    let text = invoke(world.cmd(&["host"]));
    assert_eq!(text.code, Some(0), "{label}: {}", text.stderr);
    renders(&text.stdout, &reading, label);
    (reading, text.stdout)
}

/// A registry-less run — one written by an engine before the dispatch registry —
/// that records a node running is decided by its last recorded stop.
///
/// One whose last `run-stopped` records `teardown: signalled`, or records no
/// teardown at all (as engines before the field wrote it), with no
/// `driver-adopted` after it, is not a row: its dispatch is counted on the line of
/// dispatches proven gone, with a reason saying the run was stopped and its
/// workers signalled — never that their teardown was watched, which no record of
/// those engines says. One whose last stop is anything else, one adopted after
/// its stop, and one never stopped stay `UNPROVEN` rows with their reasons. A live
/// dispatch whose run's driver has died is still a held row, and a stale registry
/// entry is gone and not a row. And appending an adoption and a re-dispatch to a
/// run proven gone makes its dispatch an `UNPROVEN` row again.
#[test]
fn a_registry_less_runs_last_stop_decides_whether_its_dispatch_is_gone() {
    let world = World::new("host-view-stops").with_env("HOSTNAME", HOST);
    // llmlint: ignore-block[tests_mirror_real_usage] runs as engines before the dispatch registry
    // left them, and the adoption appended to one, are the generator's: see the reason at its
    // head. `an_adoption_racing_the_view…` drives a real adoption through the engine.
    let mut fixture = Fixture::empty(&world);
    let plan = |run: &str| (run.to_owned(), "plan".to_owned());
    let long_ago = now_millis() - 30 * 86_400_000;
    let registry_less = |run: &str, ending: Value| {
        fixture.run(
            &world,
            &json!({"run": run, "started_ms": long_ago, "rounds": true, "ending": ending}),
        );
    };
    registry_less("stopped-signalled", json!([{"stopped": "signalled"}]));
    registry_less("stopped-unrecorded", json!([{"stopped": null}]));
    registry_less("stopped-partly", json!([{"stopped": "partly-signalled"}]));
    // An adoption abandons the dispatch in flight, so the node is running again only
    // once the adopting driver dispatches it again.
    registry_less(
        "stopped-then-adopted",
        json!([{"stopped": "signalled"}, {"adopted": true}, {"dispatched": "plan"}]),
    );
    registry_less("never-stopped", json!([]));
    for run in [
        "stopped-signalled",
        "stopped-unrecorded",
        "stopped-partly",
        "stopped-then-adopted",
        "never-stopped",
    ] {
        fixture.ended.push(run.to_owned());
    }
    fixture.expected.stopped.insert(plan("stopped-signalled"));
    fixture.expected.stopped.insert(plan("stopped-unrecorded"));
    for run in ["stopped-partly", "stopped-then-adopted", "never-stopped"] {
        fixture.expected.unproven.insert(plan(run));
    }

    // A live dispatch whose run's driver has died, and a stale registry entry.
    let started = now_millis() - 600_000;
    let (pid, token) = fixture.sleeper();
    fixture.run(
        &world,
        &json!({"run": "orphaned", "started_ms": started,
            "registry": [{"node": "plan", "pid": pid, "token": token, "recorded_ms": started}]}),
    );
    fixture.expected.held.insert(plan("orphaned"));
    fixture.run(
        &world,
        &json!({"run": "stale", "started_ms": started,
            "registry": [{"node": "plan", "recorded_ms": started}]}),
    );
    fixture.expected.stale.insert(plan("stale"));
    fixture.ended.push("stale".to_owned());
    // And a run that dispatched its node again after the only entry its registry
    // holds for it was written: that stale entry is an earlier attempt's, and says
    // nothing of the dispatch the run records now.
    fixture.run(
        &world,
        &json!({"run": "redispatched", "started_ms": started,
            "registry": [{"node": "plan", "recorded_ms": started}],
            "ending": [{"dispatched": "plan"}]}),
    );
    fixture.expected.unproven.insert(plan("redispatched"));
    fixture.ended.push("redispatched".to_owned());

    let (reading, text) = both_forms(&world, "first read");
    holds(&fixture.expected, &reading, "first read");
    let line = |starting: &str| {
        text.lines()
            .find(|line| line.trim_start().starts_with(starting))
            .unwrap_or_else(|| panic!("no line starting {starting:?}:\n{text}"))
            .to_owned()
    };
    let stopped_line = line("2 dispatches of stopped runs proven gone:");
    for run in ["stopped-signalled", "stopped-unrecorded"] {
        assert!(
            stopped_line.contains(&format!("{run}/plan: its run was stopped ")),
            "{stopped_line}"
        );
    }
    assert!(
        stopped_line.contains(" ago and its workers signalled"),
        "{stopped_line}"
    );
    assert!(!stopped_line.contains("teardown"), "{stopped_line}");
    assert!(
        line("1 stale registry entry ignored:").contains("stale/plan"),
        "{text}"
    );
    let row = |run: &str| line(run);
    assert!(row("orphaned").contains("plan") && !row("orphaned").contains("UNPROVEN"));
    for run in ["stopped-partly", "stopped-then-adopted", "never-stopped"] {
        assert!(
            row(run).contains("UNPROVEN: the run's dispatch registry cannot be read"),
            "{}",
            row(run)
        );
    }
    assert!(
        row("redispatched").contains("UNPROVEN: the run recorded this dispatch after every entry"),
        "{}",
        row("redispatched")
    );

    // A summary a reader cannot serve — malformed, at another version, or another
    // run's — is refolded, and the answer does not move.
    let summary = |run: &str| world.runs.join(run).join("summary.json");
    std::fs::write(summary("stale"), "{").expect("a malformed summary");
    let mut other_version = read_json(&summary("stopped-partly"));
    other_version["schema_version"] = json!(999);
    write(&summary("stopped-partly"), &other_version);
    let mut another_run = read_json(&summary("never-stopped"));
    another_run["run_id"] = json!("someone-else");
    write(&summary("never-stopped"), &another_run);
    let (refolded, _) = both_forms(&world, "over summaries it cannot serve");
    holds(
        &fixture.expected,
        &refolded,
        "over summaries it cannot serve",
    );
    for run in ["stale", "stopped-partly", "never-stopped"] {
        let rewritten = read_json(&summary(run));
        assert_eq!(rewritten["run_id"], run, "{run}'s summary was not refolded");
        assert_ne!(
            rewritten["schema_version"], 999,
            "{run}'s summary was not refolded"
        );
    }

    // The read proved `stopped-signalled` ended and left its checkpoint covering
    // its final stop; the next read, every ended journal unreadable, takes the
    // same verdicts off the documents alone.
    let checkpoint = read_json(&world.runs.join("stopped-signalled/checkpoint.json"));
    let journal = fixture.journal("stopped-signalled");
    assert_eq!(
        checkpoint["coverage"]["bytes"].as_u64(),
        Some(std::fs::metadata(&journal).expect("a journal").len()),
        "the ended run's checkpoint does not cover its final stop"
    );
    assert_eq!(checkpoint["state"]["stop"], "workers-signalled");
    fixture.readable(&fixture.ended, false);
    let steady = traced(
        &world,
        &crate::harness::binary(),
        &world.root.join("stops.strace"),
        true,
        None,
    );
    holds(&fixture.expected, &steady.reading, "the steady read");
    opened_no_ended_journal(&fixture.ended, &steady, "the steady read");
    fixture.readable(&fixture.ended, true);

    // An adoption past the stop, and the adopting driver's re-dispatch of the node:
    // the run is driven again, and its dispatch is no longer proven gone.
    fixture.append(
        &world,
        "stopped-signalled",
        &json!([{"adopted": true}, {"dispatched": "plan"}]),
    );
    fixture.expected.stopped.remove(&plan("stopped-signalled"));
    fixture.expected.unproven.insert(plan("stopped-signalled"));
    // llmlint: ignore-end[tests_mirror_real_usage]
    let (reading, _) = both_forms(&world, "after an adoption");
    holds(&fixture.expected, &reading, "after an adoption");
}

/// No journal past the 1 GiB bound is opened, in any state — each of these is
/// readable, and the kernel is asked whether the read touched it — and a run that
/// bound leaves undecided is refused, flagged, never left out. A registry-less
/// run past it with no current summary is listed in `skipped` with its reason,
/// in both forms, and so is one whose driver may still be running with no entry
/// live. A run past it with a registry is decided from its entries otherwise: a
/// held one is a row, and a stale one is gone. One whose writer-maintained summary
/// is current is decided as that summary allows: live from its entries, and ended
/// off a checkpoint covering the whole journal — or, with none, from its registry,
/// or refused.
///
/// And a run under the bound whose journal the read has to fold, and cannot open,
/// is refused the same way, rather than folded to nothing and left out.
#[test]
fn a_journal_past_the_bound_is_never_opened_and_a_run_it_leaves_undecided_is_refused() {
    let world = World::new("host-view-bound").with_env("HOSTNAME", HOST);
    // llmlint: ignore-block[tests_mirror_real_usage] journals past the bound are the
    // generator's, grown sparse: see the reason at its head.
    let mut fixture = Fixture::empty(&world);
    let started = now_millis() - 86_400_000;
    let past = (1u64 << 30) + MIB;
    let stale = json!([{"node": "plan", "recorded_ms": started}]);

    fixture.run(
        &world,
        &json!({"run": "huge-registry-less", "started_ms": started, "grow_to": past}),
    );
    fixture.run(
        &world,
        &json!({"run": "huge-stale", "started_ms": started, "registry": stale, "grow_to": past}),
    );
    let (driver, driver_token) = fixture.sleeper();
    let (pid, token) = fixture.sleeper();
    fixture.run(
        &world,
        &json!({"run": "huge-live", "started_ms": started, "grow_to": past,
            "driver": {"pid": driver, "token": driver_token},
            "registry": [{"node": "plan", "pid": pid, "token": token, "recorded_ms": started}]}),
    );
    // A driver that may still be running and no entry live: what it has running
    // is not in its registry, and only the journal could say.
    let (driver, driver_token) = fixture.sleeper();
    fixture.run(
        &world,
        &json!({"run": "huge-driven", "started_ms": started, "grow_to": past,
            "driver": {"pid": driver, "token": driver_token}, "registry": stale}),
    );

    // And a run under the bound whose journal the read would have to fold, cold, and
    // cannot open: a fold of nothing would leave its dispatch out.
    fixture.run(&world, &json!({"run": "unreadable", "started_ms": started}));
    fixture.readable(&["unreadable".to_owned()], false);

    // Three runs whose writer-maintained summary is current for a journal past the
    // bound — the shape of a long run's 3.6 GiB journal on a real host. Each is read
    // once while small, as its writer and readers left it, then grown past the
    // bound with its summary stamped to the grown file, standing in for the writer
    // that keeps it current; one ended run's checkpoint is stamped to cover the whole
    // grown journal, as a reader that proved it ended leaves it, and the other's is not.
    let (driver, driver_token) = fixture.sleeper();
    let (pid, token) = fixture.sleeper();
    fixture.run(
        &world,
        &json!({"run": "huge-live-current", "started_ms": started,
            "driver": {"pid": driver, "token": driver_token},
            "registry": [{"node": "plan", "pid": pid, "token": token, "recorded_ms": started}]}),
    );
    for run in ["huge-ended-covered", "huge-ended-uncovered"] {
        fixture.run(
            &world,
            &json!({"run": run, "started_ms": started, "rounds": true,
                "ending": [{"stopped": "signalled"}]}),
        );
    }
    let primed = invoke(world.cmd(&["host"]));
    assert_eq!(primed.code, Some(0), "{}", primed.stderr);
    for run in [
        "huge-live-current",
        "huge-ended-covered",
        "huge-ended-uncovered",
    ] {
        let journal = fixture.journal(run);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&journal)
            .and_then(|file| file.set_len(past))
            .expect("a journal past the bound");
        let journal = std::fs::metadata(journal).expect("a journal");
        let summary = world.runs.join(run).join("summary.json");
        let mut stamped = read_json(&summary);
        assert!(
            stamped["node_counts"]["running"].as_u64() >= Some(1),
            "{run}: {stamped}"
        );
        stamped["journal_len"] = json!(journal.len());
        stamped["journal_mtime_ms"] = json!(journal
            .modified()
            .expect("a modification time")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_millis() as u64);
        write(&summary, &stamped);
    }
    let checkpoint = world.runs.join("huge-ended-covered/checkpoint.json");
    let mut covering = read_json(&checkpoint);
    assert_eq!(
        sealed(&covering),
        covering["coverage"]["digest"],
        "this journey's seal no longer matches the one the build wrote"
    );
    covering["coverage"]["bytes"] = json!(past);
    covering["coverage"]["digest"] = sealed(&covering);
    write(&checkpoint, &covering);

    // llmlint: ignore-end[tests_mirror_real_usage]
    let read = traced(
        &world,
        &crate::harness::binary(),
        &world.root.join("bound.strace"),
        true,
        None,
    );
    let huge = [
        "huge-live-current",
        "huge-ended-covered",
        "huge-ended-uncovered",
        "huge-registry-less",
        "huge-stale",
        "huge-live",
        "huge-driven",
    ];
    let reached: Vec<&&str> = huge
        .iter()
        .filter(|run| read.opened.contains_key(**run) || read.refused.contains(**run))
        .collect();
    assert!(
        reached.is_empty(),
        "a journal past the bound was reached for: {reached:?}"
    );

    let (reading, text) = both_forms(&world, "past the bound");
    assert_eq!(verdicts(&reading), verdicts(&read.reading));
    let skipped: BTreeMap<String, String> = reading["skipped"]
        .as_array()
        .expect("skipped")
        .iter()
        .map(|skipped| {
            let path = Path::new(skipped["path"].as_str().expect("a path"));
            (
                path.file_name()
                    .expect("a name")
                    .to_string_lossy()
                    .into_owned(),
                skipped["reason"].as_str().expect("a reason").to_owned(),
            )
        })
        .collect();
    assert_eq!(
        skipped.keys().collect::<Vec<_>>(),
        [
            "huge-driven",
            "huge-ended-uncovered",
            "huge-registry-less",
            "unreadable"
        ],
        "{reading}"
    );
    assert!(
        skipped["huge-ended-uncovered"].contains("its dispatch registry cannot be read"),
        "{}",
        skipped["huge-ended-uncovered"]
    );
    assert!(
        skipped["huge-driven"].contains("past the 1073741824 a host read opens")
            && skipped["huge-driven"].ends_with("its driver is not proved gone"),
        "{}",
        skipped["huge-driven"]
    );
    let reason = &skipped["huge-registry-less"];
    assert!(
        reason.contains("past the 1073741824 a host read opens")
            && reason.contains("its dispatch registry cannot be read"),
        "{reason}"
    );
    assert!(
        skipped["unreadable"].starts_with("its journal cannot be read"),
        "{}",
        skipped["unreadable"]
    );
    assert!(
        text.contains("4 run root(s) skipped:")
            && text.contains("huge-registry-less: its journal is")
            && text.contains(", and 1 more"),
        "{text}"
    );
    let held = |run: &str| (run.to_owned(), "plan".to_owned(), "held".to_owned());
    assert_eq!(
        verdicts(&reading).rows,
        vec![held("huge-live"), held("huge-live-current")],
        "{reading}"
    );
    assert_eq!(
        verdicts(&reading).gone,
        BTreeSet::from([
            ("huge-ended-covered".to_owned(), "plan".to_owned()),
            ("huge-stale".to_owned(), "plan".to_owned())
        ])
    );
    let stopped = reading["gone"]
        .as_array()
        .expect("gone")
        .iter()
        .find(|gone| gone["run"] == "huge-ended-covered")
        .expect("the covered run's dispatch is gone");
    assert!(
        stopped["reason"]
            .as_str()
            .is_some_and(|why| why.starts_with("its run was stopped")),
        "the covered run was not decided off its checkpoint: {stopped}"
    );
}

/// The bytes of `journal` the first `bytes` cover, as whole records: `None` where
/// they end inside one.
fn records_in(journal: &[u8], bytes: u64) -> Option<u64> {
    let prefix = journal.get(..usize::try_from(bytes).ok()?)?;
    (prefix.is_empty() || prefix.ends_with(b"\n"))
        .then(|| prefix.iter().filter(|byte| **byte == b'\n').count() as u64)
}

/// The view's checkpoint extension races a real driver adopting the run it
/// extended, and loses nothing.
///
/// A run is stopped with its dispatch's registry entry left stale; a read of the
/// view proves it ended and extends its checkpoint over the stop. Then the real
/// engine adopts it, through the path `onepipeline adopt` takes, with only the
/// paid model doubled: it appends `driver-adopted`, dispatches the node again
/// with a live registry entry, goes on recording, and writes the run's checkpoint
/// and summary as it always does — while the view reads the root over and over,
/// extending what it may. The adopting driver then dies, and its dispatch
/// outlives it.
///
/// The journal holds every record the driver appended, in order; the checkpoint
/// and the summary are each a document whose coverage is a prefix of the journal
/// as it now stands; a fold from that checkpoint is a fold of the whole journal;
/// and the adopted run's live dispatch is a row on every read after its entry was
/// written, never reported gone.
#[test]
fn an_adoption_racing_the_view_loses_no_record_and_its_dispatch_is_never_gone() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    let world = World::new("host-view-adopted");
    world.script("build.wait", "hold");
    let run = "raced";
    let path = world.plan(
        run,
        &crate::harness::plan_of(run, vec![crate::harness::agent("build", &[])]),
    );
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the dispatch to record where it is running", |world| {
        !world.dispatch_records(run).is_empty()
    });
    let pid_of = |record: &PathBuf| read_json(record)["pid"].as_u64().expect("a pid");
    let first = pid_of(&world.dispatch_records(run)[0]);
    world.run(&["stop", run]).exited(0);

    let journal = world.run_file(run, "events.jsonl");
    let reading = invoke(world.cmd(&["host", "--json"])).reading();
    assert!(
        verdicts(&reading)
            .gone
            .contains(&(run.to_owned(), "build".to_owned())),
        "the stopped run's dispatch is not gone: {reading}"
    );
    let checkpoint = read_json(&world.run_file(run, "checkpoint.json"));
    assert_eq!(
        checkpoint["coverage"]["bytes"].as_u64(),
        Some(std::fs::metadata(&journal).expect("a journal").len()),
        "the view did not extend the ended run's checkpoint"
    );

    // The view reads on and on while the adoption lands.
    let reading_on = Arc::new(AtomicBool::new(true));
    let reads: Arc<Mutex<Vec<(Instant, Value)>>> = Arc::default();
    let reader = {
        let (reading_on, reads) = (Arc::clone(&reading_on), Arc::clone(&reads));
        // The command `World` composes, carried into the thread as its parts.
        let template = world.cmd(&["host", "--json"]);
        let program = template.get_program().to_owned();
        let args: Vec<_> = template.get_args().map(ToOwned::to_owned).collect();
        let envs: Vec<_> = template
            .get_envs()
            .map(|(k, v)| (k.to_owned(), v.map(ToOwned::to_owned)))
            .collect();
        std::thread::spawn(move || {
            while reading_on.load(Ordering::SeqCst) {
                let mut command = Command::new(&program);
                command.args(&args).stdin(Stdio::null());
                for (key, value) in &envs {
                    match value {
                        Some(value) => command.env(key, value),
                        None => command.env_remove(key),
                    };
                }
                let began = Instant::now();
                let reading = invoke(command).reading();
                reads.lock().expect("the reads").push((began, reading));
            }
        })
    };

    let mut adopting = world
        .cmd(&["adopt", run])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the adopting driver starts");
    world.until("the adopting driver to dispatch the node again", |world| {
        world
            .journal(run)
            .iter()
            .skip_while(|event| event["kind"] != "driver-adopted")
            .any(|event| event["kind"] == "node-dispatched")
    });
    world.until("the new dispatch to record where it is running", |world| {
        world
            .dispatch_records(run)
            .iter()
            .any(|record| pid_of(record) != first)
    });
    let entry_written = Instant::now();
    world.until("the view to read the run a few times since", |_| {
        reads
            .lock()
            .expect("the reads")
            .iter()
            .filter(|(began, _)| *began > entry_written)
            .count()
            >= 3
    });
    // The adopting driver dies, as a crash would end it; its dispatch outlives it.
    adopting.kill().expect("the adopting driver is ended");
    let _ = adopting.wait();
    reading_on.store(false, Ordering::SeqCst);
    reader.join().expect("the reader");

    let second = world
        .dispatch_records(run)
        .iter()
        .map(pid_of)
        .find(|pid| *pid != first)
        .expect("the new dispatch's entry");
    for (began, reading) in reads.lock().expect("the reads").iter() {
        let verdict = verdicts(reading);
        let key = (run.to_owned(), "build".to_owned());
        for gone in reading["gone"].as_array().expect("gone") {
            assert!(
                !gone["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(&second.to_string()),
                "the adopted run's live dispatch was reported gone: {reading}"
            );
        }
        if *began > entry_written {
            assert!(
                verdict
                    .rows
                    .iter()
                    .any(|(r, n, _)| (r, n) == (&key.0, &key.1))
                    && !verdict.gone.contains(&key),
                "a read after the new dispatch's entry was written left it out: {reading}"
            );
        }
    }

    // Every record the driver appended is there, in order: each stream's
    // sequence numbers run unbroken from where they start, in file order.
    let bytes = std::fs::read(&journal).expect("the journal");
    let mut seqs: BTreeMap<String, u64> = BTreeMap::new();
    for line in String::from_utf8_lossy(&bytes).lines() {
        let record: Value = serde_json::from_str(line).expect("a whole record");
        let stream = record["stream"].as_str().expect("a stream").to_owned();
        let seq = record["seq"].as_u64().expect("a seq");
        if let Some(previous) = seqs.insert(stream.clone(), seq) {
            assert_eq!(
                seq,
                previous + 1,
                "stream {stream} skipped or reordered a record"
            );
        }
    }
    assert!(
        String::from_utf8_lossy(&bytes).contains("\"driver-adopted\""),
        "the adoption is not in the journal"
    );

    // The derived documents are each a valid prefix of the journal as it stands.
    let checkpoint = read_json(&world.run_file(run, "checkpoint.json"));
    assert_eq!(checkpoint["schema_version"], 15);
    assert_eq!(
        records_in(
            &bytes,
            checkpoint["coverage"]["bytes"].as_u64().expect("bytes")
        ),
        checkpoint["coverage"]["records"].as_u64(),
        "the checkpoint claims records the journal does not hold where it says: {}",
        checkpoint["coverage"]
    );
    let summary = read_json(&world.run_file(run, "summary.json"));
    assert_eq!(
        records_in(&bytes, summary["journal_len"].as_u64().expect("a length")),
        summary["event_count"].as_u64(),
        "the summary's stamp is not a prefix of the journal"
    );

    // A fold from that checkpoint is a fold of the whole journal.
    // llmlint: ignore-block[tests_mirror_real_usage] the claim compares a fold resumed from
    // the checkpoint with a fold of the whole journal, and no verb folds a run while ignoring
    // its checkpoint. So the run's own journal, launch record and plan are copied byte for byte
    // into a root holding no derived document, and both folds are the real binary's `results`.
    let whole = World::new("host-view-adopted-whole");
    let copied = whole.runs.join(run);
    std::fs::create_dir_all(&copied).expect("a copy");
    for file in ["events.jsonl", "launch.json", "plan.json"] {
        std::fs::copy(world.run_file(run, file), copied.join(file)).expect("a copy");
    }
    // llmlint: ignore-end[tests_mirror_real_usage]
    let from_checkpoint = world.run(&["results", run]);
    from_checkpoint.exited(0);
    let from_nothing = whole.run(&["results", run]);
    from_nothing.exited(0);
    assert_eq!(from_checkpoint.stdout, from_nothing.stdout);

    // And the next read shows the dispatch that outlived the adopting driver.
    let reading = invoke(world.cmd(&["host", "--json"])).reading();
    let row = reading["dispatches"]
        .as_array()
        .expect("dispatches")
        .iter()
        .find(|row| row["run"] == run && row["node"] == "build")
        .unwrap_or_else(|| panic!("the adopted run's dispatch is no row: {reading}"));
    assert!(
        ["held", "unproven"].contains(&row["proof"].as_str().expect("a proof")),
        "{row}"
    );
    world.release("build.go");
}

/// A runs root that cannot be read is the reading's `root_error`, never one of
/// its `skipped` roots, and the text form says it was refused rather than that the
/// host is idle.
#[test]
fn a_runs_root_that_cannot_be_read_is_the_readings_root_error() {
    let world = World::new("host-view-root").with_env("HOSTNAME", HOST);
    let root = world.root.join("not-a-directory");
    std::fs::write(&root, "a file where the runs root should be").expect("a file");
    let mut json = world.cmd(&["host", "--json"]);
    json.env("ONEPIPELINE_RUNS_DIR", &root);
    let reading = invoke(json).reading();
    has_the_version_1_keys(&reading);
    assert_eq!(reading["root"], json!(root.to_string_lossy()), "{reading}");
    assert!(
        reading["root_error"]
            .as_str()
            .is_some_and(|why| why.starts_with("the runs root cannot be read")),
        "{reading}"
    );
    assert_eq!(reading["skipped"], json!([]), "{reading}");
    assert_eq!(reading["dispatches"], json!([]), "{reading}");
    let mut text = world.cmd(&["host"]);
    text.env("ONEPIPELINE_RUNS_DIR", &root);
    let text = invoke(text);
    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert!(
        text.stdout.contains("1 run root(s) skipped:")
            && text
                .stdout
                .contains("not-a-directory: the runs root cannot be read"),
        "{}",
        text.stdout
    );
}

/// A reading that cannot reach standard output is the CLI's refusal, said on
/// stderr, never a panic: `/dev/full` refuses every write with `ENOSPC`, in both
/// forms.
#[test]
fn a_reading_that_cannot_be_written_is_refused_not_panicked_on() {
    let world = World::new("host-view-full").with_env("HOSTNAME", HOST);
    for args in [&["host"][..], &["host", "--json"][..]] {
        let mut command = world.cmd(args);
        command.stdout(
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .expect("/dev/full"),
        );
        let refused = world.run_on(command, &args.join(" "));
        assert!(
            refused.code != 0 && refused.code != 101,
            "`{}` exited {} onto a full stdout: {}",
            args.join(" "),
            refused.code,
            refused.stderr
        );
        refused.err_has("`host` could not write to standard output");
    }
}

/// The phase record is measurement, never a condition of the answer: asked to
/// append to a path it cannot write, `host` still answers and exits as it always
/// does; asked to append to a file, it appends one line of its phases.
#[test]
fn a_phase_record_that_cannot_be_written_costs_the_read_nothing() {
    let world = World::new("host-view-phases").with_env("HOSTNAME", HOST);
    let mut unwritable = world.cmd(&["host", "--json"]);
    unwritable.env(PHASES_ENV, &world.root);
    let read = invoke(unwritable);
    has_the_version_1_keys(&read.reading());

    let recorded = world.root.join("phases.jsonl");
    let mut written = world.cmd(&["host"]);
    written.env(PHASES_ENV, &recorded);
    let read = invoke(written);
    assert_eq!(read.code, Some(0), "{}", read.stderr);
    assert!(
        read.stdout.contains("no live dispatches"),
        "{}",
        read.stdout
    );
    let lines = std::fs::read_to_string(&recorded).expect("the phase record");
    assert_eq!(lines.lines().count(), 1, "{lines}");
    let line: Value = serde_json::from_str(lines.trim()).expect("one JSON line");
    for name in ["event-registry", "list", "free-space", "render"] {
        assert!(
            line["phases"][name]["wall_ms"].is_number(),
            "no `{name}`: {line}"
        );
    }
}

/// A legacy `ps lstart` token is compared within the one second it is spelled to:
/// a live process whose entry reads a second off what `ps` says now is held, and
/// one two seconds off is an `UNPROVEN` row, never gone.
///
/// `ps` is a double on the view's `PATH` for those two pids alone, answering one
/// fixed start, so the second apart is the test's to set rather than the host
/// clock's; every other pid, and the liveness of these two, is the host's own.
// llmlint: ignore-block[tests_mirror_real_usage] the drift a legacy token suffers is a
// recomputed boot time, which no test may move on a shared host: the `ps` double fixes the
// start it answers for two real live processes, and nothing else. Their run, as an engine
// before procfs tokens left it, is the generator's, for the reason at its head; no build of
// this crate writes that spelling any more. The read is the real binary's.
#[test]
fn a_legacy_start_token_a_second_off_is_held_and_two_seconds_off_is_unproven() {
    let world = World::new("host-view-legacy-second").with_env("HOSTNAME", HOST);
    let mut fixture = Fixture::empty(&world);
    let (near, _) = fixture.sleeper();
    let (far, _) = fixture.sleeper();
    let (driver, driver_token) = fixture.sleeper();
    let real_ps = Command::new("sh")
        .args(["-c", "command -v ps"])
        .output()
        .expect("sh answers");
    let doubles = world.root.join("ps-double");
    std::fs::create_dir_all(&doubles).expect("a directory for the double");
    let double = doubles.join("ps");
    std::fs::write(
        &double,
        format!(
            "#!/bin/sh\ncase \"$2\" in {near}|{far}) echo 'Sun Oct 11 19:23:36 2026'; exit 0;; esac\nexec {} \"$@\"\n",
            String::from_utf8_lossy(&real_ps.stdout).trim()
        ),
    )
    .expect("the double");
    std::fs::set_permissions(&double, std::fs::Permissions::from_mode(0o755))
        .expect("an executable double");
    let started = now_millis() - 600_000;
    fixture.run(
        &world,
        &json!({"run": "legacy-second", "started_ms": started,
        "nodes": ["build"], "ending": [{"dispatched": "build"}],
        "driver": {"pid": driver, "token": driver_token},
        "registry": [
            {"node": "plan", "pid": near, "token": "Sun Oct 11 19:23:37 2026", "recorded_ms": started},
            {"node": "build", "pid": far, "token": "Sun Oct 11 19:23:38 2026", "recorded_ms": now_millis()},
        ]}),
    );
    fixture
        .expected
        .held
        .insert(("legacy-second".to_owned(), "plan".to_owned()));
    fixture
        .expected
        .unproven
        .insert(("legacy-second".to_owned(), "build".to_owned()));
    let mut command = world.cmd(&["host", "--json"]);
    let path = command
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value)
        .expect("the world names a PATH")
        .to_owned();
    let mut leading = doubles.into_os_string();
    leading.push(":");
    leading.push(path);
    command.env("PATH", leading);
    let reading = invoke(command).reading();
    holds(&fixture.expected, &reading, "a second off");
    let build = reading["dispatches"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["node"] == "build")
        .expect("the two-seconds-off entry's row");
    assert!(
        build["unproven"]
            .as_str()
            .is_some_and(|why| why.contains("older `ps` spelling")),
        "{build}"
    );
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A live dispatch whose registry entry an engine before procfs tokens wrote — the
/// `ps -o lstart=` spelling, read in UTC and the C locale — is still a held row: the
/// view asks `ps` for that spelling only when an entry carries it, and compares it
/// to the second it is spelled at, so the second Linux's recomputed boot time can
/// move it by between the two asks does not decide the row. A live process whose
/// legacy entry reads further off — a stepped clock moves it so — is an `UNPROVEN`
/// row with its reason, never gone: that spelling cannot prove a process ended.
#[test]
fn a_live_dispatch_recorded_with_a_legacy_ps_start_token_is_held() {
    let world = World::new("host-view-legacy-token").with_env("HOSTNAME", HOST);
    let mut fixture = Fixture::empty(&world);
    let (pid, _) = fixture.sleeper();
    // Asked of `ps` the way the former engine asked it, rather than of the crate.
    let listed = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .output()
        .expect("ps answers");
    let legacy = String::from_utf8(listed.stdout)
        .expect("UTF-8")
        .trim()
        .to_owned();
    assert!(
        !legacy.is_empty() && !legacy.starts_with("linux-proc-stat:"),
        "{legacy}"
    );
    let (shifted_pid, _) = fixture.sleeper();
    let shifted = Command::new("ps")
        .args(["-p", &shifted_pid.to_string(), "-o", "lstart="])
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .output()
        .expect("ps answers");
    let shifted = String::from_utf8(shifted.stdout).expect("UTF-8");
    // Twelve hours off on the same day: the same spelling and weekday, read as no
    // second near this process's start.
    let fields: Vec<&str> = shifted.split_whitespace().collect();
    let [weekday, month, day, time, year] = fields.as_slice() else {
        panic!("an lstart answer: {shifted}");
    };
    let (hours, rest) = time.split_once(':').expect("a clock");
    let hours: u32 = hours.parse().expect("hours");
    let shifted = format!(
        "{weekday} {month} {day} {:02}:{rest} {year}",
        (hours + 12) % 24
    );
    let (driver, driver_token) = fixture.sleeper();
    let started = now_millis() - 600_000;
    // llmlint: ignore[tests_mirror_real_usage] no build of this crate writes the former spelling any more, so the entry an engine before procfs tokens wrote is written here in the shape it took, naming a real live process; the read is the real binary's.
    fixture.run(
        &world,
        &json!({"run": "legacy", "started_ms": started,
        "nodes": ["build"], "ending": [{"dispatched": "build"}],
        "driver": {"pid": driver, "token": driver_token},
        "registry": [
            {"node": "plan", "pid": pid, "token": legacy, "recorded_ms": started},
            {"node": "build", "pid": shifted_pid, "token": shifted, "recorded_ms": now_millis()},
        ]}),
    );
    fixture
        .expected
        .held
        .insert(("legacy".to_owned(), "plan".to_owned()));
    fixture
        .expected
        .unproven
        .insert(("legacy".to_owned(), "build".to_owned()));
    let (reading, _) = both_forms(&world, "a legacy token");
    holds(&fixture.expected, &reading, "a legacy token");
    let build = reading["dispatches"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["node"] == "build")
        .expect("the shifted entry's row");
    assert!(
        build["unproven"].as_str().is_some_and(
            |why| why.contains("older `ps` spelling") && why.contains(&shifted_pid.to_string())
        ),
        "{build}"
    );
}

/// The `host-view-time` measurement, run whole over this suite's build, records for
/// each scaling size what the kernel says that size's read opened — no ended-journal
/// byte, and the live runs' journals alone — and records nothing when a traced read
/// cannot be observed, even after earlier sizes were.
///
/// Its tracer is the real `strace`, behind a stand-in on `PATH` that notes every
/// trace it is asked for — the measurement's up-front probe of the tracer first, then
/// each traced read — and, where `REFUSE_TRACE` names a read, refuses that one as a
/// host without ptrace would.
#[test]
fn the_budget_measurement_records_what_each_scaling_read_opened() {
    let world = World::new("host-view-measure");
    let _fixture = Fixture::generate(&world);
    let cache = fixture_cache(&world);
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("strace"))
        .find(|strace| strace.is_file())
        .expect("strace on PATH: this journey's claim is what the tracer saw");
    let bin = world.root.join("bin");
    std::fs::create_dir_all(&bin).expect("a bin directory");
    let asked = world.root.join("traces-asked");
    std::fs::write(
        bin.join("strace"),
        format!(
            "#!/bin/sh\n\
             for arg; do case $prev in -o) into=$arg;; esac; prev=$arg; done\n\
             echo \"$into\" >> {asked:?}\n\
             case $into in *\"${{REFUSE_TRACE:-unset}}\"*) echo 'ptrace refused' >&2; exit 1;; esac\n\
             exec {real:?} \"$@\"\n"
        ),
    )
    .expect("the stand-in tracer");
    std::fs::set_permissions(bin.join("strace"), std::fs::Permissions::from_mode(0o755))
        .expect("executable");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // The measurement's strip guard refuses an unstripped binary, so it is handed a
    // stripped copy of this suite's build, and told which step built it.
    let stripped = world.root.join("stripped").join("onepipeline");
    std::fs::create_dir_all(stripped.parent().expect("a parent")).expect("a directory");
    std::fs::copy(crate::harness::binary(), &stripped).expect("a copy of the binary");
    let strip = Command::new("strip")
        .arg(&stripped)
        .status()
        .expect("binutils' strip runs");
    assert!(strip.success(), "strip refused the suite's build");
    let measure = |record: &Path, refuse: Option<&str>, template: &Path| {
        let mut command = Command::new("node");
        command
            .arg(crate::harness::repo_file("scripts/host-view-measure.mjs"))
            .arg(&stripped)
            .arg(record)
            .env(
                "ONEPIPELINE_HOST_VIEW_BUILT_BY",
                "tests/e2e/host_view.rs: a stripped copy of the suite's build",
            )
            .env("PATH", &path)
            .env("ONEPIPELINE_TEST_HOST_FIXTURE", template)
            .stdin(Stdio::null());
        if let Some(refuse) = refuse {
            command.env("REFUSE_TRACE", refuse);
        }
        command.output().expect("node runs the measurement")
    };
    let traces = || -> Vec<String> {
        std::fs::read_to_string(&asked)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| Path::new(line).file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect()
    };

    let record = world.root.join("host-view.json");
    let measured = measure(&record, None, &cache);
    assert!(
        measured.status.success(),
        "{}",
        String::from_utf8_lossy(&measured.stderr)
    );
    assert_eq!(
        traces(),
        [
            "tracer.strace",
            "steady-traced.strace",
            "scaling-400-traced.strace",
            "scaling-800-traced.strace"
        ],
        "the tracer's own probe, then one traced read per size"
    );
    let held = read_json(&record);
    assert_eq!(held["steady"]["strip_guard"], "stripped", "{held}");
    assert_eq!(
        held["steady"]["built_by"], "tests/e2e/host_view.rs: a stripped copy of the suite's build",
        "{held}"
    );
    assert_eq!(
        held["binary"],
        json!(stripped.display().to_string()),
        "{held}"
    );
    let sizes = held["scaling"]["sizes"].as_array().expect("the sizes");
    assert_eq!(sizes.len(), 3, "{held}");
    for (size, roots) in sizes.iter().zip([100, 400, 800]) {
        assert_eq!(size["run_roots"], roots, "{size}");
        assert_eq!(size["ended_journal_bytes_opened"], 0, "{size}");
        assert_eq!(
            size["runs_opened"],
            json!(["live-0", "live-1", "live-2"]),
            "{size}"
        );
        assert_eq!(size["reads_ms"].as_array().map(Vec::len), Some(5), "{size}");
    }

    std::fs::remove_file(&asked).expect("the noted traces");
    let refused_record = world.root.join("refused.json");
    let refused = measure(&refused_record, Some("scaling-400"), &cache);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("scaling-400-traced: `strace ")
            && stderr.contains(" host --json` exited 1: ptrace refused"),
        "{stderr}"
    );
    assert_eq!(
        traces(),
        [
            "tracer.strace",
            "steady-traced.strace",
            "scaling-400-traced.strace"
        ],
        "the measurement stopped at the read it could not observe"
    );
    assert!(
        !refused_record.exists(),
        "a measurement whose read went unobserved recorded it"
    );

    // A template the suite never wrote is refused before anything is measured, with the
    // step that repairs the override or drops it rather than the measurement's own.
    let missing_record = world.root.join("missing-template.json");
    let missing = world.root.join("no-such-template");
    let refused = measure(&missing_record, None, &missing);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!("the host fixture template {}", missing.display()))
            && stderr.contains("next: unset ONEPIPELINE_TEST_HOST_FIXTURE"),
        "{stderr}"
    );
    assert!(
        !missing_record.exists(),
        "a measurement over a template it could not copy recorded it"
    );
}
