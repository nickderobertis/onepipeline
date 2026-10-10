//! Flows — `onepipeline flow run`, what `unwatched` and `stop-guard` owe one,
//! `watch --flow`, and the flow's own channel — driven through the compiled
//! binary.
//!
//! What this exists for is the hole entry 114 of `docs/contract-divergences.md`
//! records: a manager that started a multi-stage launcher was owed nothing
//! whenever no run happened to exist, so its turn ended between two stages and
//! what the launcher did next woke nobody. Every journey runs a **real** flow — a
//! shell program under a real `flow run`, which launches real runs with the real
//! binary between steps of its own — and every claim is read off that binary's
//! streams, its exit status, or the files it wrote. Each step of a program waits
//! on a file this journey writes, so a journey says when the flow moves on.
//!
//! Unix-only: the programs are shell scripts, and `flow run`'s forwarding of
//! terminating signals is a Unix seam.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven here as a
// real compiled binary against a real run store; `harness.rs` carries the same suppression
// and the full rationale. Every claim below is read off that binary's own streams.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{
    agent, plan_of, World, NODE_SETTLED, REFUSED, RUNS_UNWATCHED, SURFACE_WAITING, WATCH_ELAPSED,
};

use onepipeline::cli::{FLOW_ENV, WAKE_BUDGET_ENV, WAKE_RESERVE_SECONDS};
use onepipeline::error::{EXIT_FLOW_FAILED, EXIT_RUN_JOINED, EXIT_SUCCESS};

/// The variable the harness names the binary under test in, which a flow's
/// program launches runs with.
const CLI: &str = "\"$ONEPIPELINE_FAKE_CLI_BIN\"";

/// One flow this journey started: the `flow run` process, and where its
/// program's steps meet the journey.
struct Flowing {
    id: String,
    child: Child,
    steps: PathBuf,
    stderr: PathBuf,
}

/// The directory a world's programs mark their steps in and wait on gates in.
fn steps(world: &World) -> PathBuf {
    let steps = world.root.join("steps");
    std::fs::create_dir_all(&steps).expect("a steps directory");
    steps
}

/// A program line launching the run `project` names, detached.
fn launch(project: &str) -> String {
    format!("{CLI} start '{project}' --detach >/dev/null || exit 70")
}

/// A program line saying it reached `step`.
fn mark(world: &World, step: &str) -> String {
    format!("touch '{}'", steps(world).join(step).display())
}

/// A program line waiting until the journey opens `gate` — the non-run step a
/// launcher spends between its runs.
fn gate(world: &World, gate: &str) -> String {
    format!(
        "while [ ! -f '{}' ]; do sleep 0.05; done",
        steps(world).join(format!("{gate}.go")).display()
    )
}

impl Flowing {
    /// Start `flow run --name <id> -- sh <script>` from `world`, and return once
    /// the flow is registered.
    fn start(world: &World, id: &str, script: &[String]) -> Self {
        let steps = steps(world);
        let path = world.root.join(format!("{id}.sh"));
        std::fs::write(&path, script.join("\n") + "\n").expect("the program is written");
        let stderr = world.root.join(format!("{id}.stderr"));
        let child = world
            .cmd(&[
                "flow",
                "run",
                "--name",
                id,
                "--",
                "sh",
                path.to_str().expect("a path"),
            ])
            // A manager's flow, started outside any dispatch: the run id this
            // suite's own dispatch may carry is not the program's, and `ask`
            // inside it asks on the flow because it names no run.
            .env_remove("ONEPIPELINE_RUN_ID")
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&stderr).expect("a stderr file"))
            .spawn()
            .expect("flow run starts");
        let flowing = Self {
            id: id.to_owned(),
            child,
            steps,
            stderr,
        };
        world.until("the flow to register", |world| {
            world
                .runs
                .join(".flows")
                .join(id)
                .join("flow.json")
                .is_file()
        });
        flowing
    }

    /// Wait until the program has reached `step`.
    fn reached(&self, world: &World, step: &str) {
        let marked = self.steps.join(step);
        world.until(&format!("flow {} to reach {step}", self.id), |_| {
            marked.is_file()
        });
    }

    /// Let the program past `gate`.
    fn open(&self, gate: &str) {
        std::fs::write(self.steps.join(format!("{gate}.go")), "go").expect("the gate opens");
    }

    /// What `flow run` wrote on standard error.
    fn said(&self) -> String {
        std::fs::read_to_string(&self.stderr).unwrap_or_default()
    }

    /// Wait for `flow run` to exit, and answer its status.
    fn ended(mut self) -> i32 {
        self.child
            .wait()
            .expect("flow run is waited for")
            .code()
            .unwrap_or(-1)
    }
}

