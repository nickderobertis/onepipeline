//! Every command envelope the reconciler claims is answered — applied, or refused
//! by name — and a superseded human action has a recorded way out.
//!
//! `onepipeline reply` refuses a command this build cannot decode before anything
//! is queued. The bus does not: the planner channel's layout reads only each
//! command's `op`, so `onemessagebus send replies` — the path a host's own
//! channel-reply script takes — appends a `drop` with no `dependents` or a
//! `settle … cancelled` onto the durable queue as readily as a well-formed one.
//! These journeys put envelopes there that way and drive the compiled binary
//! over them.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven as a real
// compiled binary; the run-end hook is the suite's real fixture command. `harness.rs`
// carries the same suppression and the full rationale.

use std::sync::Arc;

use onemessagebus::{Config, Layouts, LocalTransport, QueueName, Transport, TransportKinds};
use onepipeline::channel::layout::{PlannerChannel, COMMANDS, PLANNER_CHANNEL, REPLIES};
use serde_json::{json, Value};

use crate::harness::{agent, human, plan_of, World, NOTHING_DRIVING, REFUSED};
use crate::run_end_hooks::{hook, invocations, records, RECORD_ENV};

/// What every refusal of an undecodable envelope begins with, before the
/// decoder's own message.
const MALFORMED: &str = "refused: the envelope is malformed: ";

fn queue(name: &str) -> QueueName {
    name.parse().expect("a queue name")
}

/// Send a reply envelope the way `onemessagebus send replies` does: offered to
/// the `replies` queue under the planner channel's layout, which routes its
/// commands onto the run's command queue without decoding them.
fn sent_through_the_bus(world: &World, run: &str, envelope: &Value) {
    let bus = Config::local(world.run_file(run, "channel"), Some(PLANNER_CHANNEL))
        .resolve(
            &Layouts::new().with(Arc::new(PlannerChannel)),
            &TransportKinds::builtin(),
        )
        .expect("the planner channel's bus resolves over the run's channel");
    bus.send(&queue(REPLIES), envelope.clone())
        .unwrap_or_else(|error| panic!("the bus refused {envelope}: {error}"));
}

/// Append one record to the command queue as the channel's transport does,
/// for the record shapes no layout passes — a `commands` that is not a list, an
/// envelope with no `id`. A host writing the queue under its own layout, or an
/// older build, is what puts one there.
// llmlint: ignore-block[tests_mirror_real_usage] deliberately beneath every layout: the
// records below are ones the planner channel's layout would turn away, and the
// reconciler's answer to a record it cannot read is the behaviour under test. The local
// transport's append is the real boundary — it takes the queue's lock and lands each
// record in one write, as `bus_config.rs` says of the same call.
fn appended_beneath_the_layout(world: &World, run: &str, record: &Value) {
    let transport =
        LocalTransport::open(world.run_file(run, "channel")).expect("the run's channel opens");
    Transport::append(&transport, &queue(COMMANDS), record.to_string().as_bytes())
        .expect("the record is appended");
}
// llmlint: ignore-end[tests_mirror_real_usage]

/// Every record on the run's command queue, in queue order.
fn queued(world: &World, run: &str) -> Vec<Value> {
    std::fs::read_to_string(world.run_file(run, "channel/commands.jsonl"))
        .expect("the command queue is there")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("a queued record is JSON"))
        .collect()
}

fn cursor(world: &World, run: &str) -> u64 {
    std::fs::read_to_string(world.run_file(run, "channel/commands-cursor.json"))
        .expect("the reconciler's cursor is there")
        .trim()
        .parse()
        .expect("the cursor is a number")
}

/// The `edit-rejected` surfaces the run raised, as the journal records them.
fn rejected_surfaces(world: &World, run: &str) -> Vec<Value> {
    world
        .events_of(run, "planner-surface-queued")
        .into_iter()
        .map(|event| event["payload"].clone())
        .filter(|surface| surface["kind"] == "edit-rejected")
        .collect()
}

