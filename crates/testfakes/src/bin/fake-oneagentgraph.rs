//! A real `oneagentgraph` executable, scripted from a directory.
//!
//! It speaks the sibling's command surface — `run`, `reset-timer`, `interrupt`,
//! `health` — emits envelope NDJSON on stdout, and records every invocation so a
//! test can assert on what `onepipeline` actually asked for.
//!
//! It stands in for the real `oneagentgraph` so a journey can *state* a scenario
//! — a node that fails its gate, a dispatch held open, a driver that dies —
//! rather than arrange one out of real agent turns. That makes it an oracle, and
//! an oracle is only worth what it refuses: where the real CLI says no, this one
//! says no the same way. `tests/e2e/dispatch.rs` drives the real binary.

use onepipeline_testfakes as fake;
use std::process::ExitCode;

/// A readable document a settlement can be made to *point at* but which nothing
/// should ever read back.
///
/// One recognisable string, in a valid report shape — and in **both** valid
/// report shapes, because two readers open a retained report: a transcript is
/// read out of a two-party member's conversation, and a drafted change request
/// body out of a single-sided member's validated answer. A test asserts the
/// words below reach neither, so a reader that followed the path it was handed
/// fails on them rather than on a missing file.
pub const PLANTED: &str = r#"{"transcript":{"messages":[{"role":"assistant",
    "content":"planted-and-never-read"}]},
    "results":[{"harness":"claude-code","status":"ok","text":"planted-and-never-read",
    "structured":{"body":"planted-and-never-read"},"schema_valid":true}]}"#;

/// The exit code the real CLI answers an invalid configuration with — its own
/// constant, so a double cannot answer a refusal with a code the sibling stopped
/// using.
fn invalid_config() -> ExitCode {
    ExitCode::from(u8::try_from(oneagentgraph::error::EXIT_INVALID_CONFIG).unwrap_or(1))
}

/// The labels one envelope carries, with the conversation its turn belongs to
/// stamped on when `kind` names one and taken off when it does not — the same
/// two directions, under the same name, as the sibling's own `stamp_session`.
///
/// Both halves come from that library rather than from a copy of its rule here:
/// [`EventKind::carries_session`] decides *which* kinds carry the label and
/// [`session_label`] computes the value. The exclusion is the load-bearing half.
/// A consumer renders every labelled envelope that is not a `turn-activity` or a
/// `turn-interrupted` as one transcript turn, so a double that stamped a
/// heartbeat would be an oracle for a transcript nothing produces — and a
/// hand-written `"{stream}.{member}"` would keep writing the old value the day
/// that function's sanitising or its length bound moved.
///
/// [`EventKind::carries_session`]: oneagentgraph::event::EventKind::carries_session
/// [`session_label`]: oneagentgraph::event::session_label
fn stamp_session(
    labels: &serde_json::Map<String, serde_json::Value>,
    stream: &str,
    kind: oneagentgraph::event::EventKind,
) -> serde_json::Map<String, serde_json::Value> {
    let mut labels = labels.clone();
    let session = kind
        .carries_session()
        .then(|| labels.get("member").and_then(serde_json::Value::as_str))
        .flatten()
        .and_then(|member| oneagentgraph::event::session_label(stream, member));
    match session {
        Some(session) => labels.insert(
            oneagentgraph::event::SESSION_LABEL.to_string(),
            session.into(),
        ),
        None => labels.remove(oneagentgraph::event::SESSION_LABEL),
    };
    labels
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "oneagentgraph", &args);

    match args.first().map(String::as_str) {
        Some("run") => run(&args, &dir),
        Some("reset-timer") => reset_timer(&args, &dir),
        Some("interrupt") => interrupt(&args, &dir),
        Some("health") => {
            println!("fake-provider: 1 identity bound, 0% utilized");
            ExitCode::SUCCESS
        }
        Some(other) => fake::refuse(&format!("unknown oneagentgraph command '{other}'")),
        None => fake::refuse("oneagentgraph takes a command"),
    }
}

