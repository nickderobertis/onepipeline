//! The engine's views of a dispatch whose turn records reference their history.
//!
//! With history on, the linked `oneagentgraph` relays a turn's records as
//! `transcript` references into the oneharness session file instead of copies.
//! These journeys drive a real conversation over the real sibling — the
//! `fake-oneharness` double writing its session through oneharness's own writer,
//! each event on disk before it reaches the stream, as the real CLI does — and
//! read it back through the verbs an operator reads a run with: `monitor`,
//! `watch` in both its forms, `transcript` and `status`.

// llmlint: ignore-file[e2e_not_mocked] the rationale is `harness.rs`'s; what is specific
// here is that the sibling, its relay and the history writer are all real, and every
// assertion is on the compiled binary's exit code and output.

use std::path::PathBuf;

use crate::harness::{agent, plan_of, World};
use serde_json::{json, Value};

const NODE: &str = "build";

/// Placed past the 4,096th byte of the node's task, so only the whole prompt —
/// never the copy a journal record was cut to — carries it.
const PAST_THE_BOUND: &str = "SENTINEL-PAST-THE-JOURNAL-BOUND";

/// The command the held turn runs once released: two lines, so its one-line
/// description is the first and the whole input carries both.
const HELD_CALL: &str = "cargo nextest run --workspace --no-fail-fast\necho held-call-finished";

/// Ends what the held call returns, past every ceiling a copy or a view ever
/// held a tool's output to.
const OUTPUT_TAIL: &str = "HELD-OUTPUT-TAIL";

/// What the held call returns: longer than the 4,096 characters a journal copy
/// or the transcript's report ceiling ever kept, with [`OUTPUT_TAIL`] last.
fn held_output() -> String {
    format!("{}{OUTPUT_TAIL}", "test result: ok. 1 passed\n".repeat(240))
}

/// A node task longer than a journal copy holds, with [`PAST_THE_BOUND`] at its
/// end.
fn long_task() -> String {
    let filler =
        "The engine reads every turn's content through its transcript reference.\n".repeat(70);
    assert!(filler.len() > onepipeline::event::MAX_PAYLOAD_TEXT_BYTES);
    format!(
        "## What\n{filler}\n## Why\nSo the run can settle.\n\n## Acceptance criteria\n- \
         {PAST_THE_BOUND} is read whole."
    )
}

/// The journal's relayed `turn-activity` records of `node`, in order.
fn activities(world: &World, run: &str) -> Vec<Value> {
    world
        .journal(run)
        .into_iter()
        .filter(|event| event["source"] == "agentgraph" && event["kind"] == "turn-activity")
        .collect()
}

/// The history file a relayed reference names.
fn referenced_file(event: &Value) -> PathBuf {
    let reference = &event["payload"]["transcript"];
    let part = |key: &str| {
        reference[key]
            .as_str()
            .unwrap_or_else(|| panic!("the reference names no {key}: {event}"))
            .to_string()
    };
    PathBuf::from(part("history_dir"))
        .join(part("history_project"))
        .join(format!("{}.jsonl", part("history_session")))
}

/// Every machine record a `watch` wrote.
fn machine(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("a watch record is not JSON: {error}: {line}"))
        })
        .collect()
}

/// The machine record a `watch` wrote for the relayed record at `seq` on
/// `stream`.
fn watched_record<'a>(records: &'a [Value], event: &Value) -> &'a Value {
    records
        .iter()
        .find(|record| {
            record["watch"] == "event"
                && record["event"]["stream"] == event["stream"]
                && record["event"]["seq"] == event["seq"]
        })
        .unwrap_or_else(|| panic!("`watch` wrote no record for {event}: {records:#?}"))
}

/// Start a supervised dispatch whose worker's turn holds, release it as far as
/// the call it makes once released, and answer the record relaying that call.
fn held_mid_turn(world: &World, run: &str) -> Value {
    world.write_graphs();
    world.write_supervised_node_graph();
    world.script("turn.hold", "hold");
    world.script("turn.held-call", HELD_CALL);
    world.script("turn.held-output", &held_output());
    let mut node = agent(NODE, &[]);
    node["task"] = Value::String(long_task());
    let path = world.plan(run, &plan_of(run, vec![node]));
    world
        .run_on_agentgraph(&["start", &path, "--detach"])
        .exited(0);
    world.until("the worker's turn to reach its hold", |world| {
        activities(world, run).len() >= 3
    });
    world.release("turn.go");
    world.until("the held call to be relayed", |world| {
        activities(world, run)
            .iter()
            .any(|event| event["payload"]["index"] == 3)
    });
    activities(world, run)
        .into_iter()
        .find(|event| event["payload"]["index"] == 3)
        .expect("the held call is in the journal")
}

