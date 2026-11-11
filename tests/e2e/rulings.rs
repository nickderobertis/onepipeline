//! Whose words a ruling is, recorded on what the manager sends and rendered into
//! everything a dispatch reads — and a manager's ruling declined with a reason.
//!
//! Every journey here drives the compiled binary: `reply` with and without
//! `--as user`, the dispatches the run then makes, `decline-ruling`, and the
//! manager's own reads of the channel (`next`, `channel queue`). What each
//! dispatch was handed is read off what the sibling's double really received —
//! the `--task` of the `oneagentgraph run` this crate issued, or the prompts the
//! real `oneagentgraph` handed each party of a supervised conversation — never
//! off anything this crate wrote down about it.
//!
//! Each kind of ruling is driven in both directions: one sent with `--as user`,
//! which a dispatch reads as binding and under no reference, and one sent with no
//! `--as`, which a dispatch reads as the manager's, declinable by the reference
//! rendered beside it — the reference `decline-ruling` then takes.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` (or, for the
// supervised journeys, the harness under the real `oneagentgraph`) at its subprocess
// boundary and nothing inside the crate under test, which is driven as a real compiled
// binary over real run directories. `harness.rs` carries the same suppression and the full
// rationale.

// llmlint: ignore-file[tests_mirror_real_usage] what a dispatch was asked to do is read off
// the `--task` of the `oneagentgraph run` this crate really issued, and off the prompts the
// real sibling handed each party, for the reason `amend.rs` gives: that argv and those
// prompts are the product boundary the attribution crosses, and no view renders a held
// dispatch's prose. Everything a manager does — reply, read the channel, see a declined
// ruling — goes through the CLI.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{agent, end_driver, lifecycle, plan_of, World, REFUSED};
use onepipeline::plan::{
    AMENDMENT_HEADING, AMENDMENT_PRECEDENCE, PLANNER_CONTEXT_HEADING, USER_AMENDMENT_PRECEDENCE,
};

const USER_RULING: &str = "Leave the four comment lines as they are; the user wants them.";
const MANAGER_RULING: &str = "Keep the diff under fifty lines.";

/// The opening of the attribution a user's ruling is rendered under.
const BINDS: &str = "Ruling from the user, relayed by the manager: it binds.";

/// The opening of the attribution a manager's ruling is rendered under, naming
/// its reference.
fn declinable(reference: &str) -> String {
    format!(
        "Ruling from the manager (ruling reference `{reference}`): a strong default you may \
         decline with your reason. To decline it, run `onepipeline decline-ruling \
         \"$ONEPIPELINE_RUN_ID\" --ruling {reference}`"
    )
}

fn envelope(commands: Value) -> String {
    json!({"version": 4, "commands": commands}).to_string()
}

/// `reply RUN`, as the user's words when `user` is set and with no `--as`
/// otherwise, answering the receipt's envelope id.
fn reply(world: &World, run: &str, body: &str, user: bool) -> u64 {
    let mut args = vec!["reply", run];
    if user {
        args.extend(["--as", "user"]);
    }
    let replied = world.run_with_stdin(&args, body);
    replied.exited(0);
    replied.json()["reply"]
        .as_u64()
        .unwrap_or_else(|| panic!("the receipt names no envelope: {}", replied.stdout))
}

/// Every `--task` this crate handed a dispatch of `node`, in dispatch order.
fn tasks_dispatched(world: &World, node: &str) -> Vec<String> {
    let mine = format!("## What\nDo {node}.");
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneagentgraph")
        .filter_map(|call| {
            let args = call["args"].as_array()?.clone();
            let at = args.iter().position(|arg| arg == "--task")?;
            args.get(at + 1)?.as_str().map(str::to_owned)
        })
        .filter(|task| task.starts_with(&mine))
        .collect()
}

/// A run whose `slow` node is held running, beside nodes waiting on it.
fn held(world: &World, name: &str, waiting: &[&str]) -> String {
    world.script("slow.turn-open", "");
    world.script("slow.wait", "hold");
    let mut nodes = vec![agent("slow", &[])];
    nodes.extend(waiting.iter().map(|node| agent(node, &["slow"])));
    let path = world.plan(name, &plan_of(name, nodes));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the held node to be running", |world| {
        world
            .run(&["status", name])
            .stdout
            .contains("slow: running")
    });
    name.to_string()
}