/// `oneagentgraph reset-timer RUN MEMBER`
fn reset_timer(args: &[String], dir: &std::path::Path) -> ExitCode {
    for (at, name) in [(1, "RUN"), (2, "MEMBER")] {
        if let Err(refusal) = fake::required(args, at, name) {
            return refusal;
        }
    }
    if dir.join("reset-timer.fail").exists() {
        eprintln!("no resettable schedule named that member");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

/// The exit code the real CLI answers "no controllable turn in flight" with —
/// its own constant, because that code is a *fact* rather than a failure and is
/// what this crate's `auto` fall-through and `live` refusal are both built on.
fn no_controllable_turn() -> ExitCode {
    ExitCode::from(u8::try_from(oneagentgraph::error::EXIT_NO_CONTROLLABLE_TURN).unwrap_or(1))
}

/// Where a dispatch records that it has a turn an `interrupt` can reach.
///
/// Keyed by the *graph run's* id, because that is the only handle `interrupt`
/// is given, and holding the member beside it so an interrupt naming another
/// member of the same run finds no turn — which is the answer the real verb
/// gives.
fn turn_record(dir: &std::path::Path, run: &str) -> std::path::PathBuf {
    dir.join("turns").join(fake::segment(run))
}

/// `oneagentgraph interrupt RUN MEMBER [--input TEXT]`
///
/// Its three answers are the real verb's, and each is scripted by the state a
/// held dispatch left behind rather than by a file the test writes: a run with a
/// turn open takes the redirection and exits 0, and one with none — never
/// opened, closed at settlement, or opened by another member — exits
/// [`EXIT_NO_CONTROLLABLE_TURN`] with the reason. Both publish the
/// `turn-interrupted` envelope, because the real verb publishes it either way.
///
/// [`EXIT_NO_CONTROLLABLE_TURN`]: oneagentgraph::error::EXIT_NO_CONTROLLABLE_TURN
fn interrupt(args: &[String], dir: &std::path::Path) -> ExitCode {
    let run = match fake::required(args, 1, "RUN") {
        Ok(run) => run,
        Err(refusal) => return refusal,
    };
    let member = match fake::required(args, 2, "MEMBER") {
        Ok(member) => member,
        Err(refusal) => return refusal,
    };
    // The sibling's own rule, through the sibling's own predicate: the member
    // becomes a path there, so a name that is not one is refused before
    // anything is addressed.
    if !oneagentgraph::config::is_member_name(&member) {
        eprintln!("oneagentgraph: member {member:?} would name a path outside the run");
        return invalid_config();
    }
    let input = fake::flag(args, "--input");
    // A blank redirection stops the turn and says nothing, which is `cancel`
    // spelled the long way round — the real verb refuses it rather than
    // delivering it.
    if input.as_deref().is_some_and(|text| text.trim().is_empty()) {
        eprintln!("oneagentgraph: --input: a redirection with no words in it says nothing");
        return invalid_config();
    }

    // A lever that broke: the delivery was attempted and failed, which the real
    // verb reports on stderr with the member-failed code rather than with the
    // "no turn" one. The distinction is the whole reason that code exists.
    if dir.join("interrupt.fail").exists() {
        eprintln!(
            "oneagentgraph: {run}: could not interrupt member {member}: the control socket refused"
        );
        return ExitCode::from(u8::try_from(oneagentgraph::error::EXIT_MEMBER_FAILED).unwrap_or(1));
    }

    let open = std::fs::read_to_string(turn_record(dir, &run))
        .ok()
        .and_then(|held| serde_json::from_str::<serde_json::Value>(&held).ok());
    let holding = open
        .as_ref()
        .filter(|held| held["member"] == serde_json::Value::String(member.clone()))
        .and_then(|held| held["key"].as_str().map(str::to_string));

    let reason = match &holding {
        Some(key) => {
            // What the running turn was told to do instead. The dispatch reads
            // it back when it is released, which is what makes a delivered
            // redirection change what the worker did rather than only what the
            // ledger says.
            if let Some(text) = &input {
                fake::append(&dir.join(format!("{key}.redirect")), text);
            }
            None
        }
        None => Some(
            "this member opened no controllable turn: its run is not listening, or the turn \
             has already ended"
                .to_string(),
        ),
    };

    // A line from a build whose envelope shape this one cannot read, emitted
    // *before* the good one, so a reader that stopped at it would lose the
    // answer that follows.
    if dir.join("interrupt.unreadable").exists() {
        println!("{{\"from\":\"a newer oneagentgraph\"}}");
    }
    // The verb's own stream — an envelope's `stream` is a unique id per producing
    // process, and this process is the one producing it — which is also half of
    // the conversation the interrupted turn belongs to.
    let stream = format!("{run}-interrupt-{}", std::process::id());
    let labels = serde_json::Map::from_iter([
        ("run_id".to_string(), serde_json::Value::from(run.clone())),
        (
            "member".to_string(),
            serde_json::Value::from(member.clone()),
        ),
    ]);
    println!(
        "{}",
        serde_json::json!({
            "v": 1,
            "ts": fake::now(),
            "stream": stream,
            "seq": 0,
            "source": "agentgraph",
            "kind": oneagentgraph::event::EventKind::TurnInterrupted.as_str(),
            "labels": stamp_session(
                &labels,
                &stream,
                oneagentgraph::event::EventKind::TurnInterrupted,
            ),
            "payload": {
                "member": member,
                "delivered": reason.is_none(),
                "input_bytes": input.as_ref().map_or(0, String::len),
                "reason": reason,
            },
        })
    );
    if reason.is_some() {
        return no_controllable_turn();
    }
    ExitCode::SUCCESS
}

/// `oneagentgraph run GRAPH --task T --output json [--label k=v]...`
fn run(args: &[String], dir: &std::path::Path) -> ExitCode {
    let graph = match fake::required(args, 1, "GRAPH") {
        Ok(graph) => graph,
        Err(refusal) => return refusal,
    };
    // The real CLI requires both, so a caller that stopped sending either would
    // otherwise go unnoticed here and fail only against the sibling itself.
    let Some(task) = fake::flag(args, "--task") else {
        return fake::refuse("oneagentgraph run requires --task");
    };
    if fake::flag(args, "--output").as_deref() != Some("json") {
        return fake::refuse("oneagentgraph run requires --output json");
    }
    // Every `--label` goes through the sibling's *own* parser, so this double
    // refuses exactly what the real CLI refuses — a key it stamps itself, one
    // that is not `k=v`, one that is not an identifier — and cannot drift from
    // it. A double that accepted a label the sibling reserves was the weak
    // oracle that let every dispatch be refused while this suite stayed green.
    for label in fake::flags(args, "--label") {
        if let Err(refusal) = oneagentgraph::run::parse_label(&label) {
            eprintln!("oneagentgraph: {refusal}");
            return invalid_config();
        }
    }

    // A refusal a launcher cannot catch by glancing: the real CLI validates
    // before it announces itself, and how long that takes is the host's
    // business — a config fetched over the network, a loaded machine. Scripted
    // in milliseconds so a journey can put one past any window a launcher might
    // have waited instead of waiting for an answer.
    if let Some(delay) = fake::node_script(dir, "run", "refuse-after") {
        let Ok(millis) = delay.trim().parse::<u64>() else {
            fake::fail(&format!(
                "run.refuse-after holds {delay:?}, which is not a number of milliseconds"
            ));
        };
        std::thread::sleep(std::time::Duration::from_millis(millis));
        eprintln!("oneagentgraph: invalid config: the graph names a member that does not exist");
        return invalid_config();
    }

    // A graph that neither announces itself nor exits — the third answer, and
    // the only one a launcher cannot wait out. The rendezvous is bounded by the
    // double's own timeout, so a launcher that failed to end this process leaves
    // a test failing on that rather than a stray one behind.
    if dir.join("run.hang").exists() {
        fake::wait_for(&dir.join("run.go"));
    }

    // A graph that finishes before it announces anything: it did what it was
    // given, so the launch succeeded even though nothing is driving anything
    // afterwards.
    if dir.join("run.exit-quietly").exists() {
        return ExitCode::SUCCESS;
    }

    // The dag-scope graph is the run's *observer*: its monitor member watches
    // and changes nothing, because `onepipeline start` drives the run itself.
    if graph.contains("dag-scope") {
        // Announced before any work, as the real CLI announces a run. This is
        // the line the launcher's startup handshake waits for; a double that
        // stayed silent until it settled would make every launch wait out the
        // whole run. A node's dispatch is not waited on that way, and scripting
        // one to produce *nothing* is a scenario this suite needs, so the
        // announcement belongs to the launched graph rather than to every run.
        announce(args, &graph);
        return fake::observe(dir);
    }

    let node = fake::label(args, "onepipeline.node").unwrap_or_else(|| "unknown".into());
    let step = fake::label(args, "onepipeline.step");
    let persona = fake::label(args, "onepipeline.persona");
    // A node dispatches under more than one persona — its own worker, and the
    // `pr-author` that drafts its change request — so a script may name either
    // the persona or the node/step it applies to.
    let key = match (&step, &persona) {
        (_, Some(persona)) if dir.join(format!("{node}.{persona}.fail")).exists() => {
            format!("{node}.{persona}")
        }
        (Some(step), _) => format!("{node}.{step}"),
        (None, _) => node.clone(),
    };

    // A dispatch scripted to produce *nothing* produces nothing at all — not
    // even the announcement a turn opens with. That is the whole case boundary
    // retry exists for, and a double that spoke first would turn every one of
    // those scenarios into an attempt that answered.
    let silent = dir.join(format!("{key}.silent")).exists();

    // `<key>.turn-open` announces the member and its turn *before* the work,
    // as the real sibling announces one, and leaves the turn addressable for as
    // long as the dispatch runs — which is what a live `context` delivery
    // reaches. A plain hold is the other scenario and stays the default: a
    // dispatch that has started and recorded nothing yet, which is what the
    // stall watch and the `UNDRIVEN` node row are about.
    if !silent && dir.join(format!("{key}.turn-open")).exists() {
        open_turn(args, dir, &key, &node, step.as_deref());
    }

    // Hold the dispatch open until the test releases it, so an edit, a stall
    // watch, or a driver death can happen while a node is genuinely in flight.
    //
    // `<key>.stops-when-interrupted` is a worker that *takes* the ask: a
    // redirection delivered into its open turn ends the hold exactly as the
    // test's own release would, so the dispatch stops on its own and nothing
    // has to reap it. Without it every held dispatch ignores an interrupt,
    // which is the other scenario — and a suite that could only script that
    // one cannot tell a turn that stopped politely from one that was killed.
    //
    // `<key>.ignores-the-ask` is the worker at the other extreme: it keeps the
    // hold through a `SIGTERM`, which is what a wedged dispatch looks like to
    // the teardown aimed at it — signalled, and still there. A suite without one
    // cannot tell a stop that ended a tree from one that only signalled it,
    // because every other dispatch here goes on the first ask.
    if dir.join(format!("{key}.ignores-the-ask")).exists() {
        fake::ignore_the_polite_ask();
    }
    if dir.join(format!("{key}.wait")).exists() {
        let go = dir.join(format!("{key}.go"));
        let until = if dir.join(format!("{key}.stops-when-interrupted")).exists() {
            vec![go, dir.join(format!("{key}.redirect"))]
        } else {
            vec![go]
        };
        hold(args, dir, &key, &node, step.as_deref(), &until);
    }
    // Whatever an `interrupt` delivered while the turn was held. Read after the
    // hold, because that is when the running turn would have acted on it.
    let redirected = fake::node_script(dir, &key, "redirect");
    close_turn(dir);

    // A dispatch that produces nothing and fails is the case boundary retry
    // exists for: the failure carries no work to lose.
    if silent {
        let attempts = dir.join(format!("{key}.attempts"));
        fake::append(&attempts, "attempt");
        let so_far = std::fs::read_to_string(&attempts)
            .unwrap_or_default()
            .lines()
            .count();
        // A scripted count that is not a count is a test that means something
        // other than what it says: read leniently it becomes "never recovers",
        // so the scenario written to prove recovery would prove the opposite.
        let recover_after: usize = match fake::node_script(dir, &key, "recover-after") {
            None => usize::MAX,
            Some(text) => match text.trim().parse() {
                Ok(after) => after,
                Err(_) => fake::fail(&format!(
                    "{key}.recover-after holds {text:?}, which is not an attempt count"
                )),
            },
        };
        if so_far < recover_after {
            eprintln!("provider refused before the first turn");
            return ExitCode::from(1);
        }
    }

    // A line from a build whose envelope shape this one cannot read. It is
    // emitted *before* the good one, so a reader that stopped at it would lose
    // the turn that follows.
    if dir.join(format!("{key}.unreadable")).exists() {
        println!("{{\"from\":\"a newer oneagentgraph\"}}");
    }
    emit(
        args,
        dir,
        &key,
        &node,
        step.as_deref(),
        &task,
        redirected.as_deref(),
    );

    // A redirection the running turn took changes what it *did*, not only what
    // it said: the work it leaves in the workspace is the redirection's, under a
    // name a journey can look for. This is the whole difference between a note
    // that was accepted and one that landed.
    if let Some(text) = &redirected {
        write_work(args, &format!("{}-redirected", fake::segment(&key)), text);
    }

    // A dispatch that changed nothing is a branch with nothing to publish, so a
    // journey that means to reach a real publication says what its worker wrote.
    // Scripted rather than always: every other journey here is about the
    // dispatch, and a file appearing in the workspace would be a change nobody
    // asked for.
    if let Some(body) = fake::node_script(dir, &key, "work") {
        write_work(args, &fake::segment(&key), &body);
    }

    // A worker that stops and puts a question to its manager, which is what the
    // operator's `ask-manager` wrapper is for. Scripted here because it is the
    // *agent's* behaviour, and an agent is what this program stands in for: every
    // other journey is about the dispatch, and a question nobody asked would be a
    // surface the planner never had a reason to get.
    if let Some(question) = fake::node_script(dir, &key, "asks") {
        fake::ask_manager(&question);
    }

    // Records on the session's own stream that no producer writes. Scripted here
    // because a stream is what the dispatch's session is being followed through,
    // and a record a reader cannot act on arrives the same way every other one
    // does.
    if let Some(script) = fake::node_script(dir, &key, "session-records") {
        session_records(args, &script);
    }

    // A worker that publishes its own branch before it is finished with. This is
    // the incident: an agent that ran `onevcs publish` in its final turn opened
    // a change request the engine's own publication step never ran, and then
    // failed its judge. Scripted here because it is the *agent's* behaviour, and
    // an agent is what this program stands in for.
    if let Some(title) = fake::node_script(dir, &key, "publishes") {
        publish_session(args, &title);
    }

    // Every candidate this dispatch's identity chains stepped past, published
    // one per candidate exactly as the real CLI publishes them. Scripted
    // `<key>.refused`, one `ROLE IDENTITY REASON` per line, with `-` for the
    // role of a single-sided member — which has one side and so nothing to
    // distinguish.
    if let Some(script) = fake::node_script(dir, &key, "refused") {
        refuse_candidates(args, &node, step.as_deref(), &script);
    }

    if let Some(code) = fake::node_script(dir, &key, "fail") {
        // A scripted code that is not a code is a test that means something
        // other than what it says. Defaulting it to 1 would quietly pass the
        // scenario the author did not write.
        let Ok(code) = code.parse::<u8>() else {
            fake::fail(&format!(
                "{key}.fail holds {code:?}, which is not an exit code"
            ));
        };
        eprintln!("the node failed its gate");
        return ExitCode::from(code);
    }
    ExitCode::SUCCESS
}

/// Publish one `fallback-advanced` per candidate an identity chain stepped past.
///
/// Built through the sibling's **own** payload type, so what this writes is what
/// that library writes: a double that hand-rolled the fields would be an oracle
/// for a payload nothing produces, and the whole point of the journey it serves
/// is that a consumer reads the identity and the side off the real one.
fn refuse_candidates(args: &[String], node: &str, step: Option<&str>, script: &str) {
    let labels = member_labels(args, node, step);
    // Above every seq `emit` uses — the turn's own envelopes and one per
    // invocation it published — because these are written after it: a
    // producer's seq is its own statement of the order it wrote things in.
    for (offset, line) in script
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        let mut columns = line.split_whitespace();
        // Exactly three, and the fourth column is checked for: a script this
        // read leniently would emit a candidate the test author did not write,
        // and a double that publishes something other than what its script says
        // is an oracle for nothing.
        let (Some(role), Some(identity), Some(reason), None) = (
            columns.next(),
            columns.next(),
            columns.next(),
            columns.next(),
        ) else {
            fake::fail(&format!(
                "a `.refused` line reads {line:?}, which is not `ROLE IDENTITY REASON`"
            ));
        };
        let advanced = oneagentgraph::event::FallbackAdvanced {
            identity: identity.to_string(),
            reason: reason.to_string(),
            // `-` is a single-sided member: one side, so no side to name.
            // Every word but `-` is read through the sibling's **own** `Role`, so
            // the script's grammar is that library's spelling rather than a copy
            // of it that keeps parsing after a rename.
            role: match role {
                "-" => None,
                other => Some(
                    serde_json::from_value::<oneagentgraph::event::Role>(other.into())
                        .unwrap_or_else(|error| {
                            fake::fail(&format!(
                                "a `.refused` line names the role {other:?}: {error}"
                            ))
                        }),
                ),
            },
            turn: Some(1),
        };
        // The sibling's **own** envelope, serialized through the sibling's own
        // type: a hand-rolled object here would be an independent copy of a
        // schema that library owns, and it would keep serializing after the
        // schema moved. The labels cross the same boundary — `Labels` carries
        // what it declares and flattens the rest, which is what a `--label` the
        // caller passed is.
        let envelope = oneagentgraph::event::Envelope {
            v: oneagentgraph::event::ENVELOPE_VERSION,
            ts: fake::now(),
            stream: stream(),
            seq: 100 + offset as u64,
            source: oneagentgraph::event::Source::Agentgraph,
            kind: oneagentgraph::event::EventKind::FallbackAdvanced,
            labels: serde_json::from_value(serde_json::Value::Object(labels.clone()))
                .unwrap_or_else(|error| fake::fail(&format!("the labels are not labels: {error}"))),
            payload: match serde_json::to_value(&advanced) {
                Ok(serde_json::Value::Object(payload)) => payload,
                other => fake::fail(&format!("an advance is not an object: {other:?}")),
            },
            artifacts: Vec::new(),
        };
        println!(
            "{}",
            serde_json::to_string(&envelope).unwrap_or_else(|error| fake::fail(&format!(
                "the envelope will not write: {error}"
            )))
        );
    }
}

/// Publish the session this dispatch is working in, as its own final act.
///
/// It runs the **real** `onevcs`, resolved from the `PATH` the process under
/// test was given and against the same state root — the one thing a double must
/// never do is answer *for* a sibling, and this does not: what is scripted here
/// is the agent's behaviour, and an agent is what this program stands in for. A
/// hand-written `change-opened` on the session's stream would be a second
/// producer of a record that library owns, and the journey would prove the
/// fixture rather than the composition.
///
/// The session's token is the name of the directory above the worktree —
/// `$ONEVCS_HOME/<identity>/runs/<token>/worktree` — which is the same
/// derivation `tests/e2e/gate.sh` documents and uses. A `--dir` that is not one
/// is a misconfigured test rather than a scenario, so it ends the process.
fn publish_session(args: &[String], title: &str) {
    let worktree = session_worktree(args, "publishing a dispatch's session");
    let token = session_token(&worktree);

    let published = std::process::Command::new("onevcs")
        .args(["publish", &token, "--title", title])
        .current_dir(&worktree)
        .stdin(std::process::Stdio::null())
        .output();
    let published = match published {
        Ok(published) => published,
        Err(error) => fake::fail(&format!("cannot run `onevcs publish {token}`: {error}")),
    };
    // Recorded whatever it answered, so a journey can assert the agent really
    // reached that sibling — and refused when it did not land, because a
    // publication nobody made is a journey asserting against a change request
    // that was never opened.
    fake::record(
        &fake::script_dir(),
        "onevcs",
        &[
            "publish".to_owned(),
            token.to_owned(),
            "--title".to_owned(),
            title.to_owned(),
        ],
    );
    if !published.status.success() {
        fake::fail(&format!(
            "`onevcs publish {token}` exited {}: {}",
            published.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&published.stderr).trim()
        ));
    }
}