/// **A dispatch in flight shows each tool call's description, and the whole of
/// what it was asked.** The worker's turn is held after the history file has
/// gained a tool call the turn has not finished: the relayed record carries a
/// reference and no copy, and `monitor`, both forms of `watch`, `status` and
/// `transcript` all read it through the engine's resolver — the call's
/// one-line description on the line, the whole multi-line command on the machine
/// record, and the whole prompt, past the bound a copy was cut to, in the
/// transcript.
#[test]
fn a_dispatch_in_flight_shows_each_tool_calls_description_and_the_whole_instruction() {
    let world = World::new("transcript-in-flight");
    let run = "inflight";
    let held = held_mid_turn(&world, run);

    // The record is a reference, exactly as the sibling relayed it, and the turn
    // it belongs to has not ended.
    let payload = &held["payload"];
    assert!(
        payload.get("transcript").is_some()
            && payload.get("detail").is_none()
            && payload.get("output").is_none(),
        "the held call was relayed as something other than a bare reference: {held}"
    );
    assert!(
        referenced_file(&held).is_file(),
        "the reference names no file: {held}"
    );
    let ended = |world: &World| {
        world
            .events_of(run, "turn-completed")
            .iter()
            .any(|event| event["payload"]["role"] == "assistant")
    };
    assert!(!ended(&world), "the worker's turn ended before it was read");

    let description = "cargo nextest run --workspace --no-fail-fast";
    let monitor = world.run(&["monitor", run, "--all"]);
    monitor.exited(0);
    assert!(
        monitor
            .stdout
            .lines()
            .any(|line| line.ends_with(&format!("turn-activity {description}"))),
        "`monitor` does not describe the call in flight:\n{}",
        monitor.stdout
    );

    let watched = world.run(&["watch", run, "--all", "--timeout", "0"]);
    assert!(
        watched
            .stderr
            .lines()
            .any(|line| line.ends_with(&format!("turn-activity {description}"))),
        "`watch` does not describe the call in flight:\n{}",
        watched.stderr
    );
    let records = machine(&watched.stdout);
    let record = watched_record(&records, &held);
    assert_eq!(record["event"], held, "`watch` rewrote the relayed record");
    assert_eq!(record["transcript"]["description"], json!(description));
    let input: Value = serde_json::from_str(
        record["transcript"]["input"]
            .as_str()
            .unwrap_or_else(|| panic!("the machine record carries no input: {record}")),
    )
    .expect("a structured input is served as its JSON");
    assert_eq!(
        input["command"],
        json!(HELD_CALL),
        "the machine record does not carry the call's whole input: {record}"
    );
    // The reasoning before the call, and the result answering the first call,
    // each under the field naming what it is.
    let activity = activities(&world, run);
    let at = |index: u64| {
        let event = activity
            .iter()
            .find(|event| event["payload"]["index"] == index)
            .unwrap_or_else(|| panic!("no activity {index} was relayed"));
        watched_record(&records, event)["transcript"].clone()
    };
    let reasoned = at(0);
    assert_eq!(
        reasoned["reasoning"],
        json!("The quickest proof is to run it."),
        "{reasoned}"
    );
    assert_eq!(
        reasoned["description"],
        json!("The quickest proof is to run it.")
    );
    let returned = at(2);
    assert_eq!(returned["output"], json!("the turn ran"), "{returned}");
    assert_eq!(returned["input"], Value::Null, "{returned}");
    // Through the `planner` profile a bare `watch` reads with, no relayed turn
    // is shown, reference or not: that profile is what a supervisor wakes on.
    let planner = world.run(&["watch", run, "--timeout", "0"]);
    assert!(
        machine(&planner.stdout)
            .iter()
            .all(|record| record["event"]["source"] != "agentgraph"),
        "the planner profile showed a relayed turn:\n{}",
        planner.stdout
    );
    assert!(
        !planner.stderr.contains("turn-activity"),
        "{}",
        planner.stderr
    );
    // A record carrying no reference is written exactly as it always was.
    assert!(
        records
            .iter()
            .filter(|record| record["watch"] == "event")
            .filter(|record| record["event"]["payload"].get("transcript").is_none())
            .all(|record| record.get("transcript").is_none()),
        "{records:#?}"
    );

    let status = world.run(&["status", run]);
    status.exited(0);
    assert!(
        status.stdout.contains(&format!("now {description} (")),
        "`status` does not say what the dispatch is doing now:\n{}",
        status.stdout
    );

    // A checkpoint the build before references left — schema 13, its fold of
    // the same record the copy-era `bash` with no reference — is rebuilt from
    // the journal rather than read: the readout is the reference's, still.
    let checkpoint = world.run_file(run, "checkpoint.json");
    let mut earlier: Value = serde_json::from_slice(
        &std::fs::read(&checkpoint).expect("reading the run left a checkpoint"),
    )
    .expect("the checkpoint is JSON");
    assert_eq!(earlier["schema_version"], json!(14), "{earlier}");
    let doing = &mut earlier["state"]["activity"][NODE]["doing"];
    assert!(
        doing["activity"]["history_id"].is_string(),
        "the checkpoint does not keep the reference it folded: {doing}"
    );
    // llmlint: ignore-block[tests_mirror_real_usage] a schema-13 checkpoint is one only
    // the build before this change wrote, which no invocation of this build can produce
    // and which this suite cannot run; the document is this run's own checkpoint with
    // exactly the two things that build wrote differently — its version, and its fold of
    // the referenced call as the copy-era tool name — so what the compiled binary is
    // asked is the one question the version bump answers, as `checkpoint.rs`'s journeys
    // ask it of the other states a cache can be left in.
    *doing = json!("bash");
    earlier["schema_version"] = json!(13);
    std::fs::write(&checkpoint, earlier.to_string()).expect("the earlier checkpoint is laid down");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let status = world.run(&["status", run]);
    status.exited(0);
    assert!(
        status.stdout.contains(&format!("now {description} ("))
            && !status.stdout.contains("now bash ("),
        "a schema-13 checkpoint was read rather than rebuilt from the journal:\n{}",
        status.stdout
    );

    let transcript = world.run(&["transcript", run]);
    transcript.exited(0);
    assert!(
        transcript.stdout.contains(PAST_THE_BOUND),
        "`transcript` shows the instruction cut short of what the turn was asked:\n{}",
        transcript.stdout
    );
    assert!(
        transcript
            .stdout
            .contains("tool_call bash  {\"command\":\"cargo nextest run --workspace --no-fail-fast\\necho held-call-finished\"}"),
        "`transcript` does not show the call's whole input:\n{}",
        transcript.stdout
    );
    // What the call returned, whole: its tail past the ceiling a view ever cut
    // a tool's output at, with no note of anything left out.
    world.until("the held call's result to be relayed", |world| {
        activities(world, run)
            .iter()
            .any(|event| event["payload"]["index"] == 4)
    });
    let transcript = world.run(&["transcript", run]);
    let result_line = transcript
        .stdout
        .lines()
        .take_while(|line| !line.starts_with("  report "))
        .find(|line| line.contains(OUTPUT_TAIL))
        .unwrap_or_else(|| {
            panic!(
                "`transcript` does not show the held call's output to its end:\n{}",
                transcript.stdout
            )
        });
    assert!(
        result_line.trim_start().starts_with("tool_result")
            && result_line.chars().count() > held_output().len()
            && !result_line.contains(" characters]"),
        "the held call's output was cut or noted as cut: {result_line}"
    );
    assert!(!ended(&world), "the turn ended while it was being read");

    // Its history lost mid-turn — the events first, then the whole file — and
    // every view, `status` among them, names the file and why rather than
    // going blank. Put back before the turn goes on, which appends to it.
    let file = referenced_file(&held);
    let whole = std::fs::read_to_string(&file).expect("the session file reads");
    let opening_only: String = whole
        .lines()
        .filter(|line| !line.contains("\"type\":\"event\""))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&file, opening_only).expect("the session file is rewritten");
    let missing_entry = format!("transcript unavailable: {}: missing-entry", file.display());
    assert_unavailable_everywhere(&world, run, &held, &missing_entry, true);
    std::fs::remove_file(&file).expect("the session file is removed");
    let missing_file = format!("transcript unavailable: {}: missing-file", file.display());
    assert_unavailable_everywhere(&world, run, &held, &missing_file, true);
    std::fs::write(&file, whole).expect("the session file is put back");
    assert!(!ended(&world), "the turn ended while its history was gone");

    world.release("turn.settle");
    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });

    // What the agent said once released, both as its own activity and as the
    // turn's closing words, served whole on the machine record.
    let settled = world.run(&["watch", run, "--all", "--timeout", "0"]);
    let records = machine(&settled.stdout);
    let said: Vec<&Value> = records
        .iter()
        .filter(|record| {
            record["event"]["payload"]["role"] == "assistant"
                && record["event"]["kind"] == "turn-message"
                || record["event"]["payload"]["kind"] == "message"
        })
        .collect();
    assert_eq!(said.len(), 2, "{records:#?}");
    let transcript = world.run(&["transcript", run]);
    transcript.exited(0);
    assert!(
        transcript
            .stdout
            .contains("    assistant said\n      Ran what the task asked for.\n"),
        "`transcript` does not show the turn's closing words under its party:\n{}",
        transcript.stdout
    );
    for record in said {
        assert_eq!(
            record["transcript"]["message"],
            json!("Ran what the task asked for."),
            "{record}"
        );
    }
}

