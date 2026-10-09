//! Where a manager's note lands: in the live conversation, in both parties' hands,
//! and in the bar its judge decides against — or nowhere, said out loud.
//!
//! **Its own test binary and its own Nx project**, `onepipeline-note-journeys`,
//! because each journey starts a real two-party conversation and holds one side's
//! turn open — the most expensive shape this repository runs.
//!
//! Two things about that split are not recoverable from the files that make it.
//! The 95% floor is still measured over the *whole* offline tier: the two
//! instrumented runs report nothing and one merge reports both, so splitting the
//! run does not split the floor. And `src/**` and `crates/**` cannot come out of
//! this project's inputs, however narrow the rest of them are — every journey
//! here drives the compiled binary and the doubles that crate builds, so dropping
//! either would let Nx report a cached pass over a binary that no longer exists,
//! and a hand-listed subset of `src` is the same hole with a delay on it.
//!
//! These journeys drive the **real** `oneagentgraph` and a real two-party
//! conversation, because that is the only place the claim can be made: which side
//! of a member is live, what a live turn does with a note, and what the judge is
//! shown beside the transcript are all decided there, and a double standing in for
//! the sibling would be this suite asserting its own fixture. What each journey
//! reads is what the two sides were really given — the prompts the harness under
//! them recorded — and what the run wrote down about it.
//!
//! The one thing standing in is the paid model turn, at `oneharness`'s own seam.

// llmlint: ignore-file[e2e_not_mocked] nothing between the note and the assertion is
// substituted: `oneagentgraph` is the linked library, the conversation is onejudge's own
// engine, and what is read back is the prompt each side was handed. The stand-in is the
// paid turn, one layer below both parties, exactly as `turns.rs` runs it — and it is what
// makes the conversation's shape scriptable rather than billed. `harness.rs` carries the
// same suppression and the full rationale.

#[path = "../e2e/harness.rs"]
mod harness;

use std::path::Path;
use std::time::{Duration, Instant};

use oneagentgraph::event::{Origin, TurnMessage, TurnStarted};
use onemessagebus::{Config, Layouts, TransportKinds};
use onepipeline::channel::layout::{PlannerChannel, PLANNER_CHANNEL, REPLIES};
use onepipeline::channel::Command;
use onepipeline::channel::Deliver;
use onepipeline::note::{deliver, deliver_with, Addressee, Delivered, Note, Reached};
use onepipeline::views::RunPaths;
use serde_json::{json, Value};

use harness::{agent, lifecycle, plan_of, World, CANCEL_GRACE_ENV, REFUSED};

/// The correction a manager sends at the moment it matters: while the worker is
/// still working, and before its judge has ruled on anything.
const NOTE: &str = "the reviewer asked for a smaller diff; stop editing src/old.rs";

/// A note that changes what the finished tree must contain, rather than only how
/// the worker should go about it.
const CRITERION: &str = "`version.txt` holds `v: 2`";

/// The planner's own note a node can be launched with, rendered as observed state.
const PLANNER_CONTEXT: &str = "the fixture moved to fixtures/v2 before this node was launched";

/// The instruction the shipped judge side opens with, which is how a recorded
/// prompt says which party it was for.
///
/// The supervisor's own prompt is not relayed as any turn's instruction — a
/// supervisor turn opens on the *worker's* reply — so the only place the judge's
/// whole brief exists is the process that answered it, and this is how that
/// process's record is told from the worker's.
const SUPERVISOR_OPENING: &str = "You are the simulated USER and completion supervisor";

fn envelope(command: Value) -> String {
    json!({"version": 2, "commands": [command]}).to_string()
}

fn note_op(node: &str, addressee: &str, text: &str, criterion: Option<&str>) -> Value {
    let mut op = json!({"op": "note", "id": node, "addressee": addressee, "text": text});
    if let Some(criterion) = criterion {
        op["criterion"] = json!(criterion);
    }
    op
}

/// The same, naming both axes rather than taking their defaults.
fn note_op_with(node: &str, text: &str, deliver: &str, persist: bool) -> Value {
    let mut op = note_op(node, "worker", text, None);
    op["deliver"] = json!(deliver);
    op["persist"] = json!(persist);
    op
}

/// The instruction each turn of one node opened on, grouped by **dispatch**.
///
/// Read out of the run's own merged store rather than out of the doubles: a
/// `node-dispatched` opens a group and every `turn-started` under that node joins
/// the one it is in, so `[0]` is what the node's first dispatch was given and
/// `[1]` what the dispatch after it was. That grouping is what a claim about "the
/// node's *next* dispatch" needs and a flat list of prompts cannot give, and it
/// races nothing: the journal is ordered.
fn dispatches_of(world: &World, run: &str, node: &str) -> Vec<Vec<String>> {
    let mut dispatched: Vec<Vec<String>> = Vec::new();
    for event in world.journal(run) {
        if event["labels"]["node"] != node {
            continue;
        }
        match event["kind"].as_str() {
            Some("node-dispatched") => dispatched.push(Vec::new()),
            Some("turn-started") => {
                if let (Some(turns), Some(instruction)) = (
                    dispatched.last_mut(),
                    event["payload"]["instruction"].as_str(),
                ) {
                    turns.push(instruction.to_string());
                }
            }
            _ => {}
        }
    }
    dispatched
}

/// Every turn one node opened, read back through the producer's own payload type.
///
/// Through that type rather than by field name, because `deny_unknown_fields`
/// on it is what makes this an assertion about the *whole* payload this engine
/// relayed: a relay that stamped something of its own onto the sibling's payload
/// fails on the unknown field, and one that dropped a field fails on the missing
/// one.
fn openings_of(world: &World, run: &str, node: &str) -> Vec<TurnStarted> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["labels"]["node"] == node && event["kind"] == "turn-started")
        .map(|event| {
            serde_json::from_value(event["payload"].clone()).unwrap_or_else(|error| {
                panic!("a relayed turn-started is not the payload the linked oneagentgraph declares: {error}: {event}")
            })
        })
        .collect()
}

/// Every word a party of one node said, read back the same way.
fn words_of(world: &World, run: &str, node: &str) -> Vec<TurnMessage> {
    world
        .journal(run)
        .iter()
        .filter(|event| event["labels"]["node"] == node && event["kind"] == "turn-message")
        .map(|event| {
            serde_json::from_value(event["payload"].clone()).unwrap_or_else(|error| {
                panic!("a relayed turn-message is not the payload the linked oneagentgraph declares: {error}: {event}")
            })
        })
        .collect()
}

/// The `note-shown` records of one node, in order: one per presentation the
/// run saw happen.
fn presentations_of(world: &World, run: &str, node: &str) -> Vec<Value> {
    world
        .events_of(run, "note-shown")
        .into_iter()
        .filter(|event| event["labels"]["node"] == node)
        .collect()
}

/// The `node-dispatched` records of one node, in order: one per dispatch.
fn dispatch_records_of(world: &World, run: &str, node: &str) -> Vec<Value> {
    world
        .events_of(run, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == node)
        .collect()
}

/// Start a run whose nodes are two-party members, against the real sibling.
///
/// Whatever a journey scripted before calling this is what the conversation then
/// does: the scripts are read by the doubles under the members, so they are
/// written before the run starts rather than passed in here.
fn supervised_run(world: &World, run: &str, nodes: Vec<Value>) {
    world.write_graphs();
    world.write_supervised_node_graph();
    let path = world.plan(run, &plan_of(run, nodes));
    world
        .run_on_agentgraph(&["start", &path, "--detach"])
        .exited(0);
}

/// Start a supervised run whose worker turn is held open, and wait until it is.
///
/// The judge asks once and then completes, which is the shortest conversation with
/// two decisions in it — so a note delivered into the held worker turn is in the
/// judge's hands for the *first* of them, and "before the verdict" is a claim about
/// a verdict that really came later.
fn held_conversation(world: &World, run: &str, nodes: Vec<Value>) {
    world.script(
        "judge.asks-again",
        "Run the check again and report what it said.",
    );
    world.script("turn.hold", "hold");
    supervised_run(world, run, nodes);
    world.until("the worker's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
    });
}

/// Start a supervised run whose **judge** turn is held open, and wait until it is.
///
/// The other half of [`held_conversation`], and the only way to offer a note while
/// the supervisor is the party taking a turn: holding the worker holds the wrong
/// party, and every turn either party takes is otherwise over in milliseconds.
fn held_judge(world: &World, run: &str, nodes: Vec<Value>) {
    world.script("judge.hold", "hold");
    supervised_run(world, run, nodes);
    world.until("the judge's turn to open", |world| {
        world.fakes.join("judge.holding").exists()
    });
}

/// Release the held turn once the note is really on its way to it.
///
/// The wait is on the run's own durable command queue rather than on a clock: the
/// note is in it before the reconciler can offer it, and the held turn cannot end
/// until this releases it — so the note reaches a turn that is still live rather
/// than whichever party happened to be speaking when a timer went off. The short
/// pause after it is the reconciler's own pass, which is the one step with nothing
/// durable to watch for.
fn release_when_the_note_is_queued(
    world: &World,
    run: &str,
    gates: &[&str],
) -> std::thread::JoinHandle<()> {
    let queue = world.run_file(run, "channel/commands.jsonl");
    let fakes = world.fakes.clone();
    let gates: Vec<String> = gates.iter().map(|gate| (*gate).to_string()).collect();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline && !a_note_is_queued(&queue) {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_secs(2));
        for gate in &gates {
            release(&fakes, gate);
        }
    })
}

/// Whether the run's durable command queue already carries a `note` op.
///
/// Read as the records it holds rather than as text: the queue is a ledger of
/// submitted envelopes, and asking a substring whether one has arrived would
/// answer yes to a note *named* inside some other op's prose. A queue file that
/// does not exist yet is the state this waits out, and is the only read failure
/// treated as one — anything else, and any record this build cannot parse as the
/// commands it is, ends the journey rather than reading as "not yet".
fn a_note_is_queued(queue: &Path) -> bool {
    let text = match std::fs::read_to_string(queue) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(error) => panic!(
            "the run's command queue at {} could not be read: {error}",
            queue.display()
        ),
    };
    let mut lines = text.lines().peekable();
    let mut queued = false;
    while let Some(line) = lines.next() {
        let envelope: Value = match serde_json::from_str(line) {
            Ok(envelope) => envelope,
            // The last line, and only the last, may be an append still in
            // flight; an unreadable record before it is a queue this journey is
            // wrong about rather than one it should wait longer on.
            Err(_) if lines.peek().is_none() => break,
            Err(error) => panic!("the command queue holds an unreadable record: {error}: {line}"),
        };
        let commands: Vec<Command> = serde_json::from_value(envelope["commands"].clone())
            .unwrap_or_else(|error| {
                panic!("the command queue holds commands this build cannot read: {error}: {line}")
            });
        queued |= commands
            .iter()
            .any(|command| matches!(command, Command::Note { .. }));
    }
    queued
}

fn release(fakes: &Path, name: &str) {
    std::fs::write(fakes.join(name), "go").expect("the rendezvous is released");
}

/// Every prompt either side of the conversation was really handed, in order.
fn prompts(world: &World) -> Vec<String> {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneharness-config")
        .filter_map(|call| call["args"][0].as_str().map(str::to_string))
        .collect()
}

/// The judge's, which is every prompt opening on its own brief.
fn judged(world: &World) -> Vec<String> {
    prompts(world)
        .into_iter()
        .filter(|prompt| prompt.contains(SUPERVISOR_OPENING))
        .collect()
}

/// The worker's, which is every other one.
fn worked(world: &World) -> Vec<String> {
    prompts(world)
        .into_iter()
        .filter(|prompt| !prompt.contains(SUPERVISOR_OPENING))
        .collect()
}

/// What the run recorded about the one note it committed.
fn recorded(world: &World, run: &str) -> Value {
    let committed: Vec<Value> = world
        .events_of(run, "edit-committed")
        .into_iter()
        .filter(|event| event["payload"]["command"]["op"] == "note")
        .collect();
    let [one] = &committed[..] else {
        panic!(
            "the run recorded {} committed notes, not one",
            committed.len()
        );
    };
    one["payload"]["operations"][0].clone()
}

/// A note driven into a live dispatch reaches whoever is speaking, and the other
/// party has it before the judge rules on anything.
///
/// The defect this stands against is the seam that cost whole dispatches: a
/// correction delivered by interrupting the worker's turn reached the worker and
/// nobody else, and the node's own judge then reviewed against a task that never
/// mentioned it — so the worker held two instructions of equal authority and
/// resolving it took a retry that killed a live, gate-green dispatch.
///
/// So what is asserted is the pair, and its order: **both** parties were handed the
/// note, and the judge had it before the first decision it took.
#[test]
fn a_note_into_a_live_dispatch_reaches_both_parties_before_the_judges_verdict() {
    let world = World::new("note-live");
    let run = "live";
    held_conversation(&world, run, vec![agent("build", &[])]);

    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    let replied = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &envelope(note_op("build", "worker", NOTE, None)),
    );
    releasing.join().expect("the releasing thread finishes");
    replied.exited(0).out_has("\"state\":\"applied\"");

    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });

    // The worker had it: one of its turns opened on the note, framed as an update
    // to its own task rather than as narration beside one.
    let worker = worked(&world);
    assert!(
        worker.iter().any(|prompt| prompt.contains(NOTE)),
        "no worker turn was handed the note:\n{worker:#?}"
    );

    // And the judge had it — in the **first** decision it took, which is what
    // makes this a note that reached it before a verdict rather than after one.
    // The judge asks again before completing, so there really was a later verdict
    // for this one to be before.
    let judge = judged(&world);
    assert!(
        judge.len() >= 2,
        "the judge took {} decisions, so nothing here is 'before the verdict':\n{judge:#?}",
        judge.len()
    );
    assert!(
        judge[0].contains(NOTE),
        "the judge's first decision was taken without the note:\n{}",
        judge[0]
    );

    // And the run says which party actually took it, which is the one thing no
    // reader of the transcript can work out for itself — and which parties it
    // was put in front of, so a note for both is verifiable from this record
    // alone rather than from what the disposition is documented to imply.
    let operation = recorded(&world, run);
    assert_eq!(operation["node"], json!("build"), "{operation}");
    assert_eq!(operation["addressee"], json!("worker"), "{operation}");
    assert_eq!(operation["text"], json!(NOTE), "{operation}");
    assert_eq!(
        operation["reached"],
        json!("worker"),
        "the note reached a party the note was not delivered to first: {operation}"
    );
    // The acknowledgement confirms nothing: the conversation acknowledges a
    // reopened worker turn before that turn opens, so the record says where it
    // is routed and claims no presentation yet.
    assert!(
        operation.get("shown_to").is_none(),
        "the delivery record claims a presentation the conversation had not made: {operation}"
    );
    assert_eq!(
        operation["routed_to"],
        json!(["worker", "supervisor"]),
        "{operation}"
    );
    // Each presentation is then recorded as the stream showed it happening:
    // the worker's turn that opened on the note, and then the judge's turn
    // that answered it — in that order, and each once.
    let shown = presentations_of(&world, run, "build");
    assert_eq!(
        shown
            .iter()
            .map(|event| event["payload"]["party"].clone())
            .collect::<Vec<_>>(),
        vec![json!("worker"), json!("supervisor")],
        "the presentations the run recorded are not the worker's and then the judge's:\n{shown:#?}"
    );
    assert!(
        shown
            .iter()
            .all(|event| event["payload"]["text"] == json!(NOTE)
                && event["payload"]["reached"] == json!("worker")),
        "{shown:#?}"
    );
    // And each says what it was decided from: the producer's own stamp on the
    // worker's turn, and the turn that answered it for the judge.
    assert_eq!(
        shown[0]["payload"]["evidence"],
        json!("delivered-origin"),
        "{shown:#?}"
    );
    assert_eq!(
        shown[1]["payload"]["evidence"],
        json!("answering-turn"),
        "{shown:#?}"
    );
    let worker_turn = shown[0]["payload"]["turn"].as_u64().expect("a turn");
    let judge_turn = shown[1]["payload"]["turn"].as_u64().expect("a turn");
    assert!(judge_turn >= worker_turn, "{shown:#?}");

    // And a reader of the run's own stream can tell the turn that carried the
    // manager's note from the one the simulated supervisor improvised, without
    // consulting anything outside it. This engine relays the sibling's payload
    // untouched, so the field reaching the store is the producer's own — and
    // the delivery went through the path that stamps it, which is what this
    // assertion is really about: a note handed to the conversation by any other
    // lever would arrive as the supervisor's own words.
    let openings = openings_of(&world, run, "build");
    let by_origin = |origin: Origin| -> Vec<&TurnStarted> {
        openings
            .iter()
            .filter(|opening| opening.origin == Some(origin))
            .collect()
    };
    let delivered = by_origin(Origin::Delivered);
    assert!(
        delivered.len() == 1 && delivered[0].instruction.contains(NOTE),
        "the turn that carried the manager's note is not stamped as a delivery:\n{openings:#?}"
    );
    assert_eq!(
        delivered[0].turn, worker_turn,
        "the worker's recorded presentation is not the turn the producer stamped as the \
         delivery"
    );
    let task = by_origin(Origin::Task);
    assert!(
        task.len() == 1 && task[0].turn == 1,
        "the opening turn is not stamped as the composed task:\n{openings:#?}"
    );
    let supervised = by_origin(Origin::Supervisor);
    assert!(
        !supervised.is_empty()
            && supervised
                .iter()
                .all(|opening| opening.instruction.contains("Run the check again")),
        "the turn the supervisor sent the worker back on is not stamped as the \
         supervisor's own:\n{openings:#?}"
    );
    // The supervisor's words themselves, as they were said, carry the same
    // attribution; the worker's carry none of the three, because none names
    // them, and absent reads as unknown rather than as any of them.
    let words = words_of(&world, run, "build");
    assert!(
        words
            .iter()
            .filter(|word| word.role == "user")
            .all(|word| word.origin == Some(Origin::Supervisor))
            && words.iter().any(|word| word.role == "user"),
        "the supervisor's own words are not stamped as its own:\n{words:#?}"
    );
    assert!(
        words
            .iter()
            .filter(|word| word.role == "assistant")
            .all(|word| word.origin.is_none()),
        "the worker's words were attributed to a party that did not author them:\n{words:#?}"
    );
}

