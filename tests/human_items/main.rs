//! The human-items read budget: the complete host-wide `human-items list`, timed
//! as a subprocess of the release-built binary over the workload
//! [`generate`] launches, with every phase of what it cost recorded beside it.
//!
//! Its own binary and its own Nx project (`tests/human_items/project.json`,
//! `human-items-e2e`, a budget tier), run only by that project's `test` target,
//! so only a change that selects that project pays for a million-record
//! workload: none of the crate's own test tiers runs this binary. The recording is
//! `target/budget-records/human-items.json`; `report.mjs` reads it and nothing
//! re-measures.
//!
//! What it records, separately:
//! - **cold**: the first read with no index, then after the index is deleted and
//!   after it is corrupted — each a full replay, disclosed, never the budget;
//! - **warm**: ten unchanged reads of each of `list --json`, `list` and `report`,
//!   each asserted to replay nothing — the budget's operation of record;
//! - **tail**: ten item events appended behind a saved index, consumed by exactly
//!   one of the readers that follow.

#![cfg(target_os = "linux")]

mod generate;
#[path = "../e2e/harness.rs"]
mod harness;

use std::path::{Path, PathBuf};
use std::time::Instant;

use harness::World;
use serde_json::{json, Value};

/// The release-built binary: `HUMAN_ITEMS_BINARY`, which the project's `test`
/// target builds and names.
fn binary() -> PathBuf {
    let binary = std::env::var_os("HUMAN_ITEMS_BINARY")
        .map(PathBuf::from)
        .expect("HUMAN_ITEMS_BINARY names the release-built binary; run `just human-items-budget`");
    assert!(binary.is_file(), "{} is not a file", binary.display());
    binary
}

struct Sample {
    elapsed_s: f64,
    stdout: String,
    reads: Vec<Value>,
    process: Value,
}

fn sample(world: &World, args: &[&str], telemetry: &Path) -> Sample {
    let _ = std::fs::remove_file(telemetry);
    let mut all = vec!["human-items"];
    all.extend_from_slice(args);
    let mut command = world.cmd_on(&binary(), &all);
    command.env("ONEPIPELINE_HUMAN_ITEMS_TELEMETRY", telemetry);
    let began = Instant::now();
    let output = command.output().unwrap();
    let elapsed_s = began.elapsed().as_secs_f64();
    assert!(
        output.status.success(),
        "{all:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<Value> = std::fs::read_to_string(telemetry)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    Sample {
        elapsed_s,
        stdout: String::from_utf8(output.stdout).unwrap(),
        reads: lines
            .iter()
            .filter_map(|l| l.get("read").cloned())
            .collect(),
        process: lines
            .iter()
            .find_map(|l| l.get("process").cloned())
            .unwrap_or(Value::Null),
    }
}

fn record(name: &str, s: &Sample) -> Value {
    let read = s.reads.first().cloned().unwrap_or(Value::Null);
    let process_ms = s.process["process_ms"].as_f64().unwrap_or(0.0);
    json!({
        "phase": name,
        "elapsed_s": s.elapsed_s,
        "mode": read["mode"],
        "item_events_replayed": read["item_events_replayed"],
        "item_bytes_replayed": read["item_bytes_replayed"],
        "run_events_replayed": read["run_events_replayed"],
        "runs_rederived": read["runs_rederived"],
        "runs_discovered": read["runs_discovered"],
        "discovery_ms": read["discovery_ms"],
        "replay_ms": read["replay_ms"],
        "index_io_ms": read["index_io_ms"],
        "blocker_ms": read["blocker_ms"],
        "render_ms": s.process["render_ms"],
        "process_ms": process_ms,
        // What the process took that its own phases do not: start, exit and the
        // pipe this harness captured stdout through.
        "capture_ms": (s.elapsed_s * 1000.0 - process_ms).max(0.0),
        "rebuilt_because": read["rebuilt_because"],
    })
}