/// The session worktree this dispatch is running in.
///
/// `--dir` is this process's external input, and a dispatch that names one which
/// is not a session worktree is a misconfigured test rather than a scenario, so
/// it ends the process.
fn session_worktree(args: &[String], what: &str) -> std::path::PathBuf {
    let Some(workspace) = fake::flag(args, "--dir") else {
        fake::fail(&format!("{what} needs its --dir to find the session from"));
    };
    let worktree = std::path::PathBuf::from(&workspace);
    if worktree.file_name().and_then(|name| name.to_str()) != Some("worktree") {
        fake::fail(&format!(
            "a dispatch scripted for {what} ran in {workspace}, which is not a session worktree"
        ));
    }
    worktree
}

/// The session's own token, which is the name of the directory above its
/// worktree — `$ONEVCS_HOME/<identity>/runs/<token>/worktree`, the same
/// derivation `tests/e2e/gate.sh` documents and uses.
fn session_token(worktree: &std::path::Path) -> String {
    let token = worktree
        .parent()
        .and_then(|run| run.file_name())
        .and_then(|token| token.to_str())
        .unwrap_or_else(|| fake::fail(&format!("no session token above {}", worktree.display())));
    // The name is walked out of `--dir`, which is this process's external input,
    // and it goes on to address a session and to name a file. A directory that is
    // not a token is a misconfigured test rather than a scenario, and saying so
    // here beats writing a stream somewhere nobody looks.
    if token.is_empty()
        || !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        fake::fail(&format!(
            "the directory above {} is {token:?}, which is no session token",
            worktree.display()
        ));
    }
    token.to_owned()
}

