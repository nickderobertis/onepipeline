//! The turn a dispatch relays, field for field.
//!
//! What these journeys hold is the relay: that every field the producer declares
//! arrives with its name and its meaning intact, and that nothing was invented
//! beside it. The fields are read back through the *producer's own* payload
//! types, which deny unknown fields, so a shape restated in this file would go on
//! passing the day that library moved.
//!
//! The first two drive the real sibling and differ only in the member kind, which
//! is what decides how far down the stand-in sits. The two-party one earns its
//! cost: `turn-message` is published by a two-party member alone, because a reply
//! is only somebody's *words* when there is a second party to receive them.

// llmlint: ignore-file[e2e_not_mocked] the rationale is `harness.rs`'s; what is specific
// here is that nothing between the producer and the assertion is substituted, and that
// every field is read back through the producing library's own `deny_unknown_fields`
// payload type rather than by name.

use crate::harness::{agent, plan_of, World};
use oneagentgraph::event::{
    EventKind, JudgeDecided, Party, TurnActivity, TurnCompleted, TurnMessage, TurnStarted,
    MAX_PAYLOAD_TEXT_BYTES,
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
        .run_on_agentgraph(&["start", &path, "--attach"])
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
        .run_on_agentgraph(&["start", &path, "--attach"])
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

/// What each judge of a **panel** decided reaches the merged store, payload
/// intact, and the schema-12 report recording the same decisions still reads.
///
/// The relay in `src/agentgraph.rs` matches on no kind, which is why this is
/// proven rather than assumed: a relay that dropped a kind it had never seen
/// would fail nothing else in this suite. The double is the one producer a
/// panel's decisions can be made to arrive from without a real panel.
#[test]
fn a_panels_decisions_are_relayed_whole_and_the_report_recording_them_still_reads() {
    // llmlint: ignore[tests_mirror_real_usage] the double is the one producer a panel's
    // decisions can be made to arrive from without a harness credential, which is the
    // credentialled tier's to spend; its `.judged` script says what the member decided the
    // way `.verdict` and `.fail` say what it settled on in every other journey here, and
    // everything downstream of it — the binary, the relay, the journal, the report reader
    // and the rendered views — is the real one.
    let world = World::new("judge-decided");
    // Two judges, in the panel's list order: one of each kind a bare harness
    // side is not, so a relay that read the kind as an enum of the kinds it
    // knew would fail on the second.
    world.script(
        &format!("{NODE}.judged"),
        "lint|llmlint|continue|two findings under comments_earn_their_place\n\
         reviewer|oneharness|done|every criterion is met, and the tree is clean\n",
    );
    world.script(
        &format!("{NODE}.verdict"),
        "false|the change builds|cargo build fails in src/views.rs\n",
    );
    world.script(&format!("{NODE}.fail"), "1");
    let path = world.plan("judged", &plan_of("judged", vec![agent(NODE, &[])]));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file("judged", "result.json").is_file()
    });

    // The decisions, read back through the producing library's own type — its
    // `deny_unknown_fields` is what makes this the whole payload rather than
    // the fields this file thought to name.
    let decided = relayed(&world, "judged", EventKind::JudgeDecided);
    let decisions: Vec<JudgeDecided> = decided
        .iter()
        .map(|event| payload(event, EventKind::JudgeDecided))
        .collect();
    let [lint, reviewer] = &decisions[..] else {
        panic!(
            "the panel relayed {} decisions, not one per judge: {decided:?}",
            decisions.len()
        );
    };
    assert_eq!(
        lint,
        &JudgeDecided {
            turn: 1,
            judge: "lint".into(),
            kind: "llmlint".into(),
            decision: "continue".into(),
            reason: "two findings under comments_earn_their_place".into(),
        },
        "{decided:?}"
    );
    assert_eq!(
        reviewer,
        &JudgeDecided {
            turn: 1,
            judge: "reviewer".into(),
            kind: "oneharness".into(),
            decision: "done".into(),
            reason: "every criterion is met, and the tree is clean".into(),
        },
        "{decided:?}"
    );
    // Stamped with the node it belongs to, as every relayed envelope is: a
    // decision nobody can attribute to a dispatch is one no view can render.
    for event in &decided {
        assert_eq!(event["labels"]["node"], NODE, "{event}");
        assert_eq!(event["labels"]["member"], "worker", "{event}");
    }
    // And in the producer's own order — the panel's list order, which is the
    // one thing that says which judge spoke first.
    let seqs: Vec<u64> = decided
        .iter()
        .map(|event| event["seq"].as_u64().expect("a relayed seq"))
        .collect();
    assert!(seqs[0] < seqs[1], "the panel's order was lost: {decided:?}");

    // The report half. This run's own copy of the settled report carries the
    // same decisions at the schema the linked onejudge writes, read back through
    // that library's own type.
    let settlements = relayed(&world, "judged", EventKind::MemberSettled);
    let [settlement] = &settlements[..] else {
        panic!("{} settlements, not one", settlements.len());
    };
    let kept = onepipeline::views::RunPaths::under(&world.runs, "judged").report_for(
        settlement["stream"].as_str().expect("a stream"),
        settlement["seq"].as_u64().expect("a seq"),
    );
    let report: Value = serde_json::from_str(
        &std::fs::read_to_string(&kept).expect("this run kept its own copy of the report"),
    )
    .expect("the retained report is a document");
    assert_eq!(
        report["schema_version"],
        Value::from(onejudge::SCHEMA_VERSION),
        "the report is not at the schema the linked onejudge writes: {report}"
    );
    let judged: Vec<onejudge::JudgedTurn> =
        serde_json::from_value(report["judge_decisions"].clone()).unwrap_or_else(|error| {
            panic!("the report's judge_decisions are not onejudge's: {error}: {report}")
        });
    assert_eq!(judged.len(), 1, "{report}");
    assert_eq!(judged[0].turn, 1, "{report}");
    let recorded: Vec<(&str, &str, onejudge::Decision)> = judged[0]
        .decisions
        .iter()
        .map(|decision| {
            (
                decision.judge.as_str(),
                decision.kind.as_str(),
                decision.decision,
            )
        })
        .collect();
    assert_eq!(
        recorded,
        vec![
            ("lint", "llmlint", onejudge::Decision::Continue),
            ("reviewer", "oneharness", onejudge::Decision::Done),
        ],
        "the report records decisions the member never published: {report}"
    );

    // What this crate reads off that report and its settlement is unchanged by
    // the addition: the verdict that failed the node is still the reason the
    // node failed, and the transcript still renders the turn's words.
    world
        .run(&["results", "judged"])
        .exited(0)
        .out_has("verdict: 'the change builds' failed — cargo build fails in src/views.rs");
    world
        .run(&["transcript", "judged"])
        .exited(0)
        .out_has("tool_call bash")
        .out_has(ANSWERED);
}

