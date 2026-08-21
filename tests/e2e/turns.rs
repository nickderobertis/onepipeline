//! The turn a dispatch relays, field for field.
//!
//! `oneagentgraph` publishes a member's turn as it happens — the turn opening
//! and what it was asked, each tool call **and the observation that answered
//! it**, each party's own words, and the turn's own close and account — and this
//! crate relays all of it into the run's merged store. What these journeys hold
//! is the relay: that every field the producer declares arrives with its name
//! and its meaning intact, and that nothing was invented beside it.
//!
//! Worth journeys of its own, and for the reason `session.rs` states about the
//! `session` label: the fields are read back through the *producer's own*
//! payload types, which deny unknown fields, so a relay that had renamed one,
//! dropped one, or added one of its own fails here. A shape restated in this
//! file would go on passing the day that library moved.
//!
//! **The first two drive the real sibling**, through
//! [`World::run_on_agentgraph`], and they differ only in which member kind the
//! graph declares — which is what decides how far down the stand-in sits:
//!
//! * a **single-sided** member's turn is an `oneharness_core` library call inside
//!   `oneagentgraph`'s own process, so the only stand-in is the paid model turn
//!   and the envelopes below are ones the real normalizer built out of a real
//!   provider stream;
//! * a **two-party** member's conversation is onejudge's own run driver, also in
//!   that process, and it spawns one `oneharness` per side per turn — because
//!   `oneagentgraph` installs the spawn hook it needs to reap a paid harness
//!   nothing else can reach. So the stand-in there is that process, one layer
//!   above the model, and everything that decides, composes, parses and publishes
//!   a turn is still the real thing.
//!
//! Either way what is asserted on is what the producer really published, and the
//! relay carrying it is the real relay.
//!
//! [`World::run_on_agentgraph`]: crate::harness::World::run_on_agentgraph

// llmlint: ignore-file[e2e_not_mocked] the rationale is `harness.rs`'s; what is specific
// here is that nothing between the producer and the assertion is substituted — the graph,
// the member, the conversation engine and the relay are all the real ones, and the
// stand-in is below the producer at the boundary each journey above names — and every
// field is read back through the producing library's own `deny_unknown_fields` payload
// type rather than by name.

use crate::harness::{agent, plan_of, World};
use oneagentgraph::event::{
    EventKind, Party, TurnActivity, TurnCompleted, TurnMessage, TurnStarted, MAX_PAYLOAD_TEXT_BYTES,
};
use serde_json::Value;

const NODE: &str = "work";

fn relayed(world: &World, run: &str, kind: EventKind) -> Vec<Value> {
    world
        .journal(run)
        .into_iter()
        .filter(|event| event["source"] == "agentgraph" && event["kind"] == kind.as_str())
        .collect()
}

/// One relayed payload, read back through the producing library's own type.
///
/// The `deny_unknown_fields` on that type is doing the work: it is what makes
/// this an assertion about the *whole* payload rather than about the fields this
/// file thought to name. A relay that dropped one fails on the missing field, and
/// one that stamped something of its own onto a sibling's payload fails on the
/// unknown one.
fn payload<T: serde::de::DeserializeOwned>(event: &Value, kind: EventKind) -> T {
    serde_json::from_value(event["payload"].clone()).unwrap_or_else(|error| {
        panic!(
            "a relayed {} payload is not the one the linked oneagentgraph declares: {error}: \
             {event}. If the field named is one that library added, `Cargo.lock` is behind \
             the producer this engine needs and `cargo update -p oneagentgraph` is the fix.",
            kind.as_str()
        )
    })
}