/// Write records onto the session's own stream, as a process holding its token.
///
/// A session's stream is a file any process holding the token appends to, and a
/// dispatch is one: it runs *inside* the session's worktree, and the token is
/// the name of the directory above it. What a reader of the merged store meets
/// is therefore whatever that file holds, and this is a double writing to it —
/// the same thing the `<key>.publishes` neighbour does through `onevcs`, one
/// layer lower down.
///
/// It is deliberately not how a `change-opened` is produced: a real command
/// makes those, so writing one here would prove the fixture rather than the
/// composition. What is written here is what no command makes — records whose
/// values a reader cannot act on — for the same reason the `<key>.unreadable`
/// line above is written by hand: a reader's refusal has no other producer.
///
/// Scripted `<key>.session-records`, a JSON array of `{token?, branch?, node?}`.
/// The values are the journey's; the record around them, and the stream it goes
/// on, are this dispatch's own session.
fn session_records(args: &[String], script: &str) {
    let worktree = session_worktree(args, "writing a session record");
    let token = session_token(&worktree);
    let Ok(serde_json::Value::Array(records)) = serde_json::from_str::<serde_json::Value>(script)
    else {
        fake::fail("a session-records script holds a JSON array of records");
    };
    let home = std::env::var("ONEVCS_HOME")
        .unwrap_or_else(|_| fake::fail("writing a session record needs ONEVCS_HOME"));
    let stream = std::path::Path::new(&home)
        .join("streams")
        .join(format!("{token}.ndjson"));
    let mut lines = String::new();
    for record in records {
        let serde_json::Value::Object(record) = record else {
            fake::fail(&format!(
                "a session-records script holds objects of {{token?, branch?, node?}}, and \
                 one element is {record}"
            ));
        };
        // Every key it declares and nothing else, each a string or `null`: a typo
        // would otherwise become a record the journey did not write and did not
        // mean, and it would assert against that record's refusal rather than the
        // one it was about. `null` is how a journey says a field is *absent*,
        // which is a different record from one naming it empty.
        for (key, value) in &record {
            if !["token", "branch", "node"].contains(&key.as_str()) {
                fake::fail(&format!(
                    "a session-records element names {key:?}, which is no field of \
                     {{token?, branch?, node?}}"
                ));
            }
            if !value.is_string() && !value.is_null() {
                fake::fail(&format!(
                    "a session-records element gives {key:?} as {value}, which is neither \
                     a string nor null"
                ));
            }
        }
        let named = |key: &str| record.get(key).and_then(serde_json::Value::as_str);
        // The session this dispatch is working in, and the shape `onevcs` writes
        // a record of one in — its own [`Session`], serialized as the payload the
        // library builds from the same value. What the script names replaces a
        // field of it; what it names as `null` takes the field out.
        let session = onevcs::Session {
            token: onevcs::SessionToken(named("token").unwrap_or(&token).to_owned()),
            worktree: worktree.clone(),
            branch: named("branch").unwrap_or("onevcs/scripted").to_owned(),
            base: "main".to_owned(),
        };
        let Ok(serde_json::Value::Object(mut payload)) = serde_json::to_value(&session) else {
            fake::fail("a onevcs session no longer renders as an event payload");
        };
        for key in ["token", "branch"] {
            if record.get(key).is_some_and(serde_json::Value::is_null) {
                payload.remove(key);
            }
        }
        let envelope = onevcs::Envelope {
            v: 1,
            ts: fake::now(),
            stream: token.clone(),
            // A number the session's own writer never uses: `onevcs` numbers a
            // stream from one, so this cannot move the mark a follow reads on
            // from and cannot hide a record the session really wrote.
            seq: 0,
            source: onevcs::Source::Vcs,
            kind: onevcs::EventKind::SessionOpened,
            labels: onevcs::Labels {
                node: named("node").map(str::to_owned),
                ..onevcs::Labels::default()
            },
            payload,
            artifacts: Vec::new(),
        };
        match serde_json::to_string(&envelope) {
            Ok(line) => {
                lines.push_str(&line);
                lines.push('\n');
            }
            Err(error) => fake::fail(&format!("cannot render a session record: {error}")),
        }
    }
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stream)
        .and_then(|mut file| std::io::Write::write_all(&mut file, lines.as_bytes()));
    if let Err(error) = written {
        fake::fail(&format!(
            "cannot write the session stream at {}: {error}",
            stream.display()
        ));
    }
}