/// **A reference whose history is gone says so, on every view, rather than
/// rendering blank** — and `scratch discard` is not what takes it. The run ends
/// failed and its kept scratch is discarded, and the session file every
/// reference names is still there. Then its event lines are lost — every activity names an
/// entry its file no longer holds — and then the file itself, and each view
/// names the file and why.
#[test]
fn a_reference_whose_history_is_gone_renders_as_unavailable_on_every_view() {
    let world = World::new("transcript-unavailable");
    let run = "unavailable";
    // The turn fails, so the run ends failed with its dispatch's scratch kept
    // — the one ending `scratch discard` removes anything from.
    world.script("harness.fail", "1");
    let held = held_mid_turn(&world, run);
    world.release("turn.settle");
    world.until("the failed ending to keep its scratch", |world| {
        !world.events_of(run, "scratch-retained").is_empty()
    });
    world.until("the driver to let go", |world| {
        world.run(&["status", run, "--json"]).json()["driven"] == false
    });
    let file = referenced_file(&held);

    // Scratch is the run's, and the history is the store's: discarding the one
    // leaves the other.
    let discarded = world.run(&["scratch", "discard", run]);
    discarded.exited(0);
    assert!(
        discarded.stdout.starts_with("removed "),
        "`scratch discard` removed nothing, so it proves nothing about the history: {}",
        discarded.stdout
    );
    assert!(
        file.is_file(),
        "`scratch discard` removed the history file a reference names: {}",
        file.display()
    );

    // The events lost, the opening kept: each activity names a missing entry.
    let kept: String = std::fs::read_to_string(&file)
        .expect("the session file reads")
        .lines()
        .filter(|line| !line.contains("\"type\":\"event\""))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&file, kept).expect("the session file is rewritten");
    let missing_entry = format!("transcript unavailable: {}: missing-entry", file.display());
    assert_unavailable_everywhere(&world, run, &held, &missing_entry, false);
    let transcript = world.run(&["transcript", run]);
    assert!(
        transcript.stdout.contains(PAST_THE_BOUND),
        "the opening the file still holds stopped resolving:\n{}",
        transcript.stdout
    );

    // And the file itself.
    std::fs::remove_file(&file).expect("the session file is removed");
    let missing_file = format!("transcript unavailable: {}: missing-file", file.display());
    assert_unavailable_everywhere(&world, run, &held, &missing_file, false);
}

