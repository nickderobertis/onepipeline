//! A driver's ending, on the run's own record.
//!
//! Every driver that lets go of a run without crashing journals one
//! `driver-exited` — the run settled, paused on a decision, left with nothing it
//! can drive, or a driver-side error it reports — after its last outcome line,
//! before any run-end hook of that ending, and while it still holds the run's
//! ownership lock. A driver killed by a signal writes nothing, so the absence of
//! one after the last `driver-adopted` is how a crash reads; the panic half of
//! that is held by `engine::tests`, because nothing a journey can do to the
//! compiled binary makes it panic. `status` prints the last record, and names
//! every command envelope claimed off the queue that has no outcome line.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary. The run-end hook and the node validator are not substitutions either:
// each is the operator's own command, and these journeys supply real ones. `harness.rs`
// carries the same suppression and the full rationale.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[cfg(unix)]
use crate::harness::end_process;
use crate::harness::{agent, human, plan_of, World, NOTHING_DRIVING};
use crate::run_end_hooks::{hook, records, HOOK_TIMEOUT, RECORD_ENV};

/// A world whose run-end hook fixture records into the world's own scratch.
fn hooked_world(name: &str) -> World {
    let world = World::new(name);
    std::fs::create_dir_all(records(&world)).expect("a record directory");
    let record = records(&world).to_string_lossy().into_owned();
    world.with_env(RECORD_ENV, &record)
}

/// Start `run` over `nodes`, naming the fixture as both run-end hooks.
///
/// Detached, so the driver is a process of its own: the pid it announces is the
/// one its record has to name, and the lock it holds is one a journey can watch
/// from outside. Answers that pid.
fn start_detached(world: &World, run: &str, nodes: Vec<Value>, extra: &[&str]) -> u32 {
    let hook = hook(world);
    let path = world.plan(run, &plan_of(run, nodes));
    let mut args = vec![
        "start",
        path.as_str(),
        "--detach",
        "--success-hook",
        &hook,
        "--failure-hook",
        &hook,
        "--hook-timeout",
        HOOK_TIMEOUT,
    ];
    args.extend_from_slice(extra);
    let started = world.run_from(&world.project, &args);
    started.exited(0);
    let announced: Value = serde_json::from_str(started.stdout.trim())
        .unwrap_or_else(|error| panic!("a detached launch announces itself: {error}"));
    announced["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or_else(|| panic!("the launch announced no driver: {announced}"))
}

/// The same launch, attached: it returns once the driver has let go.
fn start_attached(world: &World, run: &str, nodes: Vec<Value>) -> crate::harness::Run {
    let hook = hook(world);
    let path = world.plan(run, &plan_of(run, nodes));
    world.run_from(
        &world.project,
        &[
            "start",
            &path,
            "--attach",
            "--success-hook",
            &hook,
            "--failure-hook",
            &hook,
            "--hook-timeout",
            HOOK_TIMEOUT,
        ],
    )
}

/// Where one kind first appears in a run's journal.
fn first(world: &World, run: &str, kind: &str) -> usize {
    world
        .kinds(run)
        .iter()
        .position(|written| written == kind)
        .unwrap_or_else(|| panic!("{run} recorded no {kind}: {:?}", world.kinds(run)))
}

/// The one `driver-exited` a run carries, refused unless there is exactly one.
fn the_exit(world: &World, run: &str) -> Value {
    let exits = world.events_of(run, "driver-exited");
    assert_eq!(
        exits.len(),
        1,
        "{run} carries {} driver-exited records: {:?}",
        exits.len(),
        world.kinds(run)
    );
    exits[0].clone()
}

/// What the run's launch record names as its driver.
fn recorded_driver(world: &World, run: &str) -> Value {
    world.run_json(run, "launch.json")["pid"].clone()
}

/// Submit one edit the driver applies, and wait for its answer.
fn an_applied_edit(world: &World, run: &str, model: &str) {
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 3, "commands": [
                {"op": "set-run-node-sets", "sets": [format!("members.worker.agent.model={model}")]}
            ]})
            .to_string(),
        )
        .exited(0)
        .out_has("\"applied\"");
}

