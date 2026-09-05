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

use onepipeline::channel::Command;
use onepipeline::channel::Deliver;
use onepipeline::note::{deliver, deliver_with, Addressee, Delivered, Note, Reached};
use onepipeline::views::RunPaths;
use serde_json::{json, Value};

use harness::{agent, plan_of, World, CANCEL_GRACE_ENV, REFUSED};

/// The correction a manager sends at the moment it matters: while the worker is
/// still working, and before its judge has ruled on anything.
const NOTE: &str = "the reviewer asked for a smaller diff; stop editing src/old.rs";

/// A note that changes what the finished tree must contain, rather than only how
/// the worker should go about it.
const CRITERION: &str = "`version.txt` holds `v: 2`";

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
    // reader of the transcript can work out for itself.
    let operation = recorded(&world, run);
    assert_eq!(operation["node"], json!("build"), "{operation}");
    assert_eq!(operation["addressee"], json!("worker"), "{operation}");
    assert_eq!(operation["text"], json!(NOTE), "{operation}");
    assert_eq!(
        operation["reached"],
        json!("worker"),
        "the note reached a party the note was not delivered to first: {operation}"
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
    world.until("the worker's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
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

    // Released so the held turns end with the journey rather than waiting out the
    // doubles' own bound on a hold.
    for gate in ["turn.go", "turn.settle", "judge.go"] {
        release(&world.fakes, gate);
    }
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
fn a_monitor_is_refused_note_by_name_and_nothing_of_it_is_queued() {
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
        .err_has("'note' is not an op the monitor may issue")
        .err_has("the planner's decision rather than an observation")
        .err_has("Surface it to the planner instead");

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
