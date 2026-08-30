//! Where a manager's note lands: in the live conversation, in both parties' hands,
//! and in the bar its judge decides against — or nowhere, said out loud.
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

use std::path::Path;
use std::time::{Duration, Instant};

use onepipeline::channel::Command;
use onepipeline::note::{deliver, Addressee, Delivered, Note, Reached};
use onepipeline::views::RunPaths;
use serde_json::{json, Value};

use crate::harness::{agent, plan_of, World};

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
    json!({"version": 1, "commands": [command]}).to_string()
}

fn note_op(node: &str, addressee: &str, text: &str, criterion: Option<&str>) -> Value {
    let mut op = json!({"op": "note", "id": node, "addressee": addressee, "text": text});
    if let Some(criterion) = criterion {
        op["criterion"] = json!(criterion);
    }
    op
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
        .err_has("build");

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
    // so there is no conversation of its own for a note to be handed to, and
    // saying so is the whole point.
    let refused = deliver(&paths, "later", &Note::to(Addressee::Worker, NOTE))
        .expect_err("a node with no conversation takes no note");
    let said = refused.to_string();
    assert!(
        said.contains("later") && said.contains("no conversation"),
        "the refusal does not name the node or say what was missing: {said}"
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
