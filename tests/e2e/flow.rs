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
        Self::start_with(world, id, &[], script)
    }

    /// [`start`](Self::start), with more of `flow run`'s own flags before `--`.
    fn start_with(world: &World, id: &str, flags: &[&str], script: &[String]) -> Self {
        let steps = steps(world);
        let path = world.root.join(format!("{id}.sh"));
        std::fs::write(&path, script.join("\n") + "\n").expect("the program is written");
        let stderr = world.root.join(format!("{id}.stderr"));
        let mut argv = vec!["flow", "run", "--name", id];
        argv.extend_from_slice(flags);
        argv.extend(["--", "sh", path.to_str().expect("a path")]);
        let child = world
            .cmd(&argv)
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
/// is on disk beside the flow, or once it has already returned.
///
/// A watch with an event already waiting, or given `--timeout 0`, writes its
/// lease and removes it again within one look of this wait, so on a loaded host
/// the lease alone can come and go unseen; the watch having exited is the same
/// proof that it armed, and what it said is read by [`Watching::returned`].
fn watching(world: &World, flow: &str, args: &[&str]) -> Watching {
    let mut argv = vec!["watch", "--flow", flow];
    argv.extend_from_slice(args);
    let mut child = world
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
        }) || child.try_wait().is_ok_and(|exited| exited.is_some())
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
    let read = world.run(&["next", "--flow", "ship"]);
    read.exited(0);
    assert_eq!(read.json()["status"], json!("finished"), "{}", read.stdout);

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
    // Reaped before anything reads it: a SIGKILL is sent when `kill` returns,
    // and on a loaded host the holder can still be running a moment later.
    let _ = flow.child.wait();

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
    // Handed out by `next`, and unanswered, it is still a question: a watch of
    // the flow still wakes on it.
    let read = world.run(&["next", "--flow", "deploy"]);
    read.exited(0);
    assert_eq!(read.json()["surface"]["kind"], json!("planner-question"));
    let watch = watching(&world, "deploy", &["--timeout", "600"]);
    watch.returned().ended_on(SURFACE_WAITING, "surface");
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

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the wait this journey
// measures is the behaviour under test — a watch returning within its wake budget, which
// only a clock can show — and it is `flow`, `flowwatch`, `watch`, `watchers` and `unwatched`
// at once, so the narrowest edge it can honestly sit behind is the crate's, as the `mod
// flow` declaration in `tests/e2e/main.rs` says; `tests/e2e/wake_budget.rs` keeps its own
// timed journeys behind the same edge for the same reason.
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
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

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
        // The flow this process was started in, if any, is handed back to the
        // program rather than dropped: its environment is this process's.
        .env(FLOW_ENV, "inherited")
        .output()
        .expect("flow run runs");
    assert_eq!(unowned.status.code(), Some(4), "{unowned:?}");
    assert_eq!(String::from_utf8_lossy(&unowned.stdout), "inherited");
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

    // Each terminating signal is the program's: it traps it, and its status —
    // not the signal's — is what `flow run` records and exits with.
    for (signal, status) in [("INT", 41), ("TERM", 42), ("HUP", 43)] {
        let id = format!("trapped-{}", signal.to_lowercase());
        let flow = Flowing::start(
            &world,
            &id,
            &[
                format!("trap 'exit {status}' {signal}"),
                mark(&world, &id),
                format!(
                    "while :; do sleep 0.05; done; touch '{}'",
                    steps.join("unreachable").display()
                ),
            ],
        );
        flow.reached(&world, &id);
        let signalled = std::process::Command::new("kill")
            .args([&format!("-{signal}"), &flow.child.id().to_string()])
            .status()
            .expect("kill runs");
        assert!(signalled.success());
        assert_eq!(
            flow.ended(),
            status,
            "SIG{signal} did not reach the program"
        );
        unwatched(&world, &[])
            .exited(RUNS_UNWATCHED)
            .out_has(&format!("flow {id}"))
            .out_has(&format!("it ended with status {status}"));
    }

    // A flow that cannot be registered is refused, and its program never runs.
    permissions_bind(&world);
    let flows = world.runs.join(".flows");
    let ran = world.root.join("ran");
    chmod(&flows, 0o555);
    let refused = world.run(&[
        "flow",
        "run",
        "--name",
        "unregistered",
        "--",
        "touch",
        ran.to_str().expect("a path"),
    ]);
    chmod(&flows, 0o755);
    refused.exited(REFUSED);
    assert!(
        !ran.exists(),
        "the program of a flow that was never registered ran"
    );
    assert!(!flows.join("unregistered").exists());

    // An acknowledgement that cannot be written is refused, and the closure it
    // would have made is still owed.
    let held = flows.join("trapped-int");
    chmod(&held, 0o555);
    let refused = world.run(&[
        "unwatched",
        "--acknowledge-flow",
        "trapped-int",
        "--reason",
        "seen",
    ]);
    chmod(&held, 0o755);
    refused.exited(REFUSED);
    unwatched(&world, &[])
        .exited(RUNS_UNWATCHED)
        .out_has("flow trapped-int");
}

/// Set a path's Unix permission bits, for the journeys that make a directory
/// this host refuses to read or write — and put back once they are done.
fn chmod(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

// llmlint: ignore-block[tests_mirror_real_usage] no verb writes a flow document it did not
// mean to — a holder writes its own record and ending, `start` its membership, `unwatched`
// its acknowledgement, a watch its terms — and what these journeys put on disk is what
// another build, a writer that died mid-write, a copied runs root or an engine before the
// terms record leaves: states the readers must answer for, which no verb of this build
// reaches. Every such write and removal is made here, at this one site, and every claim
// afterwards is read off the compiled binary's own streams.
/// Put `body` at `path` under a flow's directory, or take what is there away
/// where `body` is `None`: the one way a journey stages a flow document no verb
/// writes.
fn tampered(path: &std::path::Path, body: Option<&str>) {
    match body {
        Some(body) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).expect("a directory");
            }
            std::fs::write(path, body).expect("the document is written");
        }
        None => std::fs::remove_file(path).expect("the document goes"),
    }
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// A journey running as root is not refused by permission bits, so a journey
/// whose refusals rest on them fails saying so — never passes having staged
/// nothing.
fn permissions_bind(world: &World) {
    let probe = world.root.join("probe-permissions");
    std::fs::create_dir_all(&probe).expect("a probe directory");
    chmod(&probe, 0o000);
    let binds = std::fs::read_dir(&probe).is_err();
    chmod(&probe, 0o755);
    assert!(
        binds,
        "this journey runs where permission bits refuse nothing (as root?), so the refusals it \
         stages cannot be staged"
    );
}

