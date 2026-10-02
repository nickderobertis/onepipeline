//! A run's ending, read back the three ways a reader asks for it, driven end to end
//! against the compiled binary.
//!
//! Every journey drives a real run into one state — complete, ended failed, ended
//! stopped, ended unfinished, paused on a human action or on a blocking question,
//! abandoned with work it could move, or still driven — and reads it as a person
//! and as a script do: the word `status <RUN>` prints, the word on its `runs` row,
//! and the one JSON line `status <RUN> --json` prints. Each ended run names a real
//! run-end hook, the fixture `run_end_hooks.rs` drives, and the ending the reading
//! reports is held to the hook that same run fired. The SDK's two readers of the
//! word — `views::standing_word_of` over the run's summary document and
//! `views::liveness_word` over its fold — are held to the word the binary printed.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary; the hook is the operator's own command, and this suite supplies a real
// one. `harness.rs` carries the same suppression and the full rationale.

// llmlint: ignore-file[expensive_tests_stay_behind_their_own_edge] measured rather than
// assumed: the eight journeys here took about 205 seconds summed and 54 on the wall under
// nextest's parallelism on a loaded host, each starting real drivers, real run-end hooks and
// real dispatches. What they exercise is one reading across `views`, `hooks`, `summary`,
// `unwatched` and the `status` verb in `driver` together, through the compiled binary, so
// the narrowest edge they can honestly sit behind is the crate itself, which is this
// target's; the note-journey dependency is this binary's own, shared by every module in it.
// `mod run_ending` in `main.rs` carries the same reason, because that declaration is the
// other site the rule reads.

use serde_json::{json, Value};

use crate::harness::{
    agent, human, plan_of, rows, World, NOTHING_DRIVING, RUNS_UNWATCHED, USAGE_ERROR,
};
use crate::run_end_hooks::{hook, invocations, records, HOOK_TIMEOUT, RECORD_ENV};

/// A world whose hook fixture records into the world's own scratch, and whose
/// runs root is the one this process's SDK reads.
fn ending_world(name: &str) -> World {
    let world = World::new(name);
    std::fs::create_dir_all(records(&world)).expect("a record directory");
    let record = records(&world).to_string_lossy().into_owned();
    // `standing_word_of` reads the run's channel under the runs root this process
    // reads, as `liveness_of` does; nextest runs each journey in its own process,
    // so the variable is this journey's alone.
    std::env::set_var("ONEPIPELINE_RUNS_DIR", &world.runs);
    world.with_env(RECORD_ENV, &record)
}

fn start(world: &World, run: &str, nodes: Vec<Value>, mode: &str) -> crate::harness::Run {
    let path = world.plan(run, &plan_of(run, nodes));
    let hook = hook(world);
    world.run_from(
        &world.project,
        &[
            "start",
            &path,
            mode,
            "--success-hook",
            &hook,
            "--failure-hook",
            &hook,
            "--hook-timeout",
            HOOK_TIMEOUT,
        ],
    )
}