#[test]
fn the_host_wide_list_is_warm_within_its_budget_and_every_recovery_is_disclosed() {
    let world = World::new("human-items-budget");
    let config = world.root.join("xdg/onepipeline");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.yaml"),
        "human_items:\n  enabled: true\n",
    )
    .unwrap();
    let generated = Instant::now();
    let runs = generate::generate(&world);
    let generate_s = generated.elapsed().as_secs_f64();
    assert_eq!(runs.len(), generate::RUNS);
    let telemetry = world.root.join("telemetry.ndjson");
    let index = world.runs.join("human-items/index.json");

    // Cold: no index yet, then deleted, then corrupt. Each replays every run and
    // every item event, and each answers what the journal holds.
    std::fs::remove_file(&index).ok();
    let cold = sample(&world, &["list", "--json"], &telemetry);
    let authoritative: Value = serde_json::from_str(&cold.stdout).unwrap();
    let views = authoritative.as_array().unwrap();
    // The totals as the binary discovered them: runs off the runs root, steps off
    // each run's own DAG through the host hold's blocked set — which is every
    // undispatched step on the host — and owners and plans off the items.
    let discovered = cold.reads[0]["runs_discovered"].as_u64().unwrap();
    assert_eq!(discovered, 100, "retained runs discovered");
    assert_eq!(views.len(), 100, "open items");
    let owners: std::collections::BTreeSet<&str> = views
        .iter()
        .map(|v| v["item"]["owner"]["session"].as_str().unwrap())
        .collect();
    assert_eq!(owners.len(), 10, "managers");
    let host: Vec<&Value> = views
        .iter()
        .filter(|v| v["item"]["block"]["scope"] == "host")
        .collect();
    assert_eq!(host.len(), 1);
    let blocked = &host[0]["blocked"];
    let steps = blocked["total"].as_u64().unwrap();
    assert_eq!(steps, 275, "steps across every discovered run");
    let plans = blocked["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 10, "plans holding steps");
    let mut sizes: Vec<u64> = plans.iter().map(|p| p["count"].as_u64().unwrap()).collect();
    sizes.sort_unstable();
    assert_eq!(
        sizes,
        (1..=10).map(|n| n * 5).collect::<Vec<u64>>(),
        "5 to 50 steps a plan"
    );
    let detail = sample(
        &world,
        &["show", host[0]["item"]["id"].as_str().unwrap(), "--json"],
        &telemetry,
    );
    let detail: Value = serde_json::from_str(&detail.stdout).unwrap();
    assert_eq!(
        detail["nodes"].as_array().unwrap().len() as u64,
        steps,
        "the full blocked set"
    );
    for view in views {
        assert!(view["item"]["body"].as_str().unwrap().len() > 200);
        for plan in view["blocked"]["plans"].as_array().unwrap() {
            let preview = plan["preview"].as_array().unwrap().len() as u64;
            assert_eq!(preview, plan["count"].as_u64().unwrap().min(3));
        }
    }
    // Each live gate holds its plan's steps behind it; each plan hold, its whole plan.
    let gates: Vec<&Value> = views
        .iter()
        .filter(|v| v["item"]["dedup_key"].as_str().unwrap().ends_with("#gate"))
        .collect();
    assert_eq!(gates.len(), 10, "one live gate per plan");
    let behind: u64 = gates
        .iter()
        .map(|v| v["blocked"]["total"].as_u64().unwrap())
        .sum();
    assert_eq!(
        behind,
        steps - 10,
        "every step but the gates waits behind them"
    );
    let mut phases = vec![record("cold", &cold)];
    std::fs::remove_file(&index).unwrap();
    let deleted = sample(&world, &["list", "--json"], &telemetry);
    assert_eq!(
        serde_json::from_str::<Value>(&deleted.stdout).unwrap(),
        authoritative
    );
    phases.push(record("cold-after-deletion", &deleted));
    std::fs::write(&index, b"{\"version\": 1, \"checksum\": \"not it\"}").unwrap();
    let corrupt = sample(&world, &["list", "--json"], &telemetry);
    assert_eq!(
        serde_json::from_str::<Value>(&corrupt.stdout).unwrap(),
        authoritative
    );
    phases.push(record("cold-after-corruption", &corrupt));
    for p in &phases {
        assert_eq!(p["mode"], "cold", "{p}");
    }

    // Warm: ten unchanged reads of each printed form; none replays anything.
    let mut warm = Vec::new();
    for (form, args) in [
        ("list-json", vec!["list", "--json"]),
        ("list", vec!["list"]),
        ("report", vec!["report"]),
    ] {
        for _ in 0..10 {
            let s = sample(&world, &args, &telemetry);
            let r = record(form, &s);
            assert_eq!(r["mode"], "warm", "{r}");
            assert_eq!(r["item_events_replayed"], 0);
            assert_eq!(r["item_bytes_replayed"], 0);
            assert_eq!(r["run_events_replayed"], 0);
            if form == "list-json" {
                assert_eq!(
                    serde_json::from_str::<Value>(&s.stdout).unwrap(),
                    authoritative
                );
            }
            warm.push(r);
        }
    }
    let warm_max = warm
        .iter()
        .map(|r| r["elapsed_s"].as_f64().unwrap())
        .fold(0.0, f64::max);

    // Tail: ten item events behind a saved index, consumed once across readers.
    let saved = std::fs::read(&index).unwrap();
    for n in 0..10 {
        let path = world.root.join(format!("tail-{n}.json"));
        std::fs::write(
            &path,
            json!({
                "schema_version": 1, "dedup_key": format!("tail-{n}"), "kind": "question",
                "title": format!("Tail question {n}"), "body": "Asked after the index was saved.",
                "priority": "low", "owner": {"session": "manager-00", "run": null, "plan": null},
                "block": {"scope": "nothing"}, "approval": null
            })
            .to_string(),
        )
        .unwrap();
        let mut raise = world.cmd_on(
            &binary(),
            &["human-items", "raise", "--input", path.to_str().unwrap()],
        );
        assert!(raise.output().unwrap().status.success());
    }
    std::fs::write(&index, &saved).unwrap();
    let tailed = sample(&world, &["list", "--json"], &telemetry);
    let again = sample(&world, &["list", "--json"], &telemetry);
    let tail = [record("tail", &tailed), record("tail-again", &again)];
    assert_eq!(tail[0]["mode"], "tail");
    assert_eq!(tail[0]["item_events_replayed"], 10);
    assert_eq!(tail[1]["item_events_replayed"], 0);
    assert_eq!(
        serde_json::from_str::<Value>(&tailed.stdout)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        110
    );

    let recording = json!({
        "host": std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default().trim(),
        "workload": {
            "runs": discovered,
            "events_per_run": generate::EVENTS,
            "plans": plans.len(),
            "managers": owners.len(),
            "steps": steps,
            "open_items": views.len(),
            "generate_s": generate_s,
        },
        "warm_max_elapsed_s": warm_max,
        "warm": warm,
        "cold": phases,
        "tail": tail,
    });
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/budget-records");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(
        out.join("human-items.json"),
        serde_json::to_string_pretty(&recording).unwrap(),
    )
    .unwrap();
}