/// What `--flow` cannot be asked, refused by the parser or before anything
/// moves: a run and a flow at once, a run's profile flags on a flow, an id that
/// is not one, a flow that is not there, a record this build did not write, a
/// run watch's condition, a cursor this flow cannot place, and a positional too
/// many. A flow's surface and reply read the file `--flow` leaves the positional
/// to name.
#[test]
fn the_flow_flags_refuse_what_a_flow_cannot_be_asked() {
    let world = World::new("flow-refusals");
    let member = held_plan(&world, "member", "memberwork");
    let flow = Flowing::start(
        &world,
        "live",
        &[
            launch(&member),
            mark(&world, "launched"),
            gate(&world, "end"),
        ],
    );
    flow.reached(&world, "launched");
    let solo = held_plan(&world, "solo", "solowork");
    world.run(&["start", &solo, "--detach"]).exited(0);

    for argv in [
        vec!["next"],
        vec!["next", "solo", "--flow", "live"],
        vec!["watch", "--flow", "live", "--all"],
        vec!["watch", "--flow", "live", "--filter", "planner"],
        vec!["channel", "queue", "solo", "--flow", "live"],
        vec![
            "unwatched",
            "--acknowledge-flow",
            "live",
            "--acknowledge",
            "solo",
            "--reason",
            "r",
        ],
    ] {
        world.run(&argv).exited(REFUSED);
    }
    world
        .run(&["next", "--flow", "../live"])
        .exited(REFUSED)
        .err_has("is not a flow id");
    world
        .run(&["watch", "--flow", "nope", "--timeout", "0"])
        .exited(REFUSED)
        .err_has("no flow 'nope'");

    // Records this build did not write, each written by hand: one that is not a
    // record, and one naming another flow.
    let flows = world.runs.join(".flows");
    tampered(&flows.join("broken").join("flow.json"), Some("{"));
    world
        .run(&["next", "--flow", "broken"])
        .exited(REFUSED)
        .err_has("record cannot be read");
    tampered(
        &flows.join("copied").join("flow.json"),
        Some(&std::fs::read_to_string(flows.join("live").join("flow.json")).expect("a record")),
    );
    // A record this host will not read is refused as one, naming why.
    permissions_bind(&world);
    let sealed = Flowing::start(&world, "sealed", &[gate(&world, "sealed-end")]);
    let record = flows.join("sealed").join("flow.json");
    chmod(&record, 0o000);
    let refused = world.run(&["next", "--flow", "sealed"]);
    chmod(&record, 0o644);
    refused
        .exited(REFUSED)
        .err_has("record cannot be read")
        .err_has("denied");
    sealed.open("sealed-end");
    assert_eq!(sealed.ended(), 0);
    world
        .run(&["next", "--flow", "copied"])
        .exited(REFUSED)
        .err_has("names flow 'live'");
    // Ownership is a positive claim: neither is anybody's to report.
    let asked = unwatched(&world, &[]);
    assert!(
        !asked.stdout.contains("broken") && !asked.stderr.contains("broken"),
        "{}\n{}",
        asked.stdout,
        asked.stderr
    );

    // Conditions belong to their own kind of watch.
    world
        .run(&[
            "watch",
            "--flow",
            "live",
            "--until",
            "settled",
            "--timeout",
            "0",
        ])
        .exited(REFUSED)
        .err_has("run watch's condition");
    world
        .run(&["watch", "solo", "--until", "run-joined", "--timeout", "0"])
        .exited(REFUSED)
        .err_has("flow watch's condition");

    // Cursors this flow cannot place.
    for (cursor, said) in [
        ("1:solo:0", "not a cursor this build reads for a flow"),
        ("flow:1:other:", "printed by a watch of flow 'other'"),
        ("flow:1:live:solo@0", "not a run of flow 'live'"),
        ("flow:1:live:member@99999999", "whose store holds"),
        (
            "flow:1:live:/member@0",
            "not a cursor this build reads for a flow",
        ),
        (
            "flow:1:live:member@0//member@0",
            "not a cursor this build reads for a flow",
        ),
        ("flow:1:live:member@0/member@0", "names run 'member' twice"),
    ] {
        world
            .run(&[
                "watch",
                "--flow",
                "live",
                "--timeout",
                "0",
                "--cursor",
                cursor,
            ])
            .exited(REFUSED)
            .err_has(said);
    }

    // Each document a flow keeps is read closed. Written by hand: records at
    // another version, naming nobody, and stamped with no instant are each
    // refused, naming what they were refused for; an ending at another version
    // leaves its flow unknown rather than ended.
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(flows.join("live").join("flow.json")).expect("the record"),
    )
    .expect("JSON");
    for (id, key, value) in [
        ("versioned", "schema_version", json!(2)),
        ("nobodys", "session", json!("  ")),
        ("unstamped", "began_at", json!("yesterday")),
    ] {
        let mut written = record.clone();
        written["flow_id"] = json!(id);
        written[key] = value;
        tampered(
            &flows.join(id).join("flow.json"),
            Some(&written.to_string()),
        );
        world
            .run(&["next", "--flow", id])
            .exited(REFUSED)
            .err_has("record cannot be read");
    }
    let mut ended = Flowing::start(&world, "versioned-ending", &[gate(&world, "never")]);
    ended.child.kill().expect("SIGKILL reaches the holder");
    let _ = ended.child.wait();
    tampered(
        &flows.join("versioned-ending").join("ending.json"),
        Some(
            &json!({"schema_version": 2, "flow_id": "versioned-ending", "status": 0,
                    "at": "2026-10-10T00:00:00.000Z"})
            .to_string(),
        ),
    );
    unwatched(&world, &[])
        .err_has("flow versioned-ending: its ending cannot be read")
        .err_has("schema_version");
    ended.open("never");

    // A surface that says nothing is refused, and nothing is queued.
    world
        .run(&[
            "surface",
            "--flow",
            "live",
            "--kind",
            "finding",
            "--message",
            "   ",
        ])
        .exited(REFUSED)
        .err_has("carried nothing");
    let queue = world.run(&["channel", "queue", "--flow", "live"]);
    queue.exited(0);
    assert_eq!(queue.json()["surfaces"], json!([]), "{}", queue.stdout);

    // Membership is the document `start` writes, naming its own run: written
    // by hand, one that is not that document and one naming another run are no
    // run of the flow, and no watch reports either joining.
    let members = flows.join("live").join("members");
    tampered(&members.join("forged.json"), Some("{"));
    tampered(
        &members.join("ghost.json"),
        Some(
            &json!({"schema_version": 1, "run_id": "member", "at": "2026-10-10T00:00:00.000Z"})
                .to_string(),
        ),
    );
    let placed = format!(
        "flow:1:live:member@{}",
        std::fs::metadata(world.run_file("member", "events.jsonl"))
            .map_or(0, |journal| journal.len())
    );
    world
        .run(&[
            "watch",
            "--flow",
            "live",
            "--timeout",
            "0",
            "--until",
            "run-joined",
            "--cursor",
            &placed,
        ])
        .exited(WATCH_ELAPSED);

    // A flow's surface and reply read the file `--flow` leaves the positional
    // to name; a second positional, or a file beside `--message`, is refused.
    let text = world.root.join("finding.txt");
    std::fs::write(&text, "the review found two gaps").expect("a message file");
    let text = text.to_str().expect("a path");
    world
        .run(&["surface", "--flow", "live", "--kind", "finding", text, text])
        .exited(REFUSED)
        .err_has("one positional");
    world
        .run(&[
            "surface",
            "--flow",
            "live",
            "--kind",
            "finding",
            "--message",
            "x",
            text,
        ])
        .exited(REFUSED)
        .err_has("never both");
    world
        .run(&["surface", "--flow", "live", "--kind", "finding", text])
        .exited(EXIT_SUCCESS);
    let read = world.run(&["next", "--flow", "live"]);
    read.exited(0);
    assert_eq!(
        read.json()["surface"]["message"],
        json!("the review found two gaps")
    );
    let verdict_file = world.root.join("verdict.json");
    std::fs::write(
        &verdict_file,
        json!({"version": 2, "completion": false, "message": "address both"}).to_string(),
    )
    .expect("an envelope file");
    world
        .run(&[
            "reply",
            "--flow",
            "live",
            verdict_file.to_str().expect("a path"),
        ])
        .exited(EXIT_SUCCESS);
    let queue = world.run(&["channel", "queue", "--flow", "live"]);
    queue.exited(0);
    assert_eq!(
        queue.json()["replies"][0]["reply"]["message"],
        json!("address both"),
        "{}",
        queue.stdout
    );
    flow.open("end");
    assert_eq!(flow.ended(), 0);
}