/// One run, read the three ways a reader asks for it, each held to `word`.
///
/// Returns the `--json` document, after holding it to the contract's shape: one
/// line, the seven fields, `driven` the negation of an undriven liveness, never an
/// ending and a pause at once, and neither while the run is driven.
fn read_as(world: &World, run: &str, word: &str) -> Value {
    let status = world.run(&["status", run]);
    status.exited(0);
    let first = status.stdout.lines().next().unwrap_or_default();
    assert!(
        first.starts_with(&format!("{run}  {word}  ")),
        "`status {run}` does not read `{word}`:\n{}",
        status.stdout
    );

    let listing = world.run(&["runs"]);
    listing.exited(0);
    let row = rows(&listing.stdout)
        .into_iter()
        .find(|line| line.split_whitespace().nth(1) == Some(run))
        .unwrap_or_else(|| panic!("`runs` lists no row for {run}:\n{}", listing.stdout));
    assert!(
        row.split("  ").any(|part| part.trim() == word),
        "the `runs` row for {run} does not read `{word}`: {row}"
    );

    let machine = world.run(&["status", run, "--json"]);
    machine.exited(0);
    assert_eq!(
        machine.stdout.lines().count(),
        1,
        "`status {run} --json` printed other than one line:\n{}",
        machine.stdout
    );
    let reading: Value = serde_json::from_str(machine.stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "`status {run} --json` printed no JSON object ({error}):\n{}",
            machine.stdout
        )
    });
    let fields: Vec<&str> = reading
        .as_object()
        .expect("the reading is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        [
            "schema_version",
            "run_id",
            "word",
            "liveness",
            "driven",
            "ending",
            "paused"
        ]
    );
    assert_eq!(reading["schema_version"], 1);
    assert_eq!(reading["run_id"], run);
    assert_eq!(reading["word"], word, "{reading}");
    let liveness = reading["liveness"].as_str().expect("a liveness word");
    assert!(
        ["ACTIVE", "PARKED", "DRIVER DEAD", "UNDRIVEN"].contains(&liveness),
        "{reading}"
    );
    assert_eq!(
        reading["driven"],
        json!(liveness != "DRIVER DEAD"),
        "{reading}"
    );
    assert!(
        reading["ending"].is_null() || reading["paused"].is_null(),
        "a reading both ended and paused: {reading}"
    );
    if reading["driven"] == json!(true) {
        assert!(
            reading["ending"].is_null() && reading["paused"].is_null(),
            "a driven run read as ended or paused: {reading}"
        );
    }

    let paths = onepipeline::views::RunPaths::under(&world.runs, run);
    let summary = onepipeline::views::RunSummary::of(&paths).expect("the run's summary reads");
    let view = onepipeline::views::RunView::open(&paths).expect("the run folds");
    assert_eq!(
        onepipeline::views::standing_word_of(&summary),
        word,
        "the summary's word"
    );
    assert_eq!(
        onepipeline::views::liveness_word(&view),
        word,
        "the fold's word"
    );
    assert_eq!(
        onepipeline::views::standing_word_of(&summary),
        onepipeline::views::liveness_word(&view)
    );

    // An ended run is given no advice naming `adopt`, by either view.
    if word.starts_with("ENDED") || word == "SETTLED" {
        status.out_lacks("adopt");
        assert!(
            !listing.stdout.contains(&format!("onepipeline adopt {run}")),
            "`runs` advises adopting the ended run {run}:\n{}",
            listing.stdout
        );
    }
    reading
}

/// `unwatched` reports a run of this session's that nothing watches with the word
/// `runs` gives it: its line names the run and carries `word`.
fn unwatched_reads_as(world: &World, run: &str, word: &str) {
    let asked = world.run(&["unwatched"]);
    asked.exited(RUNS_UNWATCHED);
    let line = asked
        .stdout
        .lines()
        .find(|line| line.split_whitespace().next() == Some(run))
        .unwrap_or_else(|| panic!("`unwatched` does not report {run}:\n{}", asked.stdout));
    assert!(
        line.contains(&format!(" {word} ")),
        "`unwatched` does not report {run} as `{word}`: {line}"
    );
}

/// The ending the run-end hook a run fired names, in the reading's own words.
fn fired_ending(world: &World, run: &str) -> (Value, Value) {
    let fired = world.events_of(run, "run-hook-fired");
    let last = fired
        .last()
        .unwrap_or_else(|| panic!("run {run} fired no run-end hook"));
    let payload = &last["payload"];
    match (payload["hook"].as_str(), payload["reason"]["kind"].as_str()) {
        (Some("success"), None) => (json!("complete"), json!([])),
        (Some("failure"), Some("nodes")) => (json!("failed"), payload["reason"]["nodes"].clone()),
        (Some("failure"), Some(kind @ ("unfinished" | "stopped"))) => {
            (json!(kind), payload["reason"]["nodes"].clone())
        }
        other => panic!("run {run} fired a hook this journey cannot read: {other:?}"),
    }
}

fn ending_is_the_hook_it_fired(world: &World, run: &str, reading: &Value) {
    let (kind, nodes) = fired_ending(world, run);
    assert_eq!(reading["ending"]["kind"], kind, "{reading}");
    assert_eq!(reading["ending"]["nodes"], nodes, "{reading}");
}