/// A real `watch --flow` this journey armed, and what it said once it returned.
struct Watching {
    child: Child,
}

/// Arm `watch --flow <flow>` from `world` with `args`, and return once its lease
/// is on disk beside the flow.
fn watching(world: &World, flow: &str, args: &[&str]) -> Watching {
    let mut argv = vec!["watch", "--flow", flow];
    argv.extend_from_slice(args);
    let child = world
        .cmd(&argv)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the watch starts");
    let mine = format!("{}-", child.id());
    let leases = world.runs.join(".flows").join(flow).join("watchers");
    world.until("the flow watch to record itself", |_| {
        std::fs::read_dir(&leases).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(&mine))
            })
        })
    });
    Watching { child }
}

/// How one watch returned: its status, its return record, and its human lines.
struct Returned {
    code: i32,
    record: Value,
    human: String,
}

impl Watching {
    fn returned(self) -> Returned {
        let output = self.child.wait_with_output().expect("the watch returns");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let human = String::from_utf8_lossy(&output.stderr).into_owned();
        let record = stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|record| record["watch"] == "return")
            .unwrap_or_else(|| panic!("the watch printed no return record: {stdout}\n{human}"));
        Returned {
            code: output.status.code().unwrap_or(-1),
            record,
            human,
        }
    }

    fn end(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Returned {
    /// Assert the status and the condition, and that the human ending line
    /// carries the cursor its record does.
    fn ended_on(&self, code: i32, condition: &str) -> &Self {
        assert_eq!(
            (self.code, self.record["condition"].as_str()),
            (code, Some(condition)),
            "{}\n{}",
            self.record,
            self.human
        );
        let cursor = self.cursor();
        assert!(
            self.human.contains(&format!("cursor {cursor}")),
            "the ending line carries no `cursor {cursor}`: {}",
            self.human
        );
        self
    }

    fn cursor(&self) -> String {
        self.record["cursor"]
            .as_str()
            .expect("every ending carries a cursor")
            .to_owned()
    }
}

/// A run whose one node is held dispatched until the journey releases it.
fn held_plan(world: &World, name: &str, node: &str) -> String {
    world.script(&format!("{node}.wait"), "hold");
    world.plan(name, &plan_of(name, vec![agent(node, &[])]))
}

/// The one verdict `stop-guard` answers for `world`'s session.
fn verdict(world: &World, args: &[&str]) -> Value {
    let mut argv = vec!["stop-guard", "--session", world.session.as_str()];
    argv.extend_from_slice(args);
    let asked = world.run(&argv);
    asked.exited(0);
    serde_json::from_str(asked.stdout.trim()).expect("one verdict object")
}

fn unwatched(world: &World, args: &[&str]) -> crate::harness::Run {
    let mut argv = vec!["unwatched", "--session", world.session.as_str()];
    argv.extend_from_slice(args);
    world.run(&argv)
}

/// A flow whose program launches a real run, spends a non-run step, launches a
/// second, and ends cleanly holds its session for its whole life, and its watch
/// wakes the session on each thing it does.
///
/// In order: the one line `flow run` prints; the run's launch record naming the
/// flow; the stop guard blocking on the live flow nothing watches; a live
/// `watch --flow` clearing it and counting the member run as watched;
/// `run-joined` (8) on the second launch; `surface` (4) on a member run's
/// planner surface; `0` on the program's clean exit, which `flow run` exits with;
/// and, once the flow is not live, each member run owed a watch of its own.
#[test]
fn a_flow_holds_its_session_between_runs_and_its_watch_wakes_on_each() {
    let world = World::new("flow-stages");
    let draft = held_plan(&world, "draft", "drafting");
    let finalize = held_plan(&world, "finalize", "finalizing");
    let flow = Flowing::start(
        &world,
        "plan",
        &[
            launch(&draft),
            mark(&world, "drafted"),
            gate(&world, "reviewed"),
            launch(&finalize),
            mark(&world, "finalized"),
            gate(&world, "done"),
        ],
    );
    flow.reached(&world, "drafted");
    let said = flow.said();
    assert_eq!(
        said.matches("watch it with").count(),
        1,
        "`flow run` did not print exactly one line before its program: {said:?}"
    );
    assert!(
        said.starts_with("flow plan: watch it with: onepipeline watch --flow plan\n"),
        "{said:?}"
    );
    world.until("the draft run to dispatch", |world| {
        !world.events_of("draft", "node-dispatched").is_empty()
    });
    assert_eq!(
        world.run_json("draft", "launch.json")["flow"],
        json!("plan")
    );

    // Live, and nothing watches it: the guard holds the session, naming the flow
    // and the watch that clears it.
    let blocked = verdict(&world, &[]);
    assert_eq!(blocked["verdict"], json!("block"), "{blocked}");
    let reason = blocked["reason"].as_str().expect("a block says why");
    assert!(
        reason.contains("flow plan ") && reason.contains("onepipeline watch --flow plan\n"),
        "{reason}"
    );

    // A live flow watch clears it, and counts the member run as watched.
    let watch = watching(&world, "plan", &["--timeout", "600"]);
    assert_eq!(verdict(&world, &[]), json!({"verdict": "none"}));
    unwatched(&world, &[]).exited(EXIT_SUCCESS);

    // The non-run step ends and the next run launches: the watch wakes on it.
    flow.open("reviewed");
    let joined = watch.returned();
    joined.ended_on(EXIT_RUN_JOINED, "run-joined");
    assert_eq!(
        joined.record["run_id"],
        json!("finalize"),
        "{}",
        joined.record
    );
    assert!(
        joined.human.contains("run-joined finalize"),
        "{}",
        joined.human
    );
    flow.reached(&world, "finalized");
    world.until("the finalize run to dispatch", |world| {
        !world.events_of("finalize", "node-dispatched").is_empty()
    });

    // Re-armed from its cursor: a member run's planner surface wakes it.
    let watch = watching(
        &world,
        "plan",
        &["--timeout", "600", "--cursor", &joined.cursor()],
    );
    world
        .run(&[
            "surface",
            "finalize",
            "--kind",
            "finding",
            "--message",
            "the finalize planner has a finding",
        ])
        .exited(0);
    let surfaced = watch.returned();
    surfaced.ended_on(SURFACE_WAITING, "surface");
    assert_eq!(surfaced.record["run_id"], json!("finalize"));
    world.run(&["next", "finalize"]).exited(0);

    // A member run's node settling wakes it, naming the run and the node.
    let watch = watching(
        &world,
        "plan",
        &["--timeout", "600", "--cursor", &surfaced.cursor()],
    );
    world.release("finalizing.go");
    let settled = watch.returned();
    settled.ended_on(NODE_SETTLED, "node-settled");
    assert_eq!(
        (&settled.record["run_id"], &settled.record["node"]),
        (&json!("finalize"), &json!("finalizing")),
        "{}",
        settled.record
    );

    // The program ends cleanly: the watch returns `0`, and so does `flow run`.
    // Told `--until run-joined` alone, so nothing the settled run says after
    // ends it first: the flow's ending ends every wait.
    let watch = watching(
        &world,
        "plan",
        &[
            "--timeout",
            "600",
            "--until",
            "run-joined",
            "--cursor",
            &settled.cursor(),
        ],
    );
    flow.open("done");
    watch.returned().ended_on(EXIT_SUCCESS, "ended");
    assert_eq!(flow.ended(), 0);

    // Not live, the flow is owed nothing, and each member run is judged on its
    // own: the one still working owed a watch of its own, the settled one its
    // closure.
    let after = unwatched(&world, &[]);
    after.exited(RUNS_UNWATCHED);
    assert!(!after.stdout.contains("flow plan"), "{}", after.stdout);
    let line = |run: &str| {
        after
            .stdout
            .lines()
            .find(|line| line.starts_with(&format!("{run} ")))
            .unwrap_or_else(|| panic!("{run} was not judged on its own: {}", after.stdout))
            .to_owned()
    };
    assert!(line("draft").ends_with("watch draft"), "{}", after.stdout);
    assert!(
        line("finalize").contains("--acknowledge finalize"),
        "{}",
        after.stdout
    );
}

/// A flow whose program ends non-zero wakes its watch with `flow-failed` (9),
/// naming the status and the command that closes it; `flow run` exits with that
/// status; and the flow is owed closure — reported, exit 6 — until a reason
/// closes it.
#[test]
fn a_flow_that_ends_non_zero_is_owed_closure_until_acknowledged() {
    let world = World::new("flow-fails");
    let flow = Flowing::start(&world, "ship", &[gate(&world, "fail"), "exit 3".into()]);
    let watch = watching(&world, "ship", &["--timeout", "600"]);
    flow.open("fail");
    let failed = watch.returned();
    failed.ended_on(EXIT_FLOW_FAILED, "flow-failed");
    assert_eq!(failed.record["status"], json!(3));
    assert!(
        failed.human.contains("ended with status 3")
            && failed
                .human
                .contains("onepipeline unwatched --acknowledge-flow ship --reason <TEXT>"),
        "{}",
        failed.human
    );
    assert_eq!(flow.ended(), 3);

    let owed = unwatched(&world, &[]);
    owed.exited(RUNS_UNWATCHED);
    assert_eq!(
        owed.stdout,
        format!(
            "{:<24} {:<12} it ended with status 3 — close it with: onepipeline unwatched \
             --acknowledge-flow ship --reason <TEXT>\n",
            "flow ship", "ENDED"
        )
    );
    assert_eq!(verdict(&world, &[])["verdict"], json!("block"));

    // A reason is required, and a closure is recorded beside the flow.
    world
        .run(&["unwatched", "--acknowledge-flow", "ship"])
        .exited(REFUSED);
    world
        .run(&[
            "unwatched",
            "--acknowledge-flow",
            "ship",
            "--reason",
            "the release was rolled back by hand",
        ])
        .exited(EXIT_SUCCESS)
        .out_has("flow ship: acknowledged for session");
    unwatched(&world, &[]).exited(EXIT_SUCCESS);
    assert_eq!(verdict(&world, &[]), json!({"verdict": "none"}));
}

/// A flow whose holder is killed with SIGKILL has died: it is reported as died,
/// naming the command that closes it, a watch of it returns `flow-failed`, and
/// it stays owed until `--acknowledge-flow` with a reason closes it.
#[test]
fn a_flow_whose_holder_is_killed_is_reported_died_until_acknowledged() {
    let world = World::new("flow-dies");
    let mut flow = Flowing::start(&world, "deploy", &[gate(&world, "never")]);
    flow.child.kill().expect("SIGKILL reaches the holder");

    let died = unwatched(&world, &[]);
    died.exited(RUNS_UNWATCHED)
        .out_has("flow deploy")
        .out_has("DIED")
        .out_has("it died")
        .out_has("onepipeline unwatched --acknowledge-flow deploy --reason <TEXT>");
    let watch = watching(&world, "deploy", &["--timeout", "600"]);
    let returned = watch.returned();
    returned.ended_on(EXIT_FLOW_FAILED, "flow-failed");
    assert_eq!(returned.record["died"], json!(true));
    // Reaped now, and still died: the holder is gone with no ending recorded.
    let _ = flow.child.wait();
    unwatched(&world, &[])
        .exited(RUNS_UNWATCHED)
        .out_has("DIED");

    world
        .run(&[
            "unwatched",
            "--acknowledge-flow",
            "deploy",
            "--reason",
            "redeployed by hand",
        ])
        .exited(EXIT_SUCCESS);
    unwatched(&world, &[]).exited(EXIT_SUCCESS);
    // The program the holder left behind is let go.
    flow.open("never");
}

/// A `flow run` inside a live flow of the same session joins it — registering
/// nothing and printing nothing — and a run launched there is the outer flow's;
/// and no node dispatch of that run is handed `ONEPIPELINE_FLOW`.
#[test]
fn a_nested_flow_joins_its_parent_and_no_dispatch_carries_the_flow() {
    let world = World::new("flow-nested");
    let nested = world.plan("nested", &plan_of("nested", vec![agent("nestedwork", &[])]));
    let steps = steps(&world);
    let inner = format!(
        "printf %s \"${FLOW_ENV}\" > '{seen}'; {CLI} start '{nested}' --attach >/dev/null",
        seen = steps.join("inner.flow").display()
    );
    let flow = Flowing::start(
        &world,
        "outer",
        &[
            format!(
                "{CLI} flow run --name inner -- sh -c '{}' 2> '{}' || exit 71",
                inner.replace('\'', "'\\''"),
                steps.join("inner.stderr").display()
            ),
            mark(&world, "nested"),
            gate(&world, "end"),
        ],
    );
    flow.reached(&world, "nested");
    assert_eq!(
        std::fs::read_to_string(steps.join("inner.flow")).expect("the nested program ran"),
        "outer"
    );
    assert!(
        !world.runs.join(".flows").join("inner").exists(),
        "a nested `flow run` registered a flow of its own"
    );
    let nested_said = std::fs::read_to_string(steps.join("inner.stderr")).unwrap_or_default();
    assert!(
        !nested_said.contains("watch it with"),
        "a nested `flow run` printed a line: {nested_said}"
    );
    assert_eq!(
        world.run_json("nested", "launch.json")["flow"],
        json!("outer")
    );

    let turns: Vec<Value> = world
        .journal("nested")
        .into_iter()
        .filter(|event| event["kind"] == "turn-activity")
        .collect();
    assert!(!turns.is_empty(), "the nested run dispatched no turn");
    for turn in &turns {
        assert!(
            turn["payload"].get("runs_dir").is_some(),
            "the double reported nothing of its environment: {turn}"
        );
        assert_eq!(
            turn["payload"]["flow"],
            Value::Null,
            "a node dispatch was handed {FLOW_ENV}: {turn}"
        );
    }
    flow.open("end");
    assert_eq!(flow.ended(), 0);
}

/// A flow that launches no run reports and asks on its own channel: a surface
/// raised on it ends a live `watch --flow` with `4`, `next --flow` reads it, and
/// an `ask` from inside the program waits on the flow's channel until
/// `reply --flow` answers it. A reply carrying commands is refused whole.
#[test]
fn a_flow_that_launches_no_run_reports_and_asks_on_its_own_channel() {
    let world = World::new("flow-channel");
    let steps = steps(&world);
    let flow = Flowing::start(
        &world,
        "deploy",
        &[
            gate(&world, "ask"),
            format!(
                "{CLI} ask 'Promote the canary to production?' > '{}' || exit 72",
                steps.join("answer.json").display()
            ),
            mark(&world, "answered"),
            gate(&world, "end"),
        ],
    );

    let watch = watching(&world, "deploy", &["--timeout", "600"]);
    world
        .run(&[
            "surface",
            "--flow",
            "deploy",
            "--kind",
            "finding",
            "--message",
            "the canary is at 5%",
        ])
        .exited(0)
        .out_has("\"state\":\"queued\"");
    let surfaced = watch.returned();
    surfaced.ended_on(SURFACE_WAITING, "surface");
    assert!(
        surfaced.record.get("run_id").is_none()
            && surfaced.human.contains("surface on flow deploy"),
        "{}\n{}",
        surfaced.record,
        surfaced.human
    );
    let read = world.run(&["next", "--flow", "deploy"]);
    read.exited(0);
    let next = read.json();
    assert_eq!(next["status"], json!("surface"), "{next}");
    assert_eq!(next["surface"]["message"], json!("the canary is at 5%"));
    assert_eq!(next["events"], json!([]));

    // The program asks, and waits on the flow's channel.
    flow.open("ask");
    let question = {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let queue = world.run(&["channel", "queue", "--flow", "deploy"]);
            queue.exited(0);
            let found = queue.json()["waiting"].as_array().and_then(|waiting| {
                waiting
                    .iter()
                    .find(|surface| surface["kind"] == "planner-question")
                    .cloned()
            });
            if let Some(found) = found {
                break found;
            }
            assert!(
                Instant::now() < deadline,
                "the program's question never arrived"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let correlation = question["correlation"]
        .as_str()
        .expect("a blocking question carries its correlation");
    world
        .run_with_stdin(
            &["reply", "--flow", "deploy", "--correlation", correlation],
            &json!({"version": 2, "completion": false, "message": "promote it"}).to_string(),
        )
        .exited(EXIT_SUCCESS);
    flow.reached(&world, "answered");
    let answer: Value = serde_json::from_str(
        std::fs::read_to_string(steps.join("answer.json"))
            .expect("the answer")
            .trim(),
    )
    .expect("one answer object");
    assert_eq!(answer["answer"], json!("reply"), "{answer}");
    assert_eq!(answer["reply"]["reply"]["message"], json!("promote it"));

    // A flow has no graph: a reply carrying commands is refused, nothing queued.
    world
        .run_with_stdin(
            &["reply", "--flow", "deploy"],
            &json!({"version": 2, "commands": [{"op": "attest", "ref": "x"}]}).to_string(),
        )
        .exited(REFUSED)
        .err_has("no graph");
    flow.open("end");
    assert_eq!(flow.ended(), 0);
}

/// Under a wake budget a `watch --flow` given no `--timeout` returns within it,
/// measured on the wall clock; one whose `--timeout` equals the budget does not
/// qualify, naming the reserve; and with the budget as a flag, the line names
/// `--timeout <budget less the reserve>`, which qualifies.
#[test]
fn a_flow_watch_returns_within_its_wake_budget_and_only_a_reserved_one_qualifies() {
    let world = World::new("flow-reserve");
    let flow = Flowing::start(&world, "quiet", &[gate(&world, "end")]);
    let budgeted = world
        .as_session(&world.session)
        .with_env(WAKE_BUDGET_ENV, "10");

    // No event, no timeout: it gives up at the budget less the reserve.
    let spawned = Instant::now();
    let watch = budgeted
        .cmd(&["watch", "--flow", "quiet"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the watch starts");
    let output = watch.wait_with_output().expect("the watch returns");
    let took = spawned.elapsed();
    assert_eq!(output.status.code(), Some(WATCH_ELAPSED), "{output:?}");
    assert!(
        took <= Duration::from_secs(10),
        "the watch took {took:?} to return under a 10s wake budget"
    );
    assert!(
        took >= Duration::from_secs(10 - WAKE_RESERVE_SECONDS - 1),
        "the watch returned after {took:?}, before the budget less the reserve"
    );

    // A watch whose timeout is the budget does not qualify, and says why.
    let at_the_budget = watching(&budgeted, "quiet", &["--timeout", "10"]);
    unwatched(&budgeted, &[])
        .exited(RUNS_UNWATCHED)
        .out_has("flow quiet")
        .out_has(&format!("less its {WAKE_RESERVE_SECONDS}s wake reserve"));
    at_the_budget.end();

    // Given as a flag, the line names the watch that qualifies...
    let named = "onepipeline watch --flow quiet --timeout 8";
    unwatched(&world, &["--wake-budget", "10"])
        .exited(RUNS_UNWATCHED)
        .out_has(&format!("{named}\n"));
    let blocked = verdict(&world, &["--wake-budget", "10"]);
    assert_eq!(blocked["verdict"], json!("block"));
    assert!(
        blocked["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains(named)),
        "{blocked}"
    );
    // ...and that very command counts.
    let qualifying = watching(&world, "quiet", &["--timeout", "8"]);
    unwatched(&world, &["--wake-budget", "10"]).exited(EXIT_SUCCESS);
    assert_eq!(
        verdict(&world, &["--wake-budget", "10"]),
        json!({"verdict": "none"})
    );
    qualifying.end();
    flow.open("end");
    assert_eq!(flow.ended(), 0);
}

/// The workload the before-and-after pictures show: one session owning one
/// settled run closed with `unwatched --acknowledge`, plus one flow. With the
/// flow live and unwatched the guard blocks on the flow alone; a watch armed
/// between its runs ends `run-joined` on the next run with a cursor; and once
/// its holder dies, `unwatched` lists it as died with the command that closes it.
#[test]
fn the_pictured_workload_blocks_on_the_flow_wakes_on_its_run_and_reports_its_death() {
    let world = World::new("flow-pictured");
    let settled = world.plan("shipped", &plan_of("shipped", vec![agent("shipwork", &[])]));
    let _ = world.run(&["start", &settled, "--attach"]);
    world.until("the shipped run to settle", |world| {
        world.run_file("shipped", "result.json").is_file()
    });
    world
        .run(&[
            "unwatched",
            "--acknowledge",
            "shipped",
            "--reason",
            "merged and released",
        ])
        .exited(EXIT_SUCCESS);

    let next = held_plan(&world, "spikes", "spiking");
    let mut flow = Flowing::start(
        &world,
        "plan",
        &[gate(&world, "review"), launch(&next), gate(&world, "end")],
    );
    let blocked = verdict(&world, &["--wake-budget", "2100"]);
    assert_eq!(blocked["verdict"], json!("block"), "{blocked}");
    let reason = blocked["reason"].as_str().expect("a block says why");
    assert!(
        reason.starts_with("flow plan ")
            && reason.contains("onepipeline watch --flow plan --timeout 2098\n")
            && !reason.contains("shipped"),
        "{reason}"
    );

    let watch = watching(&world, "plan", &["--timeout", "2098"]);
    flow.open("review");
    let joined = watch.returned();
    joined.ended_on(EXIT_RUN_JOINED, "run-joined");
    assert_eq!(joined.record["run_id"], json!("spikes"));
    assert!(joined.cursor().starts_with("flow:1:plan:spikes@"));

    flow.child.kill().expect("SIGKILL reaches the holder");
    let _ = flow.child.wait();
    let died = unwatched(&world, &[]);
    died.exited(RUNS_UNWATCHED);
    assert!(
        died.stdout
            .lines()
            .any(|line| line.starts_with("flow plan ")
                && line.contains("DIED")
                && line.ends_with("onepipeline unwatched --acknowledge-flow plan --reason <TEXT>")),
        "{}",
        died.stdout
    );
    flow.open("end");
}

/// For a session with no flow nothing changes: a run launched outside any flow
/// has no `flow` key in its launch record, and is reported, watched and guarded
/// exactly as before; and a runs root holding a flow lists exactly its runs,
/// naming no skipped root.
#[test]
fn a_run_outside_any_flow_reads_as_before_and_flows_are_no_run_roots() {
    let world = World::new("flow-regression");
    let plan = held_plan(&world, "alone", "alonework");
    world.run(&["start", &plan, "--detach"]).exited(0);
    world.until("the run to dispatch", |world| {
        !world.events_of("alone", "node-dispatched").is_empty()
    });
    let launch = world.run_json("alone", "launch.json");
    assert!(launch.get("flow").is_none(), "{launch}");
    unwatched(&world, &[])
        .exited(RUNS_UNWATCHED)
        .out_has("watch it with: onepipeline watch alone\n");
    let watch = world
        .cmd(&["watch", "alone", "--timeout", "0"])
        .output()
        .expect("the watch runs");
    assert_eq!(watch.status.code(), Some(WATCH_ELAPSED), "{watch:?}");

    // Another session's flow under the same runs root: `runs` lists the run and
    // nothing else, and names no skipped root.
    let other = world.as_session("another-session");
    let flow = Flowing::start(&other, "elsewhere", &[gate(&world, "end")]);
    let listed = world.run(&["runs", "--flat"]);
    listed.exited(0).out_has("alone");
    assert!(
        !listed.stdout.contains("skipped") && !listed.stderr.contains("skipped"),
        "{}\n{}",
        listed.stdout,
        listed.stderr
    );
    assert!(!listed.stdout.contains(".flows"), "{}", listed.stdout);
    // And this session owes nothing for another's flow.
    let owed = unwatched(&world, &[]);
    assert!(!owed.stdout.contains("elsewhere"), "{}", owed.stdout);
    flow.open("end");
    assert_eq!(flow.ended(), 0);
}

/// The edges of `flow run` itself: with no session it registers nothing and
/// says so, still running the program and exiting with its status; ids are
/// minted as the first free of `<NAME>`, `<NAME>-2`, ..., from the program's
/// file name where `--name` gives none; a `--name` outside the id alphabet is
/// refused with nothing run; and a SIGTERM to `flow run` is forwarded to the
/// program, whose status — not the signal's — is what `flow run` records and
/// exits with.
#[test]
fn flow_run_forwards_signals_mints_free_ids_and_registers_nothing_without_a_session() {
    let world = World::new("flow-edges");
    let steps = steps(&world);

    // No session: the program runs as no flow, and one line says so.
    let unowned = world
        .cmd(&[
            "flow",
            "run",
            "--",
            "sh",
            "-c",
            "printf %s \"${ONEPIPELINE_FLOW-none}\"; exit 4",
        ])
        .env_remove("ONEPIPELINE_LAUNCHER_SESSION")
        .output()
        .expect("flow run runs");
    assert_eq!(unowned.status.code(), Some(4), "{unowned:?}");
    assert_eq!(String::from_utf8_lossy(&unowned.stdout), "none");
    let said = String::from_utf8_lossy(&unowned.stderr);
    assert_eq!(said.lines().count(), 1, "{said}");
    assert!(
        said.contains("nothing will hold a session for it"),
        "{said}"
    );
    assert!(!world.runs.join(".flows").exists(), "a flow was registered");

    // Ids: the program's file name, then the first free of the name.
    for expected in ["sh", "sh-2"] {
        let ran = world
            .cmd(&[
                "flow",
                "run",
                "--",
                "sh",
                "-c",
                "printf %s \"$ONEPIPELINE_FLOW\"",
            ])
            .output()
            .expect("flow run runs");
        assert_eq!(ran.status.code(), Some(0), "{ran:?}");
        assert_eq!(String::from_utf8_lossy(&ran.stdout), expected);
        assert_eq!(
            String::from_utf8_lossy(&ran.stderr),
            format!("flow {expected}: watch it with: onepipeline watch --flow {expected}\n")
        );
    }
    world
        .run(&["flow", "run", "--name", "../escape", "--", "true"])
        .exited(REFUSED)
        .err_has("--name ../escape");

    // A SIGTERM is the program's: it traps it, and its status is recorded.
    let flow = Flowing::start(
        &world,
        "trapped",
        &[
            "trap 'exit 42' TERM".to_owned(),
            mark(&world, "trapping"),
            format!(
                "while :; do sleep 0.05; done; touch '{}'",
                steps.join("unreachable").display()
            ),
        ],
    );
    flow.reached(&world, "trapping");
    let signalled = std::process::Command::new("kill")
        .args(["-TERM", &flow.child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(signalled.success());
    assert_eq!(flow.ended(), 42, "SIGTERM did not reach the program");
    unwatched(&world, &[])
        .exited(RUNS_UNWATCHED)
        .out_has("flow trapped")
        .out_has("it ended with status 42");
}