/// The run's channel as `onepipeline channel queue` reports it: every surface.
fn surfaces(world: &World, run: &str) -> Vec<Value> {
    let queue = world.run(&["channel", "queue", run]);
    queue.exited(0);
    queue.json()["surfaces"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// How many records the run's surfaces queue holds, read off its log.
fn surface_records(world: &World, run: &str) -> usize {
    std::fs::read_to_string(world.run_file(run, "channel/surfaces.jsonl"))
        .unwrap_or_default()
        .lines()
        .count()
}

/// `onepipeline decline-ruling RUN --ruling REF` with `reason` on stdin.
fn decline(world: &World, run: &str, reference: &str, reason: &str) -> crate::harness::Run {
    world.run_with_stdin(&["decline-ruling", run, "--ruling", reference], reason)
}

/// The `declined-ruling` surface `next` hands over, read the way a manager
/// reads the channel.
fn declined_from_next(world: &World, run: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let next = world.run(&["next", run]);
        next.exited(0);
        let surface = next.json()["surface"].clone();
        if surface["kind"] == "declined-ruling" {
            return surface;
        }
        assert!(
            Instant::now() < deadline,
            "`next` never handed over the declined ruling on {run}"
        );
        onepipeline_testfakes::profile::thread::sleep(Duration::from_millis(50));
    }
}

/// Decline the manager's ruling `reference` and hold the one surface it raises
/// to every fact the test sent: what `next` hands over and what the queue holds
/// carry the same typed record, field for field, and the message names them.
fn declined_as_sent(world: &World, run: &str, reference: &str, sent: &Value) {
    let reason = "The user asked for the opposite in the plan's own words.";
    let before = surface_records(world, run);
    let declined = decline(world, run, reference, reason);
    declined.exited(0).out_has("\"state\":\"queued\"");
    let expected = json!({
        "node": sent["node"],
        "ruling": reference,
        "provenance": "manager",
        "kind": sent["kind"],
        "reply": sent["reply"],
        "text": sent["text"],
        "reason": reason,
    });
    let queued: Vec<Value> = surfaces(world, run)
        .into_iter()
        .filter(|surface| surface["kind"] == "declined-ruling")
        .collect();
    let [one] = &queued[..] else {
        panic!("the decline raised {} surfaces, not one", queued.len());
    };
    assert_eq!(one["declined_ruling"], expected, "{one}");
    assert_eq!(one["blocking"], json!(false), "{one}");
    assert_eq!(one["workstream"], sent["node"], "{one}");
    let message = one["message"].as_str().expect("a message");
    for fact in [
        sent["node"].as_str().expect("a node"),
        reference,
        sent["text"].as_str().expect("a text"),
        reason,
    ] {
        assert!(
            message.contains(fact),
            "the message does not name {fact:?}: {message}"
        );
    }
    assert!(surface_records(world, run) > before);
    let next = declined_from_next(world, run);
    assert_eq!(next["declined_ruling"], expected, "{next}");
    // No surface of any other kind carries the record.
    for surface in surfaces(world, run) {
        if surface["kind"] != "declined-ruling" {
            assert!(
                surface.get("declined_ruling").is_none(),
                "a {} surface carries a declined-ruling record: {surface}",
                surface["kind"]
            );
        }
    }
}

/// A ruling that is the user's cannot be declined, and the refusal says why
/// with nothing appended to the channel.
fn user_ruling_refused(world: &World, run: &str, reference: &str) {
    let before = surface_records(world, run);
    decline(world, run, reference, "I disagree.")
        .exited(REFUSED)
        .err_has("is the user's")
        .err_has("cannot be declined");
    assert_eq!(
        surface_records(world, run),
        before,
        "a refused decline appended"
    );
}

/// The payload of the one committed record whose command is `op` and came in
/// envelope `id`.
fn committed_of(world: &World, run: &str, op: &str, id: u64) -> Value {
    world
        .journal(run)
        .into_iter()
        .filter(|event| {
            matches!(
                event["kind"].as_str(),
                Some("edit-committed" | "command-accepted")
            )
        })
        .map(|event| event["payload"].clone())
        .find(|payload| payload["command"]["op"] == op && payload["envelope"] == json!(id))
        .unwrap_or_else(|| panic!("no committed `{op}` came in envelope {id}"))
}

/// What a record says of whose words it carried: its `provenance`, which a
/// record of the manager's own leaves out and reads back as the manager's.
fn provenance_of(record: &Value) -> &str {
    record
        .get("provenance")
        .map_or("manager", |word| word.as_str().expect("a provenance word"))
}

/// An amendment names whose ruling it is in the task each dispatch is handed;
/// the manager's carries the reference `decline-ruling` takes, and the user's
/// binds and cannot be declined. The records say the same.
#[test]
fn an_amendment_names_whose_ruling_it_is_and_only_a_managers_can_be_declined() {
    let world = World::new("rulings-amend");
    let run = held(&world, "rulingsamend", &["users", "managers"]);

    let by_user = reply(
        &world,
        &run,
        &envelope(json!([{"op": "amend", "id": "users", "text": USER_RULING}])),
        true,
    );
    let by_manager = reply(
        &world,
        &run,
        &envelope(json!([{"op": "amend", "id": "managers", "text": MANAGER_RULING}])),
        false,
    );

    // The record of each: `user` where it was sent as the user's, and the
    // manager's — no key, read as `manager` — where it was sent with no `--as`.
    for (id, whose) in [(by_user, "user"), (by_manager, "manager")] {
        let committed = committed_of(&world, &run, "amend", id);
        assert_eq!(provenance_of(&committed), whose, "{committed}");
        assert_eq!(
            provenance_of(&committed["operations"][0]),
            whose,
            "{committed}"
        );
        assert_eq!(committed["operations"][0]["kind"], json!("task-amended"));
    }
    let queued: Vec<Value> =
        std::fs::read_to_string(world.run_file(&run, "channel/commands.jsonl"))
            .expect("the command queue")
            .lines()
            .map(|line| serde_json::from_str(line).expect("a queued envelope"))
            .collect();
    assert_eq!(
        queued[usize::try_from(by_user).unwrap()]["provenance"],
        json!("user")
    );
    assert!(queued[usize::try_from(by_manager).unwrap()]
        .get("provenance")
        .is_none());

    world.release("slow.go");
    world.until("both amended nodes to be dispatched", |world| {
        !tasks_dispatched(world, "users").is_empty()
            && !tasks_dispatched(world, "managers").is_empty()
    });
    let users = &tasks_dispatched(&world, "users")[0];
    assert!(
        users.contains(&format!("{AMENDMENT_HEADING}\n{BINDS}"))
            && users.contains(USER_AMENDMENT_PRECEDENCE)
            && users.contains(USER_RULING),
        "the user's amendment is not rendered as the user's binding ruling:\n{users}"
    );
    assert!(
        !users.contains("ruling reference") && !users.contains(AMENDMENT_PRECEDENCE),
        "the user's amendment names a reference or the manager:\n{users}"
    );
    let reference = format!("c{by_manager}.0");
    let managers = &tasks_dispatched(&world, "managers")[0];
    assert!(
        managers.contains(&format!("{AMENDMENT_HEADING}\n{}", declinable(&reference)))
            && managers.contains(AMENDMENT_PRECEDENCE)
            && managers.contains(MANAGER_RULING),
        "the manager's amendment is not rendered as a declinable ruling:\n{managers}"
    );

    declined_as_sent(
        &world,
        &run,
        &reference,
        &json!({"node": "managers", "kind": "amendment", "reply": by_manager, "text": MANAGER_RULING}),
    );
    user_ruling_refused(&world, &run, &format!("c{by_user}.0"));

    // A reference the run does not know, and a reason with nothing in it, are
    // refused with nothing appended.
    let before = surface_records(&world, &run);
    decline(&world, &run, "c999.0", "no")
        .exited(REFUSED)
        .err_has("names no ruling");
    decline(&world, &run, "not-a-reference", "no").exited(REFUSED);
    decline(&world, &run, &reference, "  \n")
        .exited(REFUSED)
        .err_has("carried none");
    assert_eq!(surface_records(&world, &run), before);
}

/// `decline-ruling` reads its reason from a file as well as from stdin, as
/// `surface` reads its message; a file that cannot be read is refused with
/// nothing appended.
#[test]
fn a_decline_reads_its_reason_from_a_file_and_refuses_one_it_cannot_read() {
    let world = World::new("rulings-decline-file");
    let run = held(&world, "rulingsdeclinefile", &["managers"]);
    let id = reply(
        &world,
        &run,
        &envelope(json!([{"op": "amend", "id": "managers", "text": MANAGER_RULING}])),
        false,
    );
    let reference = format!("c{id}.0");
    let before = surface_records(&world, &run);
    let missing = world.root.join("no-such-reason.md");
    world
        .run(&[
            "decline-ruling",
            &run,
            "--ruling",
            &reference,
            missing.to_str().unwrap(),
        ])
        .exited(REFUSED);
    assert_eq!(
        surface_records(&world, &run),
        before,
        "an unread reason appended"
    );

    let reason = "The diff has to touch the whole module.\nThe user said so.\n";
    let file = world.root.join("reason.md");
    std::fs::write(&file, reason).unwrap();
    world
        .run(&[
            "decline-ruling",
            &run,
            "--ruling",
            &reference,
            file.to_str().unwrap(),
        ])
        .exited(0)
        .out_has("\"state\":\"queued\"");
    let declined = surfaces(&world, &run)
        .into_iter()
        .find(|surface| surface["kind"] == "declined-ruling")
        .expect("the decline is on the channel");
    assert_eq!(declined["declined_ruling"]["reason"], json!(reason.trim()));
    assert_eq!(declined["declined_ruling"]["kind"], json!("amendment"));
    assert_eq!(declined["declined_ruling"]["ruling"], json!(reference));
}

/// The two ways `reply` is told whose words an envelope carries cannot
/// disagree: an envelope stating the manager's sent `--as user` is refused with
/// nothing queued, and a verdict sent `--as user` is recorded as the user's.
#[test]
fn an_envelope_stating_one_provenance_sent_as_the_other_is_refused_with_nothing_queued() {
    let world = World::new("rulings-conflict");
    let run = held(&world, "rulingsconflict", &["later"]);
    let queue = world.run_file(&run, "channel/commands.jsonl");
    let before = std::fs::read_to_string(&queue).unwrap_or_default();
    world
        .run_with_stdin(
            &["reply", &run, "--as", "user"],
            &json!({"version": 4, "provenance": "manager",
                "commands": [{"op": "amend", "id": "later", "text": USER_RULING}]})
            .to_string(),
        )
        .exited(REFUSED)
        .err_has("--as")
        .err_has("Nothing was queued");
    assert_eq!(std::fs::read_to_string(&queue).unwrap_or_default(), before);
    assert!(world
        .events_of(&run, "edit-committed")
        .iter()
        .all(|event| event["payload"]["command"]["op"] != "amend"));
    // An unknown word is refused by the flag itself.
    world
        .run_with_stdin(&["reply", &run, "--as", "planner"], "{}")
        .exited(2);

    // Stating the same thing twice is no conflict, and a verdict carries it to
    // `planner-replied`.
    world
        .run_with_stdin(
            &["reply", &run, "--as", "user"],
            &json!({"provenance": "user", "completion": false, "message": "carry on"}).to_string(),
        )
        .exited(0);
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({"completion": false, "message": "go"}).to_string(),
        )
        .exited(0);
    // The library's own form, against the same live run: `verbs::reply_as`
    // records the user's words, and refuses an envelope stating the other.
    let paths = onepipeline::views::RunPaths::under(&world.runs, &run);
    let amended = json!({"version": 4,
        "commands": [{"op": "amend", "id": "later", "text": USER_RULING}]})
    .to_string();
    let conflict = json!({"version": 4, "provenance": "manager",
        "commands": [{"op": "amend", "id": "later", "text": USER_RULING}]})
    .to_string();
    assert!(onepipeline::verbs::reply_as(
        &paths,
        None,
        &conflict,
        onepipeline::provenance::Provenance::User
    )
    .is_err());
    let receipt = onepipeline::verbs::reply_as(
        &paths,
        None,
        &amended,
        onepipeline::provenance::Provenance::User,
    )
    .expect("the library takes the user's amendment");
    world.until("the library's amendment to commit", |world| {
        world
            .events_of(&run, "edit-committed")
            .iter()
            .any(|event| event["payload"]["envelope"] == json!(receipt.reply()))
    });
    let committed = committed_of(&world, &run, "amend", receipt.reply());
    assert_eq!(provenance_of(&committed), "user", "{committed}");

    let replied: Vec<String> = world
        .events_of(&run, "planner-replied")
        .iter()
        .map(|event| provenance_of(&event["payload"]).to_owned())
        .collect();
    assert_eq!(replied, vec!["user", "manager"]);
}