/// A paused run's advice names each waiting human action with `attest` and `drop`
/// or its blocking question with `reply`, names `stop`, and never `adopt`.
fn advised_to_settle_or_stop(world: &World, run: &str, actions: &[&str], blocking: bool) {
    for rendered in [world.run(&["status", run]), world.run(&["runs"])] {
        rendered.exited(0);
        for action in actions {
            rendered.out_has(&format!("onepipeline attest {run} {action}"));
            rendered.out_has(&format!(
                "complete the waiting human action {action} with: onepipeline attest {run} \
                 {action} — or retire it with a `drop` on: onepipeline reply {run}"
            ));
        }
        if blocking {
            rendered.out_has(&format!(
                "answer the blocking planner question with: onepipeline reply {run}"
            ));
        }
        rendered.out_has(&format!("or end the run with: onepipeline stop {run}"));
        assert!(
            !rendered
                .stdout
                .contains(&format!("onepipeline adopt {run}")),
            "a paused run is advised to adopt:\n{}",
            rendered.stdout
        );
    }
}

/// An undriven run waiting on a person reads `PAUSED`, naming the action and how
/// to settle it; once it is attested and adopted, every node is `done`, the success
/// hook fires, and the run reads `SETTLED` with ending `complete` — and still does
/// once a stop is recorded over it.
#[test]
fn a_run_paused_on_a_human_action_reads_paused_and_once_attested_reads_settled() {
    let world = ending_world("ending-paused-human");
    let run = "signoff";
    start(&world, run, vec![human("approve", &[])], "--attach")
        .exited(0)
        .out_has("\"settlement\":\"awaiting-planner\"");

    let reading = read_as(&world, run, "PAUSED");
    assert_eq!(reading["driven"], false);
    assert_eq!(reading["ending"], Value::Null);
    assert_eq!(
        reading["paused"],
        json!({"human_actions": ["approve"], "blocking_surface": false})
    );
    advised_to_settle_or_stop(&world, run, &["approve"], false);
    assert!(invocations(&world, run).is_empty());
    unwatched_reads_as(&world, run, "PAUSED");

    world.run(&["attest", run, "approve"]).exited(0);
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");
    assert_eq!(invocations(&world, run), ["success"]);

    let reading = read_as(&world, run, "SETTLED");
    assert_eq!(reading["driven"], false);
    assert_eq!(reading["paused"], Value::Null);
    assert_eq!(reading["ending"], json!({"kind": "complete", "nodes": []}));
    ending_is_the_hook_it_fired(&world, run, &reading);

    // A stop recorded over a complete run changes nothing it reads: complete
    // outranks a stop, and the ending is still the success hook it fired.
    world.run(&["stop", run]).exited(0);
    assert_eq!(world.events_of(run, "run-stopped").len(), 1);
    assert_eq!(read_as(&world, run, "SETTLED"), reading);
    ending_is_the_hook_it_fired(&world, run, &reading);
}

/// A human node settled `failed` from evidence, with nothing else unfinished, ends
/// the run failed: the failure hook fires with reason `nodes`, and the run reads
/// `ENDED failed`, listing the node as the hook was handed it — and an `adopt`
/// of it moves nothing.
#[test]
fn a_human_node_settled_failed_from_evidence_ends_the_run_failed() {
    let world = ending_world("ending-failed");
    let run = "refused";
    start(
        &world,
        run,
        vec![human("approve", &[]), human("sign-off", &[])],
        "--attach",
    )
    .exited(0);
    world.run(&["attest", run, "approve"]).exited(0);
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 3, "commands": [
                {"op": "settle", "id": "sign-off", "outcome": "failed",
                 "evidence": "the reviewer declined it"}
            ]})
            .to_string(),
        )
        .exited(0);
    // The driver that judges it fires the hook it ended under, and settles at once.
    world.run(&["adopt", run]).exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);

    let reading = read_as(&world, run, "ENDED failed");
    assert_eq!(reading["driven"], false);
    assert_eq!(reading["paused"], Value::Null);
    assert_eq!(reading["ending"]["kind"], "failed");
    assert_eq!(
        reading["ending"]["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .map(|node| (node["id"].clone(), node["status"].clone()))
            .collect::<Vec<_>>(),
        [(json!("sign-off"), json!("failed"))],
        "{reading}"
    );
    ending_is_the_hook_it_fired(&world, run, &reading);
    unwatched_reads_as(&world, run, "ENDED failed");

    // Adopting it again settles at once and changes nothing about the reading.
    world.run(&["adopt", run]).exited(NOTHING_DRIVING);
    assert_eq!(read_as(&world, run, "ENDED failed"), reading);
}