const FINDING: &str = "the fixture this node reads moved";

/// How long a held conversation's member lets its supervision go unconfirmed, in
/// seconds: short, so it publishes `member-heartbeat` every couple of seconds and
/// a journey can watch the run keep recording one while a note waits.
const HEARTBEAT_BOUND_SECONDS: &str = "8";

/// [`held_conversation`], with the member heartbeating on a clock a journey can
/// watch.
fn held_heartbeating_conversation(world: &World, run: &str, nodes: Vec<Value>) {
    world.script("turn.hold", "hold");
    world.write_graphs();
    world.write_supervised_node_graph();
    let path = world.plan(run, &plan_of(run, nodes));
    let mut start = world.agentgraph_cmd(&["start", &path, "--detach"]);
    start.env("ONEAGENTGRAPH_HEARTBEAT_TIMEOUT", HEARTBEAT_BOUND_SECONDS);
    world.run_on(start, "start --detach").exited(0);
    world.until("the worker's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
    });
}

/// Submit one envelope with `reply` and leave it waiting on its answer.
///
/// `reply` waits for the reconciler to answer the envelope, and the answer to a
/// note is owed only once its conversation has taken it — which these journeys
/// hold back on purpose — so the submitter runs beside the journey rather than in
/// front of it. The wait is long enough to outlast the hold.
fn submitted(world: &World, run: &str, envelope: &str) -> std::process::Child {
    use std::io::Write;
    let mut reply = world.agentgraph_cmd(&["reply", run]);
    reply
        .env("ONEPIPELINE_REPLY_TIMEOUT_SECONDS", "300")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = reply.spawn().expect("the binary starts");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(envelope.as_bytes())
        .expect("the envelope is written");
    child
}

fn answered(child: std::process::Child) -> String {
    let output = child.wait_with_output().expect("the binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "reply exited {:?}:\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

/// The notes a conversation's inbox holds and has not answered yet.
///
/// Read off the member's note spool itself, which is where `oneagentgraph` puts a
/// note it has been handed: `<id>.offer.json` until the conversation's courier
/// takes it, `<id>.taken.json` from then, and `<id>.answer.json` beside it once
/// the conversation has said what became of it. A note counted here is one the
/// run has **offered** and is waiting on — the exact window the writer used to
/// spend doing nothing else.
fn awaiting_an_answer(world: &World) -> usize {
    fn walk(dir: &Path, found: &mut usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(id) = name
                .strip_suffix(".taken.json")
                .or_else(|| name.strip_suffix(".offer.json"))
            else {
                continue;
            };
            if dir.ends_with("notes") && !dir.join(format!("{id}.answer.json")).exists() {
                *found += 1;
            }
        }
    }
    let mut found = 0;
    walk(&world.graph_state(), &mut found);
    found
}

fn position_of(journal: &[Value], kind: &str, node: Option<&str>) -> Option<usize> {
    journal.iter().position(|event| {
        event["kind"] == kind && node.is_none_or(|node| event["labels"]["node"] == node)
    })
}

/// A note waiting on a turn that takes it only as it ends leaves the run
/// recording everything else.
///
/// A run's writer used to wait on that answer itself, so nothing the dispatch did
/// reached the journal until the turn was over. Here the note stays unanswered
/// while the turn reports work and its member heartbeats, and both must be
/// recorded meanwhile; an edit queued behind the note waits for it rather than
/// overtaking it; and once the turn ends the run records the delivery and then
/// its presentation.
#[test]
fn a_note_waiting_on_a_held_turn_leaves_the_run_journalling_its_dispatch() {
    let world = World::new("note-unfrozen");
    let run = "unfrozen";
    held_heartbeating_conversation(&world, run, vec![agent("build", &[])]);

    let reply = submitted(
        &world,
        run,
        &envelope(note_op("build", "worker", NOTE, None)),
    );
    world.until("the note to wait in the conversation's inbox", |world| {
        awaiting_an_answer(world) == 1
    });
    let finding = submitted(
        &world,
        run,
        &envelope(json!({"op": "finding", "id": "build", "message": FINDING})),
    );
    let queue = world.run_file(run, "channel/commands.jsonl");
    world.until("the edit behind the note to be queued", |_| {
        std::fs::read_to_string(&queue).is_ok_and(|text| text.contains(FINDING))
    });
    let activity = world.events_of(run, "turn-activity").len();
    let beats = world.events_of(run, "member-heartbeat").len();

    // The held turn goes on working — it reports its next tool call and stays
    // open — and the member goes on heartbeating, while the note is still
    // waiting on the turn to end.
    release(&world.fakes, "turn.go");
    world.until("the turn's further work to reach the journal", |world| {
        world.events_of(run, "turn-activity").len() > activity
    });
    world.until(
        "the member's heartbeat to keep reaching the journal",
        |world| world.events_of(run, "member-heartbeat").len() >= beats + 2,
    );
    assert_eq!(
        awaiting_an_answer(&world),
        1,
        "the conversation answered the note before its turn ended, so nothing above was \
         recorded while one was waiting"
    );
    assert!(
        world
            .events_of(run, "edit-committed")
            .iter()
            .all(|event| event["payload"]["command"]["op"] != "note"),
        "the note was recorded before its conversation answered it"
    );
    assert!(
        world
            .journal(run)
            .iter()
            .all(|event| event["payload"]["command"]["op"] != "finding"),
        "the edit queued behind the note was applied ahead of it"
    );

    // The turn ends, the conversation takes the note, and the record is the one
    // a delivery has always left: the delivery, and then its presentation.
    release(&world.fakes, "turn.settle");
    assert!(
        answered(reply).contains("\"state\":\"applied\""),
        "the note's envelope was not applied"
    );
    world.until("the note's presentation to be recorded", |world| {
        !presentations_of(world, run, "build").is_empty()
    });
    let operation = recorded(&world, run);
    assert_eq!(operation["kind"], json!("note-delivered"), "{operation}");
    assert_eq!(operation["reached"], json!("worker"), "{operation}");
    assert_eq!(operation["text"], json!(NOTE), "{operation}");
    let journal = world.journal(run);
    let delivered = position_of(&journal, "edit-committed", None).expect("the note was recorded");
    let shown = position_of(&journal, "note-shown", Some("build")).expect("its presentation");
    assert!(
        delivered < shown,
        "the note's presentation was recorded before its delivery"
    );
    assert!(
        answered(finding).contains("\"state\":\"applied\""),
        "the edit queued behind the note was not applied"
    );
    let journal = world.journal(run);
    let behind = journal
        .iter()
        .position(|event| event["payload"]["command"]["op"] == "finding")
        .expect("the edit behind the note was recorded");
    assert!(
        delivered < behind,
        "the edit queued behind the note was recorded ahead of it"
    );
    assert_eq!(
        journal[shown]["payload"]["party"],
        json!("worker"),
        "{}",
        journal[shown]
    );

    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });
}

/// A later note to one conversation never overtakes an earlier one: each reaches
/// it, and the run's record, in the order the channel claimed them, and none is
/// offered while another is still waiting.
///
/// The writer no longer waits on a conversation, so envelopes arrive while a note
/// is still being delivered. Offering one then would put two notes in one inbox
/// in whatever order the conversation's courier happened to take them; judging it
/// then would judge it against a record the first had not committed to yet. Both
/// places the loop keeps them are driven: the queue it leaves unclaimed while a
/// note is outstanding, and the envelopes it claims together once that note is
/// answered and holds behind the first of them it hands on.
#[test]
fn notes_to_one_conversation_are_delivered_and_recorded_in_the_order_they_were_claimed() {
    let world = World::new("note-ordered");
    let run = "ordered";
    // Every worker turn holds, and consumes its gates as it ends, so each turn a
    // note opens is held open for the next note exactly as the first turn was.
    world.script("turn.hold-each", "");
    held_heartbeating_conversation(&world, run, vec![agent("build", &[])]);

    let first = submitted(
        &world,
        run,
        &envelope(note_op("build", "worker", NOTE, None)),
    );
    world.until(
        "the first note to wait in the conversation's inbox",
        |world| awaiting_an_answer(world) == 1,
    );
    // Two more, both queued while the first is outstanding, so the pass after its
    // answer claims them together.
    let second = submitted(
        &world,
        run,
        &envelope(note_op("build", "worker", PLANNER_CONTEXT, None)),
    );
    let queue = world.run_file(run, "channel/commands.jsonl");
    world.until("the second note to be queued", |_| {
        std::fs::read_to_string(&queue).is_ok_and(|text| text.contains(PLANNER_CONTEXT))
    });
    let third = submitted(
        &world,
        run,
        &envelope(note_op("build", "worker", FINDING, None)),
    );
    world.until("the third note to be queued", |_| {
        std::fs::read_to_string(&queue).is_ok_and(|text| text.contains(FINDING))
    });
    // A pass is the one step with nothing durable to watch for, so the journey
    // watches the run keep recording its dispatch across several of them.
    let beats = world.events_of(run, "member-heartbeat").len();
    world.until("the run to go on recording its dispatch", |world| {
        world.events_of(run, "member-heartbeat").len() >= beats + 2
    });
    assert_eq!(
        awaiting_an_answer(&world),
        1,
        "a later note was offered while the first was still waiting on its conversation"
    );

    // The first turn ends and the conversation takes the first note; only then is
    // the second offered, into the turn that opened on the first — held too, so it
    // is still there to take it — and the third, claimed with it, is held behind.
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    answered(first);
    world.until(
        "the second note to wait in the conversation's inbox",
        |world| awaiting_an_answer(world) == 1,
    );
    let beats = world.events_of(run, "member-heartbeat").len();
    world.until("the run to go on recording its dispatch", |world| {
        world.events_of(run, "member-heartbeat").len() >= beats + 2
    });
    assert_eq!(
        awaiting_an_answer(&world),
        1,
        "the note claimed behind the second was offered while the second was still waiting"
    );

    // The second note's turn ends; the third is offered into the turn it opened.
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    answered(second);
    world.until(
        "the third note to wait in the conversation's inbox",
        |world| awaiting_an_answer(world) == 1,
    );
    // No turn after this one is held, so the conversation runs to its end.
    std::fs::remove_file(world.fakes.join("turn.hold")).expect("the hold is lifted");
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    answered(third);
    let texts: Vec<Value> = world
        .events_of(run, "edit-committed")
        .into_iter()
        .filter(|event| event["payload"]["command"]["op"] == "note")
        .map(|event| event["payload"]["operations"][0]["text"].clone())
        .collect();
    assert_eq!(
        texts,
        vec![json!(NOTE), json!(PLANNER_CONTEXT), json!(FINDING)],
        "the notes were not recorded in the order the channel claimed them"
    );

    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });
}

/// What the manager rules on a node the outstanding note's own envelope also
/// amends, and what it rules on the same node afterwards.
const FIRST_RULING: &str = "docs: describe only the flags that shipped";
const SECOND_RULING: &str = "docs: and link the changelog entry";

/// A ruling on a node nothing outstanding names.
const OTHER_RULING: &str = "other: keep the fixture where it is";

/// A node added behind the note, naming the note's node only among its `deps`.
const ADDED: &str = "after-build";

/// A ruling on that added node alone, claimed while its `add` is still held.
const ADDED_RULING: &str = "after-build: only once build's note is read";

/// A pending node reparented onto the note's node behind the note.
const REPARENTED: &str = "late";

/// How the reconciler opens its refusal of a record it cannot decode.
const MALFORMED: &str = "refused: the envelope is malformed: ";

/// Send one envelope through the planner channel's own bus, which reads only each
/// command's `op` — so a record `reply` would refuse reaches the queue this way.
fn sent_through_the_bus(world: &World, run: &str, envelope: &Value) {
    let bus = Config::local(world.run_file(run, "channel"), Some(PLANNER_CHANNEL))
        .resolve(
            &Layouts::new().with(std::sync::Arc::new(PlannerChannel)),
            &TransportKinds::builtin(),
        )
        .expect("the planner channel's bus resolves over the run's channel");
    bus.send(&REPLIES.parse().expect("a queue name"), envelope.clone())
        .unwrap_or_else(|error| panic!("the bus refused {envelope}: {error}"));
}

