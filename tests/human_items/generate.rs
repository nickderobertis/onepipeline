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
//! the rest holding nothing — for 100 open items. Each of those 90 then carries a
//! history of 2 to 5 events that leaves it open — raised and raised again with a
//! new title, or resolved, noted and reopened — and the first manager's first
//! question carries exactly [`LONGEST_HISTORY`] events, ten times the realistic
//! longest of 5: the item the budget's detail read shows. The 10 live gates keep
//! the one raise their run recorded, since resolving one releases its run's steps
//! and so would change the blocked set this workload states. Every run's journal is grown to
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

/// The history of the one item the detail read shows.
pub const LONGEST_HISTORY: usize = 50;

/// The dedup key of that item: the first manager's first question.
pub const LONGEST_KEY: &str = "manager-00-0";

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
            runs.push(run);
        }
        raise_more(world, &manager, &project, plan);
    }
    // Grown last: a hold resolved and reopened while its item's history is made
    // is journalled in the run it holds, so a journal grown before that would
    // end past the size this workload states.
    for run in &runs {
        grow(&world.runs.join(run).join("events.jsonl"), run);
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
        let id = act(
            world,
            manager,
            &["raise", "--input", path.to_str().unwrap()],
        )["item"]["item"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let key = format!("{manager}-{n}");
        let events = if key == LONGEST_KEY {
            LONGEST_HISTORY
        } else {
            2 + (plan + n) % 4
        };
        history(world, manager, &id, &path, &input, events);
    }
}

/// Give the open item `id`, just raised from `input` at `path`, `events` events
/// in all, leaving it open: a second raise with a new title where only one or two
/// are left, and otherwise rounds of a resolution, up to two notes and a reopen.
/// Only a completed or dismissed item takes a note or a reopen, so a task's
/// rounds alternate a completion and a dismissal and a question's are dismissals.
fn history(
    world: &World,
    manager: &str,
    id: &str,
    path: &Path,
    input: &serde_json::Value,
    events: usize,
) {
    let mut left = events - 1;
    let mut round = 0;
    let mut retitled = 0;
    while left > 0 {
        if left < 3 {
            for _ in 0..left {
                retitled += 1;
                let mut again = input.clone();
                again["title"] = json!(format!(
                    "{} (revised {retitled})",
                    input["title"].as_str().unwrap()
                ));
                std::fs::write(path, again.to_string()).unwrap();
                act(
                    world,
                    manager,
                    &["raise", "--input", path.to_str().unwrap()],
                );
            }
            break;
        }
        if input["kind"] == "task" && round % 2 == 0 {
            act(world, manager, &["complete", id]);
        } else {
            act(
                world,
                manager,
                &["dismiss", id, "--reason", "Not needed yet"],
            );
        }
        let notes = (left - 2).min(2);
        for note in 0..notes {
            act(
                world,
                manager,
                &[
                    "note",
                    id,
                    "--text",
                    &format!("Note {note} of round {round}."),
                ],
            );
        }
        act(world, manager, &["reopen", id]);
        left -= 2 + notes;
        round += 1;
    }
}

/// `onepipeline human-items ARGS` as `manager`, answering what it printed.
fn act(world: &World, manager: &str, args: &[&str]) -> serde_json::Value {
    let mut all = vec!["human-items"];
    all.extend_from_slice(args);
    let output = world
        .cmd(&all)
        .env("ONEPIPELINE_LAUNCHER_SESSION", manager)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{all:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null)
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