/// The whole of a real dispatched turn reaches the merged store: its opening,
/// its exchange with a tool, and its close.
///
/// The defect this stands against is not a field read wrongly — it is a run that
/// relays an *outline*: a turn's opening with nothing but a number on it, an
/// activity naming what the agent asked for and never what came back, a close
/// that closes no turn in particular. Everything downstream still works and
/// there is simply nothing in it, which is invisible to every test that asserts
/// a dispatch *happened*.
#[test]
fn a_real_dispatched_turn_relays_every_field_its_producer_publishes() {
    let world = World::new("real-turn-fields");
    world.write_graphs();
    let plan = plan_of("turns", vec![agent(NODE, &[])]);
    let task = plan["tasks"][0]["task"]
        .as_str()
        .expect("the node states its task")
        .to_string();
    let path = world.plan("turns", &plan);
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0)
        .settled();

    // The opening. Four fields, and each answers a question an operator watching
    // a live dispatch has no second source for: which turn, who is taking it,
    // what it was asked, and when it began.
    let opened = relayed(&world, "turns", EventKind::TurnStarted);
    let [opening] = &opened[..] else {
        panic!(
            "the dispatch opened {} turns, not the one it takes",
            opened.len()
        );
    };
    let started: TurnStarted = payload(opening, EventKind::TurnStarted);
    assert_eq!(started.turn, 1, "{opening}");
    assert_eq!(started.role, Party::Assistant.as_str(), "{opening}");
    // The instruction is **the node's own task prose**, which is the whole point
    // of the field: a turn's opening that carried some other text would say the
    // dispatch was asked something it was not.
    assert_eq!(started.instruction, task, "{opening}");
    assert!(!started.instruction_truncated, "{opening}");
    assert!(
        started.started_at.ends_with('Z'),
        "the turn opened at no instant: {opening}"
    );

    // The exchange. A call and the observation that answered it, joined by the
    // harness's own id — which is what makes a pair of them one exchange rather
    // than two unrelated lines.
    let activity = relayed(&world, "turns", EventKind::TurnActivity);
    let acts: Vec<TurnActivity> = activity
        .iter()
        .map(|event| payload(event, EventKind::TurnActivity))
        .collect();
    let [call, result] = &acts[..] else {
        panic!(
            "the turn relayed {} activities, not the call and the answer it is: {activity:?}",
            acts.len()
        );
    };
    assert_eq!(call.kind, "tool_call", "{activity:?}");
    assert_eq!(call.name.as_deref(), Some("bash"), "{activity:?}");
    assert_eq!(call.detail, "echo the turn ran", "{activity:?}");
    assert_eq!(
        call.output, None,
        "a call carries an observation it has not been given yet: {activity:?}"
    );
    assert_eq!(result.kind, "tool_result", "{activity:?}");
    // A result names no tool, because it answers one already named. `None`
    // rather than an empty string: the field is a fact about this event.
    assert_eq!(result.name, None, "{activity:?}");
    assert_eq!(
        result.output.as_deref(),
        Some("the turn ran"),
        "the observation the tool returned did not survive the relay: {activity:?}"
    );
    assert!(!result.output_truncated, "{activity:?}");
    assert_eq!(
        call.tool_call_id, result.tool_call_id,
        "the call and its answer reached the store joined to nothing: {activity:?}"
    );
    assert!(
        call.tool_call_id.is_some(),
        "the exchange carries no identity at all: {activity:?}"
    );
    // And their order within the turn is expressible, which is the only thing
    // that survives a merge with a second member's activity interleaved.
    assert!(
        call.index < result.index,
        "the answer is not after the ask: {activity:?}"
    );

    // The close. One turn's close, on that turn's own account — the same turn
    // and the same party its opening named, over an interval that starts where
    // the opening said it did.
    let closed = relayed(&world, "turns", EventKind::TurnCompleted);
    let [closing] = &closed[..] else {
        panic!(
            "the dispatch closed {} turns, not the one it takes",
            closed.len()
        );
    };
    let completed: TurnCompleted = payload(closing, EventKind::TurnCompleted);
    assert_eq!(completed.turn, started.turn, "{closing}");
    assert_eq!(completed.role, started.role, "{closing}");
    assert_eq!(
        completed.started_at, started.started_at,
        "the turn closed on an instant its opening never named: {closing}"
    );
    assert!(
        completed.finished_at >= completed.started_at,
        "the turn finished before it began: {closing}"
    );
    // The account is this turn's own, in the producer's own spelling. Every
    // figure is independently optional and an absent one means the provider
    // reported none — so what is asserted is the one the stand-in really
    // reported, not that all five arrived.
    assert_eq!(
        completed.usage.input_tokens,
        Some(1),
        "the turn's own account did not survive the relay: {closing}"
    );
    assert_eq!(completed.usage.output_tokens, Some(1), "{closing}");
}