/// A run paused on a waiting human action, its driver gone.
fn paused(world: &World, run: &str, nodes: Vec<Value>, extra: &[&str]) {
    let path = world.plan(run, &plan_of(run, nodes));
    let mut args = vec!["start", path.as_str(), "--attach"];
    args.extend_from_slice(extra);
    world
        .run_from(&world.project, &args)
        .exited(0)
        .out_has("\"settlement\":\"awaiting-planner\"");
}

/// Six envelopes no build decodes reach a run with a waiting human node by the
/// bus's path, and the adopting driver answers each one: one outcome line under
/// its own id, an `edit-rejected` per command under its own author, one surface —
/// and nothing of any of them applies, the valid note beside a malformed drop
/// included.
#[test]
fn every_envelope_the_reconciler_cannot_decode_is_answered_and_nothing_of_it_applies() {
    let world = World::new("malformed-envelopes");
    let run = "malformed";
    paused(
        &world,
        run,
        vec![human("sign-off", &[]), agent("after", &["sign-off"])],
        &[],
    );
    let note = json!({"op": "note", "id": "after", "addressee": "worker",
                      "text": "use the second design", "deliver": "next"});
    let undecodable_drop = json!({"op": "drop", "id": "sign-off"});
    let cancelled = json!({"op": "settle", "id": "sign-off", "outcome": "cancelled",
                           "evidence": "superseded by the later sign-off"});

    appended_beneath_the_layout(&world, run, &json!({"commands": [undecodable_drop]}));
    for commands in [
        json!([undecodable_drop]),
        json!([cancelled]),
        json!([note, undecodable_drop]),
    ] {
        sent_through_the_bus(&world, run, &json!({"version": 3, "commands": commands}));
    }
    let next = u64::try_from(queued(&world, run).len()).expect("a queue length");
    appended_beneath_the_layout(
        &world,
        run,
        &json!({"id": next, "commands": {"op": "drop", "id": "sign-off", "dependents": "detach"}}),
    );
    appended_beneath_the_layout(
        &world,
        run,
        &json!({"id": next + 1, "author": "monitor", "commands": [cancelled]}),
    );
    // And the corners of a record's own fields: an author no build reads, a
    // command that is not an object, and a command list that is empty.
    appended_beneath_the_layout(
        &world,
        run,
        &json!({"id": next + 2, "author": 7, "commands": ["drop sign-off", undecodable_drop]}),
    );
    appended_beneath_the_layout(
        &world,
        run,
        &json!({"id": next + 3, "author": ["monitor"], "commands": []}),
    );
    let records = queued(&world, run);
    assert_eq!(records.len(), 8, "{records:?}");
    assert!(
        world.command_outcomes(run).is_empty(),
        "something answered before a driver held the run"
    );

    let adopted = world.run(&["adopt", run]);
    adopted
        .exited(0)
        .out_has("\"settlement\":\"awaiting-planner\"")
        .err_lacks("panicked");

    // One outcome line per envelope, under the envelope's own id, refusing it
    // whole with the decoder's own words.
    let outcomes = world.command_outcomes(run);
    assert_eq!(outcomes.len(), records.len(), "{outcomes:?}");
    assert_eq!(
        cursor(&world, run),
        u64::try_from(outcomes.len()).expect("a count")
    );
    let reasons: Vec<String> = records
        .iter()
        .map(|record| {
            let id = record["id"].as_u64().unwrap_or(0);
            let answered: Vec<&Value> = outcomes
                .iter()
                .filter(|outcome| outcome["id"] == id)
                .collect();
            let [outcome] = &answered[..] else {
                panic!(
                    "envelope {id} was answered {} times: {outcomes:?}",
                    answered.len()
                );
            };
            assert_eq!(outcome["applied"], json!(false), "{outcome}");
            assert!(outcome.get("results").is_none(), "{outcome}");
            let reason = outcome["reason"].as_str().expect("a refusal states why");
            let decoder = reason
                .strip_prefix(MALFORMED)
                .unwrap_or_else(|| panic!("not refused as malformed: {reason}"));
            assert!(
                !decoder.trim().is_empty(),
                "the decoder said nothing: {reason}"
            );
            reason.to_owned()
        })
        .collect();
    assert!(
        reasons[1].contains("missing field `dependents`"),
        "{reasons:?}"
    );
    assert!(
        reasons[2].contains("unknown variant `cancelled`"),
        "{reasons:?}"
    );
    assert!(
        reasons[3].contains("missing field `dependents`"),
        "{reasons:?}"
    );
    assert!(reasons[4].contains("invalid type"), "{reasons:?}");

    // One `edit-rejected` per command sent, carrying it as sent under the
    // envelope's own author; the record with no command list carries itself.
    let rejected: Vec<Value> = world
        .events_of(run, "edit-rejected")
        .into_iter()
        .map(|event| event["payload"].clone())
        .collect();
    let mut expected = Vec::new();
    for (record, reason) in records.iter().zip(&reasons) {
        let author = record["author"].as_str().unwrap_or("planner");
        match record["commands"].as_array() {
            Some(commands) if !commands.is_empty() => {
                for command in commands {
                    let command = if command.is_object() {
                        command.clone()
                    } else {
                        json!({"op": "unreadable", "value": command})
                    };
                    expected.push(json!({"author": author, "command": command, "reason": reason}));
                }
            }
            _ => expected.push(json!({"author": author,
                "command": {"op": "unreadable", "value": record}, "reason": reason})),
        }
    }
    assert_eq!(rejected, expected);
    assert_eq!(rejected.len(), 10);
    assert_eq!(rejected[6]["author"], "monitor");
    assert_eq!(
        rejected[7],
        json!({"author": "planner", "command": {"op": "unreadable", "value": "drop sign-off"},
               "reason": reasons[6]})
    );
    assert_eq!(
        rejected[9]["command"],
        json!({"op": "unreadable", "value": records[7]})
    );

    // One non-blocking surface per envelope, naming its id and its reason.
    let surfaces = rejected_surfaces(&world, run);
    assert_eq!(surfaces.len(), records.len(), "{surfaces:?}");
    for (record, reason) in records.iter().zip(&reasons) {
        let id = record["id"].as_u64().unwrap_or(0);
        assert!(
            surfaces.iter().any(|surface| {
                surface["blocking"] == json!(false)
                    && surface["message"].as_str().is_some_and(|message| {
                        message.contains(&format!("envelope {id} ")) && message.contains(reason)
                    })
            }),
            "no surface names envelope {id} and {reason}: {surfaces:?}"
        );
    }

    // Nothing applied: no edit committed, the human action still waits, and the
    // note that rode beside a malformed drop reached nobody.
    assert!(world.events_of(run, "edit-committed").is_empty());
    assert!(world.events_of(run, "command-accepted").is_empty());
    assert!(world.events_of(run, "node-dropped").is_empty());
    assert!(world
        .events_of(run, "node-settled")
        .iter()
        .all(|event| event["payload"]["node"] != "sign-off"));
    // The note was a valid one: sent alone, it applies.
    world
        .run_with_stdin(
            &["reply", run],
            &json!({"version": 3, "commands": [note]}).to_string(),
        )
        .exited(0);
    assert_eq!(
        world.events_of(run, "edit-committed").len()
            + world.events_of(run, "command-accepted").len(),
        1
    );
}