/// The queue id of the one queued envelope whose record mentions `text`.
fn queued_id_of(world: &World, run: &str, text: &str) -> u64 {
    std::fs::read_to_string(world.run_file(run, "channel/commands.jsonl"))
        .expect("the command queue is there")
        .lines()
        .filter(|line| line.contains(text))
        .map(|line| {
            serde_json::from_str::<Value>(line).expect("a queued record is JSON")["id"]
                .as_u64()
                .expect("a queued record has an id")
        })
        .next()
        .unwrap_or_else(|| panic!("nothing queued mentions {text}"))
}

/// The answer the reconciler gave envelope `id`, if it has given one.
fn outcome_of(world: &World, run: &str, id: u64) -> Option<Value> {
    world
        .command_outcomes(run)
        .into_iter()
        .find(|outcome| outcome["id"] == id)
}

/// A monitor's finding about the run rather than any one node.
const UNADDRESSED: &str = "the run's queue depth keeps growing";

/// Where the journal records the command `op` whose `field` reads `value` as
/// applied: an edit as `edit-committed`, and a finding, which edits nothing, as
/// `command-accepted`.
fn committed_at(journal: &[Value], op: &str, field: &str, value: &str) -> Option<usize> {
    journal.iter().position(|event| {
        (event["kind"] == "edit-committed" || event["kind"] == "command-accepted")
            && event["payload"]["command"]["op"] == op
            && event["payload"]["command"][field] == value
    })
}

/// While a note waits on a turn, only an envelope naming a node the waiting
/// envelope names waits with it; every other one is judged and answered on the
/// pass that claims it.
///
/// A worker turn can run for many minutes, and a writer that held every command
/// behind a note to it held edits and findings about the rest of the run for all
/// of that turn, unanswered and with nothing saying why. Here the note's envelope
/// also amends `docs`. An amendment to `other` and a finding about no node are
/// claimed behind it and are both committed and answered while the note is still
/// waiting, and a record naming `build` that does not decode is refused as it is
/// claimed; a finding about `build`, a second amendment to `docs`, and an `add` and
/// a `reparent` naming `build` only among their `deps` are held, as is an `amend`
/// naming only the node that held `add` creates, unanswered, and applied once the
/// note is — after it, and in the order they were claimed, so no edit to `docs`
/// overtakes the one the note's envelope carried.
#[test]
fn only_an_envelope_naming_what_a_waiting_note_names_is_held_behind_it() {
    let world = World::new("note-unrelated");
    let run = "unrelated";
    held_heartbeating_conversation(
        &world,
        run,
        vec![
            agent("build", &[]),
            agent("docs", &["build"]),
            agent("other", &["build"]),
            agent(REPARENTED, &["other"]),
        ],
    );
    let queue = world.run_file(run, "channel/commands.jsonl");
    let queued = |text: &str| {
        let text = text.to_string();
        let queue = queue.clone();
        move |_: &World| std::fs::read_to_string(&queue).is_ok_and(|held| held.contains(&text))
    };

    let note = submitted(
        &world,
        run,
        &json!({"version": 2, "commands": [
            note_op("build", "worker", NOTE, None),
            {"op": "amend", "id": "docs", "text": FIRST_RULING},
        ]})
        .to_string(),
    );
    world.until("the note to wait in the conversation's inbox", |world| {
        awaiting_an_answer(world) == 1
    });
    // Claimed behind the note, and each naming a node its envelope names.
    let mut about_build = submitted(
        &world,
        run,
        &envelope(json!({"op": "finding", "id": "build", "message": FINDING})),
    );
    world.until("the finding about build to be queued", queued(FINDING));
    let mut about_docs = submitted(
        &world,
        run,
        &envelope(json!({"op": "amend", "id": "docs", "text": SECOND_RULING})),
    );
    world.until(
        "the second ruling on docs to be queued",
        queued(SECOND_RULING),
    );
    let mut after_build = submitted(
        &world,
        run,
        &envelope(json!({"op": "add", "node": agent(ADDED, &["build"])})),
    );
    world.until("the node added after build to be queued", queued(ADDED));
    // Naming only the node that held `add` creates, which no outstanding envelope
    // names: held behind the `add`, so it is judged against a record that has
    // the node rather than refused for one that does not yet. Sent through the
    // bus, as a monitor sends, because `reply` checks it against the record
    // before queueing it and the node is not in the record yet.
    sent_through_the_bus(
        &world,
        run,
        &json!({"version": 2, "commands": [
            {"op": "amend", "id": ADDED, "text": ADDED_RULING},
        ]}),
    );
    world.until(
        "the ruling on the added node to be queued",
        queued(ADDED_RULING),
    );
    let on_added = queued_id_of(&world, run, ADDED_RULING);
    let mut reparented = submitted(
        &world,
        run,
        &envelope(json!({"op": "reparent", "id": REPARENTED, "deps": ["build"]})),
    );
    world.until(
        "the node reparented onto build to be queued",
        queued("reparent"),
    );
    // Claimed after both, and naming nothing the note's envelope names.
    let about_other = submitted(
        &world,
        run,
        &envelope(json!({"op": "amend", "id": "other", "text": OTHER_RULING})),
    );
    let unaddressed = submitted(
        &world,
        run,
        &envelope(json!({"op": "finding", "message": UNADDRESSED})),
    );

    // Naming `build`, and refused as it is claimed rather than held: nothing of a
    // record that does not decode can be applied, so nothing waits on its turn.
    sent_through_the_bus(
        &world,
        run,
        &json!({"version": 2, "commands": [{"op": "drop", "id": "build"}]}),
    );
    world.until("the record that does not decode to be refused", |world| {
        world.command_outcomes(run).iter().any(|outcome| {
            outcome["applied"] == json!(false)
                && outcome["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.starts_with(MALFORMED))
        })
    });
    world.until(
        "the ruling on other and the finding about no node to be committed",
        |world| {
            let journal = world.journal(run);
            committed_at(&journal, "amend", "text", OTHER_RULING).is_some()
                && committed_at(&journal, "finding", "message", UNADDRESSED).is_some()
        },
    );
    assert!(
        answered(about_other).contains("\"state\":\"applied\""),
        "the ruling on a node nothing outstanding names was not applied"
    );
    assert!(
        answered(unaddressed).contains("\"state\":\"applied\""),
        "the finding about no node was not applied"
    );
    assert_eq!(
        awaiting_an_answer(&world),
        1,
        "the conversation answered the note before its turn ended, so nothing above was \
         applied while one was waiting"
    );
    let journal = world.journal(run);
    assert!(
        committed_at(&journal, "note", "id", "build").is_none(),
        "the note was recorded before its conversation answered it"
    );
    assert!(
        committed_at(&journal, "finding", "message", FINDING).is_none()
            && committed_at(&journal, "amend", "text", SECOND_RULING).is_none(),
        "an envelope naming a node the waiting envelope names was applied ahead of it"
    );
    assert!(
        journal.iter().all(|event| {
            event["payload"]["command"]["op"] != "add"
                && event["payload"]["command"]["op"] != "reparent"
        }),
        "an envelope naming the note's node among its deps was applied ahead of it"
    );
    assert!(
        committed_at(&journal, "amend", "text", ADDED_RULING).is_none()
            && journal
                .iter()
                .all(|event| event["payload"]["command"]["text"] != ADDED_RULING),
        "the ruling on the added node was judged ahead of the held add that creates it"
    );
    assert!(
        outcome_of(&world, run, on_added).is_none(),
        "the ruling on the added node was answered while the add it waits behind was not"
    );
    for (what, reply) in [
        ("build", &mut about_build),
        ("docs", &mut about_docs),
        ("the node depending on build", &mut after_build),
        ("the node reparented onto build", &mut reparented),
    ] {
        assert!(
            reply.try_wait().expect("the reply is readable").is_none(),
            "the envelope about {what} was answered while the note it waits behind was not"
        );
    }

    // The turn ends and the conversation takes the note; what waited behind it is
    // applied after it, in the order it was claimed.
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    for (what, reply) in [
        ("the note's", note),
        ("the finding about build's", about_build),
        ("the second ruling on docs'", about_docs),
        ("the added node's", after_build),
        ("the reparented node's", reparented),
    ] {
        assert!(
            answered(reply).contains("\"state\":\"applied\""),
            "{what} envelope was not applied"
        );
    }
    world.until("the ruling on the added node to be answered", |world| {
        outcome_of(world, run, on_added).is_some()
    });
    let outcome = outcome_of(&world, run, on_added).expect("answered");
    assert_eq!(outcome["applied"], json!(true), "{outcome}");
    let journal = world.journal(run);
    let at = |op, field, value| {
        committed_at(&journal, op, field, value)
            .unwrap_or_else(|| panic!("no {op} reading {value} was committed"))
    };
    // The two unheld envelopes were submitted together, so only their place
    // before the note is theirs to keep, not an order between them.
    let unheld = at("amend", "text", OTHER_RULING).max(at("finding", "message", UNADDRESSED));
    let order = [
        unheld,
        at("note", "id", "build"),
        at("amend", "text", FIRST_RULING),
        at("finding", "message", FINDING),
        at("amend", "text", SECOND_RULING),
        journal
            .iter()
            .position(|event| {
                event["kind"] == "edit-committed"
                    && event["payload"]["command"]["node"]["id"] == ADDED
            })
            .expect("the added node was committed"),
        at("amend", "text", ADDED_RULING),
        at("reparent", "id", REPARENTED),
    ];
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "the envelopes were not committed unheld first, then the note's, then what it \
         held in the order it was claimed: {order:?}"
    );

    world.until("the run to settle", |world| {
        world.events_of(run, "node-settled").len() == 5
    });
}

/// What a manager tells a worker it is about to cancel, and which never reaches it.
const PREEMPTED_NOTE: &str = "and drop the second fixture too";

/// A ruling on `build` queued behind its unanswered note and ahead of its cancel.
const BUILD_RULING: &str = "build: keep the public API unchanged";

/// A ruling on `lint` carried in the envelope a cancel of `build` overtakes.
const LINT_RULING: &str = "lint: check only the files this change touched";

/// How long a cancel may take to be applied once it is queued.
///
/// The bound the ticket set: a cancel is the lever for stopping a worker that
/// will not answer, so it is applied on the pass that claims it rather than
/// after whatever that worker is waiting on.
const CANCEL_PATIENCE: Duration = Duration::from_secs(30);

/// [`held_conversation`] over every node given, with each node's dispatch
/// reaped a second after it is cancelled — so a cancel ends a held turn without
/// the journey releasing it.
fn held_cancellable_conversation(world: &World, run: &str, nodes: Vec<Value>) {
    let ids: Vec<String> = nodes
        .iter()
        .map(|node| node["id"].as_str().expect("a node id").to_string())
        .collect();
    world.script("turn.hold", "hold");
    world.write_graphs();
    world.write_supervised_node_graph();
    let path = world.plan(run, &plan_of(run, nodes));
    let mut launch = world.agentgraph_cmd(&["start", &path, "--detach"]);
    launch.env(CANCEL_GRACE_ENV, "1");
    world.run_on(launch, "start --detach").exited(0);
    world.until("every worker's turn to open", |world| {
        let opened = world.events_of(run, "turn-started");
        ids.iter()
            .all(|node| opened.iter().any(|event| event["labels"]["node"] == *node))
    });
}

/// The run's `status` line naming envelopes claimed and never answered, if any.
fn claimed_with_no_outcome(world: &World, run: &str) -> Option<String> {
    let status = world.run(&["status", run]);
    status.exited(0);
    status
        .stdout
        .lines()
        .find(|line| line.trim_start().starts_with("claimed with no outcome:"))
        .map(str::to_string)
}

/// Assert envelope `id` was answered, once, `refused` as preempted by the
/// cancel queued as `cancel`, and that its refusal is journalled once.
fn assert_preempted(world: &World, run: &str, id: u64, cancel: u64, text: &str) {
    let outcomes: Vec<Value> = world
        .command_outcomes(run)
        .into_iter()
        .filter(|outcome| outcome["id"] == id)
        .collect();
    let [outcome] = &outcomes[..] else {
        panic!(
            "the note {text:?} was answered {} times, not once: {outcomes:#?}",
            outcomes.len()
        );
    };
    assert_eq!(outcome["applied"], json!(false), "{outcome}");
    let result = &outcome["results"][0];
    assert_eq!(result["outcome"], json!("refused"), "{outcome}");
    assert!(
        result["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains(&format!("envelope {cancel}"))),
        "the refusal does not name the cancelling envelope {cancel}: {outcome}"
    );
    let rejected: Vec<Value> = world
        .events_of(run, "edit-rejected")
        .into_iter()
        .filter(|event| event["payload"]["command"]["text"] == text)
        .collect();
    assert_eq!(
        rejected.len(),
        1,
        "the note {text:?} was not journalled refused once: {rejected:#?}"
    );
    assert!(
        world
            .journal(run)
            .iter()
            .all(|event| event["kind"] != "edit-committed"
                || event["payload"]["command"]["text"] != text),
        "the preempted note {text:?} was also journalled as committed"
    );
}

/// A cancel of a node whose worker never answers its live note is applied at
/// once, rather than waiting behind that note.
///
/// A worker inside a tool call that never returns never takes a note, and the
/// delivery offering it cannot be interrupted — so a cancel of that node, the
/// one lever for stopping it, used to wait behind the note with every envelope
/// naming the node, claimed and unanswered, until the run was stopped and
/// adopted. Here `build`'s turn is held for the whole journey. Behind its note
/// are an `amend` of `build` and a second live note to it; then `build` is
/// cancelled. The amend is applied ahead of the cancel, both notes are answered
/// `refused` naming the cancel, the second never reaches the worker, and the
/// cancel parks the node — all without the held turn being released. `lint`'s
/// held turn keeps the run driven afterwards, so whatever the preempted
/// delivery's conversation says once its member is reaped reaches the writer,
/// and is shown to record nothing a second time.
#[test]
fn a_cancel_preempts_the_unanswered_note_to_its_node_and_what_waits_behind_it() {
    let world = World::new("note-cancel-preempts");
    let run = "preempts";
    held_cancellable_conversation(&world, run, vec![agent("build", &[]), agent("lint", &[])]);
    let queue = world.run_file(run, "channel/commands.jsonl");
    let queued = |text: &str| {
        let text = text.to_string();
        let queue = queue.clone();
        move |_: &World| std::fs::read_to_string(&queue).is_ok_and(|held| held.contains(&text))
    };

    let note = submitted(
        &world,
        run,
        &envelope(note_op("build", "worker", NOTE, None)),
    );
    world.until("the note to wait in build's inbox", |world| {
        awaiting_an_answer(world) == 1
    });
    let note_id = queued_id_of(&world, run, NOTE);
    let amend = submitted(
        &world,
        run,
        &envelope(json!({"op": "amend", "id": "build", "text": BUILD_RULING})),
    );
    world.until("the amend of build to be queued", queued(BUILD_RULING));
    let second = submitted(
        &world,
        run,
        &envelope(note_op("build", "worker", PREEMPTED_NOTE, None)),
    );
    world.until(
        "the second note to build to be queued",
        queued(PREEMPTED_NOTE),
    );
    let second_id = queued_id_of(&world, run, PREEMPTED_NOTE);
    let cancel = submitted(
        &world,
        run,
        &envelope(json!({"op": "cancel", "id": "build", "reason": "wedged in a tool call"})),
    );
    world.until("the cancel to be queued", queued("wedged in a tool call"));
    let cancel_id = queued_id_of(&world, run, "wedged in a tool call");

    world.until_within(CANCEL_PATIENCE, "the cancel to be applied", |world| {
        outcome_of(world, run, cancel_id).is_some()
    });
    assert!(
        answered(cancel).contains("\"state\":\"applied\""),
        "the cancel was not applied"
    );
    assert!(
        answered(amend).contains("\"state\":\"applied\""),
        "the amend queued ahead of the cancel was not applied"
    );
    let journal = world.journal(run);
    let amended = committed_at(&journal, "amend", "text", BUILD_RULING)
        .expect("the amend of build is journalled");
    let cancelled =
        committed_at(&journal, "cancel", "id", "build").expect("the cancel of build is journalled");
    assert!(
        amended < cancelled,
        "the amend claimed ahead of the cancel was committed after it"
    );
    for (text, id, reply) in [(NOTE, note_id, note), (PREEMPTED_NOTE, second_id, second)] {
        let output = reply.wait_with_output().expect("the binary runs");
        assert_eq!(
            output.status.code(),
            Some(REFUSED),
            "the reply carrying {text:?} did not end refused: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_preempted(&world, run, id, cancel_id, text);
    }
    assert!(
        worked(&world)
            .iter()
            .all(|prompt| !prompt.contains(PREEMPTED_NOTE)),
        "the second note was delivered to the worker it was preempted from"
    );
    assert_eq!(
        claimed_with_no_outcome(&world, run),
        None,
        "an envelope was left claimed with no outcome"
    );
    world.until("build to idle as cancelled", |world| {
        world.events_of(run, "node-settled").iter().any(|event| {
            event["labels"]["node"] == "build" && event["payload"]["status"] == "cancelled"
        })
    });

    // The preempted delivery's member has been reaped, so its conversation has
    // said whatever it will; `lint`'s held turn keeps the writer there to read it.
    std::thread::sleep(Duration::from_secs(5));
    assert!(
        world
            .events_of(run, "node-settled")
            .iter()
            .all(|event| event["labels"]["node"] != "lint"),
        "lint settled, so nothing was driving the run to record a late answer"
    );
    assert_preempted(&world, run, note_id, cancel_id, NOTE);
    assert_preempted(&world, run, second_id, cancel_id, PREEMPTED_NOTE);

    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    world.until("lint to settle", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "lint")
    });
    assert_preempted(&world, run, note_id, cancel_id, NOTE);
}