/// A carried note sits under `## Planner context` with its attribution, and
/// keeps its attribution and reference in the node's next dispatch after the run
/// is adopted by a new driver — one that resumes its fold from the checkpoint a
/// read wrote over the note, or (`checkpoint` false) from the whole journal.
fn carried_notes_survive_adoption(name: &str, checkpoint: bool) {
    let world = World::new(name);
    let run = held(&world, name, &["users", "managers"]);
    let carried = |node: &str, text: &str| {
        envelope(
            json!([{"op": "note", "id": node, "addressee": "worker", "text": text,
            "deliver": "next"}]),
        )
    };
    let by_user = reply(&world, &run, &carried("users", USER_RULING), true);
    let by_manager = reply(&world, &run, &carried("managers", MANAGER_RULING), false);
    for (id, whose) in [(by_user, "user"), (by_manager, "manager")] {
        let committed = committed_of(&world, &run, "note", id);
        assert_eq!(provenance_of(&committed), whose, "{committed}");
        let operation = &committed["operations"][0];
        assert_eq!(operation["reached"], json!("carried"), "{operation}");
        assert_eq!(provenance_of(operation), whose, "{operation}");
    }

    // The driver goes, leaving the notes owed, and a new one adopts the run. A
    // live run's checkpoint holds back the records stamped at the journal's
    // latest instant, so something is recorded after the notes — a verdict — and
    // the run read until its checkpoint accounts for them.
    world
        .run_with_stdin(
            &["reply", &run],
            &json!({"completion": false, "message": "carry on"}).to_string(),
        )
        .exited(0);
    world.run(&["status", &run]).exited(0);
    let checkpoint_file = world.run_file(&run, "checkpoint.json");
    if checkpoint {
        let document: Value =
            serde_json::from_slice(&std::fs::read(&checkpoint_file).expect("a checkpoint"))
                .expect("the checkpoint is JSON");
        let covered = document["coverage"]["bytes"]
            .as_u64()
            .expect("its coverage");
        // Where the manager's note's record ends in the journal file: the
        // checkpoint resumes past it only if it accounts for those bytes.
        let journal = std::fs::read_to_string(world.run_file(&run, "events.jsonl"))
            .expect("the journal reads");
        let mut ends = 0u64;
        let mut found = None;
        for line in journal.split_inclusive('\n') {
            ends += line.len() as u64;
            let event: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
            if event["kind"] == "edit-committed"
                && event["payload"]["envelope"] == json!(by_manager)
            {
                found = Some(ends);
            }
        }
        let at = found.expect("the manager's note is journalled");
        let mut covered = covered;
        let deadline = Instant::now() + Duration::from_secs(60);
        while covered < at && Instant::now() < deadline {
            world.run(&["status", &run]).exited(0);
            covered = serde_json::from_slice::<Value>(
                &std::fs::read(&checkpoint_file).expect("a checkpoint"),
            )
            .expect("the checkpoint is JSON")["coverage"]["bytes"]
                .as_u64()
                .expect("its coverage");
        }
        assert!(
            covered >= at,
            "the checkpoint does not cover the carried notes, so nothing resumes from it"
        );
    } else {
        std::fs::remove_file(&checkpoint_file).expect("the checkpoint is removed");
    }
    end_driver(&world, &run);
    world.run(&["adopt", &run, "--detach"]).exited(0);
    world.release("slow.go");
    world.until("both noted nodes to be dispatched", |world| {
        !tasks_dispatched(world, "users").is_empty()
            && !tasks_dispatched(world, "managers").is_empty()
    });

    let users = &tasks_dispatched(&world, "users")[0];
    assert!(
        users.contains(&format!("{PLANNER_CONTEXT_HEADING}\n"))
            && users.contains(&format!(
                "{BINDS} Follow it as written; a user's ruling cannot be declined.\n{USER_RULING}"
            )),
        "the user's carried note lost its attribution:\n{users}"
    );
    let reference = format!("c{by_manager}.0");
    let managers = &tasks_dispatched(&world, "managers")[0];
    assert!(
        managers.contains(&format!("{PLANNER_CONTEXT_HEADING}\n"))
            && managers.contains(&declinable(&reference))
            && managers.contains(MANAGER_RULING),
        "the manager's carried note lost its attribution or reference:\n{managers}"
    );
    // The reference resolves on the adopted run as it did before.
    declined_as_sent(
        &world,
        &run,
        &reference,
        &json!({"node": "managers", "kind": "note", "reply": by_manager, "text": MANAGER_RULING}),
    );
    user_ruling_refused(&world, &run, &format!("c{by_user}.0"));
}