/// The highest envelope id the run's channel holds an outcome line for.
fn highest_outcome(world: &World, run: &str) -> Option<u64> {
    world
        .command_outcomes(run)
        .iter()
        .filter_map(|outcome| outcome["id"].as_u64())
        .max()
}

/// A reader of the run's ownership lock, from outside the driver.
///
/// Polls the lock file for the moment it goes, and reads the journal at that
/// moment: what a reader that finds the run free finds on its record. Started
/// while the driver holds the lock, and answers `Err` for a lock it never saw
/// held or never saw released.
struct LockReader {
    reading: std::thread::JoinHandle<Result<bool, String>>,
}

impl LockReader {
    fn of(world: &World, run: &str) -> Self {
        let lock = world.run_file(run, "owner.lock");
        let journal = world.run_file(run, "events.jsonl");
        assert!(
            lock.is_file(),
            "the driver does not hold {}",
            lock.display()
        );
        let reading = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(300);
            while Instant::now() < deadline {
                if !lock.exists() {
                    let read = std::fs::read_to_string(&journal)
                        .map_err(|error| format!("{}: {error}", journal.display()))?;
                    return Ok(read
                        .lines()
                        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                        .any(|event| event["kind"] == "driver-exited"));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err("the run's ownership lock was never released".to_string())
        });
        Self { reading }
    }

    /// Whether the record was already there when the lock was found released.
    fn found_the_record(self) -> bool {
        self.reading
            .join()
            .expect("the lock reader does not panic")
            .unwrap_or_else(|why| panic!("{why}"))
    }
}