/// A cancel overtakes an envelope that names its node and also waits on another
/// node's note, which is then judged against the cancelled node once that note
/// is answered.
///
/// The one reordering the preemption makes. The overtaken envelope cannot be
/// applied in part, so it neither holds the cancel back nor is dropped: it waits
/// on `lint`'s note as it would have, and its receipt resolves once that turn
/// ends — applied or refused against a `build` that is now parked.
#[test]
fn a_cancel_overtakes_an_envelope_also_waiting_on_another_nodes_note() {
    let world = World::new("note-cancel-overtakes");
    let run = "overtakes";
    held_cancellable_conversation(&world, run, vec![agent("build", &[]), agent("lint", &[])]);
    let queue = world.run_file(run, "channel/commands.jsonl");
    let queued = |text: &str| {
        let text = text.to_string();
        let queue = queue.clone();
        move |_: &World| std::fs::read_to_string(&queue).is_ok_and(|held| held.contains(&text))
    };

    let to_build = submitted(
        &world,
        run,
        &envelope(note_op("build", "worker", NOTE, None)),
    );
    let to_lint = submitted(
        &world,
        run,
        &envelope(note_op("lint", "worker", PLANNER_CONTEXT, None)),
    );
    world.until("both notes to wait in their inboxes", |world| {
        awaiting_an_answer(world) == 2
    });
    let note_id = queued_id_of(&world, run, NOTE);
    let mut both = submitted(
        &world,
        run,
        &json!({"version": 2, "commands": [
            {"op": "amend", "id": "build", "text": BUILD_RULING},
            {"op": "amend", "id": "lint", "text": LINT_RULING},
        ]})
        .to_string(),
    );
    world.until("the envelope naming both to be queued", queued(LINT_RULING));
    let both_id = queued_id_of(&world, run, LINT_RULING);
    let cancel = submitted(
        &world,
        run,
        &envelope(json!({"op": "cancel", "id": "build", "reason": "wedged in a tool call"})),
    );
    world.until("the cancel to be queued", queued("wedged in a tool call"));
    let cancel_id = queued_id_of(&world, run, "wedged in a tool call");

    world.until_within(CANCEL_PATIENCE, "the cancel to be applied", |world| {
        outcome_of(world, run, cancel_id).is_some()
    });
    assert!(
        answered(cancel).contains("\"state\":\"applied\""),
        "the cancel was not applied"
    );
    let output = to_build.wait_with_output().expect("the binary runs");
    assert_eq!(output.status.code(), Some(REFUSED));
    assert_preempted(&world, run, note_id, cancel_id, NOTE);
    assert!(
        outcome_of(&world, run, both_id).is_none()
            && both.try_wait().expect("the reply is readable").is_none(),
        "the envelope still waiting on lint's note was answered before it was"
    );

    // lint's turn ends, its note is answered, and the overtaken envelope is
    // judged against build's cancelled state.
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    assert!(
        answered(to_lint).contains("\"state\":\"applied\""),
        "the note to lint was not applied"
    );
    world.until("the overtaken envelope to be answered", |world| {
        outcome_of(world, run, both_id).is_some()
    });
    let _ = both.wait();
    let journal = world.journal(run);
    let cancelled =
        committed_at(&journal, "cancel", "id", "build").expect("the cancel of build is journalled");
    let judged = journal
        .iter()
        .position(|event| {
            (event["kind"] == "edit-committed" || event["kind"] == "edit-rejected")
                && event["payload"]["command"]["text"] == BUILD_RULING
        })
        .expect("the overtaken amend of build is journalled, applied or refused");
    assert!(
        cancelled < judged,
        "the overtaken envelope was judged before the cancel that overtook it"
    );
    assert_eq!(
        claimed_with_no_outcome(&world, run),
        None,
        "an envelope was left claimed with no outcome"
    );
}

/// Notes to two conversations wait in both inboxes at once, and an envelope naming
/// both nodes waits for both answers.
///
/// Each envelope that offers a note is handed to a delivery thread of its own, so
/// a note to one conversation is offered while another conversation's turn still
/// holds the first: with one thread for every note, the second would wait in that
/// thread behind the first and never reach its inbox until the first turn ended.
/// The doubled turn's hold is one pair of gates for every conversation, so both
/// answers come together here; which of the two an envelope naming both waits for
/// alone is `engine`'s `an_envelope_naming_two_outstanding_notes_waits_for_both_answers`.
#[test]
fn notes_to_two_conversations_wait_in_both_inboxes_at_once() {
    let world = World::new("note-two-inboxes");
    let run = "twoinboxes";
    held_heartbeating_conversation(&world, run, vec![agent("build", &[]), agent("lint", &[])]);
    world.until("both workers' turns to open", |world| {
        let opened = world.events_of(run, "turn-started");
        ["build", "lint"]
            .iter()
            .all(|node| opened.iter().any(|event| event["labels"]["node"] == *node))
    });

    let to_build = submitted(
        &world,
        run,
        &envelope(note_op("build", "worker", NOTE, None)),
    );
    let to_lint = submitted(
        &world,
        run,
        &envelope(note_op("lint", "worker", PLANNER_CONTEXT, None)),
    );
    world.until(
        "both notes to wait in their conversations' inboxes",
        |world| awaiting_an_answer(world) == 2,
    );
    let mut about_both = submitted(
        &world,
        run,
        &json!({"version": 2, "commands": [
            {"op": "finding", "id": "build", "message": FINDING},
            {"op": "finding", "id": "lint", "message": UNADDRESSED},
        ]})
        .to_string(),
    );
    let queue = world.run_file(run, "channel/commands.jsonl");
    world.until("the envelope naming both to be queued", |_| {
        std::fs::read_to_string(&queue).is_ok_and(|text| text.contains(UNADDRESSED))
    });
    let beats = world.events_of(run, "member-heartbeat").len();
    world.until("the run to go on recording its dispatches", |world| {
        world.events_of(run, "member-heartbeat").len() >= beats + 2
    });
    assert!(
        about_both
            .try_wait()
            .expect("the reply is readable")
            .is_none()
            && world
                .journal(run)
                .iter()
                .all(|event| event["payload"]["command"]["op"] != "finding"),
        "the envelope naming both nodes was applied while both notes were waiting"
    );

    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    for (what, reply) in [
        ("the note to build's", to_build),
        ("the note to lint's", to_lint),
        ("the envelope naming both's", about_both),
    ] {
        assert!(
            answered(reply).contains("\"state\":\"applied\""),
            "{what} envelope was not applied"
        );
    }
    let journal = world.journal(run);
    let at = |op, field, value| {
        committed_at(&journal, op, field, value)
            .unwrap_or_else(|| panic!("no {op} reading {value} was committed"))
    };
    let notes = at("note", "id", "build").max(at("note", "id", "lint"));
    let findings = at("finding", "message", FINDING).min(at("finding", "message", UNADDRESSED));
    assert!(
        notes < findings,
        "the envelope naming both nodes was committed before both notes were"
    );

    world.until("the run to settle", |world| {
        world.events_of(run, "node-settled").len() == 2
    });
}

/// A note the conversation refuses while the run is still being driven is
/// journalled and surfaced exactly as a refusal answered inline is.
///
/// `build` has settled `done` while `after`, which depends on it, is held mid-turn
/// and keeps the driver alive, so the note to `build` is offered on the delivery
/// thread and its refusal comes back to the writer as that thread's answer. Its
/// member has settled, and the node will never be dispatched again for `persist` to
/// carry it to, so it reached nobody.
#[test]
fn a_note_refused_while_the_run_is_driven_is_recorded_and_surfaced() {
    let world = World::new("note-driven-refusal");
    let run = "drivenrefusal";
    world.script("turn.hold-each", "");
    held_heartbeating_conversation(
        &world,
        run,
        vec![agent("build", &[]), agent("after", &["build"])],
    );
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    world.until(
        "the dependent's turn to open while build is settled",
        |world| {
            world
                .events_of(run, "turn-started")
                .iter()
                .any(|event| event["labels"]["node"] == "after")
        },
    );

    let refused = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &envelope(note_op("build", "worker", NOTE, None)),
    );
    refused
        .exited(2)
        .err_has("was not delivered")
        .err_has("build")
        .err_has("no dispatch of it will take the note either");
    assert!(
        world.run_file(run, "owner.lock").exists(),
        "the run's driver was gone, so the refusal was not the driven one"
    );
    let rejected: Vec<Value> = world
        .events_of(run, "edit-rejected")
        .into_iter()
        .filter(|event| event["payload"]["command"]["op"] == "note")
        .collect();
    let [recorded] = &rejected[..] else {
        panic!(
            "the run recorded {} rejected notes, not one",
            rejected.len()
        );
    };
    assert!(
        recorded["payload"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("was not delivered")),
        "the record does not say the note was undelivered: {recorded}"
    );
    assert!(
        world
            .events_of(run, "edit-committed")
            .iter()
            .all(|event| event["payload"]["command"]["op"] != "note"),
        "an undelivered note was committed as though it had landed"
    );

    std::fs::remove_file(world.fakes.join("turn.hold")).expect("the hold is lifted");
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    world.until("the run to settle", |world| {
        world.events_of(run, "node-settled").len() >= 2
    });
}

/// A note submitted while **nothing** drives the run waits on no turn.
///
/// With no driver, `reply` is the run's writer itself and delivers inline, holding
/// the run's lock while it does — which is only safe because nothing it can reach
/// is a conversation that answers when a turn ends. Every conversation of a run
/// lives inside the driver that holds its lock, so with none alive the note's
/// inbox has nobody behind it. The case that could still look otherwise is a
/// driver that died mid-turn, leaving that turn's harness orphaned and still
/// working: the note is offered, withdrawn when nothing takes it within the
/// sibling's own bound, and carried to the node's next dispatch — while the
/// orphaned turn is still held, so the reply's wait was never the turn's.
///
/// A driver that tries to take the run while the reply waits is refused with the
/// reply named as the run's writer, and the run is the fresh driver's to take
/// once the reply returns.
///
/// `#[cfg(unix)]` because it ends the run's driver by pid, as the adoption
/// journeys do.
#[cfg(unix)]
#[test]
fn a_note_submitted_with_nothing_driving_waits_on_no_turn_and_leaves_the_run_adoptable() {
    let world = World::new("note-undriven");
    let run = "undriven";
    held_conversation(&world, run, vec![agent("build", &[])]);
    let lock = world.run_file(run, "owner.lock");
    let holder: Value = serde_json::from_str(
        &std::fs::read_to_string(&lock).expect("the driver holds the run's lock"),
    )
    .expect("the lock is a record");
    let driver = holder["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or_else(|| panic!("the lock names its holder's pid: {holder}"));
    harness::end_process(driver);

    // The worker's turn is never released before the reply returns: whatever the
    // reply waited on, it was not that turn ending.
    let began = Instant::now();
    let reply = submitted(
        &world,
        run,
        &envelope(note_op("build", "worker", NOTE, None)),
    );
    let writer = reply.id();
    world.until("the note to wait in the conversation's inbox", |world| {
        awaiting_an_answer(world) == 1
    });

    // While it waits, the reply is the run's writer, and a driver that tries to
    // take the run is told who has it.
    world
        .run_on(
            world.agentgraph_cmd(&["adopt", run, "--detach"]),
            "adopt --detach",
        )
        .exited(REFUSED)
        .err_has(&format!("run '{run}' is being written by pid {writer}"))
        .err_has("reply");

    assert!(
        answered(reply).contains("\"state\":\"applied\""),
        "the note's envelope was not applied"
    );
    // The sibling's own withdrawal bound, and slack for two process starts and a
    // journal write around it: a reply that outlasted this waited on something
    // other than the note's offer.
    let bound = onemessagebus::SPOOL_WAIT + Duration::from_secs(15);
    assert!(
        began.elapsed() < bound,
        "the reply took {:?}, past the note's withdrawal bound",
        began.elapsed()
    );
    assert!(
        !world.fakes.join("turn.go").exists(),
        "the held turn was released before the reply returned"
    );
    let operation = recorded(&world, run);
    assert_eq!(
        operation["reached"],
        json!("carried"),
        "a note no conversation could take was not carried to the node's next dispatch: \
         {operation}"
    );
    assert!(
        !lock.exists(),
        "the reply kept the run's lock after it answered"
    );

    // A fresh driver takes the run, and the node's next dispatch is handed the note.
    world
        .run_on(
            world.agentgraph_cmd(&["adopt", run, "--detach"]),
            "adopt --detach",
        )
        .exited(0);
    world.until("the adopted run to dispatch the node again", |world| {
        dispatch_records_of(world, run, "build").len() >= 2
    });
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    world.until("the adopted run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });
    let dispatches = dispatches_of(&world, run, "build");
    assert!(
        dispatches
            .last()
            .is_some_and(|turns| turns.first().is_some_and(|task| task.contains(NOTE))),
        "the adopted run's dispatch was not handed the carried note:\n{dispatches:#?}"
    );
}

/// A note is **not** offered to a conversation on behalf of an envelope the run
/// is going to refuse.
///
/// Compiling a `note` used to hand it to the node's live turn, so a later
/// command's refusal left a correction a worker had read and the run had no
/// record of. The refusal here is one only the reconciler can make — `reply`'s
/// submission check carries no dispatch in its frontier, so a `settle` of a
/// running node passes it and the loop refuses it — and it comes *after* the note
/// in the envelope, which is the order that used to deliver first and refuse
/// second.
#[test]
fn a_note_in_an_envelope_the_run_refuses_is_never_offered_to_the_conversation() {
    let world = World::new("note-envelope-unoffered");
    let run = "unoffered";
    held_conversation(&world, run, vec![agent("build", &[])]);

    // The turn is released once the note is really on the queue, exactly as the
    // journeys that *do* deliver release it — so the turn this note would have
    // been offered to was live and reachable for the whole window, and the reason
    // it was never offered is the pass rather than the timing.
    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    let replied = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &json!({"version": 2, "commands": [
            note_op("build", "worker", NOTE, None),
            {"op": "settle", "id": "build", "outcome": "done",
             "evidence": "the change merged while nobody was looking"},
        ]})
        .to_string(),
    );
    releasing.join().expect("the releasing thread finishes");
    replied
        .exited(REFUSED)
        .err_has("still has a dispatch in flight");

    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });

    // Nothing of the envelope is in the record, and — the point — no party of the
    // conversation was ever handed the note.
    assert!(
        world.events_of(run, "edit-committed").is_empty()
            && world.events_of(run, "command-accepted").is_empty(),
        "a command of a refused envelope reached the record: {:?}",
        world.kinds(run)
    );
    let handed = prompts(&world);
    assert!(
        !handed.iter().any(|prompt| prompt.contains(NOTE)),
        "validation offered the note to the conversation on behalf of an envelope the run \
         then refused:\n{handed:#?}"
    );

    // And the answer says which command was wrong and which was not, so a manager
    // knows the note is theirs to resend rather than theirs to fix.
    let answered = world
        .command_outcomes(run)
        .last()
        .cloned()
        .expect("the envelope was answered");
    assert_eq!(answered["results"][0]["op"], json!("note"), "{answered}");
    assert_eq!(
        answered["results"][0]["outcome"],
        json!("validated"),
        "{answered}"
    );
    assert_eq!(
        answered["results"][1]["outcome"],
        json!("refused"),
        "{answered}"
    );
}