#[test]
fn a_carried_note_keeps_its_attribution_when_a_new_driver_resumes_from_the_checkpoint() {
    carried_notes_survive_adoption("rulingscheckpoint", true);
}

#[test]
fn a_carried_note_keeps_its_attribution_when_a_new_driver_folds_the_whole_journal() {
    carried_notes_survive_adoption("rulingsadopt", false);
}

/// `ask`'s answer states whose it is outright — `manager` included — beside the
/// attribution, and a manager's answer names the reference that declines it.
#[test]
fn an_answer_to_a_question_says_whose_it_is_and_a_managers_can_be_declined() {
    let world = World::new("rulings-ask");
    let path = world.plan(
        "rulingsask",
        &plan_of("rulingsask", vec![agent("build", &[])]),
    );
    world.run(&["start", &path, "--attach"]).settled();
    let run = "rulingsask";

    let mut replies = Vec::new();
    for (user, text) in [(true, USER_RULING), (false, MANAGER_RULING)] {
        // Asked as a dispatch asks, naming no node: the dispatch's own is the one
        // the question is about.
        let asked = crate::ask::asking(
            &world,
            run,
            &["Which way?"],
            &[("ONEPIPELINE_NODE_ID", "build")],
        );
        let question = crate::ask::question_on(&world, run);
        assert_eq!(question["workstream"], json!("build"), "{question}");
        let correlation = question["correlation"].as_str().expect("a correlation");
        let mut args = vec!["reply", run, "--correlation", correlation];
        if user {
            args.extend(["--as", "user"]);
        }
        world
            .run_with_stdin(
                &args,
                &json!({"completion": false, "message": text}).to_string(),
            )
            .exited(0);
        let answered = crate::ask::waited(asked);
        assert_eq!(answered.code, 0, "{}", answered.stderr);
        replies.push(answered.answer()["reply"].clone());
    }
    let (users, managers) = (&replies[0], &replies[1]);
    assert_eq!(users["provenance"], json!("user"), "{users}");
    assert!(
        users["attribution"].as_str().unwrap().starts_with(BINDS),
        "{users}"
    );
    assert!(users.get("ruling").is_none(), "{users}");
    assert_eq!(managers["provenance"], json!("manager"), "{managers}");
    let reply_id = managers["id"].as_u64().expect("the reply's id");
    let reference = format!("a{reply_id}");
    assert_eq!(managers["ruling"], json!(reference), "{managers}");
    assert!(
        managers["attribution"]
            .as_str()
            .unwrap()
            .starts_with(&declinable(&reference)),
        "{managers}"
    );
    let user_reference = format!("a{}", users["id"].as_u64().expect("the reply's id"));

    declined_as_sent(
        &world,
        run,
        &reference,
        &json!({"node": "build", "kind": "answer", "reply": reply_id, "text": MANAGER_RULING}),
    );
    user_ruling_refused(&world, run, &user_reference);
}