/// `monitor`, both forms of `watch` and `transcript` each render the held
/// call's reference as `diagnostic` — and `status` too, for a dispatch
/// `in_flight`, the one its readout describes.
fn assert_unavailable_everywhere(
    world: &World,
    run: &str,
    held: &Value,
    diagnostic: &str,
    in_flight: bool,
) {
    if in_flight {
        let status = world.run(&["status", run]);
        status.exited(0);
        assert!(
            status.stdout.contains(&format!("now {diagnostic} (")),
            "`status` does not say the call's transcript is unavailable:\n{}",
            status.stdout
        );
    }
    let monitor = world.run(&["monitor", run, "--all"]);
    monitor.exited(0);
    assert!(
        monitor
            .stdout
            .lines()
            .any(|line| line.ends_with(&format!("turn-activity {diagnostic}"))),
        "`monitor` does not say the call's transcript is unavailable:\n{}",
        monitor.stdout
    );
    let watched = world.run(&["watch", run, "--all", "--timeout", "0"]);
    assert!(
        watched.stderr.contains(diagnostic),
        "`watch` does not say the call's transcript is unavailable:\n{}",
        watched.stderr
    );
    let records = machine(&watched.stdout);
    let record = watched_record(&records, held);
    assert_eq!(
        record["transcript"]["unavailable"],
        json!(diagnostic),
        "{record}"
    );
    let (path, why) = diagnostic
        .trim_start_matches("transcript unavailable: ")
        .rsplit_once(": ")
        .expect("the diagnostic names a path and why");
    assert_eq!(record["transcript"]["path"], json!(path), "{record}");
    assert_eq!(record["transcript"]["why"], json!(why), "{record}");

    let transcript = world.run(&["transcript", run]);
    transcript.exited(0);
    assert!(
        transcript
            .stdout
            .contains(&format!("tool_call bash  {diagnostic}")),
        "`transcript` does not say the call's transcript is unavailable:\n{}",
        transcript.stdout
    );
    // The journal's records — everything above a retained report's own
    // section, which is the settled report's copy and not the history file.
    assert!(
        transcript
            .stdout
            .lines()
            .take_while(|line| !line.starts_with("  report "))
            .filter(|line| line.trim_start().starts_with("tool_call")
                || line.trim_start().starts_with("tool_result"))
            .all(|line| line.contains("transcript unavailable: ")),
        "a call whose transcript is gone rendered as something else:\n{}",
        transcript.stdout
    );
}