/// A note to a node the run has **no conversation for** refuses before any note of
/// its envelope has been offered to anybody.
///
/// The last refusal that could follow a delivery, and the one this journey used to
/// record as unavoidable: "nothing takes it" for a node that has never reported a
/// member is not a fact about a conversation at all, and the pass used to discover
/// it only after handing the note before it to a live turn. Deciding it in
/// validation leaves the delivery refusable by one thing alone — a conversation
/// that was asked and said no, which `engine::deliver_envelope` bounds.
///
/// This tier is the only one where a live delivery can succeed at all, since a
/// suite substituting `oneagentgraph` as an *executable* gets an undelivered note
/// by construction. That is what makes the assertion below load-bearing: a note
/// that never reaches the worker here is one the pass did not offer.
#[test]
fn a_note_to_a_node_with_no_conversation_refuses_before_any_note_of_it_is_offered() {
    let world = World::new("note-envelope-refused");
    let run = "envelope";
    // A second node that never dispatches, so the run has no member for it and
    // no conversation to offer its note to.
    held_conversation(
        &world,
        run,
        vec![agent("build", &[]), agent("later", &["build"])],
    );

    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    let replied = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &json!({"version": 2, "commands": [
            note_op("build", "worker", NOTE, None),
            note_op_with("later", "start from the fixture", "live", false),
        ]})
        .to_string(),
    );
    releasing.join().expect("the releasing thread finishes");
    replied
        .exited(REFUSED)
        .err_has("composes it into no dispatch");

    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });

    // The point: the worker's turn was live and reachable for the whole window —
    // the release above waited for the note to be queued before ending it — and
    // the note was never handed to it, because the envelope was already refused.
    let worker = worked(&world);
    assert!(
        !worker.iter().any(|prompt| prompt.contains(NOTE)),
        "a note of an envelope the run refused was offered to the live turn anyway:\n\
         {worker:#?}"
    );

    // And the run committed nothing of the envelope — neither the note that would
    // have landed nor the one that refused.
    assert!(
        world.events_of(run, "edit-committed").is_empty()
            && world.events_of(run, "command-accepted").is_empty(),
        "a command of a refused envelope reached the record: {:?}",
        world.kinds(run)
    );
    // And the answer tells the two apart: nothing was wrong with the first, so a
    // manager resends it — which costs nothing, because nothing of it happened.
    let answered = world
        .command_outcomes(run)
        .last()
        .cloned()
        .expect("the envelope was answered");
    assert_eq!(answered["applied"], json!(false), "{answered}");
    assert_eq!(answered["results"][0]["op"], json!("note"), "{answered}");
    assert_eq!(
        answered["results"][0]["outcome"],
        json!("validated"),
        "the note nobody was offered was reported as delivered or as its own refusal: \
         {answered}"
    );
    assert_eq!(
        answered["results"][1]["outcome"],
        json!("refused"),
        "{answered}"
    );
}

/// A note that changes what the finished tree must contain enters the acceptance
/// criteria the judge decides against — and reaches the judge as an update to the
/// **worker's** task rather than as work for itself.
///
/// Two claims about one delivery, because they fail together: a criterion the judge
/// never sees is a bar nobody moved, and a criterion the judge reads as its own
/// instruction is a judge doing the worker's job.
#[test]
fn a_binding_note_enters_the_bar_its_judge_decides_against_as_the_workers_own() {
    let world = World::new("note-binding");
    let run = "binding";
    held_conversation(&world, run, vec![agent("build", &[])]);

    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    let replied = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &envelope(note_op("build", "worker", NOTE, Some(CRITERION))),
    );
    releasing.join().expect("the releasing thread finishes");
    replied.exited(0).out_has("\"state\":\"applied\"");

    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });

    let judge = judged(&world);
    let first = judge.first().expect("the judge decided at least once");

    // The bar itself. Not "the criterion is somewhere in the prompt": it is in the
    // completion criterion the judge is told to decide against, which is the
    // section a note that only narrated would never reach.
    // Everything between the section the prompt names the bar in and the section
    // that follows it, which is the transcript. Bounded rather than "somewhere in
    // the prompt": the notes the judge is shown *beside* the bar are a different
    // claim, made below.
    let bar = first
        .split_once("Completion criterion:")
        .map(|(_, rest)| {
            rest.split("Conversation transcript")
                .next()
                .unwrap_or(rest)
                .to_string()
        })
        .unwrap_or_else(|| panic!("the judge was given no completion criterion:\n{first}"));
    assert!(
        bar.contains(CRITERION),
        "the criterion the note bound is not in the bar the judge decides against:\n{bar}"
    );

    // And the addressing, which survived the whole way: the judge is told the note
    // was for the worker, and told not to take the worker's job on.
    assert!(
        first.contains("delivered to the WORKER"),
        "the judge was not told whose task the note updates:\n{first}"
    );
    assert!(
        first.contains(CRITERION) && first.contains(NOTE),
        "the judge was not shown the note beside the criterion it added:\n{first}"
    );

    let operation = recorded(&world, run);
    assert_eq!(operation["criterion"], json!(CRITERION), "{operation}");
}

/// A note arriving after the node's dispatch has completed is refused, naming that
/// it was not delivered and why — and the run records that non-delivery.
///
/// The silence this replaces has its own measured price: a note reached a node
/// after the worker had reported completion, was accepted with nothing said, the
/// worker did another forty minutes of correct work, and the node was failed for a
/// completion report that preceded its own subsequent commits. A refusal would have
/// let the manager relaunch instead.
///
/// **Both places the one reach-nobody rule can be decided about a settled node are
/// driven here**, because a node that will never be dispatched again is what makes
/// them one rule rather than two. Under the default the conversation answers
/// first, and `persist` then has nowhere to carry what it could not deliver; under
/// `deliver: next` no conversation is asked at all, so the run's own record decides
/// it a step earlier. Neither is a special case beside the other, and each refusal
/// names what left the note nowhere to go.
#[test]
fn a_note_arriving_after_the_dispatch_has_completed_is_refused_and_recorded() {
    let world = World::new("note-late");
    let run = "late";
    held_conversation(&world, run, vec![agent("build", &[])]);
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    // The whole run, not only the node: a driver still closing out holds the run's
    // lock, and a reply that arrived then would be queued for a reconciler about to
    // exit rather than answered by one. The lock's own absence is what says it has
    // gone, which is the same question `reply` itself asks.
    world.until("the run's driver to release it", |world| {
        !world.run_file(run, "owner.lock").exists()
    });

    let refused = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &envelope(note_op("build", "worker", NOTE, None)),
    );
    // Refused, and the refusal says the one thing a caller has to act on: that
    // nobody read it, and what to do instead.
    refused
        .exited(2)
        .err_has("was not delivered")
        .err_has("build")
        // The half of the rule only the run can decide: the live attempt found no
        // turn, and the `persist` this default carries had nowhere to carry it,
        // because a node that has settled `done` has no next dispatch.
        .err_has("no dispatch of it will take the note either");

    // The same note to the same node, asked for no live delivery at all. Nothing
    // asks the conversation this time — there is nothing a note could be carried
    // to — so the same rule is decided off the run's own record, before the run is
    // reached, and says which of the two fields left it nowhere.
    world
        .run_with_stdin_on(
            world.agentgraph_cmd(&["reply", run]),
            &envelope(note_op_with("build", NOTE, "next", true)),
        )
        .exited(REFUSED)
        .err_has("it has settled done")
        .err_has("`deliver: next` asks for no live delivery");

    // Nothing was silently accepted: no note is on the run's committed record.
    let committed: Vec<Value> = world
        .events_of(run, "edit-committed")
        .into_iter()
        .filter(|event| event["payload"]["command"]["op"] == "note")
        .collect();
    assert!(
        committed.is_empty(),
        "an undelivered note was committed as though it had landed: {committed:#?}"
    );

    // And the non-delivery is in the run's own record rather than only in the
    // caller's exit code — which is the difference between a manager finding it
    // afterwards and having to remember it.
    let rejected: Vec<Value> = world
        .events_of(run, "edit-rejected")
        .into_iter()
        .filter(|event| event["payload"]["command"]["op"] == "note")
        .collect();
    let [recorded] = &rejected[..] else {
        panic!(
            "the run recorded {} rejected notes, not one",
            rejected.len()
        );
    };
    let reason = recorded["payload"]["reason"]
        .as_str()
        .expect("the record says why");
    assert!(
        reason.contains("was not delivered"),
        "the record does not say the note was undelivered: {reason}"
    );
}

/// The same delivery, and the same refusal, through this crate's own API.
///
/// A consumer composing this engine reaches the seam without writing a reply
/// envelope by hand — and reaches the *same* seam: the call submits through the
/// same channel and is judged by the same reconciler, so the two spellings cannot
/// come to mean different things. Both answers are driven here, because a surface
/// that only proves the happy path is one whose refusal nobody has ever seen.
///
/// The refusal driven here is a note to a node with **no conversation yet** rather
/// than to one whose conversation is over — the other non-delivery, and the one
/// this run can hold still: a run whose every node has settled has no driver left
/// to answer through, so arrival-after-completion is driven where it belongs, in
/// `a_note_arriving_after_the_dispatch_has_completed_is_refused_and_recorded`,
/// against the same call this one makes.
#[test]
fn the_note_seam_answers_a_delivery_and_a_non_delivery_through_this_crates_own_api() {
    let world = World::new("note-api");
    let run = "api";
    held_conversation(
        &world,
        run,
        vec![agent("build", &[]), agent("later", &["build"])],
    );
    let paths = RunPaths::under(&world.runs, run);

    // First the refusals, while the held node keeps the run's own reconciler alive
    // to answer them. A node this run does not have at all is the ask that is
    // wrong rather than the delivery that failed, and it is answered as one —
    // before any conversation is looked for.
    let absent = deliver(&paths, "nowhere", &Note::to(Addressee::Worker, NOTE))
        .expect_err("a node the graph does not hold takes no note");
    let said = absent.to_string();
    assert!(
        said.contains("no node") && said.contains("nowhere"),
        "the refusal does not name the node the graph does not hold: {said}"
    );

    // Then the delivery that could not be made: `later` has not been dispatched,
    // so there is no conversation of its own for a note to be handed to. Under
    // the defaults that is not a refusal — the note would be carried to `later`'s
    // next dispatch — so this asks for the combination that has nowhere to carry
    // it to, which is the delivery-time half of the one reach-nobody rule and the
    // only half a run can decide.
    let refused = deliver_with(
        &paths,
        "later",
        &Note::to(Addressee::Worker, NOTE),
        Deliver::Live,
        false,
    )
    .expect_err("a note with no turn to take it and no dispatch to carry it to reaches nobody");
    let said = refused.to_string();
    assert!(
        said.contains("later") && said.contains("no conversation"),
        "the refusal does not name the node or say what was missing: {said}"
    );
    assert!(
        said.contains("`persist: false` composes it into no dispatch"),
        "the refusal does not say what left the note nowhere to go: {said}"
    );

    // Then the delivery, into the conversation that is live — answered with which
    // party took it, which is what a caller has no second source for.
    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    let delivered = deliver(
        &paths,
        "build",
        &Note::to(Addressee::Worker, NOTE)
            .binding(CRITERION)
            .expect("the seam accepts this criterion"),
    );
    releasing.join().expect("the releasing thread finishes");
    assert_eq!(
        delivered.expect("the live conversation took the note"),
        Delivered::To(Reached::Worker)
    );

    world.until("the run's driver to release it", |world| {
        !world.run_file(run, "owner.lock").exists()
    });
}