/// An answer may carry its ruling as a `reason` alone, with no `message`, and
/// the declined record's text is then that reason.
#[test]
fn a_declined_answer_given_as_a_reason_alone_carries_that_reason_as_its_text() {
    let world = World::new("rulings-ask-reason");
    let path = world.plan(
        "rulingsreason",
        &plan_of("rulingsreason", vec![agent("build", &[])]),
    );
    world.run(&["start", &path, "--attach"]).settled();
    let run = "rulingsreason";
    let asked = crate::ask::asking(&world, run, &["--about", "build", "Which way?"], &[]);
    let question = crate::ask::question_on(&world, run);
    let correlation = question["correlation"].as_str().expect("a correlation");
    world
        .run_with_stdin(
            &["reply", run, "--correlation", correlation],
            &json!({"completion": false, "reason": MANAGER_RULING}).to_string(),
        )
        .exited(0);
    let answered = crate::ask::waited(asked);
    assert_eq!(answered.code, 0, "{}", answered.stderr);
    let reply = answered.answer()["reply"].clone();
    let reply_id = reply["id"].as_u64().expect("the reply's id");
    declined_as_sent(
        &world,
        run,
        &format!("a{reply_id}"),
        &json!({"node": "build", "kind": "answer", "reply": reply_id, "text": MANAGER_RULING}),
    );
}

