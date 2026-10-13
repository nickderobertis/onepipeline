//! The pool-maintenance workload `pool-maintenance-engine-seconds` reads: one idle
//! maintenance sweep over 25 real pooled slots across 20 identities, each declaring
//! in its own `onevcs.toml` a command that does nothing, then one `onepipeline
//! results` over that run.
//!
//! The engine's share is read from what the run's journal already records — the
//! sweep's `started_at`, the `pool-maintenance` record's own `ts`, and each slot's
//! `duration_ms` — and the `results` render is timed here, around the subprocess.
//! Nothing in the engine measures for it. Only once every assertion has passed is
//! the recording written under `target/budget-records/`, which
//! `pool-maintenance-budget.mjs` reads.
//!
//! Its own binary and its own Nx project (`tests/pool_workload/project.json`), run
//! only by that project's `workload` target, so no per-change selection pays for it.
//! Unix only: the declared command is `true` on `PATH`.

#![cfg(unix)]

// llmlint: ignore-file[e2e_not_mocked] the sibling under test is not substituted: `onevcs`
// is the library this crate links, every slot is cut by a real session over real git, and
// every declaration is a real `onevcs.toml` that library reads. `oneagentgraph` is the
// double, because the workload is the idle capacity beside a dispatch and a real agent turn
// is a paid one.

#[path = "../e2e/harness.rs"]
mod harness;

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Value};

use harness::{agent, git, plan_of, World};

/// The identities the sweep visits: the 20 repositories that declare maintenance
/// on this host.
const IDENTITIES: usize = 20;

/// The identities holding a second slot, so the sweep maintains 25.
const DOUBLED: usize = 5;

const RUN: &str = "workload";

/// Where the recording is written, which the budget command reads.
const RECORDING: &str = "target/budget-records/pool-maintenance.json";

/// Size every identity's pool, naming no host `maintain`: what runs is each
/// repository's own declaration.
fn sized_pools(world: &World, names: &[String]) {
    let mut document = "version: 1\nrules:\n".to_owned();
    for (at, name) in names.iter().enumerate() {
        let slots = if at < DOUBLED { 2 } else { 1 };
        document.push_str(&format!(
            "  - match: {{host: github.com, owner: owner, name: {name}}}\n    pool: {slots}\n    \
             overflow: 0\n"
        ));
    }
    std::fs::write(world.onevcs_home().join("workspaces.yml"), document)
        .expect("the workspaces file is written");
}

/// Commit and push a declaration of a command that does nothing.
fn declare(world: &World, checkout: &Path) {
    std::fs::write(
        checkout.join("onevcs.toml"),
        "schema_version = 1\n\n[pool.maintain]\ncommand = [\"true\"]\n",
    )
    .expect("the declaration is written");
    git(world, checkout, &["add", "-A"]);
    git(
        world,
        checkout,
        &["commit", "-m", "chore: declare the pool maintenance"],
    );
    git(world, checkout, &["push", "origin", "main"]);
}

/// Cut `n` slots: `n` real sessions open at once, then closed.
fn cut_slots(world: &World, checkout: &Path, n: usize) {
    world.on_onevcs(|| {
        let vcs = onevcs::Providers::real().vcs;
        let tokens: Vec<_> = (0..n)
            .map(|_| {
                vcs.open_session(onevcs::SessionRequest {
                    repo: checkout.to_string_lossy().into_owned(),
                    branch: None,
                    branch_name: None,
                    branch_prefix: None,
                    base: None,
                    execution_checkout: None,
                    pool: None,
                    overflow: None,
                    labels: Default::default(),
                    refuse_conflicts: false,
                })
                .expect("a session opens on the pooled identity")
                .token
            })
            .collect();
        for token in &tokens {
            vcs.close_session(token)
                .expect("the session closes and hands the slot back");
        }
    });
}

#[test]
fn one_sweep_over_twenty_declaring_identities_is_recorded_for_the_budget() {
    let world = World::new("pool-workload")
        .with_env(onepipeline::executor::LOAD1_ENV, "0")
        .with_env(onepipeline::maintenance::PACE_ENV, "1");
    let mut repos: Vec<(String, PathBuf)> = vec![(
        "service".to_owned(),
        world.repository("local-direct", &[]).checkout,
    )];
    for at in 1..IDENTITIES {
        let name = format!("identity-{at:02}");
        let checkout = world.extra_repository(&name).checkout;
        repos.push((name, checkout));
    }
    let names: Vec<String> = repos.iter().map(|(name, _)| name.clone()).collect();
    sized_pools(&world, &names);
    let mut slots = 0;
    for (at, (_, checkout)) in repos.iter().enumerate() {
        declare(&world, checkout);
        let n = if at < DOUBLED { 2 } else { 1 };
        cut_slots(&world, checkout, n);
        slots += n;
    }

    let schedule = world.root.join("schedule.yml");
    std::fs::write(&schedule, "version: 1\ndefault:\n  every: 1h\n").expect("a schedule");
    world.script(&format!("{RUN}-build.wait"), "hold");
    let mut plan = plan_of(RUN, vec![agent(&format!("{RUN}-build"), &[])]);
    plan["concurrency"] = json!(2);
    let path = world.plan(RUN, &plan);
    world
        .run(&[
            "start",
            path.as_str(),
            "--detach",
            "--maintenance-config",
            &schedule.to_string_lossy(),
        ])
        .exited(0);
    world.until("the record of the sweep", |world| {
        !world.events_of(RUN, "pool-maintenance").is_empty()
    });
    world.release(&format!("{RUN}-build.go"));
    world.until("the run to settle", |world| {
        world.run_file(RUN, "result.json").is_file()
    });

    let record = world.events_of(RUN, "pool-maintenance")[0].clone();
    let identities = record["payload"]["identities"]
        .as_array()
        .expect("identities");
    assert_eq!(identities.len(), IDENTITIES, "{record}");
    let durations: Vec<u64> = identities
        .iter()
        .flat_map(|entry| {
            entry["outcome"]["slots"]
                .as_array()
                .unwrap_or_else(|| panic!("no slots: {entry}"))
                .clone()
        })
        .map(|slot| {
            let ran = &slot["outcome"]["ran"];
            assert_eq!(ran["outcome"], "succeeded", "{record}");
            ran["duration_ms"].as_u64().expect("a duration")
        })
        .collect();
    assert_eq!(durations.len(), slots, "{record}");

    let rendering = Instant::now();
    let results = world.run(&["results", RUN]);
    let results_seconds = rendering.elapsed().as_secs_f64();
    results
        .exited(0)
        .out_has("pool maintenance: last sweep that did something started");
    for name in &names {
        results.out_has(&format!("github.com/owner/{name} (every 1h): slot 1 ran"));
    }

    let started_at = record["payload"]["started_at"]
        .as_str()
        .expect("the sweep's start");
    let journalled_at = record["ts"].as_str().expect("the record's own time");
    let recording: Value = json!({
        "run": RUN,
        "started_at": started_at,
        "journalled_at": journalled_at,
        "slot_duration_ms": durations,
        "results_seconds": results_seconds,
        "identities": identities.len(),
        "slots": slots,
    });
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join(RECORDING);
    std::fs::create_dir_all(target.parent().expect("a directory")).expect("the records directory");
    std::fs::write(
        &target,
        serde_json::to_string_pretty(&recording).expect("the recording serializes"),
    )
    .expect("the recording is written");
}