/// A note that reached no running turn is carried to the node's **next** dispatch,
/// and the run says that is what happened rather than leaving it to inference.
///
/// One direction of the biconditional `persist` is defined by, and the journey the
/// default exists for: `deliver: live` attempts the running turn, `persist: true`
/// keeps the note where nothing took it, and a caller sending neither field gets
/// both. What is read back is the dispatch's **own prompt** — the node's task was
/// composed after the note was carried, so the note being in it is the carry and
/// nothing else.
///
/// The same node takes the delivery-time half of the reach-nobody rule first, which
/// is the only half a run can decide: with `persist: false` there is nowhere for the
/// note to go, so it is refused rather than accepted and lost.
#[test]
fn a_note_no_turn_took_is_carried_to_the_nodes_next_dispatch_and_named_as_carried() {
    let world = World::new("note-carried");
    let run = "carried";
    held_conversation(
        &world,
        run,
        vec![agent("build", &[]), agent("later", &["build"])],
    );

    // `later` has no dispatch yet, so nothing of it can take a note. With
    // `persist: false` that is a note with nowhere to go, and it is refused
    // naming both halves of why.
    let refused = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &envelope(note_op_with("later", NOTE, "live", false)),
    );
    refused
        .exited(REFUSED)
        .err_has("later")
        .err_has("`persist: false` composes it into no dispatch");

    // The same note under the defaults is not a refusal: it is carried.
    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    world
        .run_with_stdin_on(
            world.agentgraph_cmd(&["reply", run]),
            &envelope(note_op("later", "worker", NOTE, None)),
        )
        .exited(0)
        .out_has("\"state\":\"applied\"");
    releasing.join().expect("the releasing thread finishes");

    let operation = recorded(&world, run);
    assert_eq!(operation["node"], json!("later"), "{operation}");
    assert_eq!(
        operation["reached"],
        json!("carried"),
        "a note no turn took was not named as carried: {operation}"
    );
    // Nobody has been shown it yet, and the record says nobody rather than
    // guessing at the dispatch that will.
    assert!(
        operation.get("shown_to").is_none() && operation.get("routed_to").is_none(),
        "a note nobody has read yet is recorded as shown or routed to somebody: {operation}"
    );

    world.until("the run to settle", |world| {
        world.events_of(run, "node-settled").len() >= 2
    });

    // And the dispatch really was given it. Its task was composed when the
    // dispatch started, which was after the note was carried, so there is no
    // other way the note could be in the instruction this turn opened on.
    let dispatched = dispatches_of(&world, run, "later");
    let [first] = &dispatched[..] else {
        panic!(
            "`later` was dispatched {} times, not once",
            dispatched.len()
        );
    };
    assert!(
        first.iter().any(|instruction| instruction.contains(NOTE)),
        "the carried note did not reach the dispatch it was carried to:\n{first:#?}"
    );
    // And that dispatch's record does not call the note spent: it was composed
    // with it, as the node's own context. Only a note a conversation **read**
    // that a later dispatch is composed without is spent.
    let records = dispatch_records_of(&world, run, "later");
    assert_eq!(records.len(), 1, "{records:#?}");
    assert!(
        records[0]["payload"].get("notes_spent").is_none(),
        "the dispatch a note was carried to reported it spent: {}",
        records[0]
    );
    // It was carried through the bus's carry store, and the dispatch it reached
    // drained it: what is left is the store's header line and no note.
    let store = std::fs::read_to_string(world.run_file(run, "notes/later.carried.jsonl"))
        .expect("the note was carried through a carry store");
    let lines: Vec<&str> = store.lines().collect();
    assert!(
        lines.len() == 1 && lines[0].contains("onemessagebus-carry-store"),
        "the dispatch a note was carried to did not drain its carry store:\n{store}"
    );
    // The carried record is not the last word on the note: the dispatch it was
    // carried to shows it — the opening turn as the task, the judge's answer
    // after — and each presentation is recorded as its stream showed it.
    let shown = presentations_of(&world, run, "later");
    assert_eq!(
        shown
            .iter()
            .map(|event| (
                event["payload"]["party"].clone(),
                event["payload"]["evidence"].clone()
            ))
            .collect::<Vec<_>>(),
        vec![
            (json!("worker"), json!("opening-task")),
            (json!("supervisor"), json!("answering-turn")),
        ],
        "the dispatch a note was carried to did not record showing it:\n{shown:#?}"
    );
    assert!(
        shown
            .iter()
            .all(|event| event["payload"]["text"] == json!(NOTE)
                && event["payload"]["reached"] == json!("carried")),
        "{shown:#?}"
    );
}

/// A note recorded `carried` whose text a turn of the **same** dispatch then
/// opens on is recorded as shown — from the words, since nothing routed it.
///
/// A carried note can reach a live turn by a lever outside the note seam (an
/// interrupt issued by hand into the harness process, which this suite has no
/// double for), and a record that then still says nobody took it is a
/// presentation with no receipt at all. What is driven here against the real
/// conversation is that lever's effect: `deliver: next` asks for no live
/// attempt, so the note is `carried` while the dispatch is live, and the
/// supervising side then reads its text into the worker's next turn. The
/// stream shows a worker turn opening on the note's whole text, stamped as the
/// supervisor's own, and the record says the worker was shown it, from the
/// text, and the judge with the turn that answered.
#[test]
fn a_note_recorded_carried_whose_text_a_turn_then_opens_on_is_recorded_as_shown() {
    let world = World::new("note-carried-read");
    let run = "carriedread";
    // The supervising side's next instruction is the note's own text: the
    // lever that reads a carried note into a live turn, in this suite.
    world.script("judge.asks-again", NOTE);
    world.script("turn.hold", "hold");
    supervised_run(&world, run, vec![agent("build", &[])]);
    world.until("the worker's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
    });

    // Recorded `carried` while the conversation is live and the worker's turn is
    // held open: no live attempt was asked for, so the seam took nothing.
    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    world
        .run_with_stdin_on(
            world.agentgraph_cmd(&["reply", run]),
            &envelope(note_op_with("build", NOTE, "next", true)),
        )
        .exited(0)
        .out_has("\"state\":\"applied\"");
    releasing.join().expect("the releasing thread finishes");
    let delivery = recorded(&world, run);
    assert_eq!(delivery["reached"], json!("carried"), "{delivery}");
    assert!(
        delivery.get("shown_to").is_none() && delivery.get("routed_to").is_none(),
        "the seam took nothing, and the record says it routed something: {delivery}"
    );

    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });

    // The text reached the worker's next turn — by the supervisor's own words,
    // which the producer stamps as such — and the record says the worker was
    // shown it and how that was decided, and then the judge.
    let openings = openings_of(&world, run, "build");
    let read_in = openings
        .iter()
        .find(|opening| opening.instruction.contains(NOTE))
        .unwrap_or_else(|| panic!("no worker turn opened on the note's text:\n{openings:#?}"));
    assert_eq!(read_in.origin, Some(Origin::Supervisor), "{read_in:?}");
    let shown = presentations_of(&world, run, "build");
    assert_eq!(
        shown
            .iter()
            .map(|event| {
                (
                    event["payload"]["party"].clone(),
                    event["payload"]["evidence"].clone(),
                    event["payload"]["turn"].clone(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (
                json!("worker"),
                json!("instruction-text"),
                json!(read_in.turn)
            ),
            (
                json!("supervisor"),
                json!("answering-turn"),
                json!(read_in.turn)
            ),
        ],
        "a note recorded carried that a turn then opened on was not recorded as shown:\n\
         {shown:#?}"
    );
    assert!(
        shown
            .iter()
            .all(|event| event["payload"]["text"] == json!(NOTE)
                && event["payload"]["reached"] == json!("carried")),
        "{shown:#?}"
    );
}

/// Two notes in one envelope reach the worker on **two** turns, and each
/// presentation the run records names the note that turn really opened on — and
/// a note whose next presentation never happened gets no receipt for it.
///
/// The conversation acknowledges a note into a live worker turn only when that
/// turn ends and the next opens carrying it, and the envelope's notes are
/// offered one at a time: the second is offered while the turn carrying the
/// first is live, so it opens the turn after. The two acknowledgements reach the
/// run's record together, in one commit, at one instant — so nothing about
/// *when* they were recorded tells the two turns apart, and a watch that read
/// any `delivered` worker turn as every pending note's presentation stamped the
/// second note on the first note's turn. The dispatch is cancelled while the
/// second note's turn is held, so neither note's judge presentation happens,
/// and the record must say so for both.
#[test]
fn two_notes_in_one_envelope_are_each_recorded_on_the_turn_that_opened_on_them() {
    let world = World::new("note-two-turns");
    let run = "twoturns";
    let first = "the reviewer asked for a smaller diff; stop editing src/old.rs";
    let second = "leave the changelog alone; release-plz writes it";
    // Every worker turn of the one node is held and released on its own — an
    // empty marker holds them all — so the turn carrying the second note is
    // still open when the cancel arrives. One node only: a second one's turns
    // would take the same gates.
    world.script("turn.hold-each", "");
    world.script("turn.hold", "hold");
    world.write_graphs();
    world.write_supervised_node_graph();
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    let mut launch = world.agentgraph_cmd(&["start", &path, "--detach"]);
    launch.env(CANCEL_GRACE_ENV, "1");
    world.run_on(launch, "start --detach").exited(0);
    world.until("the worker's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
    });

    // The reply blocks until both notes are acknowledged, which is until the
    // turn carrying the first has ended and the one carrying the second has
    // opened — so it runs on its own thread while this one releases the turns.
    // And while it blocks, the run's writer relays nothing: the turn carrying
    // the first note is waited for at the harness the double records, never in
    // the journal, which cannot show it until the reply returns.
    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    let mut reply = world.agentgraph_cmd(&["reply", run]);
    let body = json!({"version": 2, "commands": [
        note_op("build", "worker", first, None),
        note_op("build", "worker", second, None),
    ]})
    .to_string();
    let replied = std::thread::spawn(move || {
        use std::io::Write;
        let mut child = reply
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the binary starts");
        child
            .stdin
            .as_mut()
            .expect("stdin is piped")
            .write_all(body.as_bytes())
            .expect("the envelope is written");
        child.wait_with_output().expect("the binary runs")
    });
    releasing.join().expect("the releasing thread finishes");
    // The first note's turn is open and held; release it so the second note is
    // acknowledged into the turn after it — promptly, because the seam waits a
    // bounded time for that acknowledgement and answers `carried` past it.
    world.until("the turn carrying the first note to open", |world| {
        worked(world).len() >= 2
    });
    // The second note is offered the moment the first is acknowledged, which is
    // the moment that turn opened; the offer is the reconciler's own step, with
    // nothing durable to watch for, so it is given the same short pause the
    // release above gives a queued note before the turn it is bound for ends.
    std::thread::sleep(Duration::from_secs(2));
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    let output = replied.join().expect("the reply thread finishes");
    assert!(
        output.status.success(),
        "the envelope was not applied: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    world.until("the turn carrying the second note to open", |world| {
        worked(world).len() >= 3
    });
    // Both notes reached a live turn, and the record says so of each.
    let committed: Vec<Value> = world
        .events_of(run, "edit-committed")
        .into_iter()
        .filter(|event| event["payload"]["command"]["op"] == "note")
        .map(|event| event["payload"]["operations"][0].clone())
        .collect();
    assert_eq!(committed.len(), 2, "{committed:#?}");
    assert!(
        committed
            .iter()
            .all(|operation| operation["reached"] == json!("worker")),
        "a note did not reach the worker's turn: {committed:#?}"
    );

    // Cancelled while the second note's turn is held: neither note's judge
    // presentation ever happens.
    world
        .run_with_stdin_on(
            world.agentgraph_cmd(&["reply", run]),
            &envelope(json!({"op": "cancel", "id": "build", "reason": "stop here"})),
        )
        .exited(0);
    world.until("the held dispatch to be reaped at its deadline", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "build")
    });

    // Which turn opened on which note, from the producer's own record.
    let openings = openings_of(&world, run, "build");
    let turn_carrying = |text: &str| -> u64 {
        let carrying: Vec<&TurnStarted> = openings
            .iter()
            .filter(|opening| opening.instruction.contains(text))
            .collect();
        assert_eq!(
            carrying.len(),
            1,
            "{text:?} opened {} turns, not one:\n{openings:#?}",
            carrying.len()
        );
        assert_eq!(
            carrying[0].origin,
            Some(Origin::Delivered),
            "{:?}",
            carrying[0]
        );
        carrying[0].turn
    };
    let first_turn = turn_carrying(first);
    let second_turn = turn_carrying(second);
    assert!(second_turn > first_turn, "{openings:#?}");

    // Each presentation names the note that turn really opened on, and nobody
    // is recorded as shown a note on a turn that did not carry it. No judge
    // presentation at all: the conversation was reaped before one.
    let shown = presentations_of(&world, run, "build");
    let mut recorded: Vec<(String, String, u64)> = shown
        .iter()
        .map(|event| {
            (
                event["payload"]["text"]
                    .as_str()
                    .expect("a note")
                    .to_string(),
                event["payload"]["party"]
                    .as_str()
                    .expect("a party")
                    .to_string(),
                event["payload"]["turn"].as_u64().expect("a turn"),
            )
        })
        .collect();
    recorded.sort();
    let mut expected = vec![
        (first.to_string(), "worker".to_string(), first_turn),
        (second.to_string(), "worker".to_string(), second_turn),
    ];
    expected.sort();
    assert_eq!(
        recorded, expected,
        "the presentations recorded are not one per note on the turn that opened on \
         it:\n{shown:#?}"
    );

    // Released so the held turn ends with the journey rather than waiting out the
    // doubles' own bound on a hold.
    for gate in ["turn.go", "turn.settle"] {
        release(&world.fakes, gate);
    }
}

/// A `retry`'s replacement is composed from the manager's own task, so the notes
/// the node it supersedes read are **spent** by it — and its record says so.
///
/// The manager-initiated re-dispatch entry 60 ruled on, kept as ruled: nothing
/// of the superseded node's conversation is composed into the replacement, and
/// `amend` or the replacement's own task is where a ruling that has to survive
/// goes. What is new is that the replacement's dispatch names what it spent, so a
/// manager who retried a node without restating a ruling learns that from the
/// record rather than from the replacement's judge.
///
/// The node is retried **while running**, which is what leaves a conversation
/// that read the note for the replacement to supersede: a node that completed
/// would refuse the retry, and one that failed would have nothing live to have
/// read it. Every worker turn of `build` is held, as in the requeue journey
/// above, so it is still in flight when the retry arrives.
#[test]
fn a_retry_replacement_spends_the_notes_the_node_it_supersedes_read_and_says_so() {
    let world = World::new("note-retry-spent");
    let run = "retryspent";
    world.script("turn.hold-each", "Do build.");
    world.script(
        "judge.asks-again",
        "Run the check again and report what it said.",
    );
    world.script("turn.hold", "hold");
    world.script("judge.hold", "hold");
    world.write_graphs();
    world.write_supervised_node_graph();
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), agent("keep", &[])]),
    );
    let mut launch = world.agentgraph_cmd(&["start", &path, "--detach"]);
    launch.env(CANCEL_GRACE_ENV, "1");
    world.run_on(launch, "start --detach").exited(0);
    // `build`'s own turn, not the first of either node's: `keep` is ready
    // beside it, and a note to `build` offered before `build` has named a turn
    // is carried rather than delivered — the Windows gate's failure, where
    // `keep` opened first and the note read back `carried`. The driver takes a
    // turn's address before it relays the turn, so once this is in the store
    // the note has a conversation to reach.
    world.until("build's worker turn to open", |world| {
        !openings_of(world, run, "build").is_empty()
    });

    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    world
        .run_with_stdin_on(
            world.agentgraph_cmd(&["reply", run]),
            &envelope(note_op("build", "worker", NOTE, None)),
        )
        .exited(0)
        .out_has("\"state\":\"applied\"");
    releasing.join().expect("the releasing thread finishes");
    assert_eq!(recorded(&world, run)["reached"], json!("worker"));

    // Superseded while its reopened turn is still held: the retry cancels that
    // dispatch and adds the replacement, which dispatches once the superseded
    // one has been reaped at its deadline.
    world
        .run_with_stdin_on(
            world.agentgraph_cmd(&["reply", run]),
            &envelope(json!({
                "op": "retry",
                "id": "build",
                "node": {
                    "id": "build-again",
                    "persona": "engineer",
                    "task": "## What\nDo build again.\n\n## Acceptance criteria\n- build is done.",
                },
            })),
        )
        .exited(0)
        .out_has("\"state\":\"applied\"");
    world.until("the replacement to be dispatched", |world| {
        !dispatch_records_of(world, run, "build-again").is_empty()
    });

    let records = dispatch_records_of(&world, run, "build-again");
    assert_eq!(records.len(), 1, "{records:#?}");
    let spent = records[0]["payload"]["notes_spent"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "the replacement does not say what its superseded node read: {}",
                records[0]
            )
        });
    assert_eq!(spent.len(), 1, "{spent:#?}");
    assert_eq!(spent[0]["text"], json!(NOTE), "{spent:#?}");
    assert_eq!(spent[0]["reached"], json!("worker"), "{spent:#?}");
    assert!(
        records[0]["payload"].get("notes_carried").is_none(),
        "a replacement composed from the manager's own task carried a note: {}",
        records[0]
    );

    // Released so the held turns end with the journey rather than waiting out the
    // doubles' own bound on a hold.
    for gate in ["turn.go", "turn.settle", "judge.go"] {
        release(&world.fakes, gate);
    }
}