const SUPERVISOR_OPENING: &str = "You are the simulated USER and completion supervisor";

/// Every prompt either side of a supervised conversation was really handed.
fn prompts(world: &World) -> Vec<String> {
    world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "oneharness-config")
        .filter_map(|call| call["args"][0].as_str().map(str::to_string))
        .collect()
}

/// How many records of a note — a commit or a refusal — or admissions of one
/// into a conversation's inbox the journal at `path` holds.
fn note_progress_in(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| {
            event["kind"] == "note-admitted"
                || ((event["kind"] == "edit-committed" || event["kind"] == "edit-rejected")
                    && event["payload"]["command"]["op"] == "note")
        })
        .count()
}

/// A note delivered live reaches the worker and the supervisor under its
/// attribution, and the continuation the engine composes after a failed check
/// renders it under `## Manager notes` with the same.
fn a_live_note_and_its_continuation(name: &str, user: bool) {
    const NOTE: &str = "The reviewer asked for a smaller diff; stop editing src/old.rs.";
    let world = World::new(name).with_env("ONEPIPELINE_PUBLICATION_ATTEMPTS", "2");
    world.repository("change-auto", &[]);
    world.script("harness.work", "the worker wrote this\n");
    world.script("gh.checks", "llmlint completed failure required");
    world.script(
        "judge.asks-again",
        "Run the check again and report what it said.",
    );
    world.script("turn.hold", "hold");
    world.write_graphs();
    world.write_supervised_node_graph();
    let path = world.plan(name, &plan_of(name, vec![lifecycle("service", &[])]));
    world
        .run_on_agentgraph(&["start", &path, "--detach"])
        .exited(0);
    world.until("the worker's turn to open", |world| {
        !world.events_of(name, "turn-started").is_empty()
    });

    // Released once the note is in the conversation's hands.
    let journal = world.run_file(name, "events.jsonl");
    let fakes = world.fakes.clone();
    let before = note_progress_in(&journal);
    let releasing = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline && note_progress_in(&journal) <= before {
            onepipeline_testfakes::profile::thread::sleep(Duration::from_millis(20));
        }
        for gate in ["turn.go", "turn.settle"] {
            std::fs::write(fakes.join(gate), "go").expect("the turn is released");
        }
    });
    let mut args = vec!["reply", name];
    if user {
        args.extend(["--as", "user"]);
    }
    let replied = world.run_with_stdin_on(
        world.agentgraph_cmd(&args),
        &envelope(json!([{"op": "note", "id": "service", "addressee": "both", "text": NOTE}])),
    );
    releasing.join().expect("the releasing thread finishes");
    replied.exited(0);
    let id = replied.json()["reply"].as_u64().expect("the envelope's id");
    let reference = format!("c{id}.0");
    let attribution = if user {
        BINDS.to_owned()
    } else {
        declinable(&reference)
    };
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });

    let all = prompts(&world);
    let (judge, worker): (Vec<&String>, Vec<&String>) = all
        .iter()
        .partition(|prompt| prompt.contains(SUPERVISOR_OPENING));
    let live = attribution.clone();
    assert!(
        worker
            .iter()
            .any(|prompt| prompt.contains(&live) && prompt.contains(NOTE)),
        "no worker turn was handed the note under its attribution:\n{worker:#?}"
    );
    assert!(
        judge
            .iter()
            .any(|prompt| prompt.contains(&live) && prompt.contains(NOTE)),
        "the supervisor was never handed the note under its attribution:\n{judge:#?}"
    );
    if user {
        assert!(
            all.iter()
                .all(|prompt| !prompt.contains("ruling reference")),
            "a user's note was handed with a reference"
        );
    }

    // The record of the delivery says whose it was.
    let committed = committed_of(&world, name, "note", id);
    assert_eq!(
        provenance_of(&committed),
        if user { "user" } else { "manager" }
    );
    assert_eq!(
        provenance_of(&committed["operations"][0]),
        if user { "user" } else { "manager" }
    );

    // The engine's own continuation, after the failed check, composes the note
    // back in under `## Manager notes`, attributed, and records it carried.
    let continued = world
        .events_of(name, "node-dispatched")
        .into_iter()
        .find(|event| event["payload"]["attempt"] == json!(2))
        .expect("the node is dispatched again after its check failed");
    let carried = &continued["payload"]["notes_carried"][0];
    assert_eq!(carried["text"], json!(NOTE), "{continued}");
    assert_eq!(
        provenance_of(carried),
        if user { "user" } else { "manager" },
        "{continued}"
    );
    assert!(
        worker
            .iter()
            .any(|prompt| prompt.contains("## Manager notes")
                && prompt.contains(&format!("   {attribution}"))
                && prompt.contains(NOTE)),
        "the continuation did not carry the note under its attribution:\n{worker:#?}"
    );

    if user {
        user_ruling_refused(&world, name, &reference);
    } else {
        declined_as_sent(
            &world,
            name,
            &reference,
            &json!({"node": "service", "kind": "note", "reply": id, "text": NOTE}),
        );
    }
}

#[test]
fn a_users_live_note_binds_both_parties_and_its_continuation() {
    a_live_note_and_its_continuation("rulingsliveuser", true);
}

