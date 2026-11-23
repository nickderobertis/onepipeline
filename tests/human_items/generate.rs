//! The budget's workload at [`Scale::FULL`]: 100 retained runs of 10,000 events each,
//! launched for real and then grown; 10 managers, each with one plan; 275 steps in all;
//! and 100 open human items. Every count below is the full scale's; a [`Scale`] states
//! the same workload with fewer plans, launches, records and history, and
//! [`Scale::shape`] is what it holds at any scale.
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
//! question carries exactly [`Scale::longest_history`] events, ten times the realistic
//! longest of 5: the item the budget's detail read shows. The 10 live gates keep
//! the one raise their run recorded, since resolving one releases its run's steps
//! and so would change the blocked set this workload states. Every run's journal is grown to
//! exactly 10,000 records with relayed `agentgraph` activity, the kind a long
//! run's journal is mostly made of, which a fold reads and nothing acts on.
//!
//! Expiry conditions sit on top, none of them holding: every open item carries a
//! time condition ten years out — each gate's through its node's metadata, resolved
//! when its item is raised — so a fixture reused for months still holds none due; 20
//! of the manager-raised items a node condition on an unsettled step of their plan's
//! live run, 10 a run condition on that live run settling, and 10 a check whose
//! interval is a day, the longest a check may wait. One read made once the store is
//! seeded runs each check, so they are fresh as generated; a fixture restored more
//! than a day later has them due again, and the journey's priming read, which nothing
//! counts or times, runs them once, so every read after it finds none due.

use std::io::Write;
use std::path::Path;

use crate::harness::{agent, human, plan_of, World};
use serde_json::json;

/// How large a workload is: everything else about it is the same at every scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    /// Plans, one manager each, of 5, 10, … steps.
    pub plans: usize,
    /// Launches of each plan; all but the last are retained history.
    pub launches: usize,
    /// Records every run's journal is grown to.
    pub events: usize,
    /// Events in the history of the one item the detail read shows.
    pub longest_history: usize,
}

impl Scale {
    /// The budget's workload: 100 runs of 10,000 records, 10 plans, a 50-event history.
    pub const FULL: Scale = Scale {
        plans: 10,
        launches: 10,
        events: 10_000,
        longest_history: 50,
    };

    /// The lifecycle and recovery tests' workload: the same kinds of thing, a few of each.
    pub const SMALL: Scale = Scale {
        plans: 1,
        launches: 2,
        events: 400,
        longest_history: 6,
    };

    /// The name a fixture of this scale is kept under.
    pub fn name(&self) -> String {
        format!(
            "p{}-l{}-e{}-h{}",
            self.plans, self.launches, self.events, self.longest_history
        )
    }

    /// What a fixture of this scale holds, which a restored one is checked against.
    pub fn shape(&self) -> Shape {
        let runs = self.plans * self.launches;
        Shape {
            runs,
            run_records: (runs * self.events) as u64,
            open_items: self.plans * ITEMS_PER_PLAN,
            managers: self.plans,
            plan_steps: (0..self.plans).map(steps).collect(),
            conditions: (self.plans * CONDITIONS_PER_PLAN) as u64,
            longest_history: self.longest_history,
        }
    }
}

/// The workload's stated shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    /// Retained runs.
    pub runs: usize,
    /// Records across every run's journal.
    pub run_records: u64,
    /// Open human items.
    pub open_items: usize,
    /// Manager sessions, one per plan.
    pub managers: usize,
    /// Steps of each plan, smallest first.
    pub plan_steps: Vec<usize>,
    /// Expiry conditions across the open items, none of them holding.
    pub conditions: u64,
    /// Events in the longest item history.
    pub longest_history: usize,
}

impl Shape {
    /// Steps across every plan.
    pub fn steps(&self) -> usize {
        self.plan_steps.iter().sum()
    }
}

/// Open items per plan: its live gate and the nine its manager raises.
const ITEMS_PER_PLAN: usize = 10;

/// Expiry conditions per plan: a time on each of its ten items, two node conditions,
/// one run condition and one check.
const CONDITIONS_PER_PLAN: usize = 14;

/// The dedup key of the item with the longest history: the first manager's first question.
pub const LONGEST_KEY: &str = "manager-00-0";

/// Ten years: how far out every item's time is, so no reuse of a fixture finds one due.
const HORIZON_SECONDS: u64 = 10 * 365 * 86_400;

/// A day: how long a check waits to run again, the longest a check may.
const DAY_SECONDS: u64 = 86_400;

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