/// What a flow's evidence cannot say is never passed in silence: an ending that
/// cannot be read — or another flow's — leaves the flow live and unknown, which
/// the guard warns on; an acknowledgement that cannot be read leaves a died flow
/// unknown until a readable one closes it; an acknowledgement is refused with a
/// blank reason, from another session, and for a flow still live; and a flows
/// directory this host will not read refuses the question.
#[test]
fn a_flow_whose_evidence_cannot_be_read_is_named_and_never_passed() {
    let world = World::new("flow-unreadable");
    let flows = world.runs.join(".flows");
    let garbled = Flowing::start(&world, "garbled", &[gate(&world, "end")]);
    // An ending no holder wrote, while the holder lives.
    tampered(
        &flows.join("garbled").join("ending.json"),
        Some("not a record"),
    );
    let asked = unwatched(&world, &[]);
    asked.exited(EXIT_SUCCESS);
    assert!(asked.stdout.is_empty(), "{}", asked.stdout);
    assert!(
        asked
            .stderr
            .contains("flow garbled: its ending cannot be read"),
        "{}",
        asked.stderr
    );
    let warned = verdict(&world, &[]);
    assert_eq!(warned["verdict"], json!("warn"), "{warned}");
    let read = world.run(&["next", "--flow", "garbled"]);
    read.exited(0);
    assert_eq!(read.json()["status"], json!("running"));
    world
        .run(&["watch", "--flow", "garbled", "--timeout", "0"])
        .exited(WATCH_ELAPSED);
    // Another flow's ending is not this one's.
    tampered(
        &flows.join("garbled").join("ending.json"),
        Some(
            &json!({"schema_version": 1, "flow_id": "elsewhere", "status": 0,
                    "at": "2026-10-10T00:00:00.000Z"})
            .to_string(),
        ),
    );
    unwatched(&world, &[])
        .exited(EXIT_SUCCESS)
        .err_has("flow 'elsewhere''s ending, not this flow's");
    world
        .run(&[
            "unwatched",
            "--acknowledge-flow",
            "garbled",
            "--reason",
            "x",
        ])
        .exited(REFUSED)
        .err_has("is live");
    tampered(&flows.join("garbled").join("ending.json"), None);
    garbled.open("end");
    assert_eq!(garbled.ended(), 0);

    let mut gone = Flowing::start(&world, "gone", &[gate(&world, "never")]);
    gone.child.kill().expect("SIGKILL reaches the holder");
    let _ = gone.child.wait();
    let acknowledgements = flows.join("gone").join("acknowledgements");
    // Acknowledgements that read, and are not this session's word about this
    // flow, close nothing: one of another flow, and one of another session.
    for (name, flow, session) in [
        ("another-flow", "elsewhere", world.session.as_str()),
        ("another-session", "gone", "a-stranger"),
    ] {
        tampered(
            &acknowledgements.join(format!("{name}.json")),
            Some(
                &json!({"schema_version": 1, "flow_id": flow, "session": session,
                        "reason": "not this session's word", "at": "2026-10-10T00:00:00.000Z"})
                .to_string(),
            ),
        );
    }
    unwatched(&world, &[])
        .exited(RUNS_UNWATCHED)
        .out_has("flow gone")
        .out_has("DIED");
    // An acknowledgements directory this host will not read is an unknown.
    permissions_bind(&world);
    chmod(&acknowledgements, 0o000);
    let sealed = unwatched(&world, &[]);
    let sealed_verdict = verdict(&world, &[]);
    chmod(&acknowledgements, 0o755);
    sealed.exited(EXIT_SUCCESS).err_has(
        "flow gone: whether it is closed cannot be said: its acknowledgements cannot be read",
    );
    assert_eq!(sealed_verdict["verdict"], json!("warn"), "{sealed_verdict}");
    // An acknowledgement no verb wrote.
    tampered(&acknowledgements.join("torn.json"), Some("{"));
    let asked = unwatched(&world, &[]);
    asked.exited(EXIT_SUCCESS);
    assert!(asked.stdout.is_empty(), "{}", asked.stdout);
    assert!(
        asked
            .stderr
            .contains("flow gone: whether it is closed cannot be said"),
        "{}",
        asked.stderr
    );
    assert_eq!(verdict(&world, &[])["verdict"], json!("warn"));
    world
        .run(&["unwatched", "--acknowledge-flow", "gone", "--reason", "  "])
        .exited(REFUSED)
        .err_has("blank");
    // Held for the journey: a view removes the world's root when it is dropped.
    let stranger = world.as_session("a-stranger");
    stranger
        .run(&[
            "unwatched",
            "--acknowledge-flow",
            "gone",
            "--reason",
            "mine now",
        ])
        .exited(REFUSED)
        .err_has("belongs to session");
    world
        .run(&[
            "unwatched",
            "--acknowledge-flow",
            "gone",
            "--reason",
            "redone by hand",
        ])
        .exited(EXIT_SUCCESS);
    unwatched(&world, &[]).exited(EXIT_SUCCESS);
    gone.open("never");

    permissions_bind(&world);
    {
        chmod(&flows, 0o000);
        let refused = unwatched(&world, &[]);
        chmod(&flows, 0o755);
        refused.exited(REFUSED).err_has(".flows");
    }
}