/// The other direction: a note a running turn **did** take is not also carried to
/// that node's next dispatch.
///
/// One direction alone would pass an implementation that always composes forward,
/// which is why this one exists: `persist` carries forward only what no running
/// turn took, so a note the worker has already acted on must not be re-stated to
/// the dispatch after it. The node is parked mid-flight and brought back, which is
/// the only way one node here is dispatched twice, and what is read is the prompt
/// that second dispatch was really handed.
///
/// The second node is what keeps a driver on the run while this one is parked: a
/// graph whose every node has settled has no reconciler left to pick a requeue up,
/// and a `kind: human` action does not answer for it — an unattested one settles
/// `waiting`, which the loop counts as finished with. So `keep` is an agent node
/// whose **judge** is held, which is a turn nothing in this journey releases.
#[test]
fn a_note_a_running_turn_took_is_not_carried_to_that_nodes_next_dispatch() {
    let world = World::new("note-not-carried");
    let run = "notcarried";
    // Every worker turn of *this node* is held and released on its own: the note
    // reopens the worker's turn, so the turn after the one it was offered into
    // has to be held too for the node to still be in flight when the park below
    // asks it to stop. Releasing and re-arming from here would be a race against
    // a turn that starts as soon as the last one ends.
    world.script("turn.hold-each", "Do build.");
    world.script(
        "judge.asks-again",
        "Run the check again and report what it said.",
    );
    world.script("turn.hold", "hold");
    // `keep`'s judge, held and never released, so that node is still *running*
    // when the requeue arrives however its worker turn raced `build`'s for the
    // gates above — the two share one pair, and a second node that settled would
    // take the reconciler down with it. `build` never reaches a judge at all: its
    // worker turn is held from the moment the note reopens it until the deadline
    // below reaps it.
    world.script("judge.hold", "hold");
    world.write_graphs();
    world.write_supervised_node_graph();
    let path = world.plan(
        run,
        &plan_of(run, vec![agent("build", &[]), agent("keep", &[])]),
    );
    // A deadline this journey waits *out* rather than one it waits on. The note
    // reopens the worker's turn and every turn of this node is held, so nothing
    // of the dispatch answers the cancellation's ask — the loop's own clock is
    // what ends it, and being reaped rather than judged is what leaves the node
    // parked for the requeue below instead of settled `done`.
    let mut launch = world.agentgraph_cmd(&["start", &path, "--detach"]);
    launch.env(CANCEL_GRACE_ENV, "1");
    world.run_on(launch, "start --detach").exited(0);
    // `build`'s own turn, for the reason the retry journey above gives.
    world.until("build's worker turn to open", |world| {
        !openings_of(world, run, "build").is_empty()
    });

    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    world
        .run_with_stdin_on(
            world.agentgraph_cmd(&["reply", run]),
            &envelope(note_op("build", "worker", NOTE, None)),
        )
        .exited(0)
        .out_has("\"state\":\"applied\"");
    releasing.join().expect("the releasing thread finishes");

    let operation = recorded(&world, run);
    assert_eq!(
        operation["reached"],
        json!("worker"),
        "the note this journey is about did not reach a running turn: {operation}"
    );

    // Parked mid-flight and brought back, which is the only way one node here is
    // dispatched twice — and where a note that was still owed would show up. The
    // park goes out while the reopened turn is still held, so it really is a park
    // of a running node; the requeue waits until that dispatch has settled,
    // because a node still in flight is one a requeue is refused for.
    world
        .run_with_stdin_on(
            world.agentgraph_cmd(&["reply", run]),
            &envelope(json!({"op": "cancel", "id": "build", "reason": "re-dispatch it"})),
        )
        .exited(0);

    world.until("the held dispatch to be reaped at its deadline", |world| {
        world
            .events_of(run, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "build")
    });
    world
        .run_with_stdin_on(
            world.agentgraph_cmd(&["reply", run]),
            &envelope(json!({"op": "requeue", "id": "build"})),
        )
        .exited(0)
        .out_has("\"state\":\"applied\"");
    // The second dispatch's own turn is held by the gates the first one consumed,
    // which is what makes the instruction below readable while it is still open
    // rather than a race against a turn that ends as soon as it starts.
    world.until("the requeued node to be dispatched again", |world| {
        dispatches_of(world, run, "build")
            .get(1)
            .is_some_and(|turns| !turns.is_empty())
    });

    let dispatched = dispatches_of(&world, run, "build");
    assert!(
        dispatched[0].iter().any(|turn| turn.contains(NOTE)),
        "the note never reached a turn of the dispatch it was delivered into:\n{:#?}",
        dispatched[0]
    );
    assert!(
        dispatched[1].iter().all(|turn| !turn.contains(NOTE)),
        "a note a running turn had already read was carried into the dispatch after \
         it:\n{:#?}",
        dispatched[1]
    );

    // And the run's record says so, where a manager reading the node's history
    // will find it: the dispatch that was composed without the note names it as
    // **spent**, beside the receipt that named the party that read it. Without
    // this the receipt reads as success throughout, and the ruling the first
    // dispatch obeyed is invisible to the second dispatch's judge and to the
    // manager alike.
    let records = dispatch_records_of(&world, run, "build");
    assert_eq!(records.len(), 2, "{records:#?}");
    assert!(
        records[0]["payload"].get("notes_spent").is_none(),
        "the first dispatch spent a note nothing had delivered yet: {}",
        records[0]
    );
    let spent = records[1]["payload"]["notes_spent"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "the requeued dispatch does not say what it spent: {}",
                records[1]
            )
        });
    assert_eq!(spent.len(), 1, "{spent:#?}");
    assert_eq!(spent[0]["text"], json!(NOTE), "{spent:#?}");
    assert_eq!(spent[0]["reached"], json!("worker"), "{spent:#?}");
    assert_eq!(spent[0]["addressee"], json!("worker"), "{spent:#?}");

    // This is a conversation **interrupted between the two presentations**: the
    // worker's turn reopened on the note and was reaped before the judge was
    // ever consulted. The record says exactly that — the worker was shown it,
    // the judge was not — and nothing in it claims otherwise: not the delivery,
    // which routed the note and confirmed nobody, and not a presentation the
    // stream never showed.
    let delivery = recorded(&world, run);
    assert!(
        delivery.get("shown_to").is_none(),
        "the delivery record asserted a presentation the cancelled conversation never \
         made: {delivery}"
    );
    assert_eq!(delivery["routed_to"], json!(["worker", "supervisor"]));
    let shown = presentations_of(&world, run, "build");
    assert_eq!(
        shown
            .iter()
            .map(|event| event["payload"]["party"].clone())
            .collect::<Vec<_>>(),
        vec![json!("worker")],
        "a conversation reaped before its judge was consulted recorded a presentation to \
         the judge, or none to the worker whose turn opened on the note:\n{shown:#?}"
    );

    // Released so the held turns end with the journey rather than waiting out the
    // doubles' own bound on a hold.
    for gate in ["turn.go", "turn.settle", "judge.go"] {
        release(&world.fakes, gate);
    }
}

/// A note a dispatch's conversation read **survives the engine's own re-dispatch
/// of the node**: the attempt that continues a preserved branch is composed with
/// it, both parties of the new conversation read it, and the run's record names
/// it as carried.
///
/// The regression: a re-dispatch recomposes the task from the plan, which a
/// note delivered mid-dispatch is not part of, so a ruling the worker obeyed
/// was absent from the conversation whose judge ruled on that compliance —
/// while the manager's receipt still read `worker`.
///
/// The publication fails **checks-failed**, which is preserving: the host reports
/// a required check red, the branch is handed back, and the node is asked again
/// on it. What is read is the second dispatch's own prompt — the task it was
/// composed with, which is the first message of the transcript its judge is
/// handed — and the record of that dispatch. The budget is two attempts, so the
/// run settles on the second without a host that has to be flipped green.
#[test]
fn a_note_a_dispatch_read_survives_the_engines_own_redispatch_of_the_node() {
    let world = World::new("note-redispatch").with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "2");
    let run = "redispatch";
    // `change-auto` watches the host's checks to their conclusion, which is
    // where a red one is observed at all; the worker leaves a diff behind, so
    // there is a publication to fail.
    world.repository("change-auto", &[]);
    world.script("harness.work", "the worker wrote this\n");
    world.script("gh.checks", "llmlint completed failure required");
    // The planner's own carried note, which the node is launched with: it rode
    // in as the first dispatch's context, and the continuation is the engine's
    // — not a dispatch anybody asked for — so it is owed there too.
    let mut service = lifecycle("service", &[]);
    service["context"] = json!(PLANNER_CONTEXT);
    held_conversation(&world, run, vec![service]);

    // The ruling, addressed to both parties, delivered into the held worker
    // turn — so it is read by the worker and, with the worker's response, by the
    // judge of the first conversation.
    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    let replied = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &envelope(note_op("service", "both", NOTE, Some(CRITERION))),
    );
    releasing.join().expect("the releasing thread finishes");
    replied.exited(0).out_has("\"state\":\"applied\"");
    let operation = recorded(&world, run);
    assert_eq!(operation["reached"], json!("worker"), "{operation}");

    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });

    // The node was dispatched again by the engine, on the failure the host
    // reported, and the second dispatch's record names the note it was composed
    // with — the note itself, and no presentation it has not yet made.
    let records = dispatch_records_of(&world, run, "service");
    assert_eq!(
        records.len(),
        2,
        "the node was not dispatched exactly twice:\n{records:#?}"
    );
    let again = &records[1];
    assert_eq!(again["payload"]["attempt"], json!(2), "{again}");
    assert!(
        again["payload"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with("checks-failed:")),
        "the re-dispatch is not the engine's own continuation: {again}"
    );
    let carried = again["payload"]["notes_carried"]
        .as_array()
        .unwrap_or_else(|| panic!("the re-dispatch does not name the notes it carries: {again}"));
    assert_eq!(carried.len(), 1, "{carried:#?}");
    assert_eq!(carried[0]["text"], json!(NOTE), "{carried:#?}");
    assert_eq!(carried[0]["criterion"], json!(CRITERION), "{carried:#?}");
    assert_eq!(carried[0]["addressee"], json!("both"), "{carried:#?}");
    assert_eq!(carried[0]["reached"], json!("worker"), "{carried:#?}");
    assert!(
        again["payload"].get("notes_spent").is_none(),
        "a dispatch composed with the note reported it spent: {again}"
    );
    // What the record says about the second conversation's presentations is
    // only what its stream showed — the opening worker turn that carried the
    // note as the task, and the judge's turn that answered it — after the ones
    // the first conversation made; the composition itself claims none.
    let journal = world.journal(run);
    let redispatched_at = journal
        .iter()
        .position(|event| {
            event["kind"] == "node-dispatched"
                && event["labels"]["node"] == "service"
                && event["payload"]["attempt"] == json!(2)
        })
        .expect("the re-dispatch is in the store");
    let after: Vec<Value> = journal[redispatched_at..]
        .iter()
        .filter(|event| event["kind"] == "note-shown" && event["labels"]["node"] == "service")
        .map(|event| event["payload"]["party"].clone())
        .collect();
    assert_eq!(
        after,
        vec![json!("worker"), json!("supervisor")],
        "the second conversation's presentations are not the worker's and then the \
         judge's:\n{:#?}",
        presentations_of(&world, run, "service")
    );
    assert_eq!(
        presentations_of(&world, run, "service").len(),
        4,
        "each conversation shows the note to each party once:\n{:#?}",
        presentations_of(&world, run, "service")
    );

    // The worker of the second conversation was handed it, in the task it opened
    // on — composed after the first conversation ended, so there is no other way
    // the note could be in it — as a ruling with the amendment's authority, and
    // with the criterion it bound.
    let dispatched = dispatches_of(&world, run, "service");
    assert_eq!(dispatched.len(), 2, "{dispatched:#?}");
    let opening = dispatched[1]
        .first()
        .unwrap_or_else(|| panic!("the second dispatch opened no turn:\n{dispatched:#?}"));
    for said in [
        "## Manager notes\nWhere this section and the operational notes below disagree, this \
         section wins.\n\nThe manager delivered these notes to this node during an earlier \
         dispatch of it, and this dispatch continues that node's work: each stands here exactly \
         as it stood there, for the worker and for the supervisor alike. A note that states a \
         criterion is part of the bar this node is judged against.\n\n1. Addressed to both parties",
        NOTE,
        CRITERION,
        // And the diagnosis is still there beside it: carrying the note did not
        // cost the worker the failure it was re-dispatched over — nor the
        // planner's own note the node was launched with, which the first
        // attempt was given and the continuation keeps above the diagnosis.
        "## Planner context",
        PLANNER_CONTEXT,
        "checks-failed",
    ] {
        assert!(
            opening.contains(said),
            "the re-dispatch's task lacks {said:?}:\n{opening}"
        );
    }
    assert!(
        !dispatched[0][0].contains("## Manager notes"),
        "the first dispatch was composed with a note that had not been delivered yet:\n{}",
        dispatched[0][0]
    );
    assert!(
        dispatched[0][0].contains(PLANNER_CONTEXT) && !dispatched[0][0].contains("checks-failed"),
        "the first dispatch was not composed with the planner's note alone:\n{}",
        dispatched[0][0]
    );
    let planner_note_at = opening
        .find(PLANNER_CONTEXT)
        .expect("the planner's note is in the continuation");
    let diagnosis_at = opening
        .find("The previous attempt's publication failed")
        .expect("the diagnosis is in the continuation");
    assert!(
        planner_note_at < diagnosis_at,
        "the planner's note does not lead the continuation's context:\n{opening}"
    );
    // The second conversation's opening is the composed task and nothing else,
    // which is what the stream says about it too.
    let openings = openings_of(&world, run, "service");
    let second_opening = openings
        .iter()
        .find(|turn| turn.instruction.contains("## Manager notes"))
        .expect("the second dispatch's opening turn is in the store");
    assert_eq!(second_opening.turn, 1, "{second_opening:?}");
    assert_eq!(
        second_opening.origin,
        Some(Origin::Task),
        "{second_opening:?}"
    );

    // And the judge that rendered the second verdict read it: the ruling the
    // worker obeyed reached the party that rules on the worker, inside the task
    // it judged against rather than as a delivery it never received.
    let judge = judged(&world);
    assert!(
        judge
            .iter()
            .any(|prompt| prompt.contains("## Manager notes") && prompt.contains(NOTE)),
        "no judge decision of the second dispatch was handed the note the first \
         dispatch obeyed:\n{judge:#?}"
    );
}