/// Enable human items, launch every run and grow its journal; answers the run ids.
pub fn generate(world: &World, scale: Scale) -> Vec<String> {
    let config = world.root.join("xdg/onepipeline");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.yaml"),
        "human_items:\n  enabled: true\n",
    )
    .unwrap();
    let mut runs = Vec::new();
    for plan in 0..scale.plans {
        let name = format!("plan-{plan:02}");
        let mut nodes = Vec::new();
        let mut gate = human("gate", &[]);
        gate["priority"] = json!(["urgent", "high", "medium", "low"][plan % 4]);
        gate["title"] = json!(format!("Approve the rollout of plan {plan}"));
        gate["task"] = json!(body(plan));
        gate["human_item"] = json!({
            "kind": "task",
            "priority": gate["priority"],
            "title": gate["title"],
            "body": body(plan),
            "expiry": [{"when": "time", "after_seconds": HORIZON_SECONDS}],
        });
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
        for launch in 0..scale.launches {
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
            // The launch says on stderr whether it handed its paused run to a driver that waits
            // on the decision; a hand-off it could not make is that line's reason, read here
            // rather than as the wait for a driver that never came.
            let said = String::from_utf8_lossy(&output.stderr);
            assert!(
                said.contains(&format!("run '{run}' waits on its decision in driver")),
                "the launch of {run} handed its paused run to no waiting driver: {said}"
            );
            // The launch hands its paused run to a driver that waits on the gate. It is ended
            // here, as a host going down would end it, so the item store this workload measures
            // is read by the verbs it times and by nothing else, and the drop below is applied
            // by the reply itself.
            world.paused_and_ended(&run);
            // Every launch but the last is retained history: its planner ends it by
            // dropping its gate and everything behind it.
            if launch + 1 < scale.launches {
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
        let live = runs.last().cloned().expect("the plan's live run");
        raise_more(
            world,
            &manager,
            &project,
            plan,
            &live,
            scale.longest_history,
        );
    }
    // Grown last: a hold resolved and reopened while its item's history is made
    // is journalled in the run it holds, so a journal grown before that would
    // end past the size this workload states.
    for run in &runs {
        grow(
            &world.runs.join(run).join("events.jsonl"),
            run,
            scale.events,
        );
    }
    // One read runs every check once, so each is fresh, and none due, for the reads the
    // budget times.
    act(world, "manager-00", &["list", "--json"]);
    runs
}

/// The check the 10 checked items carry: one that answers not yet.
fn check(world: &World) -> serde_json::Value {
    let script = world.root.join("not-yet.sh");
    if !script.is_file() {
        onepipeline_testfakes::executable(&script, "#!/bin/sh\necho '{\"expired\": false}'\n");
    }
    json!({"when": "check", "command": [script.display().to_string()],
           "cwd": world.root.display().to_string(), "interval_seconds": DAY_SECONDS})
}

/// The nine further items each manager holds open: one holding its plan, one
/// holding the host for the first manager, and the rest holding nothing.
const PRIORITIES: [&str; 4] = ["urgent", "high", "medium", "low"];

fn raise_more(
    world: &World,
    manager: &str,
    project: &str,
    plan: usize,
    live: &str,
    longest: usize,
) {
    for n in 0..ITEMS_PER_PLAN - 1 {
        // Out from each raise: a raise again with a new title moves it with the title.
        let mut expiry = vec![json!({"when": "time", "after_seconds": HORIZON_SECONDS})];
        match n {
            2 | 3 => expiry
                .push(json!({"when": "node", "run": live, "node": format!("step-{:02}", n - 1)})),
            4 => expiry.push(json!({"when": "run", "run": live, "on": "settled"})),
            5 => expiry.push(check(world)),
            _ => {}
        }
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
            "expiry": expiry,
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
            longest
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
/// A reopen clears an item's expiry, so every history ends with a raise again,
/// which arms its conditions once more.
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
        // One event is always left over for the raise again that ends the history.
        let notes = (left - 3).min(2);
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

/// Append relayed activity until the journal holds exactly `events` records.
fn grow(journal: &Path, run: &str, events: usize) {
    let held = std::fs::read_to_string(journal).unwrap().lines().count();
    assert!(held < events, "{run} already holds {held} records");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(journal)
        .unwrap();
    let mut out = std::io::BufWriter::new(&mut file);
    let stream = format!("{run}-graph");
    for seq in 0..events - held {
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