#[test]
fn a_managers_live_note_is_declinable_in_both_parties_and_its_continuation() {
    a_live_note_and_its_continuation("rulingslivemanager", false);
}

const RELEASED: &str = "released-rulings";

/// Seed `world` with the recorded run root the released engine wrote, its
/// placeholders filled in for this world and checkout, naming no driver.
fn seeded_with_the_released_run(world: &World) {
    let from =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/recorded/run-root/released-rulings");
    let into = world.runs.join(RELEASED);
    let repo = env!("CARGO_MANIFEST_DIR");
    let world_root = world.root.to_string_lossy().to_string();
    fn walk(from: &Path, into: &Path, fill: &dyn Fn(&str) -> String) {
        std::fs::create_dir_all(into).expect("a directory to seed");
        for entry in std::fs::read_dir(from).expect("a recorded directory") {
            let entry = entry.expect("a recorded entry");
            let path = entry.path();
            if path.is_dir() {
                walk(&path, &into.join(entry.file_name()), fill);
            } else if entry.file_name() != "README.md" {
                let text = std::fs::read_to_string(&path).expect("a recorded file reads");
                std::fs::write(into.join(entry.file_name()), fill(&text)).expect("it is seeded");
            }
        }
    }
    walk(&from, &into, &|text| {
        text.replace("@REPO@", repo)
            .replace("@WORLD@", &world_root)
            .replace("\"pid\": 1441999,", "\"pid\": 2147483647,")
    });
}

/// A run directory the released engine wrote — version 2 and 3 envelopes, and
/// records with no `provenance` at all — still reads and is adopted, and every
/// such record reads as the manager's: the carried note it owed is composed
/// into the node's dispatch as the manager's, declinable by its reference, and
/// the amendment it recorded binds as the manager's ruling.
#[test]
fn a_run_the_released_engine_wrote_reads_and_adopts_with_every_ruling_the_managers() {
    // As the host and the session that launched it: `adopt` takes over a run
    // only for the session that owns it.
    let launched = World::new("rulings-released").with_env("HOSTNAME", "lima-hp");
    let world = launched.as_session("session-released-rulings");
    seeded_with_the_released_run(&world);
    world.script("slow.turn-open", "");
    world
        .run(&["status", RELEASED])
        .exited(0)
        .out_has("Leave the four comment lines as they are.");
    world.run(&["channel", "queue", RELEASED]).exited(0);
    world.run(&["adopt", RELEASED, "--detach"]).exited(0);
    world.until("the amended, noted node to be dispatched", |world| {
        !tasks_dispatched(world, "later").is_empty()
    });
    let later = &tasks_dispatched(&world, "later")[0];
    assert!(
        later.contains(&format!("{AMENDMENT_HEADING}\n{AMENDMENT_PRECEDENCE}"))
            && later.contains("Leave the four comment lines as they are."),
        "the released engine's amendment is not the manager's binding ruling:\n{later}"
    );
    assert!(
        later.contains(&declinable("c1.0")) && later.contains("The fixture moved to fixtures/v2."),
        "the released engine's carried note is not the manager's, declinable:\n{later}"
    );
    assert!(!later.contains(BINDS), "{later}");
    // Every record it wrote reads as the manager's: none names a provenance.
    for event in world.journal(RELEASED) {
        if matches!(
            event["kind"].as_str(),
            Some("edit-committed" | "command-accepted" | "planner-replied")
        ) {
            assert_eq!(provenance_of(&event["payload"]), "manager", "{event}");
        }
    }
    declined_as_sent(
        &world,
        RELEASED,
        "c1.0",
        &json!({"node": "later", "kind": "note", "reply": 1,
            "text": "The fixture moved to fixtures/v2."}),
    );
}