/// Write one document into the dispatch's workspace.
///
/// `--dir` is this process's external input, and these writes are the one thing
/// here that touches a path outside its own scratch. The real `oneagentgraph`
/// resolves a workspace before it prepares a member, so a value that is not one
/// is a misconfigured test rather than a scenario.
fn write_work(args: &[String], name: &str, body: &str) {
    let Some(workspace) = fake::flag(args, "--dir") else {
        fake::fail("writing a dispatch's work needs its --dir to write into");
    };
    let workspace = std::path::Path::new(&workspace);
    if !workspace.is_dir() {
        fake::fail(&format!(
            "a dispatch was given --dir {}, which is not a directory",
            workspace.display()
        ));
    }
    let path = workspace.join(format!("{name}.md"));
    if let Err(error) = std::fs::write(&path, body) {
        fake::fail(&format!("cannot write {}: {error}", path.display()));
    }
}

/// Announce the member and the turn it is starting, and record where that turn
/// can be reached.
///
/// Two envelopes the real sibling really emits, in the order it emits them, and
/// both carrying the member — which is half the address an `interrupt` takes.
/// The other half is the graph run's own id, which every envelope already
/// carries.
///
/// A member with no lever at all is scripted with `<key>.no-lever`: it announces
/// its turn like any other and records nothing, so an `interrupt` finds no turn
/// to reach. That is the case a harness without out-of-band control produces,
/// and it is the one `auto` must fall through on.
///
/// `<key>.unplaceable-member-start` and `<key>.unplaceable-turn-start` announce
/// the member's arrival and the turn behind it on a clock this build cannot
/// read, each on its own. Scripted together they are a producer whose every
/// envelope so far a reader refuses — one whose stamps are rejected, not one
/// that said nothing. Scripted singly they are a clock that comes back one
/// envelope in, or one that fails one envelope in, and a reader has something
/// different to say about all three.
///
/// `<key>.also-member` names a **second** member of the same graph run, which
/// announces a turn of its own. A graph is a graph — several members work under
/// one run — and a caller that addressed only the last member it saw would leave
/// the others working. The second member's turn is announced and not recorded,
/// so an interrupt sent to it answers as one whose turn is over: two members,
/// two answers, which is what a caller has to carry on from.
fn open_turn(args: &[String], dir: &std::path::Path, key: &str, node: &str, step: Option<&str>) {
    let labels = member_labels(args, node, step);
    let unplaceable = [
        dir.join(format!("{key}.unplaceable-member-start")).exists(),
        dir.join(format!("{key}.unplaceable-turn-start")).exists(),
    ];
    let stamp = |seq: usize| {
        if unplaceable[seq] {
            unplaceable_now()
        } else {
            fake::now()
        }
    };
    // The member's arrival and the turn behind it, and only the second of them
    // names a conversation — which is the pair a consumer tells a transcript
    // turn from everything else by.
    for (seq, kind) in [
        (0, oneagentgraph::event::EventKind::MemberStarted),
        (1, oneagentgraph::event::EventKind::TurnStarted),
    ] {
        println!(
            "{}",
            serde_json::json!({
                "v": 1,
                "ts": stamp(seq),
                "stream": stream(),
                "seq": seq,
                "source": "agentgraph",
                "kind": kind.as_str(),
                "labels": stamp_session(&labels, &stream(), kind),
                "payload": {},
            })
        );
    }
    if let Some(member) = fake::node_script(dir, key, "also-member") {
        let mut labels = labels.clone();
        labels.insert("member".to_string(), member.into());
        let kind = oneagentgraph::event::EventKind::TurnStarted;
        println!(
            "{}",
            serde_json::json!({
                "v": 1,
                "ts": fake::now(),
                "stream": stream(),
                "seq": 5,
                "source": "agentgraph",
                "kind": kind.as_str(),
                // A second member of the one graph run is a second
                // conversation, because the label joins the stream to the
                // *member*: two members on one stream that shared a session
                // would render as one transcript.
                "labels": stamp_session(&labels, &stream(), kind),
                "payload": {},
            })
        );
    }
    if dir.join(format!("{key}.no-lever")).exists() {
        return;
    }
    // Written, not attempted: a record this double failed to write reads to
    // every later `interrupt` as a member with no controllable turn, which is a
    // real scenario this suite scripts on purpose. A setup failure wearing that
    // answer's clothes would pass the `auto` fall-through journey while proving
    // nothing, so it ends the process instead — the same rule `fake::append`
    // holds for the invocation log.
    let record = turn_record(dir, &graph_run());
    if let Some(parent) = record.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            fake::fail(&format!(
                "cannot make {} to open a turn in: {error}",
                parent.display()
            ));
        }
    }
    if let Err(error) = std::fs::write(
        &record,
        serde_json::json!({"key": key, "member": "worker"}).to_string(),
    ) {
        fake::fail(&format!(
            "cannot open a turn at {}: {error}",
            record.display()
        ));
    }
}

/// Hold the dispatch until the test releases it, heartbeating while it waits
/// where the script asks for one.
///
/// A held dispatch is silent by default: a worker that has started and recorded
/// nothing. `<key>.heartbeat` holds a number of milliseconds and makes it the
/// *other* case — a member alive and producing nothing, publishing the
/// sibling's own `member-heartbeat` on that clock for as long as the hold
/// lasts. The real one fires about every fifteen seconds; a journey states its
/// own so the stall it is about lands inside a test's patience.
///
/// `<key>.unplaceable-beats-after-the-first` keeps beating on a clock that stops
/// being readable: one placeable beat, then a stream of unplaceable ones, which
/// is what a reader has to keep reporting liveness through.
fn hold(
    args: &[String],
    dir: &std::path::Path,
    key: &str,
    node: &str,
    step: Option<&str>,
    until: &[std::path::PathBuf],
) {
    let Some(every) = fake::node_script(dir, key, "heartbeat") else {
        fake::wait_for_any(until);
        return;
    };
    // A scripted interval that is not one is a test that means something other
    // than what it says: read leniently it becomes a hold that never beats,
    // which is the scenario this scripting exists to tell apart from.
    let every = match every.trim().parse::<u64>() {
        Ok(millis) if millis > 0 => std::time::Duration::from_millis(millis),
        _ => fake::fail(&format!(
            "{key}.heartbeat holds {every:?}, which is not a positive number of milliseconds"
        )),
    };
    let labels = member_labels(args, node, step);
    // A producer whose clock stops being readable partway through the hold:
    // every beat but the first carries a stamp this build cannot place in time.
    // The first one still can be, which is the whole scenario — what a reader
    // has left to report liveness by once the rest arrive unplaceable.
    let loses_the_clock = dir
        .join(format!("{key}.unplaceable-beats-after-the-first"))
        .exists();
    // Above every `seq` the turn's own envelopes use, so a beat can never be
    // taken for one of them.
    let mut seq = 100;
    fake::wait_for_any_ticking(until, every, &mut || {
        let stamp = if loses_the_clock && seq > 100 {
            unplaceable_now()
        } else {
            fake::now()
        };
        publish(&serde_json::json!({
            "v": 1,
            "ts": stamp,
            "stream": stream(),
            "seq": seq,
            "source": "agentgraph",
            // The producing library's own spelling of the kind, read off its
            // enum: a double that hand-wrote the word would keep emitting it
            // after the sibling renamed it, which is an oracle for a stream
            // nothing produces.
            "kind": oneagentgraph::event::EventKind::MemberHeartbeat.as_str(),
            // Through the same stamp as every other envelope, which leaves a
            // beat with no conversation on it: a member publishes thousands
            // over a run, and a consumer counting labelled envelopes as
            // transcript turns would count every one of them.
            "labels": stamp_session(
                &labels,
                &stream(),
                oneagentgraph::event::EventKind::MemberHeartbeat,
            ),
            "payload": {},
        }));
        seq += 1;
    });
}

