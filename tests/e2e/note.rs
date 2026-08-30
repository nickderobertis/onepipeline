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

/// Start a supervised run whose worker turn is held open, and wait until it is.
///
/// The judge asks once and then completes, which is the shortest conversation with
/// two decisions in it — so a note delivered into the held worker turn is in the
/// judge's hands for the *first* of them, and "before the verdict" is a claim about
/// a verdict that really came later.
fn held_conversation(world: &World, run: &str, nodes: Vec<Value>) {
    world.write_graphs();
    world.write_supervised_node_graph();
    world.script("judge.asks-again", "Run the check again and report what it said.");
    world.script("turn.hold", "hold");
    let path = world.plan(run, &plan_of(run, nodes));
    world
        .run_on_agentgraph(&["start", &path, "--detach"])
        .exited(0);
    world.until("the worker's turn to open", |world| {
        !world.events_of(run, "turn-started").is_empty()
    });
}

/// Release the held worker turn once the note is really on its way to it.
///
/// The wait is on the run's own durable command queue rather than on a clock: the
/// note is in it before the reconciler can offer it, and the worker's turn cannot
/// end until this releases it — so the note reaches a turn that is still live
/// rather than whichever party happened to be speaking when a timer went off. The
/// short pause after it is the reconciler's own pass, which is the one step with
/// nothing durable to watch for.
fn release_when_the_note_is_queued(world: &World, run: &str) -> std::thread::JoinHandle<()> {
    let queue = world.run_file(run, "channel/commands.jsonl");
    let fakes = world.fakes.clone();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            if std::fs::read_to_string(&queue)
                .unwrap_or_default()
                .contains("\"op\":\"note\"")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_secs(2));
        release(&fakes, "turn.go");
        release(&fakes, "turn.settle");
    })
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
        panic!("the run recorded {} committed notes, not one", committed.len());
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

    let releasing = release_when_the_note_is_queued(&world, run);
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

    let releasing = release_when_the_note_is_queued(&world, run);
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
        panic!("the run recorded {} rejected notes, not one", rejected.len());
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

    // First the refusal, while the held node keeps the run's own reconciler alive
    // to answer it: `later` has not been dispatched, so there is no conversation of
    // its own for a note to be handed to, and saying so is the whole point.
    let refused = deliver(&paths, "later", &Note::to(Addressee::Worker, NOTE))
        .expect_err("a node with no conversation takes no note");
    let said = refused.to_string();
    assert!(
        said.contains("later") && said.contains("no conversation"),
        "the refusal does not name the node or say what was missing: {said}"
    );

    // Then the delivery, into the conversation that is live — answered with which
    // party took it, which is what a caller has no second source for.
    let releasing = release_when_the_note_is_queued(&world, run);
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