/// A `.judged` line naming no judge is refused at the double's boundary, and
/// the refusal is what the dispatch settles on: a script read leniently would
/// publish a decision nobody wrote, and the journey above would pass on it.
#[test]
fn a_judged_line_naming_no_judge_fails_the_dispatch_and_publishes_no_decision() {
    // llmlint: ignore[tests_mirror_real_usage] the `.judged` script is external input to
    // this suite, and its refusal is met the way its author would meet it — through a real
    // dispatch settling the node — rather than by calling into the double.
    let world = World::new("judged-nameless");
    world.script(&format!("{NODE}.judged"), " |llmlint|continue|no label\n");
    let path = world.plan("nameless", &plan_of("nameless", vec![agent(NODE, &[])]));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file("nameless", "result.json").is_file()
    });

    assert_refused(
        &world,
        "nameless",
        "a `.judged` line reads \"|llmlint|continue|no label\", which names no judge",
    );
}

/// A `.judged` line naming a kind no judge of a panel can be is refused the
/// same way: the kinds are the linked sibling's three shapes, and a fourth is
/// a script the author got wrong rather than a judge to publish.
#[test]
fn a_judged_line_naming_a_kind_no_panel_has_fails_the_dispatch_and_publishes_no_decision() {
    // llmlint: ignore[tests_mirror_real_usage] the same boundary as the journey above, for
    // the one refusal that reads the linked sibling's kinds rather than the line's shape.
    let world = World::new("judged-unkind");
    world.script(
        &format!("{NODE}.judged"),
        "lint|llmlint|continue|two findings\nreviewer|human|done|looks fine to me\n",
    );
    let path = world.plan("unkind", &plan_of("unkind", vec![agent(NODE, &[])]));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file("unkind", "result.json").is_file()
    });

    assert_refused(
        &world,
        "unkind",
        "a `.judged` line names the judge kind \"human\", which is none of [\"oneharness\", \
         \"llmlint\", \"command\"]",
    );
}