/// Write one envelope to the stream this double publishes on.
///
/// A fallible write rather than `println!`, which panics: the reader on the
/// other end can go away — a driver that stopped waiting closes the pipe — and
/// that is an ordinary I/O error rather than a scenario. Unwinding out of it
/// would reach a test as a double that crashed, which is the one thing this
/// program must never look like, so the error is reported and the process ends
/// the way every other failure a double cannot act out ends.
fn publish(envelope: &serde_json::Value) {
    use std::io::Write;
    if let Err(error) = writeln!(std::io::stdout().lock(), "{envelope}") {
        fake::fail(&format!(
            "cannot publish a {} envelope: {error}",
            envelope["kind"]
        ));
    }
}

/// Now, stamped the way a producer this build cannot read stamps it.
///
/// A real RFC 3339 instant with a numeric UTC offset instead of `Z` — which the
/// reader refuses, because the envelope fixes one spelling and a stranger's
/// clock must never become this run's timing evidence. Derived from the real
/// clock rather than frozen, so what a journey scripts is a producer whose
/// stamps cannot be *placed*, not one stuck at some moment in 2001.
fn unplaceable_now() -> String {
    format!("{}+00:00", fake::now().trim_end_matches('Z'))
}

/// The turn is over, so nothing can be delivered into it any more.
fn close_turn(dir: &std::path::Path) {
    let _ = std::fs::remove_file(turn_record(dir, &graph_run()));
}

/// Announce the run, as the sibling's first line does.
fn announce(args: &[String], graph: &str) {
    println!(
        "{}",
        serde_json::json!({
            "v": 1,
            "ts": fake::now(),
            "stream": stream(),
            "seq": 0,
            "source": "agentgraph",
            "kind": "graph-started",
            "labels": stamped(args),
            "payload": {"graph": graph},
        })
    );
}

/// The labels the sibling stamps: its own run and member, and every `--label`
/// the caller passed, carried through verbatim beside them. The two `run_id`s on
/// one line are the point — a graph run is not the pipeline run that dispatched
/// it.
fn stamped(args: &[String]) -> serde_json::Map<String, serde_json::Value> {
    let mut labels = serde_json::Map::new();
    labels.insert("run_id".to_string(), graph_run().into());
    for pair in fake::flags(args, "--label") {
        if let Some((key, value)) = pair.split_once('=') {
            labels.insert(key.to_string(), value.into());
        }
    }
    labels
}

/// The labels every envelope one member produces carries.
///
/// The sibling's own — its run and the member within it, which together are what
/// an `interrupt` addresses a turn by — plus the node and step this run is
/// acting out. Those two are stated rather than echoed: a node with no `--label`
/// of its own still belongs to one.
fn member_labels(
    args: &[String],
    node: &str,
    step: Option<&str>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut labels = stamped(args);
    labels.insert("member".to_string(), "worker".into());
    labels.insert("onepipeline.node".to_string(), node.into());
    if let Some(step) = step {
        labels.insert("onepipeline.step".to_string(), step.into());
    }
    labels
}

/// This graph run's own id, which is not the id of the run that started it.
fn graph_run() -> String {
    format!("fake-graph-{}", std::process::id())
}

/// The stream every envelope of this run carries.
fn stream() -> String {
    format!("fake-oneagentgraph-{}", std::process::id())
}

/// Emit the turn's envelopes, as the sibling would: the tool summary from
/// inside the turn, what the turn consumed, and the settlement that stores the
/// member's full report.
///
/// Three kinds the real CLI really emits. A double that answered a dispatch
/// with a kind of its own would be an oracle for a stream nothing produces —
/// the same weakness that let every dispatch be refused while this suite stayed
/// green.
///
/// `redirected` is what an `interrupt` delivered into this turn while it ran, if
/// anything did. It rides the activity summary because that is where the sibling
/// reports what the turn is doing, and a turn that took a redirection is doing
/// something else.
fn emit(
    args: &[String],
    dir: &std::path::Path,
    key: &str,
    node: &str,
    step: Option<&str>,
    task: &str,
    redirected: Option<&str>,
) {
    let labels = member_labels(args, node, step);
    let stepped_clock = dir.join(format!("{key}.clock-stepped")).exists();
    let duplicate_seq = dir.join(format!("{key}.duplicate-seq")).exists();
    // A producer whose host clock was stepped **backwards** between two records
    // it wrote: its `seq` still runs forward, because a producer knows what
    // order it wrote things in, but its timestamps no longer agree with that.
    // Real, and not rare — a clock correction under a running process does it —
    // and it is the case a consumer sorting by `ts` gets wrong. Only when
    // scripted; every other journey wants an ordinary clock.
    //
    // The earlier stamp is far enough back to be unambiguous: a reader ordering
    // by the clock puts this record before everything, which is exactly the
    // reordering the merge must not do inside one stream.
    const STEPPED_BACK: &str = "2001-01-01T00:00:00.000Z";
    let stamp = move |seq: u64| match (stepped_clock, seq) {
        (true, 3) => STEPPED_BACK.to_string(),
        _ => fake::now(),
    };
    let envelope = |seq: u64, kind: oneagentgraph::event::EventKind, payload: serde_json::Value| {
        println!(
            "{}",
            serde_json::json!({
                "v": 1,
                "ts": stamp(seq),
                "stream": stream(),
                "seq": seq,
                "source": "agentgraph",
                "kind": kind.as_str(),
                "labels": stamp_session(&labels, &stream(), kind),
                "payload": payload,
            })
        );
    };

    envelope(
        2,
        oneagentgraph::event::EventKind::TurnActivity,
        serde_json::json!({
            "kind": "tool_call",
            "name": "bash",
            "detail": "echo the turn ran",
            "message": "the dispatch ran",
            // Echoed so a test can assert the task prose the node was given,
            // including the rendered planner-context section.
            "task": task,
            "dir": fake::flag(args, "--dir"),
            // What this turn was told to do instead, while it was running.
            "redirected": redirected,
        }),
    );
    // A producer that stamps one `seq` on two records. Only a producer in error
    // does it, and the merge has nothing to be right about beyond being stable —
    // which is exactly why it needs saying: a store that shuffled these under a
    // second reading would be a run whose record changed when it was reread.
    if duplicate_seq {
        envelope(
            2,
            oneagentgraph::event::EventKind::TurnActivity,
            serde_json::json!({
                "kind": "tool_call",
                "name": "bash",
                "detail": "echo the turn ran again",
                "message": "the dispatch ran again",
            }),
        );
    }
    // Where this turn's conversation was written down. Published per oneharness
    // invocation, as the real member publishes it, and before the turn it
    // belongs to completes — one per side per turn, which is what pairs with
    // the candidates that side's chain stepped past.
    for (offset, invocation) in served_invocations(dir, key).iter().enumerate() {
        publish_oneharness_session(&labels, node, offset, invocation);
    }
    let report = report_of(task, scripted_verdicts(dir, key));
    envelope(
        3,
        oneagentgraph::event::EventKind::TurnCompleted,
        serde_json::json!({"usage": report["usage"]}),
    );
    // The report is *stored*, and the settlement says where — the sibling's own
    // contract, and the only reason a turn's tools and words survive the
    // process that produced them. Under a directory of this member's own,
    // named exactly as the real library names it: a consumer only reads a
    // `report_path` that names the file `oneagentgraph` itself writes.
    let path = fake::script_dir()
        .join("reports")
        .join(format!("{}-{}", fake::segment(node), std::process::id()))
        .join(oneagentgraph::member::REPORT_FILE);
    // Where the settlement says the report went. Each branch is a real thing a
    // producer — or something wearing one's clothes — can put on that line, and
    // a consumer has to tell every one of them from a report it may read.
    let scripted = |name: &str| fake::script_dir().join(name).exists();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let named = if scripted("report.elsewhere") {
        // A readable file the producing library never writes, under a name
        // nothing should follow.
        let planted = path.with_file_name("notes.json");
        let _ = std::fs::write(&planted, PLANTED);
        Some(planted)
    } else if scripted("report.symlink") {
        // A path wearing the producer's own file name that *delivers* another
        // file. The one case a name check alone cannot catch.
        let secret = path.with_file_name("secret.json");
        let _ = std::fs::write(&secret, PLANTED);
        let link = path.with_file_name("linked");
        let _ = std::fs::create_dir_all(&link);
        let link = link.join(oneagentgraph::member::REPORT_FILE);
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&secret, &link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&secret, &link);
        made.is_ok().then_some(link)
    } else if scripted("report.directory") {
        // A path that is not a file at all, wearing the report's name.
        let dir = path.with_file_name("as-a-directory");
        let _ = std::fs::create_dir_all(dir.join(oneagentgraph::member::REPORT_FILE));
        Some(dir.join(oneagentgraph::member::REPORT_FILE))
    } else if scripted("report.oversize") {
        // Far past what a consumer will copy. Claimed rather than written: the
        // bound is on the size the filesystem reports, and writing the bytes
        // would be a slow way to say the same thing.
        let _ = std::fs::write(&path, PLANTED);
        let sized = std::fs::OpenOptions::new().write(true).open(&path);
        if let Ok(file) = sized {
            let _ = file.set_len(64 * 1024 * 1024);
        }
        Some(path.clone())
    } else if scripted("report.missing") {
        // A member that settled on a machine whose scratch this reader cannot
        // reach: the settlement names where the report went and nothing wrote
        // one there. The evidence is missing, not the settlement.
        Some(path.clone())
    } else {
        // A report document a harness produced without a transcript: the
        // verdict fields the settlement reads inline, and nothing else.
        let document = if scripted("report.bare") {
            serde_json::json!({"completion_reason": "done_when_met", "verdicts": []})
        } else {
            report.clone()
        };
        std::fs::write(&path, document.to_string())
            .is_ok()
            .then(|| path.clone())
    };
    envelope(
        4,
        oneagentgraph::event::EventKind::MemberSettled,
        serde_json::json!({
            "completed": true,
            // The report's **own** verdicts, copied onto the settlement exactly
            // as the real member copies them: a consumer reads what failed a
            // node's judge off this line, and a double that published an empty
            // list beside a report that carried verdicts would be an oracle for
            // a settlement nothing writes.
            "verdict": report.get("verdicts").cloned()
                .unwrap_or(serde_json::Value::Array(Vec::new())),
            "completion_reason": "done_when_met",
            "report_path": named.map(|path| path.display().to_string()),
        }),
    );
}