/// A run stopped while its human nodes wait has ended `stopped`: the stop fires the
/// failure hook as `stopped`, and the run reads `ENDED stopped`, listing both nodes
/// `waiting` — not `PAUSED`, though a decision is still outstanding.
#[test]
fn a_run_stopped_while_human_nodes_wait_reads_ended_stopped() {
    let world = ending_world("ending-stopped");
    let run = "halted";
    start(
        &world,
        run,
        vec![human("first", &[]), human("second", &[])],
        "--attach",
    )
    .exited(0);
    read_as(&world, run, "PAUSED");

    world
        .run(&["stop", run])
        .exited(0)
        .out_has("\"stopped\":true");
    assert_eq!(invocations(&world, run), ["failure"]);

    let reading = read_as(&world, run, "ENDED stopped");
    assert_eq!(reading["paused"], Value::Null);
    assert_eq!(
        reading["ending"],
        json!({"kind": "stopped", "nodes": [
            {"id": "first", "status": "waiting", "outcome": null},
            {"id": "second", "status": "waiting", "outcome": null},
        ]})
    );
    ending_is_the_hook_it_fired(&world, run, &reading);
}

/// A run whose only unfinished node a planner's `cancel` parked has ended
/// `unfinished`: the failure hook fires with that reason, and the run reads
/// `ENDED unfinished` with no advice to requeue or adopt.
#[test]
fn a_run_whose_only_unfinished_node_was_cancelled_reads_ended_unfinished() {
    let world = ending_world("ending-unfinished");
    world.script("slow.turn-open", "");
    world.script("slow.wait", "hold");
    world.script("slow.stops-when-interrupted", "");
    let run = "idled";
    start(
        &world,
        run,
        vec![agent("build", &[]), agent("slow", &[])],
        "--detach",
    )
    .exited(0);
    world.until("the held node's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
    });
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 2, "commands": [{"op": "cancel", "id": "slow"}]}).to_string(),
        )
        .exited(0);
    world.until("the run's hook to be recorded as ended", |world| {
        !world.events_of(run, "run-hook-finished").is_empty()
    });
    assert_eq!(invocations(&world, run), ["failure"]);

    let reading = read_as(&world, run, "ENDED unfinished");
    assert_eq!(reading["paused"], Value::Null);
    assert_eq!(
        reading["ending"],
        json!({"kind": "unfinished", "nodes": [
            {"id": "slow", "status": "parked", "outcome": null},
        ]})
    );
    ending_is_the_hook_it_fired(&world, run, &reading);
    for rendered in [world.run(&["status", run]), world.run(&["runs"])] {
        rendered.out_lacks("requeue");
    }
    world.release("slow.go");
}

/// An undriven run held on a blocking question nobody has answered reads `PAUSED`
/// with `blocking_surface` true, and is told to answer it with `reply` — even one
/// whose graph has otherwise ended.
#[test]
fn a_run_held_on_a_blocking_question_reads_paused_on_it() {
    let world = ending_world("ending-paused-blocking");
    let mut later = agent("later", &["build"]);
    later["parked"] = json!(true);
    let run = "asking";
    start(&world, run, vec![agent("build", &[]), later], "--attach").exited(NOTHING_DRIVING);
    assert_eq!(invocations(&world, run), ["failure"]);
    read_as(&world, run, "ENDED unfinished");

    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 3, "commands": [
                {"op": "finding", "blocking": true, "id": "later",
                 "message": "should later wait for the schema change?"}
            ]})
            .to_string(),
        )
        .exited(0);
    world.until("the blocking finding to be raised", |world| {
        world
            .queued_surfaces(run)
            .iter()
            .any(|surface| surface["blocking"] == json!(true))
    });

    let reading = read_as(&world, run, "PAUSED");
    assert_eq!(reading["ending"], Value::Null);
    assert_eq!(
        reading["paused"],
        json!({"human_actions": [], "blocking_surface": true})
    );
    advised_to_settle_or_stop(&world, run, &[], true);
}