/// A real **supervised conversation** relays what each party said, in that
/// party's own name.
///
/// The journey above is one member taking one turn, which is every kind this
/// producer publishes except the one that needs two parties: an agent's reply is
/// only a `turn-message` when somebody is there to receive it. So this drives the
/// two-party member the shipped node-scope graph declares — `oneagentgraph`
/// merging the persona, onejudge's own run driver deciding every turn, composing
/// both prompts and parsing both answers, and the real relay carrying each
/// observation into the merged store.
///
/// The defect it stands against is a relay that carries *a* reply: one message,
/// unattributed, from a conversation whose second half never arrives. Nothing
/// downstream can tell that from a supervisor that had nothing to say, so what is
/// held here is the pair — the agent's words and the supervisor's, each on its own
/// party, and the supervisor's turning up again as the instruction the next turn
/// answers.
///
/// The supervisor **asks once and then completes**, which is the shortest
/// conversation with two of everything in it: two turns, both parties speaking,
/// and a second turn whose opening is not the node's task.
#[test]
fn a_real_supervised_conversation_relays_what_each_party_said() {
    let world = World::new("real-conversation");
    world.write_graphs();
    world.write_supervised_node_graph();
    let ask = "Run the check again and report what it said.";
    world.script("judge.asks-again", ask);
    let plan = plan_of("talk", vec![agent(NODE, &[])]);
    let task = plan["tasks"][0]["task"]
        .as_str()
        .expect("the node states its task")
        .to_string();
    let path = world.plan("talk", &plan);
    world
        .run_on_agentgraph(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0)
        .settled();

    // Each party's own words, read back through the producing library's own type
    // — so a relay that renamed a field, dropped one, or stamped one of its own
    // onto a sibling's payload fails here rather than passing on a shape this
    // file restated.
    let said = relayed(&world, "talk", EventKind::TurnMessage);
    let messages: Vec<TurnMessage> = said
        .iter()
        .map(|event| payload(event, EventKind::TurnMessage))
        .collect();
    let [answered, asked_again, answered_again] = &messages[..] else {
        panic!(
            "the conversation relayed {} things said, not the three it is: {said:?}",
            messages.len()
        );
    };

    // The agent's reply: its own turn, its own party, and the words the turn
    // really ended on rather than a summary of them.
    assert_eq!(answered.turn, 1, "{said:?}");
    assert_eq!(answered.role, Party::Assistant.as_str(), "{said:?}");
    assert_eq!(answered.text, ANSWERED, "{said:?}");
    assert!(!answered.truncated, "{said:?}");

    // The supervisor's, on the **other** party. This is what a single-sided
    // dispatch has no counterpart for, and getting the party wrong is invisible
    // to every reader that only counts messages.
    assert_eq!(asked_again.role, Party::User.as_str(), "{said:?}");
    assert_eq!(asked_again.text, ask, "{said:?}");
    assert!(!asked_again.truncated, "{said:?}");

    // And the turn it produced: the same party as the first, a turn later.
    assert_eq!(answered_again.turn, answered.turn + 1, "{said:?}");
    assert_eq!(answered_again.role, Party::Assistant.as_str(), "{said:?}");

    // Both parties' turns opened, and each opening says what that party was
    // answering. The second agent turn's instruction is the supervisor's own
    // words — which is the whole of what makes this a conversation rather than
    // two dispatches: an opening still carrying the node's task would say the
    // supervisor was never heard.
    let opened = relayed(&world, "talk", EventKind::TurnStarted);
    let openings: Vec<TurnStarted> = opened
        .iter()
        .map(|event| payload(event, EventKind::TurnStarted))
        .collect();
    let instructions: Vec<(&str, &str)> = openings
        .iter()
        .map(|opening| (opening.role.as_str(), opening.instruction.as_str()))
        .collect();
    assert_eq!(
        instructions,
        vec![
            (Party::Assistant.as_str(), task.as_str()),
            (Party::User.as_str(), ANSWERED),
            (Party::Assistant.as_str(), ask),
            (Party::User.as_str(), ANSWERED),
        ],
        "the conversation's turns did not open on what each party was answering: {opened:?}"
    );
    assert!(
        openings
            .iter()
            .all(|opening| !opening.instruction_truncated),
        "{opened:?}"
    );

    // Every turn closed on its own account, on both sides. Asserted as the whole
    // sequence rather than one close at a time: a relay that published one
    // party's closes twice, or the second turn's under the first turn's number,
    // reads as a conversation of a different shape and every individual
    // assertion still passes.
    let closed = relayed(&world, "talk", EventKind::TurnCompleted);
    let closings: Vec<TurnCompleted> = closed
        .iter()
        .map(|event| payload(event, EventKind::TurnCompleted))
        .collect();
    let bounds: Vec<(u64, &str)> = closings
        .iter()
        .map(|closing| (closing.turn, closing.role.as_str()))
        .collect();
    assert_eq!(
        bounds,
        vec![
            (1, Party::Assistant.as_str()),
            (1, Party::User.as_str()),
            (2, Party::Assistant.as_str()),
            (2, Party::User.as_str()),
        ],
        "the conversation's turns did not close one for one on what opened them: {closed:?}"
    );
    assert!(
        closings
            .iter()
            .zip(&openings)
            .all(
                |(closing, opening)| closing.started_at == opening.started_at
                    && closing.finished_at >= closing.started_at
            ),
        "a turn closed over an interval its own opening never named: {closed:?}"
    );
    // The account is per turn rather than a run total served once at the end,
    // which is the whole reason a close carries one.
    assert!(
        closings
            .iter()
            .all(|closing| closing.usage.input_tokens == Some(1)),
        "a turn closed on an account that is not its own: {closed:?}"
    );
}