/// Every oneharness invocation this dispatch's member actually ran.
///
/// Scripted `<key>.served`, one `ROLE TURN IDENTITY` per line. With nothing
/// scripted it is the one invocation an ordinary turn makes — the agent side's
/// first — which is what every other journey here expects of a dispatch.
///
/// The role goes through the sibling's **own** [`Role`], so the script's grammar
/// is that library's spelling rather than a copy of it that keeps parsing after
/// a rename, and a line that is not three columns is fatal: a script read
/// leniently would publish an invocation the test author did not write, and a
/// double that publishes something other than what its script says is an oracle
/// for nothing.
///
/// [`Role`]: oneagentgraph::event::Role
fn served_invocations(
    dir: &std::path::Path,
    key: &str,
) -> Vec<(oneagentgraph::event::Role, u64, String)> {
    let Some(script) = fake::node_script(dir, key, "served") else {
        return vec![(
            oneagentgraph::event::Role::Agent,
            1,
            "fake-provider/claude-code".to_string(),
        )];
    };
    script
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut columns = line.split_whitespace();
            let (Some(role), Some(turn), Some(identity), None) = (
                columns.next(),
                columns.next(),
                columns.next(),
                columns.next(),
            ) else {
                fake::fail(&format!(
                    "a `.served` line reads {line:?}, which is not `ROLE TURN IDENTITY`"
                ));
            };
            let role = serde_json::from_value::<oneagentgraph::event::Role>(role.into())
                .unwrap_or_else(|error| {
                    fake::fail(&format!(
                        "a `.served` line names the role {role:?}: {error}"
                    ))
                });
            let Ok(turn) = turn.parse::<u64>() else {
                fake::fail(&format!(
                    "a `.served` line names the turn {turn:?}, which is not a turn number"
                ));
            };
            (role, turn, identity.to_string())
        })
        .collect()
}

/// The verdicts this dispatch's member settles with.
///
/// Scripted `<key>.verdict`, one `VALUE|CRITERION|REASON` per line, where
/// `VALUE` is `true` or `false` — a boolean verdict, which is the only kind
/// onejudge fails a run over. Nothing scripted is a member that was scored
/// against nothing, which is what every other journey here settles as.
///
/// Pipe-separated rather than by whitespace, because a judge's reason is a
/// sentence and splitting it on spaces would keep only its first word.
fn scripted_verdicts(dir: &std::path::Path, key: &str) -> Vec<serde_json::Value> {
    let Some(script) = fake::node_script(dir, key, "verdict") else {
        return Vec::new();
    };
    script
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut columns = line.split('|');
            let (Some(value), Some(criterion), Some(reason), None) = (
                columns.next(),
                columns.next(),
                columns.next(),
                columns.next(),
            ) else {
                fake::fail(&format!(
                    "a `.verdict` line reads {line:?}, which is not `VALUE|CRITERION|REASON`"
                ));
            };
            let Ok(value) = value.trim().parse::<bool>() else {
                fake::fail(&format!(
                    "a `.verdict` line names the value {value:?}, which is not `true` or `false`"
                ));
            };
            // onejudge's own `NamedVerdict` shape: the criterion, the kind of
            // judgement, and the verdict itself. Written out here rather than
            // built through that library's type because nothing in this
            // workspace depends on it — the report document beside it is
            // written the same way, and what holds both honest is the journey
            // driving the real `oneagentgraph`.
            serde_json::json!({
                "criterion": criterion.trim(),
                "kind": "boolean",
                "verdict": {"value": value, "reason": reason.trim()},
            })
        })
        .collect()
}