/// An undriven run held on two human actions and a blocking question at once reads
/// `PAUSED` on all three, and its advice names every way to settle them.
#[test]
fn a_run_paused_on_two_human_actions_and_a_blocking_question_names_all_three() {
    let world = ending_world("ending-paused-both");
    let run = "decisions";
    start(
        &world,
        run,
        vec![human("first", &[]), human("second", &[])],
        "--attach",
    )
    .exited(0)
    .out_has("\"settlement\":\"awaiting-planner\"");
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 3, "commands": [
                {"op": "finding", "blocking": true, "id": "first",
                 "message": "is first still the person to ask?"}
            ]})
            .to_string(),
        )
        .exited(0);
    world.until("the blocking finding to be raised", |world| {
        world
            .queued_surfaces(run)
            .iter()
            .any(|surface| surface["blocking"] == json!(true))
    });

    let reading = read_as(&world, run, "PAUSED");
    assert_eq!(reading["ending"], Value::Null);
    assert_eq!(
        reading["paused"],
        json!({"human_actions": ["first", "second"], "blocking_surface": true})
    );
    advised_to_settle_or_stop(&world, run, &["first", "second"], true);
    assert!(invocations(&world, run).is_empty());
}

/// A run with a live driver reads as driven, with neither an ending nor a pause;
/// once that driver is gone while a node is still `ready` and nothing stopped the
/// run, it reads `DRIVER DEAD` — still neither ended nor paused — and is told to
/// adopt it. A human action waiting beside that work does not make it `PAUSED`:
/// work the driver could still move outranks a decision.
#[cfg(unix)]
#[test]
fn a_driven_run_reads_driven_and_one_whose_driver_died_with_work_ready_reads_driver_dead() {
    let world = ending_world("ending-driver-dead");
    world.script("build.wait", "hold");
    let run = "crashed";
    let mut plan = plan_of(
        run,
        vec![
            agent("build", &[]),
            agent("after", &[]),
            human("approve", &[]),
        ],
    );
    // One slot, so the second node is `ready` behind the held first.
    plan["concurrency"] = json!(1);
    let path = world.plan(run, &plan);
    let started = world.run(&["start", &path, "--detach"]);
    started.exited(0);
    let driver = u32::try_from(
        started.json()["pid"]
            .as_u64()
            .expect("a detached launch names the driver it retained"),
    )
    .expect("a pid");
    world.until("the dispatch to be in flight", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });

    let reading = read_as(&world, run, "ACTIVE");
    assert_eq!(reading["driven"], true);
    assert_eq!(reading["liveness"], "ACTIVE");

    crate::harness::end_process(driver);
    let reading = read_as(&world, run, "DRIVER DEAD");
    assert_eq!(reading["driven"], false);
    assert_eq!(reading["ending"], Value::Null);
    assert_eq!(reading["paused"], Value::Null);
    world
        .run(&["status", run])
        .out_has("DRIVER DEAD: nothing is driving this run; adopt it or stop it")
        // The journey's precondition: a node is `ready`, not only one running.
        .out_has("  after: ready");
    // And a decision is outstanding beside it.
    let results = world.run(&["results", run]);
    assert!(
        results
            .stdout
            .lines()
            .any(|line| line.trim_start().starts_with("approve") && line.contains("waiting")),
        "the journey's precondition, a waiting human action, does not hold:\n{}",
        results.stdout
    );
    world.release("build.go");
}

/// `--json` reads one run: the parser refuses it without one, and a run that is
/// not there is refused exactly as `status <RUN>` refuses it.
#[test]
fn status_json_requires_a_run_and_refuses_an_unknown_one_as_status_does() {
    let world = ending_world("ending-json-refusals");
    let bare = world.run(&["status", "--json"]);
    bare.exited(USAGE_ERROR);
    assert!(bare.stdout.is_empty(), "{}", bare.stdout);
    assert!(
        bare.stderr.contains("--json") && bare.stderr.contains("<RUN>"),
        "the parser's refusal does not name the flag and the run it needs:\n{}",
        bare.stderr
    );

    let unknown = world.run(&["status", "nosuch", "--json"]);
    let plain = world.run(&["status", "nosuch"]);
    assert_ne!(plain.code, 0, "{}", plain.stderr);
    assert_eq!(unknown.code, plain.code);
    assert_eq!(unknown.stderr, plain.stderr);
    assert!(unknown.stdout.is_empty(), "{}", unknown.stdout);
}