/// What every turn of the conversation above ends on.
///
/// The double's own answer, restated here because a test binary cannot link
/// another crate's `[[bin]]`. `tests/e2e/dispatch.rs` already reads the same words
/// out of a rendered transcript, so a double that changed them fails in two
/// places at once rather than passing quietly in either.
const ANSWERED: &str = "Ran what the task asked for.";

/// A relayed payload text past this crate's own published bound is cut and said
/// to be cut, rather than served whole.
///
/// The bound is [`MAX_PAYLOAD_TEXT_BYTES`], and both siblings publish text
/// inside it, so on a stack whose three pieces agree this never fires. It is
/// held anyway because it is *this crate's* promise about its own envelope
/// rather than a restatement of a producer's: what arrives on a pipe this
/// process reads is whatever the thing on the other end wrote, and every reader
/// downstream of the store — a branch name, a rendered line, an operator's
/// terminal — already treats a payload text as bounded.
///
/// **Two fields, on two kinds, cut two ways**, because the rule is about a
/// payload text rather than about one field of one kind:
///
/// * a turn's own words, one byte past the bound — the smallest over-long value
///   there is, so a relay cutting a byte early or late fails here;
/// * the node's task prose, echoed onto the turn's activity, built so the bound
///   lands **inside a character**. Cut there by bytes, the record carries
///   something that is not UTF-8 and no reader parses the line at all.
///
/// The producer is the `oneagentgraph` double, which publishes what it really
/// did and flags nothing: the cut and the flag on the far side are the relay's
/// own rather than a fixture handed to it.
#[test]
fn a_relayed_payload_text_past_the_bound_is_cut_and_flagged_rather_than_served_whole() {
    let world = World::new("relay-bound");
    world.script(
        &format!("{NODE}.said-bytes"),
        &(MAX_PAYLOAD_TEXT_BYTES + 1).to_string(),
    );
    let mut node = agent(NODE, &[]);
    let task = task_whose_bound_falls_inside_a_character();
    node["task"] = Value::String(task.clone());
    let path = world.plan("bounded", &plan_of("bounded", vec![node]));
    world
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);

    // The words. Read back through the producer's own payload type, as every
    // other field here is: a cut value is still a `turn-message` and not a shape
    // this crate invented on the way past.
    let messages = relayed(&world, "bounded", EventKind::TurnMessage);
    let [message] = &messages[..] else {
        panic!(
            "the dispatch said {} things, not the one it says",
            messages.len()
        );
    };
    let said: TurnMessage = payload(message, EventKind::TurnMessage);
    assert_eq!(
        said.text.len(),
        MAX_PAYLOAD_TEXT_BYTES,
        "a payload text past the bound reached the store at its own length"
    );
    assert!(
        said.truncated,
        "the text was cut and the record does not say so, which reads as a turn that said \
         exactly this much: {message}"
    );
    // The rest of the payload is untouched: a bound is about one field, and a
    // relay that flagged it by rewriting the whole payload would lose which turn
    // said this and who was speaking.
    assert_eq!(said.turn, 1, "{message}");
    assert_eq!(said.role, Party::Assistant.as_str(), "{message}");

    // The task, on the activity. Read as the payload rather than through
    // `TurnActivity`: this double's summary carries scripting fields of its own
    // beside the producer's, which that type denies. What is under test here is
    // the cut, and the producer's own shape is held by the journey above.
    let activity = relayed(&world, "bounded", EventKind::TurnActivity);
    let [act] = &activity[..] else {
        panic!("the turn relayed {} activities, not one", activity.len());
    };
    let echoed = act["payload"]["task"]
        .as_str()
        .expect("the double echoes the task its dispatch was given");
    assert!(
        task.starts_with(echoed),
        "what reached the store is not a head of the task the node declared: {act}"
    );
    // **Short of the bound, not at it**: the last character starts before byte
    // 4096 and ends after it, so a cut that honoured the boundary gave up those
    // bytes and one that did not would have put invalid UTF-8 on the line.
    assert_eq!(
        echoed.len(),
        MAX_PAYLOAD_TEXT_BYTES - 1,
        "the cut landed on the bound rather than on the character boundary before it: {act}"
    );
    assert_eq!(act["payload"]["truncated"], Value::Bool(true), "{act}");

    // And a turn that says something ordinary is served whole, with no flag —
    // otherwise the assertions above hold for a relay that cuts everything.
    let plain = World::new("relay-unbounded");
    let path = plain.plan("plain", &plan_of("plain", vec![agent(NODE, &[])]));
    plain
        .run(&["start", &path.to_string_lossy(), "--attach"])
        .exited(0);
    let messages = relayed(&plain, "plain", EventKind::TurnMessage);
    let [message] = &messages[..] else {
        panic!(
            "the dispatch said {} things, not the one it says",
            messages.len()
        );
    };
    let said: TurnMessage = payload(message, EventKind::TurnMessage);
    assert!(
        !said.truncated && said.text.len() < MAX_PAYLOAD_TEXT_BYTES,
        "an ordinary turn's words were cut: {message}"
    );
    // Absent rather than `false`: the producer omits its own flag when it did
    // not cut, and a relay with nothing to cut leaves the payload alone.
    assert_eq!(message["payload"]["truncated"], Value::Null, "{message}");
}

/// Task prose whose 4096th byte is in the middle of a character.
///
/// The node's own task, padded with one-byte characters to one byte short of the
/// bound and then closed with a three-byte one — so the character that straddles
/// the bound starts at 4095 and ends at 4098, and the only cut that keeps the
/// value a string gives up the two bytes past 4095.
fn task_whose_bound_falls_inside_a_character() -> String {
    let opening = agent(NODE, &[])["task"]
        .as_str()
        .expect("the node states its task")
        .to_string();
    let pad = MAX_PAYLOAD_TEXT_BYTES - 1 - opening.len();
    format!("{opening}{}\u{2603}", ".".repeat(pad))
}