/// Publish where this turn's oneharness invocation wrote its conversation down.
///
/// The pointer a consumer renders an agent's actual transcript from, and the
/// half of the session contract that is not a label: the payload names the
/// history record and the three arguments its reader takes, and the artifact
/// beside it is that record, under the `kind` the sibling names it by.
///
/// Built through the sibling's **own** [`OneharnessSession`] and [`Artifact`]
/// types — like [`refuse_candidates`] above, and for the same reason: a
/// hand-rolled object here would be a second reading of a schema that library
/// owns, and it would keep serializing after the schema moved.
///
/// The record is really written, into this double's own scratch, so the artifact
/// reference names a file that exists and has a size. A consumer that followed
/// the path it was handed finds a conversation there rather than nothing.
///
/// [`OneharnessSession`]: oneagentgraph::event::OneharnessSession
/// [`Artifact`]: oneagentgraph::event::Artifact
fn publish_oneharness_session(
    labels: &serde_json::Map<String, serde_json::Value>,
    node: &str,
    offset: usize,
    invocation: &(oneagentgraph::event::Role, u64, String),
) {
    let (role, turn, identity) = invocation;
    let record = format!("{}-{}-{offset}", fake::segment(node), std::process::id());
    let store = fake::script_dir().join("history");
    let project = "fake-project";
    let path = store.join(project).join(format!("{record}.jsonl"));
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            fake::fail(&format!(
                "cannot make {} to write a session into: {error}",
                parent.display()
            ));
        }
    }
    // One conversation, in the shape oneharness records one. Written rather
    // than claimed: a reference to a file nobody wrote is the missing-evidence
    // scenario, and this is not it.
    let conversation =
        serde_json::json!({"role": "assistant", "content": "Ran what the task asked for."});
    let body = format!("{conversation}\n");
    if let Err(error) = std::fs::write(&path, &body) {
        fake::fail(&format!("cannot write {}: {error}", path.display()));
    }
    let session = oneagentgraph::event::OneharnessSession {
        role: *role,
        turn: *turn,
        identity: identity.clone(),
        session_id: Some(format!("fake-harness-{record}")),
        history_id: record.clone(),
        history_dir: store.display().to_string(),
        history_project: project.to_string(),
        history_session: record.clone(),
    };
    let artifact = oneagentgraph::event::Artifact {
        // The payload's own `history_id`, because the sibling's contract is that
        // the record the invocation wrote *is* the artifact beside it.
        id: record,
        kind: oneagentgraph::event::ONEHARNESS_SESSION_ARTIFACT.to_string(),
        bytes: body.len() as u64,
    };
    let kind = oneagentgraph::event::EventKind::OneharnessSession;
    // Through `publish` rather than `println!`: the reader on the other end can
    // go away, and a driver that stopped waiting closes the pipe. That is an
    // ordinary I/O error, and unwinding out of it would reach a test as a double
    // that crashed.
    publish(&serde_json::json!({
        "v": 1,
        "ts": fake::now(),
        "stream": stream(),
        // Above the turn's own envelopes and below the candidates a chain
        // stepped past, so no reader can take it for either. One per
        // invocation, because a producer's seq is its own statement of the
        // order it wrote things in and two records cannot share one.
        "seq": 6 + offset as u64,
        "source": "agentgraph",
        "kind": kind.as_str(),
        // No conversation on it: the record *names* one, and a consumer that
        // read this as a transcript turn would render the pointer beside the
        // thing it points at.
        "labels": stamp_session(labels, &stream(), kind),
        "payload": match serde_json::to_value(&session) {
            Ok(payload) => payload,
            Err(error) => fake::fail(&format!("a session is not an object: {error}")),
        },
        "artifacts": [match serde_json::to_value(&artifact) {
            Ok(artifact) => artifact,
            Err(error) => fake::fail(&format!("an artifact is not an object: {error}")),
        }],
    }));
}

/// The report a settled member stores.
///
/// Two-party, because the shipped node-scope graph's `worker` is a two-party
/// member: its agent side does the work and its judge side supervises it, and
/// the split between what each spent is what the report carries and nothing on
/// the wire does.
///
/// A **drafting** dispatch's member is single-sided instead, and its graph asks
/// oneharness for an answer validated against a schema — so its report carries
/// the run's per-harness `results`, and the validated answer is at
/// `results[].structured` of the one that ran. That is the channel this stack
/// reads a drafted change request body out of, so it is what this double
/// answers a `pr-author` dispatch with.
fn report_of(task: &str, verdicts: Vec<serde_json::Value>) -> serde_json::Value {
    if let Some(answer) = drafted_answer(task, &fake::script_dir()) {
        // A candidate the identity chain stepped past: it ran nothing, so it
        // answered nothing, and a consumer that read the first entry rather than
        // the one that ran would find no body here.
        let stepped_past = serde_json::json!({
            "harness": "codex", "status": "skipped", "text": null,
            "structured": null, "schema_valid": null,
        });
        let answered = |harness: &str, body: &str, schema_valid: bool| {
            serde_json::json!({
                "harness": harness, "status": "ok",
                "text": "Drafted the change request's body.",
                "structured": {"body": body}, "schema_valid": schema_valid,
            })
        };
        // The wire shape is a value and a flag per candidate, and only three of
        // their four combinations mean anything — which is what [`Drafted`] is:
        // the flag is derived from the answer here rather than scripted beside
        // it. A blank answer is written as the **chain** that produces one in
        // practice: an identity whose answer the schema refused, and the next
        // one, which conformed and put nothing in it. A consumer that stopped at
        // the refusal would report a schema that is working as the fault.
        let results = match &answer {
            Drafted::Body(body) => vec![stepped_past, answered("claude-code", body, true)],
            Drafted::SchemaRefused(attempted) => {
                vec![stepped_past, answered("claude-code", attempted, false)]
            }
            Drafted::Bodyless => vec![
                stepped_past,
                answered("codex", "half a bo", false),
                answered("claude-code", "", true),
            ],
        };
        return serde_json::json!({
            "schema_version": "0.6",
            "results": results,
            "usage": {"input_tokens": 40, "output_tokens": 20, "cost_usd": 0.01},
        });
    }
    serde_json::json!({
        "schema_version": 7,
        "transcript": {"messages": [
            {"role": "user", "content": task},
            {"role": "assistant", "content": "Ran what the task asked for.", "events": [
                {"kind": "tool_call", "name": "bash",
                 "input": {"command": "echo the turn ran"}, "index": 0},
            ]},
        ]},
        "verdicts": verdicts,
        "completion_reason": "done_when_met",
        "usage": {
            "input_tokens": 1_200, "output_tokens": 340,
            "cache_read_tokens": 900, "cache_write_tokens": 120, "cost_usd": 0.42,
        },
        "telemetry": {
            "wall_ms": 1_000,
            "agent": {"model_ms": 800, "usage": {
                "input_tokens": 1_000, "output_tokens": 300,
                "cache_read_tokens": 900, "cache_write_tokens": 120, "cost_usd": 0.40,
            }},
            "judge": {"model_ms": 100, "usage": {
                "input_tokens": 200, "output_tokens": 40, "cost_usd": 0.02,
            }},
            "orchestration_ms": 100,
            "sessions": [],
        },
    })
}

/// What a `pr-author` dispatch's turn answered with.
///
/// Three answers rather than a body and a flag beside it, because only three of
/// that pair's four combinations mean anything: prose the schema accepted, prose
/// it refused, and an answer inside the schema with nothing in it. The fourth —
/// a refused answer that is nonetheless the body to publish — is a state the
/// consumer must never see, so this double cannot script one.
enum Drafted {
    /// A validated answer carrying the prose the change request opens with.
    Body(String),
    /// An answer the schema **refused**, holding the value the turn last
    /// attempted — which the real library retains beside the flag, so a consumer
    /// reading `structured` without reading the flag would publish prose that
    /// never validated.
    SchemaRefused(String),
    /// An answer that conformed to the schema and put no body in it, which is a
    /// drafter to correct rather than a schema.
    Bodyless,
}

/// The answer a `pr-author` dispatch gives.
///
/// Recognised by the task this crate composes for a drafting dispatch and by
/// nothing else, so an ordinary node's dispatch never answers as one — and
/// `None` is exactly that: a dispatch which is not a drafting one, whose report
/// is the two-party transcript every other member settles with.
///
/// Each of the three is scripted, because each is a different ending for the run
/// that asked and they take three different fixes: `pr-author.body` is the prose
/// a journey reads back out of the change request and is the default where
/// nothing is scripted, `pr-author.unschematic` is the refused answer, and
/// `pr-author.bodyless` is the conforming one with nothing in it.
fn drafted_answer(task: &str, dir: &std::path::Path) -> Option<Drafted> {
    if !task.starts_with("Read this branch's diff") {
        return None;
    }
    if dir.join("pr-author.unschematic").exists() {
        return Some(Drafted::SchemaRefused("half a bo".to_string()));
    }
    if dir.join("pr-author.bodyless").exists() {
        return Some(Drafted::Bodyless);
    }
    Some(Drafted::Body(
        fake::node_script(dir, "pr-author", "body")
            .unwrap_or_else(|| "## What\nDrafted from the diff.".to_string()),
    ))
}