/// **A complete run.** The driver answers two edits while it drives, settles the
/// run, and lets go: one record, naming it, `complete`, no reason, and the later
/// of the two edits it answered — written after that answer, found by a reader
/// the instant the run is free, and on the record before the success hook fires.
/// While that hook runs the run is undriven, and `status` already says how its
/// driver ended.
#[test]
fn a_driver_that_completes_its_run_records_its_exit_before_the_run_is_free() {
    let world = hooked_world("exit-complete");
    let run = "completed";
    world.script("build.wait", "hold");
    std::fs::write(records(&world).join(format!("{run}.hold")), "").expect("the hold is scripted");
    let pid = start_detached(&world, run, vec![agent("build", &[])], &[]);
    world.until("the held node to be dispatched", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    let reader = LockReader::of(&world, run);

    // Two edits the driver answers while it drives, so it has outcome lines to
    // name — the record names the higher, and has to come after both.
    an_applied_edit(&world, run, "first");
    an_applied_edit(&world, run, "second");
    world.release("build.go");

    let started = records(&world).join(run).join("1").join("started");
    world.until("the success hook to be running", |_| started.is_file());
    // The hook runs after the let-go, so the run is free while it does — and the
    // record of how its driver ended is already there.
    assert!(
        !world.run_file(run, "owner.lock").exists(),
        "the run is still held while its hook runs"
    );
    assert!(
        reader.found_the_record(),
        "the run was free before its record"
    );
    world
        .run(&["status", run])
        .exited(0)
        .out_has("driver exited")
        .out_has("settled complete");
    std::fs::write(records(&world).join(format!("{run}.go")), "go").expect("the hold is released");
    world.until("the success hook to finish", |world| {
        !world.events_of(run, "run-hook-finished").is_empty()
    });

    let exited = the_exit(&world, run);
    assert_eq!(recorded_driver(&world, run), json!(pid));
    let answered = highest_outcome(&world, run).expect("the edits were answered");
    assert_eq!(answered, 1, "{:?}", world.command_outcomes(run));
    assert_eq!(
        exited["payload"],
        json!({
            "pid": pid,
            "settlement": "complete",
            "reason": null,
            "last_answered_command": answered,
        })
    );
    assert_eq!(exited["labels"]["run_id"], run);
    // After everything the driver recorded of the edit it answered, and before the
    // hook its ending fired.
    assert!(first(&world, run, "edit-committed") < first(&world, run, "driver-exited"));
    assert!(first(&world, run, "driver-exited") < first(&world, run, "run-hook-fired"));
    assert_eq!(world.run_json(run, "result.json")["state"], "complete");
}

/// **A run paused on a person.** The record says `awaiting-planner` — the word
/// the attached launch printed — answers no command, and comes before the hook
/// the pause withheld.
#[test]
fn a_driver_that_pauses_on_a_waiting_human_node_records_awaiting_planner() {
    let world = hooked_world("exit-awaiting");
    let run = "paused";
    start_attached(&world, run, vec![human("approve", &[])])
        .exited(0)
        .out_has("\"settlement\":\"awaiting-planner\"");

    let exited = the_exit(&world, run);
    assert_eq!(
        exited["payload"],
        json!({
            "pid": recorded_driver(&world, run),
            "settlement": "awaiting-planner",
            "reason": null,
            "last_answered_command": null,
        })
    );
    assert!(first(&world, run, "driver-exited") < first(&world, run, "run-hook-withheld"));
    assert!(world.events_of(run, "run-hook-fired").is_empty());
    world
        .run(&["status", run])
        .exited(0)
        .out_has("settled awaiting-planner");
}

/// **A run left with nothing to drive.** Its one node failed, so nothing is
/// ready and nothing is waiting on a person: `unattended`, before the failure
/// hook fires.
#[test]
fn a_driver_left_with_nothing_it_can_drive_records_unattended_before_the_failure_hook() {
    let world = hooked_world("exit-unattended");
    let run = "abandoned";
    world.script("build.fail", "1");
    start_attached(&world, run, vec![agent("build", &[])])
        .exited(NOTHING_DRIVING)
        .out_has("\"settlement\":\"unattended\"");

    let exited = the_exit(&world, run);
    assert_eq!(
        exited["payload"],
        json!({
            "pid": recorded_driver(&world, run),
            "settlement": "unattended",
            "reason": null,
            "last_answered_command": null,
        })
    );
    assert!(first(&world, run, "driver-exited") < first(&world, run, "run-hook-fired"));
    assert_eq!(
        world.events_of(run, "run-hook-fired")[0]["payload"]["hook"],
        "failure"
    );
}

/// **A driver-side error it reports.** The run's command queue stops being
/// readable under a live driver, so its next pass fails: the record says
/// `error` with the error's own message, is there the instant the run is free,
/// fires no hook, and `status` repeats the reason.
#[test]
fn a_driver_that_reports_an_error_records_it_while_it_still_holds_the_run() {
    let world = hooked_world("exit-error");
    let run = "broken";
    world.script("build.wait", "hold");
    let pid = start_detached(&world, run, vec![agent("build", &[])], &[]);
    world.until("the held node to be dispatched", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    let reader = LockReader::of(&world, run);

    // llmlint: ignore[tests_mirror_real_usage] a queue log that stops being a file under a
    // live driver is a storage fault, and no verb writes one; it is the narrowest fault that
    // makes the compiled driver's own pass fail and report it, and the driver, its journal,
    // its lock and its hooks around it are all the real ones.
    std::fs::create_dir_all(world.run_file(run, "channel/commands.jsonl"))
        .expect("the queue log is replaced by a directory");
    world.release("build.go");
    world.until("the driver to record its error", |world| {
        !world.events_of(run, "driver-exited").is_empty()
    });

    assert!(
        reader.found_the_record(),
        "the run was free before its record"
    );
    let exited = the_exit(&world, run);
    assert_eq!(exited["payload"]["pid"], json!(pid));
    assert_eq!(exited["payload"]["settlement"], "error");
    assert_eq!(exited["payload"]["last_answered_command"], Value::Null);
    let reason = exited["payload"]["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("an error carries its message: {exited}"));
    assert!(reason.contains("commands"), "{reason}");
    assert!(world.events_of(run, "run-hook-fired").is_empty());
    world
        .run(&["status", run])
        .exited(0)
        .out_has("settled error — ")
        .out_has(reason.lines().next().unwrap_or(reason));
}

/// **A driver killed by a signal writes nothing.** The record's absence after
/// the last `driver-adopted`, on a driver proved over, is how the crash reads —
/// and the driver that adopts the run and settles it writes the one record,
/// naming itself and none of the edits the killed driver answered: what it
/// answered is counted from where it took the run.
#[cfg(unix)]
#[test]
fn a_driver_killed_by_a_signal_leaves_no_record_and_its_adopter_writes_its_own() {
    let world = hooked_world("exit-signal");
    let run = "killed";
    world.script("build.wait", "hold");
    let pid = start_detached(&world, run, vec![agent("build", &[])], &[]);
    world.until("the held node to be dispatched", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    an_applied_edit(&world, run, "before-the-kill");
    end_process(pid);
    world
        .run(&["status", run])
        .exited(0)
        .out_has("DRIVER DEAD")
        .out_lacks("driver exited");
    assert!(world.events_of(run, "driver-exited").is_empty());

    let adopted = std::thread::scope(|scope| {
        scope.spawn(|| {
            world.until("the adopter to re-dispatch the held node", |world| {
                world.events_of(run, "node-dispatched").len() >= 2
            });
            world.release("build.go");
        });
        world.run(&["adopt", run])
    });
    adopted.exited(0);
    let exited = the_exit(&world, run);
    let adopter = world.events_of(run, "driver-adopted")[0]["payload"]["pid"].clone();
    assert_ne!(adopter, json!(pid));
    assert_eq!(exited["payload"]["pid"], adopter);
    assert_eq!(exited["payload"]["settlement"], "complete");
    assert_eq!(highest_outcome(&world, run), Some(0));
    assert_eq!(
        exited["payload"]["last_answered_command"],
        Value::Null,
        "the adopter named an edit the driver before it answered"
    );
    assert!(first(&world, run, "driver-adopted") < first(&world, run, "driver-exited"));
}

/// A node validator that passes the edit it is offered at submission, and holds
/// the second offer — the one the driver makes after claiming the envelope —
/// until `go` exists.
#[cfg(unix)]
fn validator_holding_its_second_offer(world: &World) -> (String, PathBuf, PathBuf) {
    let seen = world.root.join("validator.seen");
    let holding = world.root.join("validator.holding");
    let go = world.root.join("validator.go");
    let path = world.root.join("hold-second-offer.sh");
    onepipeline_testfakes::executable(
        &path,
        format!(
            "#!/bin/sh\ncat > /dev/null\nif [ -e '{seen}' ]; then\n  : > '{holding}'\n  \
             i=0\n  while [ ! -e '{go}' ] && [ \"$i\" -lt 3000 ]; do sleep 0.1; i=$((i+1)); \
             done\nfi\n: > '{seen}'\nexit 0\n",
            seen = seen.display(),
            holding = holding.display(),
            go = go.display(),
        ),
    );
    (path.to_string_lossy().into_owned(), holding, go)
}

/// A `reply` left waiting on its answer as a process of its own, patient enough
/// that it never takes the run over while a journey reads it.
#[cfg(unix)]
fn waiting_reply(world: &World, run: &str, commands: Value) -> std::process::Child {
    use std::io::Write;
    let mut reply = world.cmd(&["reply", run]);
    reply
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "600")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut replying = reply.spawn().expect("the reply starts");
    let mut stdin = replying.stdin.take().expect("the reply's stdin");
    stdin
        .write_all(
            json!({"version": 3, "commands": commands})
                .to_string()
                .as_bytes(),
        )
        .expect("the envelope is written");
    replying
}

/// The line `status` names claimed, unanswered envelopes on, if it prints one.
fn unanswered_line(world: &World, run: &str) -> Option<String> {
    let status = world.run(&["status", run]);
    status.exited(0);
    status
        .stdout
        .lines()
        .find(|line| line.trim_start().starts_with("claimed with no outcome:"))
        .map(str::to_string)
}

/// **A claimed envelope nobody answered.** The driver answers one edit, takes a
/// second off the queue, and is killed while its validator is still judging it;
/// a third is still waiting behind the cursor. `status` names the second alone —
/// not the one answered, not the one nobody claimed — and says nothing of a
/// driver exit, because none was written.
#[cfg(unix)]
#[test]
fn status_names_an_envelope_the_driver_claimed_and_never_answered() {
    let world = hooked_world("exit-unanswered");
    let run = "unanswered";
    world.script("build.wait", "hold");
    let (validator, holding, go) = validator_holding_its_second_offer(&world);
    let pid = start_detached(
        &world,
        run,
        vec![agent("build", &[])],
        &["--node-validator", &validator],
    );
    world.until("the held node to be dispatched", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    assert_eq!(unanswered_line(&world, run), None);

    // Envelope 0: answered. It names no task, so no validator is asked about it.
    an_applied_edit(&world, run, "answered");
    // Envelope 1: claimed, and held inside the driver's own validation of it.
    let mut claimed = waiting_reply(
        &world,
        run,
        json!([{"op": "add", "node": agent("fresh", &[])}]),
    );
    world.until("the driver to be judging the claimed envelope", |_| {
        holding.is_file()
    });
    // Envelope 2: queued behind the cursor, never claimed.
    let mut queued = waiting_reply(
        &world,
        run,
        json!([{"op": "set-run-node-sets", "sets": ["members.worker.agent.model=queued"]}]),
    );
    world.until("the third envelope to be on the queue", |world| {
        std::fs::read_to_string(world.run_file(run, "channel/commands.jsonl"))
            .is_ok_and(|log| log.lines().count() == 3)
    });
    end_process(pid);

    assert_eq!(
        unanswered_line(&world, run).as_deref().map(str::trim_start),
        Some(
            "claimed with no outcome: command envelope(s) 1 — taken off the queue and never \
             answered, so nothing applies them and their submitters were told nothing"
        )
    );
    world
        .run(&["status", run])
        .exited(0)
        .out_lacks("driver exited");
    let answered: Vec<u64> = world
        .command_outcomes(run)
        .iter()
        .filter_map(|outcome| outcome["id"].as_u64())
        .collect();
    assert_eq!(answered, [0]);
    assert!(world.events_of(run, "driver-exited").is_empty());

    std::fs::write(&go, "go").expect("the validator is released");
    world.release("build.go");
    for reply in [&mut claimed, &mut queued] {
        let _ = reply.kill();
        let _ = reply.wait();
    }
}

/// **A driver that cannot hand the run on.** Its handover gate cannot be taken
/// as it lets go, so it leaves its claim standing for the next writer — and
/// still writes the one record of how it ended, holding the run as it does.
#[test]
fn a_driver_that_cannot_hand_the_run_on_still_records_its_exit_and_leaves_the_claim() {
    let world = hooked_world("exit-claimed");
    let run = "unhanded";
    world.script("build.wait", "hold");
    let pid = start_detached(&world, run, vec![agent("build", &[])], &[]);
    world.until("the held node to be dispatched", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    // llmlint: ignore[tests_mirror_real_usage] a handover gate whose place is taken by
    // something that is not a directory is a run store an operator has been in, or a
    // filesystem that ran out; no verb makes one, and it is the narrowest fault that reaches
    // the let-go that cannot hand over through the compiled driver.
    std::fs::write(world.run_file(run, "channel/handover"), "not a directory")
        .expect("the gate's place is taken");
    world.release("build.go");
    world.until("the driver to record its exit", |world| {
        !world.events_of(run, "driver-exited").is_empty()
    });

    let exited = the_exit(&world, run);
    assert_eq!(
        exited["payload"],
        json!({
            "pid": pid,
            "settlement": "complete",
            "reason": null,
            "last_answered_command": null,
        })
    );
    let log = world.run_file(run, "driver.log");
    world.until("the driver to say it left the run claimed", |_| {
        std::fs::read_to_string(&log)
            .is_ok_and(|said| said.contains("is being left claimed rather than released"))
    });
    assert!(
        world.run_file(run, "owner.lock").is_file(),
        "a driver that could not hand the run on released it"
    );
    assert!(world.events_of(run, "run-hook-fired").is_empty());
}

/// **Two drivers, two records, and `status` reads the latest.** The first driver
/// pauses on a person; the person attests, and the driver that adopts the run
/// completes it. `status` says how that last driver ended, not the first.
#[test]
fn status_reads_the_last_driver_exit_when_a_run_has_had_several() {
    let world = hooked_world("exit-latest");
    let run = "twice";
    start_attached(
        &world,
        run,
        vec![human("approve", &[]), agent("build", &["approve"])],
    )
    .exited(0)
    .out_has("\"settlement\":\"awaiting-planner\"");
    world
        .run(&["status", run])
        .exited(0)
        .out_has("settled awaiting-planner");
    world.run(&["attest", run, "approve"]).exited(0);
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");

    let exits = world.events_of(run, "driver-exited");
    let settlements: Vec<&Value> = exits
        .iter()
        .map(|exited| &exited["payload"]["settlement"])
        .collect();
    assert_eq!(
        settlements,
        [&json!("awaiting-planner"), &json!("complete")]
    );
    let status = world.run(&["status", run]);
    status.exited(0);
    let said: Vec<&str> = status
        .stdout
        .lines()
        .filter(|line| line.contains("driver exited"))
        .collect();
    assert_eq!(said.len(), 1, "{}", status.stdout);
    assert!(
        said[0].contains(exits[1]["ts"].as_str().expect("a time")),
        "{}",
        said[0]
    );
    assert!(said[0].ends_with("settled complete"), "{}", said[0]);
}

/// **A record this build cannot read.** A `driver-exited` a newer build wrote,
/// with a settlement word this one does not know, is still a driver that exited
/// at that time: `status` says so and says the rest could not be read, rather
/// than guessing a settlement or dropping the line.
#[test]
fn status_says_a_driver_exit_it_cannot_read_could_not_be_read() {
    let world = hooked_world("exit-unreadable");
    let run = "newer";
    start_attached(&world, run, vec![agent("build", &[])])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");
    // At the instant this build's own record was written, from a stream that sorts
    // after it, so it is the last one in the store's merge order.
    let ours = the_exit(&world, run);
    let at = ours["ts"].as_str().expect("a time").to_string();
    // llmlint: ignore[tests_mirror_real_usage] what a newer build appends to a runs root this
    // one reads: no verb of this build writes a settlement word it does not have, and the
    // journal is the one place such a record reaches a reader.
    let mut journal = std::fs::OpenOptions::new()
        .append(true)
        .open(world.run_file(run, "events.jsonl"))
        .expect("the journal opens");
    std::io::Write::write_all(
        &mut journal,
        format!(
            "{}\n",
            json!({"v": 2, "ts": at, "stream": "~newer-build", "seq": 0,
                   "source": "pipeline", "kind": "driver-exited",
                   "labels": {"run_id": run},
                   "payload": {"pid": 1, "settlement": "handed-elsewhere", "reason": null,
                               "last_answered_command": null},
                   "artifacts": []})
        )
        .as_bytes(),
    )
    .expect("the record is appended");

    world
        .run(&["status", run])
        .exited(0)
        .out_has(&format!(
            "({at}): its driver-exited record could not be read"
        ))
        .out_lacks("handed-elsewhere");
}