/// A note whose live delivery is really **attempted and refused** is carried,
/// rather than refused with it.
///
/// The other way a note reaches no running turn, and the one only the conversation
/// can answer: `a_note_no_turn_took_is_carried_to_the_nodes_next_dispatch_and_named_as_carried`
/// drives a node that has never reported a member, so nothing is asked at all.
/// Here a member was reported and *is* asked, and the ask fails. `persist` treats
/// the two the same on purpose — what it promises is about the note reaching a
/// running turn, not about why it did not — and a journey against an absent
/// conversation cannot show that half.
///
/// The failure is the one this suite can produce on demand: a run composing the
/// `oneagentgraph` **executable**, whose command line has no verb for the note
/// seam, which is what `world.cmd` rather than `world.agentgraph_cmd` selects.
/// `a_note_is_refused_when_this_run_composes_the_sibling_as_an_executable` drives
/// the same failed ask against a node that has settled `done` and reads the
/// refusal; this one drives it against a node that has **not**, so there is a
/// dispatch ahead of it for `persist` to carry the note to, and the same failure
/// is an answer rather than a refusal.
#[test]
fn a_note_a_failed_delivery_attempt_is_carried_rather_than_refused_with_it() {
    let world = World::new("note-attempted");
    let run = "attempted";
    // The node's turn fails, so its dispatch settles and the node settles
    // `failed` — which, unlike `done`, still has a dispatch ahead of it.
    world.script("harness.fail", "");
    supervised_run(&world, run, vec![agent("build", &[])]);
    world.until("the run's driver to release it", |world| {
        !world.run_file(run, "owner.lock").exists()
    });

    world
        .run_with_stdin_on(
            world.cmd(&["reply", run]),
            &envelope(note_op("build", "worker", NOTE, None)),
        )
        .exited(0)
        .out_has("\"state\":\"applied\"");
    let operation = recorded(&world, run);
    assert_eq!(
        operation["reached"],
        json!("carried"),
        "a note whose delivery was attempted and failed was not carried: {operation}"
    );
}

/// A note offered while the **judge** is the party taking a turn re-takes that
/// decision with the note in hand, and rides the response back to the worker.
///
/// The other half of "whoever is live", and it is a different code path in the
/// conversation: the worker's turn is reopened, the judge's is *re-decided*. What
/// makes this a claim about the judge and not about a timer is that the supervisor
/// turn is held open until the note is really in the run's queue for it.
///
/// The judge sends the agent back twice here, so the decision this note re-takes
/// is not the conversation's last — which is what leaves a next worker turn for
/// the note to ride to. A re-taken decision that *completed* is the other
/// disposition, driven by
/// `a_note_the_judge_passed_the_work_with_is_recorded_as_judged_with`.
#[test]
fn a_note_reaching_the_live_judge_re_takes_its_decision_and_rides_it_to_the_worker() {
    let world = World::new("note-judge");
    let run = "judge";
    world.script(
        "judge.asks-again",
        "Run the check again and report what it said.",
    );
    world.script("judge.asks-again-times", "2");
    held_judge(&world, run, vec![agent("build", &[])]);

    let releasing = release_when_the_note_is_queued(&world, run, &["judge.go"]);
    let replied = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &envelope(note_op("build", "supervisor", NOTE, None)),
    );
    releasing.join().expect("the releasing thread finishes");
    replied.exited(0).out_has("\"state\":\"applied\"");

    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });

    // The party it reached, which is the answer no reader of the transcript can
    // work out for itself.
    let operation = recorded(&world, run);
    assert_eq!(operation["addressee"], json!("supervisor"), "{operation}");
    assert_eq!(
        operation["reached"],
        json!("supervisor"),
        "the note did not reach the party whose turn was live: {operation}"
    );
    // The judge is confirmed at delivery — its decision was re-taken with the
    // note in hand before the acknowledgement was given — and the worker is
    // only routed to, until the stream shows the turn that rode the decision.
    assert_eq!(operation["shown_to"], json!(["supervisor"]), "{operation}");
    assert_eq!(operation["routed_to"], json!(["worker"]), "{operation}");
    let shown = presentations_of(&world, run, "build");
    assert_eq!(
        shown
            .iter()
            .map(|event| event["payload"]["party"].clone())
            .collect::<Vec<_>>(),
        vec![json!("worker")],
        "the worker's presentation of a note that rode the judge's decision was not \
         recorded once and alone:\n{shown:#?}"
    );

    // The judge read it as its own, addressed to it...
    let judge = judged(&world);
    assert!(
        judge
            .iter()
            .any(|prompt| prompt.contains("delivered to YOU, the supervisor")
                && prompt.contains(NOTE)),
        "no judge decision was handed the note as its own:\n{judge:#?}"
    );

    // ...and the worker received it *with* that response, framed as the other
    // party's rather than as an instruction of its own.
    let worker = worked(&world);
    assert!(
        worker.iter().any(|prompt| prompt
            .contains("delivered to the SUPERVISOR, addressed to it and not to you")
            && prompt.contains(NOTE)),
        "the note never rode the judge's response to the worker:\n{worker:#?}"
    );
}

/// A note reaching a live judge whose re-taken decision is completion: the work
/// was passed with the note in hand, and the run records exactly that.
///
/// Not a failure and not a non-delivery — the note was read, by the party that
/// decided — but there was no next worker turn to deliver it into, and a run that
/// recorded it as an ordinary delivery to the worker would be saying something
/// false about who acted on it. The note here is addressed to **both** parties,
/// which is the addressing the other journeys do not drive.
#[test]
fn a_note_the_judge_passed_the_work_with_is_recorded_as_judged_with() {
    let world = World::new("note-passed");
    let run = "passed";
    held_judge(&world, run, vec![agent("build", &[])]);

    let releasing = release_when_the_note_is_queued(&world, run, &["judge.go"]);
    let replied = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &envelope(note_op("build", "both", NOTE, None)),
    );
    releasing.join().expect("the releasing thread finishes");
    replied.exited(0).out_has("\"state\":\"applied\"");

    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });

    let operation = recorded(&world, run);
    assert_eq!(operation["addressee"], json!("both"), "{operation}");
    assert_eq!(
        operation["reached"],
        json!("judged-with"),
        "the run does not say the work was passed with the note in hand: {operation}"
    );
    assert!(
        operation["completion_reason"].is_string(),
        "the record does not carry the reason the work was passed: {operation}"
    );
    // The one disposition a party read alone: the decision was completion, so no
    // worker turn followed for the note to ride to — and the record says so
    // rather than leaving a note addressed to both to read as reaching both.
    assert_eq!(
        operation["shown_to"],
        json!(["supervisor"]),
        "a note only the judge read is recorded as shown to the worker too: {operation}"
    );
    assert!(
        operation.get("routed_to").is_none(),
        "a note the judge completed with was routed onward: {operation}"
    );
    assert!(
        presentations_of(&world, run, "build").is_empty(),
        "a presentation was recorded for a note that reached no turn after the decision"
    );

    // And the judge really was told it, under the addressing it was sent with.
    let judge = judged(&world);
    assert!(
        judge
            .iter()
            .any(|prompt| prompt.contains("(addressed to both)") && prompt.contains(NOTE)),
        "no judge decision was handed the note addressed to both parties:\n{judge:#?}"
    );
}

/// A note is refused rather than half-delivered when this run composes the
/// `oneagentgraph` **executable** instead of the library.
///
/// The seam the sibling publishes is a library call and its command line has no
/// verb for it, so an operator who pinned an executable is told that — rather than
/// quietly served by the interrupt that reaches one party, which is the whole
/// defect this op exists to end. The refusal names the override, so the operator
/// knows which of its own decisions to change.
///
/// `world.cmd` rather than `world.agentgraph_cmd`: the difference between the two
/// is exactly this override, which every other journey here removes.
#[test]
fn a_note_is_refused_when_this_run_composes_the_sibling_as_an_executable() {
    let world = World::new("note-pinned");
    let run = "pinned";
    held_conversation(&world, run, vec![agent("build", &[])]);
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    world.until("the run's driver to release it", |world| {
        !world.run_file(run, "owner.lock").exists()
    });

    let refused = world.run_with_stdin_on(
        world.cmd(&["reply", run]),
        &envelope(note_op("build", "worker", NOTE, None)),
    );
    refused
        .exited(2)
        .err_has("was not delivered")
        .err_has("ONEPIPELINE_ONEAGENTGRAPH_BIN")
        .err_has("no verb");
}

/// An observer is refused `note` by name, and nothing durable is queued from the
/// attempt.
///
/// A note may carry a criterion, and a delivered one enters the bar the node's
/// judge decides against — which is the decision `amend` makes, taken against the
/// conversation running now, and the one the monitor's own persona reserves to the
/// planner. So the refusal is the same shape as `amend`'s: the op by name, and
/// what to do instead.
///
/// What makes this worth driving end to end rather than asserting on the allowlist
/// is the second half. The refusal has to happen *before* the envelope becomes
/// durable, because a note that was refused on the way out but committed on the
/// way in would still be offered to the live conversation by the reconciler — the
/// operator would read a refusal and the worker would read the note. So the run's
/// own queue is asked, and it is asked while the conversation is still live and
/// the reconciler is still passing over it.
#[test]
fn an_undeclared_monitor_is_refused_before_its_note_is_queued() {
    let world = World::new("note-monitor");
    let run = "notemonitor";
    held_conversation(&world, run, vec![agent("build", &[])]);

    let refused = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &json!({
            "version": 2,
            "author": "monitor",
            "commands": [note_op("build", "worker", NOTE, Some(CRITERION))],
        })
        .to_string(),
    );
    refused
        .exited(REFUSED)
        .err_has("the envelope's author `monitor` is not declared")
        .err_has("the declared authors are: planner");

    // Nothing of it is durable: the queue the reconciler reads carries no note, so
    // there is nothing for it to offer the turn that is still open — and the run
    // recorded neither a commit nor a rejection, because the refusal was taken
    // where the envelope arrived rather than after it became a record something
    // downstream had to answer.
    let queue = world.run_file(run, "channel/commands.jsonl");
    assert!(
        !a_note_is_queued(&queue),
        "a note the monitor was refused was queued anyway: {}",
        std::fs::read_to_string(&queue).unwrap_or_default()
    );
    for kind in ["edit-committed", "edit-rejected"] {
        assert!(
            world.events_of(run, kind).is_empty(),
            "the run recorded a `{kind}` for an envelope it refused at the boundary"
        );
    }

    // And the same note from the author that may send it goes through against the
    // same live node, so what was refused is the authority rather than the author.
    let releasing = release_when_the_note_is_queued(&world, run, &["turn.go", "turn.settle"]);
    let replied = world.run_with_stdin_on(
        world.agentgraph_cmd(&["reply", run]),
        &envelope(note_op("build", "worker", NOTE, Some(CRITERION))),
    );
    releasing.join().expect("the releasing thread finishes");
    replied.exited(0);

    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });
    assert_eq!(recorded(&world, run)["reached"], json!("worker"));
}

/// The shapes of note the envelope cannot carry, each refused where it arrives,
/// and nothing of any of them left durable.
///
/// The rules are the seam's own newtypes rather than checks this crate keeps:
/// `addressee` is required and closed, a note's text refuses a blank, and a
/// criterion is refused by the rules the judging side already applies to authored
/// criteria — a version literal among them, because a release cut between the note
/// being written and the work being judged makes finished work fail against it.
///
/// Three of them are not the seam's: the removed `context` op and the removed
/// `auto` delivery value are refused because this envelope declares neither, which
/// is the intended failure for a caller that has not moved; and `deliver: next`
/// with `persist: false` is refused because those two fields decide between them
/// that the note reaches nobody, before the run is reached at all.
///
/// What only an end-to-end journey can show is that every one of them holds at the
/// **wire**, on the envelope a manager really sends, rather than on a constructor a
/// test can call. A malformed note that parsed and became durable would still be
/// offered to the live conversation by the reconciler: the manager would read a
/// refusal and the worker would read the note. So the conversation is held open
/// across all of them, and the queue is asked while the reconciler is still passing
/// over it.
#[test]
fn a_note_the_envelope_cannot_carry_is_refused_at_the_wire_and_nothing_is_queued() {
    let world = World::new("note-boundary");
    let run = "noteboundary";
    held_conversation(&world, run, vec![agent("build", &[])]);

    // Each one, with the words its refusal owes a manager: which field, and what
    // about it. A bare `missing field` would say a note was rejected; these say
    // what to send instead.
    let refused = [
        // The op that was collapsed into this one. Refused by name, which is the
        // intended failure for a caller that has not moved: the envelope refuses
        // what it does not declare rather than quietly dropping it.
        (
            json!({"op": "context", "id": "build", "note": NOTE}),
            "unknown variant `context`",
        ),
        // And the delivery value that went with it, for the same reason: `auto`
        // named a combination of both axes, and its meaning is `deliver: live`
        // with `persist: true`.
        (
            json!({"op": "note", "id": "build", "addressee": "worker", "text": NOTE,
                   "deliver": "auto"}),
            "unknown variant `auto`",
        ),
        // The envelope-time half of the one reach-nobody rule: these two fields
        // decide it between them, so it never reaches a run at all.
        (
            note_op_with("build", NOTE, "next", false),
            "reaches nobody whatever the run does",
        ),
        (
            json!({"op": "note", "id": "build", "addressee": "worker", "text": "   \n"}),
            "this one was blank",
        ),
        (
            json!({"op": "note", "id": "build", "addressee": "sponsor", "text": NOTE}),
            "unknown variant `sponsor`",
        ),
        (
            json!({"op": "note", "id": "build", "text": NOTE}),
            "missing field `addressee`",
        ),
        (
            note_op(
                "build",
                "worker",
                NOTE,
                Some("the tree pins oneagentgraph 0.3.15"),
            ),
            "names a version literal",
        ),
    ];
    for (op, named) in refused {
        world
            .run_with_stdin_on(world.agentgraph_cmd(&["reply", run]), &envelope(op))
            .exited(REFUSED)
            .err_has(named);
    }

    // Nothing of any of them is durable, asked while the held turn is still open:
    // the queue the reconciler reads carries no note, and the run recorded neither
    // a commit nor a rejection, because each refusal was taken where the envelope
    // arrived rather than after it became a record something downstream had to
    // answer.
    let queue = world.run_file(run, "channel/commands.jsonl");
    assert!(
        !a_note_is_queued(&queue),
        "a note the envelope refused was queued anyway: {}",
        std::fs::read_to_string(&queue).unwrap_or_default()
    );
    for kind in ["edit-committed", "edit-rejected"] {
        assert!(
            world.events_of(run, kind).is_empty(),
            "the run recorded a `{kind}` for an envelope it refused at the wire"
        );
    }

    // And the conversation none of them reached runs to its own end, so what was
    // refused is the envelope rather than the node it named.
    release(&world.fakes, "turn.go");
    release(&world.fakes, "turn.settle");
    world.until("the run to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });
    assert!(
        worked(&world).iter().all(|prompt| !prompt.contains(NOTE)),
        "a note the wire refused was handed to the worker anyway"
    );
}