/// `start` records a flow only where `ONEPIPELINE_FLOW` names a **live** flow of
/// the launching session under **its own** runs root: a flow that died, one
/// that is not there, another session's and one under another runs root each
/// leave the run in no flow.
#[test]
fn start_records_only_a_live_flow_of_its_own_session_under_its_own_root() {
    let world = World::new("flow-joins");
    let mut gone = Flowing::start(&world, "gone", &[gate(&world, "never")]);
    gone.child.kill().expect("SIGKILL reaches the holder");
    let _ = gone.child.wait();
    let stranger = world.as_session("a-stranger");
    let theirs = Flowing::start(&stranger, "theirs", &[gate(&world, "end")]);
    let mut elsewhere = world.as_session(&world.session);
    elsewhere.runs = world.root.join("other-runs");
    std::fs::create_dir_all(&elsewhere.runs).expect("another runs root");
    let away = Flowing::start(&elsewhere, "away", &[gate(&world, "end")]);
    let ours = Flowing::start(&world, "ours", &[gate(&world, "end")]);

    for (named, run, recorded) in [
        ("gone", "joingone", None),
        ("nope", "joinnope", None),
        ("theirs", "jointheirs", None),
        ("away", "joinaway", None),
        ("ours", "joinours", Some("ours")),
    ] {
        let plan = world.plan(run, &plan_of(run, vec![agent(&format!("{run}work"), &[])]));
        let launched = world
            .cmd(&["start", &plan, "--attach"])
            .env(FLOW_ENV, named)
            .output()
            .expect("start runs");
        assert!(launched.status.success(), "{launched:?}");
        assert_eq!(
            world.run_json(run, "launch.json").get("flow"),
            recorded.map(|flow| json!(flow)).as_ref(),
            "a run launched under {FLOW_ENV}={named}"
        );
    }
    gone.open("never");
    for flow in [theirs, away, ours] {
        flow.open("end");
        assert_eq!(flow.ended(), 0);
    }
}

/// `ask` asks on the flow's own channel only where it names no run: a run named
/// through `ONEPIPELINE_RUN_ID` wins, and a blank one names none. A flow's
/// question waits the window its own bus configuration names, and the advice on
/// a timeout names `reply --flow`. A configuration this build refuses refuses
/// the flow before its program runs.
#[test]
fn ask_gives_way_to_a_named_run_and_waits_the_flows_own_window() {
    let world = World::new("flow-ask");
    let config = world.root.join("bus.yaml");
    std::fs::write(
        &config,
        "version: 1\ntransport: {kind: local}\nprofile: planner-channel\ncodecs:\n  asked:\n    \
         queue: surfaces\n    reply_window_seconds: 3\n    select: kind\n    frames:\n      \
         planner-question:\n        schema: agent.planner-surface@1\n        bindings: \
         [{do: answer, response: {}}]\n",
    )
    .expect("a bus configuration");
    let flow = Flowing::start_with(
        &world,
        "asking",
        &["--bus-config", config.to_str().expect("a path")],
        &[gate(&world, "end")],
    );
    let named = held_plan(&world, "named", "namedwork");
    world.run(&["start", &named, "--detach"]).exited(0);

    let on_the_run = world
        .cmd(&[
            "ask",
            "--timeout",
            "1",
            "Is the run's own question asked on the run?",
        ])
        .env(FLOW_ENV, "asking")
        .env("ONEPIPELINE_RUN_ID", "named")
        .output()
        .expect("ask runs");
    assert_eq!(on_the_run.status.code(), Some(1), "{on_the_run:?}");
    let questions = |queue: &str| -> usize {
        let argv: Vec<&str> = queue.split(' ').collect();
        let read = world.run(&argv);
        read.exited(0);
        read.json()["surfaces"].as_array().map_or(0, |surfaces| {
            surfaces
                .iter()
                .filter(|surface| surface["kind"] == "planner-question")
                .count()
        })
    };
    assert_eq!(questions("channel queue named"), 1);
    assert_eq!(questions("channel queue --flow asking"), 0);

    let on_the_flow = world
        .cmd(&["ask", "Is the flow's own question asked on the flow?"])
        .env(FLOW_ENV, "asking")
        .env("ONEPIPELINE_RUN_ID", " ")
        .output()
        .expect("ask runs");
    assert_eq!(on_the_flow.status.code(), Some(1), "{on_the_flow:?}");
    let said = String::from_utf8_lossy(&on_the_flow.stderr);
    assert!(said.contains("waiting up to 3 seconds"), "{said}");
    assert!(
        said.contains("onepipeline reply --flow asking --correlation"),
        "{said}"
    );
    assert_eq!(questions("channel queue --flow asking"), 1);

    let bad = world.root.join("bad-bus.yaml");
    std::fs::write(
        &bad,
        "version: 1\ntransport: {kind: memory}\nprofile: planner-channel\n",
    )
    .expect("a configuration this build refuses");
    let ran = world.root.join("ran");
    world
        .run(&[
            "flow",
            "run",
            "--name",
            "refused",
            "--bus-config",
            bad.to_str().expect("a path"),
            "--",
            "touch",
            ran.to_str().expect("a path"),
        ])
        .exited(REFUSED)
        .err_has("--bus-config");
    assert!(
        !ran.exists(),
        "the program ran under a refused configuration"
    );
    assert!(!world.runs.join(".flows").join("refused").exists());
    flow.open("end");
    assert_eq!(flow.ended(), 0);
}