/// A note a running turn read is **spent** by a dispatch composed without it —
/// the node parked and brought back — and the record of the spent note says
/// whose ruling it was and names its reference, as the carried one does.
fn a_spent_note(name: &str, user: bool) {
    const NOTE: &str = "Stop editing src/old.rs; the reviewer asked for a smaller diff.";
    let world = World::new(name);
    world.script("turn.hold-each", "Do build.");
    world.script(
        "judge.asks-again",
        "Run the check again and report what it said.",
    );
    world.script("turn.hold", "hold");
    // `keep`'s judge is held throughout, so a driver stays on the run to take
    // the requeue once `build` is parked.
    world.script("judge.hold", "hold");
    world.write_graphs();
    world.write_supervised_node_graph();
    let path = world.plan(
        name,
        &plan_of(name, vec![agent("build", &[]), agent("keep", &[])]),
    );
    let mut launch = world.agentgraph_cmd(&["start", &path, "--detach"]);
    launch.env(crate::harness::CANCEL_GRACE_ENV, "1");
    world.run_on(launch, "start --detach").exited(0);
    world.until("build's worker turn to open", |world| {
        world
            .events_of(name, "turn-started")
            .iter()
            .any(|event| event["labels"]["node"] == "build")
    });

    let journal = world.run_file(name, "events.jsonl");
    let fakes = world.fakes.clone();
    let before = note_progress_in(&journal);
    let releasing = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline && note_progress_in(&journal) <= before {
            onepipeline_testfakes::profile::thread::sleep(Duration::from_millis(20));
        }
        for gate in ["turn.go", "turn.settle"] {
            std::fs::write(fakes.join(gate), "go").expect("the turn is released");
        }
    });
    let mut args = vec!["reply", name];
    if user {
        args.extend(["--as", "user"]);
    }
    let replied = world.run_with_stdin_on(
        world.agentgraph_cmd(&args),
        &envelope(json!([{"op": "note", "id": "build", "addressee": "worker", "text": NOTE}])),
    );
    replied.exited(0);
    releasing.join().expect("the releasing thread finishes");
    let id = replied.json()["reply"].as_u64().expect("the envelope's id");
    world.until("the note to be committed", |world| {
        world
            .events_of(name, "edit-committed")
            .iter()
            .any(|event| event["payload"]["envelope"] == json!(id))
    });

    world
        .run_with_stdin_on(
            world.agentgraph_cmd(&["reply", name]),
            &envelope(json!([{"op": "cancel", "id": "build", "reason": "re-dispatch it"}])),
        )
        .exited(0);
    world.until("the held dispatch to be reaped", |world| {
        world
            .events_of(name, "node-settled")
            .iter()
            .any(|event| event["labels"]["node"] == "build")
    });
    world
        .run_with_stdin_on(
            world.agentgraph_cmd(&["reply", name]),
            &envelope(json!([{"op": "requeue", "id": "build"}])),
        )
        .exited(0);
    world.until("build to be dispatched again", |world| {
        world
            .events_of(name, "node-dispatched")
            .iter()
            .filter(|event| event["labels"]["node"] == "build")
            .count()
            >= 2
    });
    let again = world
        .events_of(name, "node-dispatched")
        .into_iter()
        .filter(|event| event["labels"]["node"] == "build")
        .nth(1)
        .expect("the second dispatch");
    let spent = &again["payload"]["notes_spent"][0];
    assert_eq!(spent["text"], json!(NOTE), "{again}");
    assert_eq!(
        provenance_of(spent),
        if user { "user" } else { "manager" },
        "{again}"
    );
    assert_eq!(spent["ruling"], json!(format!("c{id}.0")), "{again}");
}

#[test]
fn a_spent_user_note_is_recorded_as_the_users() {
    a_spent_note("rulingsspentuser", true);
}

#[test]
fn a_spent_manager_note_is_recorded_as_the_managers() {
    a_spent_note("rulingsspentmanager", false);
}

/// Every agent dispatch is handed its own node as `ONEPIPELINE_NODE_ID`, which
/// is what `onepipeline ask` names as the node a question is about when the
/// asker names none — read off the dispatch's own environment, as the
/// `oneagentgraph` double reports it.
#[test]
fn every_agent_dispatch_is_handed_its_own_node() {
    let world = World::new("rulings-node-env");
    world.script("dispatch.report-env", "ONEPIPELINE_NODE_ID\n");
    let path = world.plan(
        "rulingsnodeenv",
        &plan_of(
            "rulingsnodeenv",
            vec![agent("build", &[]), agent("ship", &["build"])],
        ),
    );
    world.run(&["start", &path, "--attach"]).settled();
    let handed: std::collections::BTreeSet<(String, String)> = world
        .invocations()
        .into_iter()
        .filter(|call| call["tool"] == "dispatch-env" && call["args"][1] == "ONEPIPELINE_NODE_ID")
        .map(|call| {
            assert_eq!(
                call["args"][2], "set",
                "a dispatch was handed no node: {call}"
            );
            (
                call["args"][0].as_str().unwrap().to_owned(),
                call["args"][3].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let nodes: std::collections::BTreeSet<&str> =
        handed.iter().map(|(_, node)| node.as_str()).collect();
    assert_eq!(nodes, ["build", "ship"].into(), "{handed:?}");
}

/// A ruling sent while nothing drives the run is applied by the replying
/// process itself, which names its reference before it records the envelope:
/// the dispatch an adopting driver then makes renders it under that reference,
/// and the reference resolves to the envelope the receipt named.
#[test]
fn a_ruling_applied_with_nothing_driving_is_declined_by_the_reference_it_was_rendered_with() {
    let world = World::new("rulings-undriven");
    let run = held(&world, "rulingsundriven", &["managers"]);
    end_driver(&world, &run);
    let replied = world.run_with_stdin(
        &["reply", &run],
        &envelope(json!([{"op": "amend", "id": "managers", "text": MANAGER_RULING}])),
    );
    replied.exited(0).out_has("\"state\":\"applied\"");
    let id = replied.json()["reply"].as_u64().expect("the envelope's id");
    let reference = format!("c{id}.0");
    world.run(&["adopt", &run, "--detach"]).exited(0);
    world.release("slow.go");
    world.until("the amended node to be dispatched", |world| {
        !tasks_dispatched(world, "managers").is_empty()
    });
    let managers = &tasks_dispatched(&world, "managers")[0];
    assert!(
        managers.contains(&format!("{AMENDMENT_HEADING}\n{}", declinable(&reference))),
        "the amendment applied with nothing driving is not rendered under its reference:\n{managers}"
    );
    declined_as_sent(
        &world,
        &run,
        &reference,
        &json!({"node": "managers", "kind": "amendment", "reply": id, "text": MANAGER_RULING}),
    );
}