/// onepipeline#455's shape: a malformed `drop` queued onto a run whose graph has
/// already settled is answered by the driver adopted onto it, before it exits.
#[test]
fn a_driver_adopted_onto_a_settled_run_answers_an_undecodable_envelope_before_it_exits() {
    let world = World::new("malformed-settled");
    world.script("build.fail", "1");
    let run = "settled";
    let path = world.plan(run, &plan_of(run, vec![agent("build", &[])]));
    world
        .run_from(&world.project, &["start", path.as_str(), "--attach"])
        .exited(NOTHING_DRIVING)
        .out_has("\"settlement\":\"unattended\"");
    let settled = world.run_json(run, "result.json");
    sent_through_the_bus(
        &world,
        run,
        &json!({"version": 3, "commands": [
            {"op": "drop", "id": "build", "reason": "the work moved to another plan"}
        ]}),
    );

    world
        .run(&["adopt", run])
        .exited(NOTHING_DRIVING)
        .err_lacks("panicked");

    let outcomes = world.command_outcomes(run);
    let [outcome] = &outcomes[..] else {
        panic!("the envelope was not answered once: {outcomes:?}");
    };
    assert_eq!(outcome["id"], queued(&world, run)[0]["id"]);
    assert_eq!(outcome["applied"], json!(false));
    assert_eq!(
        outcome["reason"],
        format!("{MALFORMED}missing field `dependents`")
    );
    assert_eq!(cursor(&world, run), 1);
    assert_eq!(world.events_of(run, "edit-rejected").len(), 1);
    assert_eq!(rejected_surfaces(&world, run).len(), 1);
    assert_eq!(
        world.run_json(run, "result.json")["nodes"],
        settled["nodes"]
    );
}