/// The rest of `flow run`'s edges: `--session` names the owner over the
/// environment, and a blank one is none; a program that cannot be started is
/// recorded as 127 and owed closure; a program a signal ends is recorded as
/// 128 + the signal; and an ending its holder cannot write leaves the flow died.
#[test]
fn flow_run_records_every_ending_its_program_can_have() {
    let world = World::new("flow-endings");
    world
        .run(&[
            "flow",
            "run",
            "--name",
            "owned",
            "--session",
            "explicit",
            "--",
            "true",
        ])
        .exited(0);
    // Held for the journey: a view removes the world's root when it is dropped.
    let explicit = world.as_session("explicit");
    unwatched(&explicit, &[]).exited(EXIT_SUCCESS);
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(world.runs.join(".flows/owned/flow.json")).expect("a record"),
    )
    .expect("JSON");
    assert_eq!(record["session"], json!("explicit"));
    world
        .run(&[
            "flow",
            "run",
            "--name",
            "blank",
            "--session",
            "",
            "--",
            "true",
        ])
        .exited(0)
        .err_has("nothing will hold a session for it");
    assert!(!world.runs.join(".flows/blank").exists());

    world
        .run(&["flow", "run", "--name", "missing", "--", "/no/such/program"])
        .exited(127)
        .err_has("could not be started");
    world
        .run(&[
            "flow",
            "run",
            "--name",
            "killed",
            "--",
            "sh",
            "-c",
            "kill -9 $$",
        ])
        .exited(137);
    let owed = unwatched(&world, &[]);
    owed.exited(RUNS_UNWATCHED);
    assert!(
        owed.stdout.contains("flow missing") && owed.stdout.contains("it ended with status 127"),
        "{}",
        owed.stdout
    );
    assert!(
        owed.stdout.contains("flow killed") && owed.stdout.contains("it ended with status 137"),
        "{}",
        owed.stdout
    );

    permissions_bind(&world);
    {
        let flow = Flowing::start(&world, "unkept", &[gate(&world, "end")]);
        let dir = world.runs.join(".flows").join("unkept");
        chmod(&dir, 0o555);
        flow.open("end");
        let said_before = flow.stderr.clone();
        assert_eq!(flow.ended(), 0);
        chmod(&dir, 0o755);
        let said = std::fs::read_to_string(said_before).unwrap_or_default();
        assert!(said.contains("could not be recorded"), "{said}");
        unwatched(&world, &[])
            .exited(RUNS_UNWATCHED)
            .out_has("flow unkept")
            .out_has("DIED");
    }
}