/// What a dispatch the double refused at its script boundary settles on: the
/// node failed its task, the refusal is the detail a reader is shown, and no
/// decision or settlement reached the journal — a partly read panel published
/// nothing.
fn assert_refused(world: &World, run: &str, refusal: &str) {
    let node = world.run_json(run, "result.json")["nodes"][0].clone();
    assert_eq!(node["status"], "failed", "{node}");
    assert_eq!(node["outcome"], "task-failed", "{node}");
    let settled = world.events_of(run, "node-settled");
    let [settled] = &settled[..] else {
        panic!("{} settlements, not one: {settled:?}", settled.len());
    };
    assert_eq!(settled["payload"]["detail"], refusal, "{settled}");
    world
        .run(&["results", run])
        .exited(0)
        .out_has("task-failed")
        .out_has(refusal);
    assert!(
        relayed(world, run, EventKind::JudgeDecided).is_empty(),
        "a refused script still published a decision: {:?}",
        relayed(world, run, EventKind::JudgeDecided)
    );
    assert!(
        relayed(world, run, EventKind::MemberSettled).is_empty(),
        "a refused script still settled the member: {:?}",
        relayed(world, run, EventKind::MemberSettled)
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
/// [`MAX_PAYLOAD_TEXT_BYTES`] is *this crate's* promise about its own envelope
/// rather than a restatement of a producer's: both siblings publish inside it, so
/// on a stack whose pieces agree this never fires — but what arrives on a pipe
/// this process reads is whatever the thing on the other end wrote.
///
/// So the producer here is the `oneagentgraph` double, the one place a text past
/// the bound can be made to arrive, and it flags nothing: the cut and the flag are
/// the relay's own rather than a fixture handed to it.
///
/// Three fields on three kinds, because the rule is about a payload text and not
/// one field of one kind.
#[test]
fn a_relayed_payload_text_past_the_bound_is_cut_and_flagged_rather_than_served_whole() {
    let world = World::new("relay-bound");
    world.script(
        &format!("{NODE}.said-bytes"),
        &(MAX_PAYLOAD_TEXT_BYTES + 1).to_string(),
    );
    world.script(
        &format!("{NODE}.asked-bytes"),
        &(MAX_PAYLOAD_TEXT_BYTES * 2).to_string(),
    );
    let mut node = agent(NODE, &[]);
    let task = task_whose_bound_falls_inside_a_character();
    node["task"] = Value::String(task.clone());
    let path = world.plan("bounded", &plan_of("bounded", vec![node]));
    world.run(&["start", &path, "--attach"]).exited(0);

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

    // The opening's instruction, on a third kind. Well past the bound rather than
    // one byte over it, because what this adds is that the rule is the *payload
    // text*'s and not one field of one kind's: a relay that had grown a
    // per-field rule would leave this one whole.
    let opened = relayed(&world, "bounded", EventKind::TurnStarted);
    let [opening] = &opened[..] else {
        panic!(
            "the dispatch opened {} turns, not the one it takes",
            opened.len()
        );
    };
    // Read as the payload rather than through `TurnStarted`, and the reason is
    // the finding: that type has no `truncated` of its own and denies unknown
    // fields, so a payload this crate cut is **no longer** one the producer's
    // type reads. That is what a payload-wide flag costs on a kind whose producer
    // declares no per-field one, and it is the contract's own rule rather than
    // this journey's — `src/event.rs` says the payload carries `truncated: true`,
    // for every kind.
    assert_eq!(
        opening["payload"]["instruction"]
            .as_str()
            .expect("the opening carries the instruction it was given")
            .len(),
        MAX_PAYLOAD_TEXT_BYTES,
        "a turn's opening instruction past the bound reached the store at its own length: \
         {opening}"
    );
    assert_eq!(
        opening["payload"]["truncated"],
        Value::Bool(true),
        "{opening}"
    );
    // The producer's **own** per-field flag is left exactly as the producer left
    // it — absent, because it omits the flag when it did not cut. It says whether
    // *that library* cut the value, and a relay answering it for the producer
    // would be this crate claiming a cut somebody else did not make.
    assert_eq!(
        opening["payload"]["instruction_truncated"],
        Value::Null,
        "the relay answered a producer's own statement about its own field: {opening}"
    );

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
    plain.run(&["start", &path, "--attach"]).exited(0);
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