/// Superseded human actions are retired with `drop`, stating why: the reason is
/// in the record, the dependant loses its edge and comes forward, a blank reason
/// is refused by name on both paths, and a run whose only unfinished nodes were
/// the retired ones settles complete and fires its success hook.
#[test]
fn a_superseded_human_action_is_retired_with_a_drop_that_states_why() {
    let world = World::new("retire-sign-off");
    std::fs::create_dir_all(records(&world)).expect("a record directory");
    let record = records(&world).to_string_lossy().into_owned();
    let world = world.with_env(RECORD_ENV, &record);
    let hook = hook(&world);
    let run = "retire";
    paused(
        &world,
        run,
        vec![
            agent("build", &[]),
            human("sign-off", &[]),
            human("sign-off-2", &["sign-off"]),
        ],
        &["--success-hook", &hook, "--failure-hook", &hook],
    );
    assert!(invocations(&world, run).is_empty());
    let drop = |id: &str, reason: &str| {
        json!({"version": 3, "commands": [
            {"op": "drop", "id": id, "dependents": "detach", "reason": reason}
        ]})
    };

    // `cancel` is not how a waiting human action is put down, and its refusal
    // names the two edits that are; any other node's refusal is as it was.
    let cancel = |id: &str| json!({"version": 3, "commands": [{"op": "cancel", "id": id}]});
    world
        .run_with_stdin(&["reply", run], &cancel("sign-off").to_string())
        .exited(REFUSED)
        .err_has(
            "cancel: node 'sign-off' is waiting, not pending or running: a waiting human action \
             is completed with `attest`, or retired with `drop` (its `dependents` `detach` or \
             `drop`, and an optional `reason`)",
        );
    world
        .run_with_stdin(&["reply", run], &cancel("build").to_string())
        .exited(REFUSED)
        .err_has("cancel: node 'build' is done, not pending or running\n");

    // `reply` refuses a blank reason by name, and accepts a stated one.
    world
        .run_with_stdin(&["reply", run], &drop("sign-off", " ").to_string())
        .exited(REFUSED)
        .err_has("drop: node 'sign-off' would be dropped stating an empty reason");
    world
        .run_with_stdin(&["reply", run], &drop("sign-off", "superseded").to_string())
        .exited(0);
    let committed = world.events_of(run, "edit-committed");
    let [committed] = &committed[..] else {
        panic!("one drop is one edit committed: {committed:?}");
    };
    assert_eq!(
        committed["payload"]["command"],
        json!({"op": "drop", "id": "sign-off", "dependents": "detach", "reason": "superseded"})
    );
    let operations = committed["payload"]["operations"]
        .as_array()
        .expect("the operations it compiled to");
    assert!(
        operations.iter().any(|operation| {
            operation["from"] == "sign-off" && operation["to"] == "sign-off-2"
        }),
        "the dependant kept its edge: {operations:?}"
    );

    // A blank reason sent through the bus is refused by name too, and leaves the
    // node in the graph — now waiting, on no dependency at all.
    sent_through_the_bus(&world, run, &drop("sign-off-2", ""));
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"awaiting-planner\"");
    let refused = world
        .command_outcomes(run)
        .last()
        .cloned()
        .expect("answered");
    assert_eq!(refused["applied"], json!(false), "{refused}");
    assert!(
        refused["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("would be dropped stating an empty reason")),
        "{refused}"
    );
    assert!(world.events_of(run, "edit-rejected").iter().any(|event| {
        event["payload"]["command"]["id"] == "sign-off-2"
            && event["payload"]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("empty reason"))
    }));
    let result = world.run_json(run, "result.json");
    let status = |result: &Value, id: &str| {
        result["nodes"]
            .as_array()
            .expect("the result lists nodes")
            .iter()
            .find(|node| node["id"] == id)
            .map(|node| node["status"].clone())
    };
    assert_eq!(status(&result, "sign-off"), None);
    assert_eq!(status(&result, "sign-off-2"), Some(json!("waiting")));
    assert!(invocations(&world, run).is_empty());

    sent_through_the_bus(&world, run, &drop("sign-off-2", "superseded"));
    world
        .run(&["adopt", run])
        .exited(0)
        .out_has("\"settlement\":\"complete\"");
    let result = world.run_json(run, "result.json");
    let nodes = result["nodes"].as_array().expect("the result lists nodes");
    assert!(
        !nodes.is_empty() && nodes.iter().all(|node| node["status"] == "done"),
        "{result}"
    );
    assert_eq!(invocations(&world, run), ["success"]);
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] same grounds as
// `driver.rs`'s `an_edit_that_arrives_while_the_driver_is_leaving_is_applied_before_it_lets_go`,
// whose window this journey uses: it is of the crate's own ownership lock, handover gate and
// command queue, which any change under `src/` can move.
/// A record no build decodes that reaches a run while its driver is **on its way
/// out** is answered by that driver before it lets go — it is work the driver
/// owes, not something it may leave on a queue nothing will claim.
///
/// The window is the run's write-back close-out, held open by a shadow store the
/// worker cannot write, exactly as the driver journey that proves the same of a
/// well-formed edit holds it.
#[test]
fn an_undecodable_envelope_arriving_while_the_driver_is_leaving_is_answered_before_it_lets_go() {
    let world = World::new("malformed-leaving");
    world.script("work.wait", "hold");
    let run = "leaving";
    let path = world.plan(run, &plan_of(run, vec![agent("work", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of(run, "node-dispatched").is_empty()
    });
    crate::driver::unwritable_shadow_store(&world, run);
    world.release("work.go");
    world.until("the only node to settle", |world| {
        !world.events_of(run, "node-settled").is_empty()
    });

    sent_through_the_bus(
        &world,
        run,
        &json!({"version": 3, "commands": [{"op": "drop", "id": "work"}]}),
    );
    world.until("the leaving driver to answer the envelope", |world| {
        !world.command_outcomes(run).is_empty()
    });
    world.until("the run to settle", |world| {
        world.run_file(run, "result.json").is_file()
    });

    let outcomes = world.command_outcomes(run);
    assert_eq!(
        outcomes,
        [json!({"id": queued(&world, run)[0]["id"], "applied": false,
                "reason": format!("{MALFORMED}missing field `dependents`")})]
    );
    assert_eq!(cursor(&world, run), 1);
    assert_eq!(world.events_of(run, "edit-rejected").len(), 1);
    assert!(
        world.events_of(run, "driver-adopted").is_empty(),
        "a second driver answered what the first one left"
    );
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]