/// What a flow watch says while it waits, and where: a heartbeat per quiet tick
/// carrying the unread count summed over the flow's channel and its runs', none
/// at `--tick-interval 0`, its human lines in `--log` when given one; it returns
/// only on what `--until` names; a cursor printed before a run joined answers
/// `run-joined` for it at once; and a flow whose runs cannot be read is refused.
#[test]
fn a_flow_watch_says_what_it_waits_on_and_where() {
    let world = World::new("flow-says");
    let first = held_plan(&world, "talkrun", "talkwork");
    let second = held_plan(&world, "laterrun", "laterwork");
    let flow = Flowing::start(
        &world,
        "talky",
        &[
            launch(&first),
            mark(&world, "first"),
            gate(&world, "second"),
            launch(&second),
            mark(&world, "launched-second"),
            gate(&world, "end"),
        ],
    );
    flow.reached(&world, "first");
    world.until("the first run to dispatch", |world| {
        !world.events_of("talkrun", "node-dispatched").is_empty()
    });
    for argv in [
        vec![
            "surface",
            "--flow",
            "talky",
            "--kind",
            "finding",
            "--message",
            "one",
        ],
        vec![
            "surface",
            "talkrun",
            "--kind",
            "finding",
            "--message",
            "two",
        ],
    ] {
        world.run(&argv).exited(0);
    }
    let heard = world
        .cmd(&[
            "watch",
            "--flow",
            "talky",
            "--timeout",
            "3",
            "--tick-interval",
            "1",
            "--until",
            "run-joined",
        ])
        .output()
        .expect("the watch runs");
    assert_eq!(heard.status.code(), Some(WATCH_ELAPSED), "{heard:?}");
    let human = String::from_utf8_lossy(&heard.stderr);
    assert!(
        human.contains("-- watching flow talky  LIVE  2 unread planner surface(s)"),
        "{human}"
    );
    let records: Vec<Value> = String::from_utf8_lossy(&heard.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON record per line"))
        .collect();
    assert!(
        records
            .iter()
            .any(|record| record["watch"] == "heartbeat" && record["unread"]["count"] == 2),
        "{records:?}"
    );
    let before_the_join = records
        .last()
        .and_then(|record| record["cursor"].as_str())
        .expect("a return carries a cursor")
        .to_owned();

    let log = world.root.join("watch.log");
    let quiet = world
        .cmd(&[
            "watch",
            "--flow",
            "talky",
            "--timeout",
            "1",
            "--tick-interval",
            "0",
            "--until",
            "run-joined",
            "--log",
            log.to_str().expect("a path"),
        ])
        .output()
        .expect("the watch runs");
    assert_eq!(quiet.status.code(), Some(WATCH_ELAPSED));
    assert!(quiet.stderr.is_empty(), "{quiet:?}");
    let stdout = String::from_utf8_lossy(&quiet.stdout);
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    let logged = std::fs::read_to_string(&log).expect("the log");
    assert!(
        logged.starts_with("-- watch flow talky elapsed"),
        "{logged}"
    );

    // Told `node-settled` alone, a run joining does not end it; a settlement does.
    let settling = watching(
        &world,
        "talky",
        &["--timeout", "600", "--until", "node-settled"],
    );
    flow.open("second");
    flow.reached(&world, "launched-second");
    world.release("talkwork.go");
    let settled = settling.returned();
    settled.ended_on(NODE_SETTLED, "node-settled");
    assert_eq!(settled.record["run_id"], json!("talkrun"));

    // A cursor from before the second run joined answers it at once.
    let replayed = watching(
        &world,
        "talky",
        &[
            "--timeout",
            "600",
            "--until",
            "run-joined",
            "--cursor",
            &before_the_join,
        ],
    );
    let joined = replayed.returned();
    joined.ended_on(EXIT_RUN_JOINED, "run-joined");
    assert_eq!(joined.record["run_id"], json!("laterrun"));

    permissions_bind(&world);
    {
        let members = world.runs.join(".flows").join("talky").join("members");
        chmod(&members, 0o000);
        let refused = world.run(&["watch", "--flow", "talky", "--timeout", "0"]);
        chmod(&members, 0o755);
        refused.exited(REFUSED).err_has("runs cannot be read");
        // And while a watch waits: it ends refusing, rather than going on blind
        // to every run that joins.
        let waiting = watching(
            &world,
            "talky",
            &["--timeout", "600", "--until", "run-joined"],
        );
        chmod(&members, 0o000);
        let ended = waiting.child.wait_with_output().expect("the watch ends");
        chmod(&members, 0o755);
        assert_eq!(ended.status.code(), Some(REFUSED), "{ended:?}");
        assert!(
            String::from_utf8_lossy(&ended.stderr).contains("runs cannot be read"),
            "{ended:?}"
        );
    }
    flow.open("end");
    assert_eq!(flow.ended(), 0);
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] the wait this journey
// measures is the behaviour under test — a watch returning within its wake budget, which
// only a clock can show — and it is `flow`, `flowwatch`, `watch`, `watchers` and `unwatched`
// at once, so the narrowest edge it can honestly sit behind is the crate's, as the `mod
// flow` declaration in `tests/e2e/main.rs` says; `tests/e2e/wake_budget.rs` keeps its own
// timed journeys behind the same edge for the same reason.
/// Under a wake budget a flow watch qualifies only on every term a run watch's
/// does: one of another session's, one with no deadline, and one that does not
/// return on a surface each fail, in their own words; one whose terms are gone
/// is an unknown the guard warns on. And a run's watch given no `--timeout`
/// gives up the reserve before the budget too, measured on the wall clock.
#[test]
fn a_flow_watch_qualifies_only_on_every_term_and_a_run_watch_keeps_the_reserve() {
    let world = World::new("flow-terms");
    let flow = Flowing::start(&world, "rules", &[gate(&world, "end")]);
    let budget = ["--wake-budget", "600"];
    let stranger = world.as_session("a-stranger");
    for (watcher, args, why) in [
        (
            &stranger,
            vec!["--timeout", "60"],
            "it is another session's",
        ),
        (&world, vec!["--timeout", "none"], "it has no deadline"),
        (
            &world,
            vec!["--timeout", "60", "--until", "run-joined"],
            "it does not return on a surface",
        ),
    ] {
        let armed = watching(watcher, "rules", &args);
        unwatched(&world, &budget)
            .exited(RUNS_UNWATCHED)
            .out_has("flow rules")
            .out_has(why);
        armed.end();
    }
    let armed = watching(&world, "rules", &["--timeout", "60"]);
    unwatched(&world, &budget).exited(EXIT_SUCCESS);
    // The terms beside the live lease taken away, as an engine before the terms
    // record leaves a lease.
    let terms = world.runs.join(".flows").join("rules").join("watch-terms");
    for entry in std::fs::read_dir(&terms)
        .expect("the terms directory")
        .flatten()
    {
        tampered(&entry.path(), None);
    }
    let asked = unwatched(&world, &budget);
    asked.exited(EXIT_SUCCESS);
    assert!(asked.stdout.is_empty(), "{}", asked.stdout);
    assert!(
        asked
            .stderr
            .contains("flow rules: whether a live watch wakes this session"),
        "{}",
        asked.stderr
    );
    assert_eq!(verdict(&world, &budget)["verdict"], json!("warn"));
    armed.end();
    flow.open("end");
    assert_eq!(flow.ended(), 0);

    let run = held_plan(&world, "reserved", "reservedwork");
    world.run(&["start", &run, "--detach"]).exited(0);
    world.until("the run to dispatch", |world| {
        !world.events_of("reserved", "node-dispatched").is_empty()
    });
    let budgeted = world
        .as_session(&world.session)
        .with_env(WAKE_BUDGET_ENV, "4");
    let spawned = Instant::now();
    let watch = budgeted
        .cmd(&["watch", "reserved", "--tick-interval", "600"])
        .output()
        .expect("the watch runs");
    let took = spawned.elapsed();
    assert_eq!(watch.status.code(), Some(WATCH_ELAPSED), "{watch:?}");
    assert!(
        took <= Duration::from_secs(4) && took >= Duration::from_secs(1),
        "a run watch under a 4s budget returned after {took:?}"
    );
    // A budget no longer than the reserve leaves `0`, which reads once and
    // returns: it neither underflows into a wait no clock reaches nor waits the
    // budget out. Bounded loosely, because what is measured is a process start
    // on a loaded host, and a wait that had not saturated would not return at
    // all.
    let spent = world
        .as_session(&world.session)
        .with_env(WAKE_BUDGET_ENV, "1");
    let spawned = Instant::now();
    let watch = spent
        .cmd(&["watch", "reserved", "--tick-interval", "0"])
        .output()
        .expect("the watch runs");
    assert_eq!(watch.status.code(), Some(WATCH_ELAPSED), "{watch:?}");
    let records = String::from_utf8_lossy(&watch.stdout);
    assert_eq!(records.lines().count(), 1, "{records}");
    assert!(
        spawned.elapsed() < Duration::from_secs(30),
        "a watch under a budget the reserve spends whole waited {:?}",
        spawned.elapsed()
    );
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// A flow's reply is judged under the bus configuration the flow was registered
/// with, and refused whole — with nothing queued — when it is not an envelope,
/// carries no verdict, names an author the configuration does not declare,
/// declares a completion its author may not, is refused by a validator the
/// configuration names, or names a question nobody asked. A check-in raised on a
/// flow is the scheduled kind's source.
#[test]
fn a_flows_reply_is_judged_under_its_own_bus_configuration() {
    let world = World::new("flow-judged");
    let validator = crate::harness::double("bus-validator")
        .to_string_lossy()
        .into_owned();
    let config = world.root.join("bus.yaml");
    std::fs::write(
        &config,
        format!(
            "version: 1\ntransport: {{kind: local}}\nauthors:\n  sentinel: {{capabilities: \
             [finding]}}\nvalidators:\n  - {{on: replies, kind: command, command: \
             [{validator:?}]}}\n"
        ),
    )
    .expect("a bus configuration");
    let flow = Flowing::start_with(
        &world,
        "judged",
        &["--bus-config", config.to_str().expect("a path")],
        &[gate(&world, "end")],
    );
    let replies = world.runs.join(".flows/judged/channel/replies.jsonl");
    let queued = || std::fs::read_to_string(&replies).unwrap_or_default();

    world.script("bus-validator.refuse", "this ruling names no evidence");
    for (envelope, said) in [
        ("not an envelope".to_owned(), "malformed"),
        (json!({"version": 2}).to_string(), "carries no verdict"),
        (
            json!({"author": "stranger", "message": "x"}).to_string(),
            "stranger",
        ),
        (
            json!({"author": "sentinel", "completion": true, "reason": "done"}).to_string(),
            "not something the sentinel may do",
        ),
        (
            json!({"completion": false, "reason": "carry on"}).to_string(),
            "this ruling names no evidence",
        ),
    ] {
        world
            .run_with_stdin(&["reply", "--flow", "judged"], &envelope)
            .exited(REFUSED)
            .err_has(said);
        assert_eq!(queued(), "", "a refused reply was queued: {envelope}");
    }
    world.unscript("bus-validator.refuse");
    world
        .run_with_stdin(
            &[
                "reply",
                "--flow",
                "judged",
                "--correlation",
                "c-nobody-asked",
            ],
            &json!({"completion": false, "message": "x"}).to_string(),
        )
        .exited(REFUSED);
    assert_eq!(queued(), "");
    world
        .run_with_stdin(
            &["reply", "--flow", "judged"],
            &json!({"completion": false, "message": "carry on"}).to_string(),
        )
        .exited(EXIT_SUCCESS);
    assert!(queued().contains("carry on"), "{}", queued());

    world
        .run(&[
            "surface",
            "--flow",
            "judged",
            "--kind",
            "check-in",
            "--message",
            "half way",
        ])
        .exited(EXIT_SUCCESS);
    let queue = world.run(&["channel", "queue", "--flow", "judged"]);
    queue.exited(0);
    assert_eq!(
        queue.json()["waiting"][0]["source"],
        json!("check-in"),
        "{}",
        queue.stdout
    );
    flow.open("end");
    assert_eq!(flow.ended(), 0);
}

/// A flow's id is minted from the program's file name — every character outside
/// the id alphabet a `-`, and `flow` where nothing of it is left — and a SIGTERM
/// `flow run` was started ignoring stays ignored: it neither ends `flow run` nor
/// reaches the program.
#[test]
fn a_flow_is_named_from_its_program_and_an_ignored_signal_stays_ignored() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new("flow-named");
    let tool = world.root.join("my tool");
    std::fs::write(&tool, "#!/bin/sh\nprintf %s \"$ONEPIPELINE_FLOW\"\n").expect("a program");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    world
        .run(&["flow", "run", "--", tool.to_str().expect("a path")])
        .exited(0)
        .out_has("my-tool");
    world
        .run(&["flow", "run", "--", ".."])
        .exited(127)
        .err_has("flow flow: watch it with: onepipeline watch --flow flow");

    let steps = steps(&world);
    let script = world.root.join("ignoring.sh");
    std::fs::write(
        &script,
        format!("{}\n{}\n", mark(&world, "ignoring"), gate(&world, "end")),
    )
    .expect("the program is written");
    let mut holder = world
        .cmd_on(
            std::path::Path::new("sh"),
            &[
                "-c",
                &format!(
                    "trap '' TERM; exec {CLI} flow run --name ignoring -- sh '{}'",
                    script.display()
                ),
            ],
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("flow run starts");
    world.until("the program to start", |_| steps.join("ignoring").is_file());
    let signalled = std::process::Command::new("kill")
        .args(["-TERM", &holder.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(signalled.success());
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        holder.try_wait().expect("a status read").is_none(),
        "an ignored SIGTERM ended `flow run`"
    );
    std::fs::write(steps.join("end.go"), "go").expect("the gate opens");
    assert_eq!(holder.wait().expect("flow run ends").code(), Some(0));
}

/// Two settlements, or two runs joining, between one watch and the next are two
/// returns, each in turn: a watch reports one and its cursor is past that one
/// alone, so the watch armed on that cursor reports the other.
#[test]
fn a_flow_watch_reports_each_settlement_and_each_join_in_turn() {
    let world = World::new("flow-in-turn");
    world.script("pairone.wait", "hold");
    world.script("pairtwo.wait", "hold");
    let pair = world.plan(
        "pair",
        &plan_of("pair", vec![agent("pairone", &[]), agent("pairtwo", &[])]),
    );
    let one = held_plan(&world, "joinone", "joinonework");
    let two = held_plan(&world, "jointwo", "jointwowork");
    let flow = Flowing::start(
        &world,
        "turns",
        &[
            launch(&pair),
            mark(&world, "paired"),
            gate(&world, "join"),
            launch(&one),
            launch(&two),
            mark(&world, "joined"),
            gate(&world, "end"),
        ],
    );
    flow.reached(&world, "paired");
    world.until("both nodes to dispatch", |world| {
        world.events_of("pair", "node-dispatched").len() >= 2
    });
    let look = |cursor: Option<&str>, until: &str| -> Returned {
        let mut args = vec!["--until", until];
        match cursor {
            Some(cursor) => args.extend(["--timeout", "600", "--cursor", cursor]),
            None => args.extend(["--timeout", "0"]),
        }
        watching(&world, "turns", &args).returned()
    };
    let before = look(None, "node-settled");
    before.ended_on(WATCH_ELAPSED, "elapsed");

    world.release("pairone.go");
    world.release("pairtwo.go");
    world.until("both nodes to settle", |world| {
        world.events_of("pair", "node-settled").len() >= 2
    });
    let first = look(Some(&before.cursor()), "node-settled");
    first.ended_on(NODE_SETTLED, "node-settled");
    let second = look(Some(&first.cursor()), "node-settled");
    second.ended_on(NODE_SETTLED, "node-settled");
    let settled: std::collections::BTreeSet<String> = [&first, &second]
        .iter()
        .filter_map(|returned| returned.record["node"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        settled,
        ["pairone", "pairtwo"]
            .map(str::to_owned)
            .into_iter()
            .collect(),
        "{} / {}",
        first.record,
        second.record
    );

    flow.open("join");
    flow.reached(&world, "joined");
    // Both runs that joined are holding a surface: the line the first join ends
    // on counts both, the second run's though its join is not reported yet.
    for run in ["joinone", "jointwo"] {
        world.until("the joined run's record", |world| {
            world.run_file(run, "launch.json").is_file()
        });
        world
            .run(&["surface", run, "--kind", "finding", "--message", "held"])
            .exited(0);
    }
    let first = look(Some(&second.cursor()), "run-joined");
    first.ended_on(EXIT_RUN_JOINED, "run-joined");
    assert_eq!(first.record["run_id"], json!("joinone"));
    assert_eq!(
        first.record["unread"]["count"],
        json!(2),
        "{}",
        first.record
    );
    let second = look(Some(&first.cursor()), "run-joined");
    second.ended_on(EXIT_RUN_JOINED, "run-joined");
    let joined: std::collections::BTreeSet<String> = [&first, &second]
        .iter()
        .filter_map(|returned| returned.record["run_id"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        joined,
        ["joinone", "jointwo"]
            .map(str::to_owned)
            .into_iter()
            .collect(),
        "{} / {}",
        first.record,
        second.record
    );
    flow.open("end");
    assert_eq!(flow.ended(), 0);
}

/// A run that joins a flow which then ends, both before the next watch looks,
/// is examined before the ending is: told not to return on a join, the watch
/// returns on that run's settlement rather than on the flow's clean exit.
#[test]
fn a_run_that_joins_as_its_flow_ends_is_examined_before_the_ending() {
    let world = World::new("flow-last-run");
    let last = world.plan("last", &plan_of("last", vec![agent("lastwork", &[])]));
    let flow = Flowing::start(
        &world,
        "closing",
        &[
            gate(&world, "go"),
            format!("{CLI} start '{last}' --attach >/dev/null || exit 70"),
        ],
    );
    let before = watching(&world, "closing", &["--timeout", "0"]).returned();
    before.ended_on(WATCH_ELAPSED, "elapsed");
    flow.open("go");
    assert_eq!(flow.ended(), 0);
    let after = watching(
        &world,
        "closing",
        &[
            "--timeout",
            "600",
            "--until",
            "node-settled",
            "--cursor",
            &before.cursor(),
        ],
    )
    .returned();
    after.ended_on(NODE_SETTLED, "node-settled");
    assert_eq!(after.record["run_id"], json!("last"), "{}", after.record);
}

/// The agent double reads `ONEPIPELINE_FLOW` by the spelling the engine exports,
/// so its report of none is a reading rather than a misspelling: a dispatch the
/// double runs with the variable set — through a wrapper that sets it, since the
/// engine never does — reports it on the run's own record.
#[test]
fn the_agent_double_reports_a_flow_a_dispatch_is_handed() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new("flow-double");
    let wrapper = world.root.join("oneagentgraph-in-a-flow");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n{FLOW_ENV}=wrapped exec '{}' \"$@\"\n",
            crate::harness::double("fake-oneagentgraph").display()
        ),
    )
    .expect("a wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let wrapped = world.as_session(&world.session).with_env(
        "ONEPIPELINE_ONEAGENTGRAPH_BIN",
        wrapper.to_str().expect("a path"),
    );
    let plan = wrapped.plan("handed", &plan_of("handed", vec![agent("handedwork", &[])]));
    wrapped.run(&["start", &plan, "--attach"]).settled();
    let turns: Vec<Value> = world
        .journal("handed")
        .into_iter()
        .filter(|event| event["kind"] == "turn-activity")
        .collect();
    assert!(!turns.is_empty(), "the run dispatched no turn");
    for turn in &turns {
        assert_eq!(turn["payload"]["flow"], json!("wrapped"), "{turn}");
    }
}

/// A run that joins a flow holding a planner surface, the flow then ending, both
/// before the next watch looks: told to return on a surface, the watch returns
/// `surface` (4) naming that run rather than the flow's clean exit.
#[test]
fn a_surface_on_a_run_that_joins_as_its_flow_ends_is_returned_before_the_ending() {
    let world = World::new("flow-last-surface");
    let last = held_plan(&world, "lastsurface", "lastsurfacework");
    let flow = Flowing::start(
        &world,
        "surfacing",
        &[
            gate(&world, "go"),
            launch(&last),
            mark(&world, "launched"),
            gate(&world, "end"),
        ],
    );
    let before = watching(&world, "surfacing", &["--timeout", "0"]).returned();
    before.ended_on(WATCH_ELAPSED, "elapsed");
    flow.open("go");
    flow.reached(&world, "launched");
    world
        .run(&[
            "surface",
            "lastsurface",
            "--kind",
            "finding",
            "--message",
            "the last stage has a finding",
        ])
        .exited(0);
    flow.open("end");
    assert_eq!(flow.ended(), 0);
    let after = watching(
        &world,
        "surfacing",
        &[
            "--timeout",
            "600",
            "--until",
            "surface",
            "--cursor",
            &before.cursor(),
        ],
    )
    .returned();
    after.ended_on(SURFACE_WAITING, "surface");
    assert_eq!(
        after.record["run_id"],
        json!("lastsurface"),
        "{}",
        after.record
    );
}
