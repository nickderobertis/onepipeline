//! The budget's workload: 100 retained runs of 10,000 events each, launched for
//! real and then grown; 10 managers, each with one plan; 275 steps in all; and
//! 100 open human items.
//!
//! Ten plans of 5, 10, … 50 steps (275 in all) — a human gate every other step
//! waits on — are each launched by their own manager session through the compiled
//! binary: the production plan store, launch and DAG. Nine launches of each are
//! retained history, ended by their planner dropping their work, which leaves
//! each run's DAG empty and retires its gate's item; the tenth is live, so its
//! gate raises the item that holds its plan's steps. Each manager then raises
//! nine more items through the CLI — one a plan hold, one manager's a host hold,
//! the rest holding nothing — for 100 open items. Every run's journal is grown to
//! exactly 10,000 records with relayed `agentgraph` activity, the kind a long
//! run's journal is mostly made of, which a fold reads and nothing acts on.

use std::io::Write;
use std::path::Path;

use crate::harness::{agent, human, plan_of, World};
use serde_json::json;

/// Runs, events per run, plans and the steps of each plan.
pub const RUNS: usize = 100;
pub const EVENTS: usize = 10_000;
pub const PLANS: usize = 10;

pub fn steps(plan: usize) -> usize {
    5 * (plan + 1)
}

/// A body as long as a real one: what every listing prints in full.
fn body(plan: usize) -> String {
    format!(
        "Approve the rollout of plan {plan}. Read the design, the risk \
         register and the staged rollout notes, then confirm in chat or complete this \
         task. Every step after this one waits on it, so nothing in this launch \
         dispatches until it is resolved; unrelated plans keep running."
    )
}

/// Launch every run and grow its journal; answers the run ids.
pub fn generate(world: &World) -> Vec<String> {
    let mut runs = Vec::new();
    for plan in 0..PLANS {
        let name = format!("plan-{plan:02}");
        let mut nodes = Vec::new();
        let mut gate = human("gate", &[]);
        gate["priority"] = json!(["urgent", "high", "medium", "low"][plan % 4]);
        gate["title"] = json!(format!("Approve the rollout of plan {plan}"));
        gate["task"] = json!(body(plan));
        nodes.push(gate);
        for step in 1..steps(plan) {
            let mut node = agent(&format!("step-{step:02}"), &["gate"]);
            node["title"] = json!(format!("Step {step} of plan {plan}"));
            nodes.push(node);
        }
        let mut document = plan_of(&name, nodes);
        document["concurrency"] = json!(4);
        let project = world.plan(&name, &document);
        // One manager session per plan, named on the launch itself: a world made
        // with `as_session` tears the shared root down when it is dropped.
        let manager = format!("manager-{plan:02}");
        for launch in 0..RUNS / PLANS {
            let before: std::collections::BTreeSet<_> = run_ids(world);
            let output = world
                .cmd(&["start", &project, "--attach"])
                .env("ONEPIPELINE_LAUNCHER_SESSION", &manager)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let run = run_ids(world)
                .difference(&before)
                .next()
                .cloned()
                .expect("the launch made a run");
            // Every launch but the last is retained history: its planner ends it by
            // dropping its gate and everything behind it.
            if launch + 1 < RUNS / PLANS {
                let ended = world
                    .cmd(&["reply", &run])
                    .env("ONEPIPELINE_LAUNCHER_SESSION", &manager)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .and_then(|mut child| {
                        child.stdin.take().unwrap().write_all(
                            json!({"version": 3, "commands": [
                                {"op": "drop", "id": "gate", "dependents": "drop",
                                 "reason": "Superseded by a later launch"}]})
                            .to_string()
                            .as_bytes(),
                        )?;
                        child.wait_with_output()
                    })
                    .unwrap();
                assert!(
                    ended.status.success(),
                    "{}",
                    String::from_utf8_lossy(&ended.stderr)
                );
            }
            grow(&world.runs.join(&run).join("events.jsonl"), &run);
            runs.push(run);
        }
        raise_more(world, &manager, &project, plan);
    }
    runs
}

/// The nine further items each manager holds open: one holding its plan, one
/// holding the host for the first manager, and the rest holding nothing.
const PRIORITIES: [&str; 4] = ["urgent", "high", "medium", "low"];

fn raise_more(world: &World, manager: &str, project: &str, plan: usize) {
    for n in 0..9 {
        let block = match n {
            0 => json!({"scope": "plan", "plan": project}),
            1 if plan == 0 => json!({"scope": "host"}),
            _ => json!({"scope": "nothing"}),
        };
        let input = json!({
            "schema_version": 1,
            "dedup_key": format!("{manager}-{n}"),
            "kind": if n % 2 == 0 { "question" } else { "task" },
            "title": format!("Decide item {n} for plan {plan}"),
            "body": body(plan),
            "priority": PRIORITIES[(plan + n) % 4],
            "owner": {"session": manager, "run": null, "plan": project},
            "block": block,
            "approval": null,
        });
        let path = world.root.join(format!("raise-{manager}-{n}.json"));
        std::fs::write(&path, input.to_string()).unwrap();
        let output = world
            .cmd(&["human-items", "raise", "--input", path.to_str().unwrap()])
            .env("ONEPIPELINE_LAUNCHER_SESSION", manager)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn run_ids(world: &World) -> std::collections::BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(&world.runs) else {
        return Default::default();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("launch.json").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Append relayed activity until the journal holds exactly [`EVENTS`] records.
fn grow(journal: &Path, run: &str) {
    let held = std::fs::read_to_string(journal).unwrap().lines().count();
    assert!(held < EVENTS, "{run} already holds {held} records");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(journal)
        .unwrap();
    let mut out = std::io::BufWriter::new(&mut file);
    let stream = format!("{run}-graph");
    for seq in 0..EVENTS - held {
        let record = json!({
            "v": 1,
            "ts": format!("2026-10-30T00:{:02}:{:02}.{:03}Z", (seq / 60_000) % 60, (seq / 1000) % 60, seq % 1000),
            "stream": stream,
            "seq": seq,
            "source": "agentgraph",
            "kind": "turn-activity",
            "labels": {"run_id": stream, "node": "step-01", "member": "worker", "persona": "engineer"},
            "payload": {"kind": "tool_call", "name": "bash", "detail": "cargo test", "tool_call_id": format!("toolu_{seq}"), "index": seq % 400},
            "artifacts": [],
        });
        writeln!(out, "{record}").unwrap();
    }
    out.flush().unwrap();
}
