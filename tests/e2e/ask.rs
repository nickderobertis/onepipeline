//! `onepipeline ask` — a dispatched agent's blocking question to its manager,
//! over the run's own planner channel.
//!
//! What this replaces is 190 lines of a consuming repository's shell that
//! JSON-encoded a plain-text question into a `planner-question` frame and
//! exec'd the bus's command line with the run's channel, asker and policy —
//! every one of which the engine already knows. So what these journeys hold is
//! the seam rather than the encoding: the frame a manager actually reads off the
//! channel, the window the question waits, the one line the bus answers with,
//! and the refusals that raise nothing at all.
//!
//! Every question below is raised by the compiled binary against a real channel,
//! and every answer is a real listener's, read back through the manager's own
//! `next` and `reply` verbs.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven here as a
// real compiled binary against a real run store and the real linked `onemessagebus`.
// `harness.rs` carries the same suppression and the full rationale. The manager's side is
// not a double either: the listener below is this crate's own `next` and `reply` verbs.

use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::harness::{agent, double, plan_of, World, REFUSED};

/// The kind and source every question this verb raises carries.
const QUESTION_KIND: &str = "planner-question";
const QUESTION_SOURCE: &str = "proposal";

/// A settled run: the launch record and the channel are what `ask` reads, and
/// neither needs the run to still be executing. A settled one is the cheapest
/// world that has both.
fn run_of(world: &World, name: &str) -> String {
    let path = world.plan(name, &plan_of(name, vec![agent("build", &[])]));
    world.run(&["start", &path, "--attach"]).settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

/// The same, under a bus configuration the launch records.
fn run_under(world: &World, name: &str, config: &str) -> String {
    let file = world.root.join(format!("{name}.yaml"));
    std::fs::write(&file, config).expect("the bus configuration is written");
    let path = world.plan(name, &plan_of(name, vec![agent("build", &[])]));
    world
        .run(&[
            "start",
            &path,
            "--bus-config",
            &file.to_string_lossy(),
            "--attach",
        ])
        .settled();
    world.until("the run to settle", |world| {
        world.run_file(name, "result.json").is_file()
    });
    name.to_string()
}

/// A bus configuration whose `surfaces` codecs name `seconds` as their reply
/// windows, one codec per window.
fn window_of(seconds: &[u64]) -> String {
    let codecs: String = seconds
        .iter()
        .enumerate()
        .map(|(index, seconds)| {
            format!(
                "  asked{index}:\n    \
                   queue: surfaces\n    \
                   reply_window_seconds: {seconds}\n    \
                   select: kind\n    \
                   frames:\n      \
                     planner-question:\n        \
                       schema: agent.planner-surface@1\n        \
                       bindings: [{{do: answer, response: {{}}}}]\n"
            )
        })
        .collect();
    format!("version: 1\ntransport: {{kind: local}}\nprofile: planner-channel\ncodecs:\n{codecs}")
}

/// `onepipeline ask` as a worker runs it: the run on `ONEPIPELINE_RUN_ID`, and
/// whatever else the caller names, started and left to wait.
fn asking(world: &World, run: &str, args: &[&str], env: &[(&str, &str)]) -> Child {
    let mut command = world.cmd(&[&["ask"], args].concat());
    command.env("ONEPIPELINE_RUN_ID", run);
    for (key, value) in env {
        command.env(key, value);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts")
}

/// What one `ask` answered, once it has.
struct Answered {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Answered {
    /// Its single line of standard output, as the object it is.
    fn answer(&self) -> Value {
        assert_eq!(
            self.stdout.lines().count(),
            1,
            "`ask` wrote more than the bus's one line on standard output: {:?}\nstderr:\n{}",
            self.stdout,
            self.stderr
        );
        serde_json::from_str(self.stdout.trim()).expect("the answer is one JSON object")
    }
}

fn waited(child: Child) -> Answered {
    let output = child.wait_with_output().expect("the question ends");
    Answered {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// The question waiting on the run's channel, read the way a manager reads it:
/// `next` until one of this verb's frames is handed over.
///
/// Polled rather than waited on, because the question is raised by another
/// process and the manager's side has no signal of its own.
fn question_on(world: &World, run: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let next = world.run(&["next", run]);
        next.exited(0);
        let surface = next.json()["surface"].clone();
        if surface["kind"] == QUESTION_KIND {
            return surface;
        }
        assert!(
            Instant::now() < deadline,
            "no `{QUESTION_KIND}` reached the channel of {run}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Answer the question `surface` by the correlation it was raised under.
fn answer(world: &World, run: &str, surface: &Value, message: &str) {
    let correlation = surface["correlation"]
        .as_str()
        .expect("a blocking question carries its correlation");
    world
        .run_with_stdin(
            &["reply", run, "--correlation", correlation],
            &json!({"version": 2, "completion": false, "message": message}).to_string(),
        )
        .exited(0);
}

/// Every `planner-question` the run has raised, read the way anybody reads a
/// run's channel without consuming it: `onepipeline channel queue`.
fn questions_on(world: &World, run: &str) -> Vec<Value> {
    let queue = world.run(&["channel", "queue", run]);
    queue.exited(0);
    queue.json()["surfaces"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|frame| frame["kind"] == QUESTION_KIND)
        .collect()
}

/// The surface the run's pending slot holds — handed over, and answered or
/// abandoned — as `onepipeline channel queue` reports it.
fn held_on(world: &World, run: &str) -> Value {
    let queue = world.run(&["channel", "queue", run]);
    queue.exited(0);
    queue.json()["held"].clone()
}

/// The window `ask` said it waits, in seconds, off its standard error.
fn window_said(answered: &Answered) -> u64 {
    answered
        .stderr
        .lines()
        .find_map(|line| {
            line.strip_prefix("waiting up to ")?
                .strip_suffix(" seconds for the reply")?
                .parse()
                .ok()
        })
        .unwrap_or_else(|| panic!("`ask` never named its window: {}", answered.stderr))
}

/// The whole seam of one question: the frame a manager reads, and the answer
/// the asker gets back.
///
/// What the listener asserts is every field the consuming repository's shell
/// used to compose by hand — the kind, the source, the text, who asked and what
/// it was about — read off the frame this crate's own `next` handed over.
#[test]
fn a_question_reaches_the_manager_as_a_planner_question_frame_and_the_reply_comes_back() {
    let world = World::new("ask-reply");
    let run = run_of(&world, "askreply");

    let asked = asking(
        &world,
        &run,
        &["--about", "build", "Should I widen the bound?"],
        &[("ONEPIPELINE_CHANNEL_ASKER", "worker-7")],
    );
    let surface = question_on(&world, &run);

    assert_eq!(surface["kind"], json!(QUESTION_KIND), "{surface}");
    assert_eq!(surface["source"], json!(QUESTION_SOURCE), "{surface}");
    assert_eq!(
        surface["message"],
        json!("Should I widen the bound?"),
        "{surface}"
    );
    assert_eq!(surface["blocking"], json!(true), "{surface}");
    assert_eq!(surface["asker"], json!("worker-7"), "{surface}");
    // What the question is `about` is what the layout records as the frame's
    // workstream, which is the field a manager reads to know which node asked.
    assert_eq!(surface["workstream"], json!("build"), "{surface}");

    answer(&world, &run, &surface, "widen it to 512");

    let answered = waited(asked);
    assert_eq!(answered.code, 0, "{}", answered.stderr);
    let answer = answered.answer();
    assert_eq!(answer["answer"], json!("reply"), "{answer}");
    assert_eq!(
        answer["correlation"], surface["correlation"],
        "the answer echoes another question: {answer}"
    );
    assert!(
        answer["reply"].to_string().contains("widen it to 512"),
        "the manager's words are not in the answer: {answer}"
    );
}

/// The three forms the question arrives in are one question: the argument words
/// joined by a single space, a file, and standard input when neither is given.
#[test]
fn the_question_is_taken_from_the_argument_words_a_file_or_standard_input_alike() {
    let world = World::new("ask-forms");
    let run = run_of(&world, "askforms");
    let file = world.root.join("question.md");
    std::fs::write(&file, "## What\nfrom a file\n").expect("the question file is written");

    // The argument words, joined by one space — several words, and the spacing
    // between them is the verb's, not the shell's.
    let words = asking(
        &world,
        &run,
        &["Should", "I", "widen", "the", "bound?"],
        &[],
    );
    let surface = question_on(&world, &run);
    assert_eq!(
        surface["message"],
        json!("Should I widen the bound?"),
        "{surface}"
    );
    answer(&world, &run, &surface, "yes");
    assert_eq!(waited(words).code, 0);

    // A file, byte for byte.
    let named = asking(&world, &run, &["--file", &file.to_string_lossy()], &[]);
    let surface = question_on(&world, &run);
    assert_eq!(
        surface["message"],
        json!("## What\nfrom a file\n"),
        "{surface}"
    );
    answer(&world, &run, &surface, "yes");
    assert_eq!(waited(named).code, 0);

    // And standard input, which is how a long question is piped in.
    let mut command = world.cmd(&["ask"]);
    command.env("ONEPIPELINE_RUN_ID", &run);
    let mut piped = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts");
    {
        use std::io::Write;
        piped
            .stdin
            .as_mut()
            .expect("stdin is piped")
            .write_all(b"## What\npiped in\n")
            .expect("the question is written");
    }
    // Closed, so the verb's read of standard input can end.
    drop(piped.stdin.take());
    let surface = question_on(&world, &run);
    assert_eq!(
        surface["message"],
        json!("## What\npiped in\n"),
        "{surface}"
    );
    answer(&world, &run, &surface, "yes");
    assert_eq!(waited(piped).code, 0);
}

/// The reply window resolves three ways, in order: `--timeout`, then the
/// `reply_window_seconds` the run's launch record's bus configuration names for
/// the `surfaces` queue — the longest, where several codecs serve it — then the
/// bus's own default.
///
/// Each is driven by a listener that answers **after** the shorter window would
/// have elapsed, so a verb that took the wrong one would have timed out before
/// the reply arrived; and each is held to the window the verb says it waits,
/// which is what tells the bus's default from any other long-enough window.
#[test]
fn the_reply_window_is_the_flags_then_the_launch_records_then_the_buss_own() {
    let world = World::new("ask-window");

    // A configuration naming one second, and a flag naming far more: the flag
    // wins, and the proof is that a reply arriving well after that second is
    // still the answer — with the asker still blocked in between.
    let flagged = run_under(&world, "askflag", &window_of(&[1]));
    let mut asked = asking(&world, &flagged, &["--timeout", "120", "which bound?"], &[]);
    let surface = question_on(&world, &flagged);
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        asked
            .try_wait()
            .expect("the child is asked about")
            .is_none(),
        "the question did not block for its answer"
    );
    answer(&world, &flagged, &surface, "the wider one");
    let answered = waited(asked);
    assert_eq!(
        answered.answer()["answer"],
        json!("reply"),
        "the flag did not outrank the launch record: {}",
        answered.stderr
    );
    assert_eq!(answered.code, 0);
    assert_eq!(window_said(&answered), 120, "{}", answered.stderr);

    // No flag, and two codecs serving the queue — one second and long enough to
    // answer in: the launch record's longer one is what is waited, and a verb
    // taking the one-second codec would have timed out.
    let recorded = run_under(&world, "askrecorded", &window_of(&[1, 120]));
    let asked = asking(&world, &recorded, &["which bound?"], &[]);
    let surface = question_on(&world, &recorded);
    std::thread::sleep(Duration::from_secs(3));
    answer(&world, &recorded, &surface, "the wider one");
    let answered = waited(asked);
    assert_eq!(
        answered.answer()["answer"],
        json!("reply"),
        "the launch record's window was not waited: {}",
        answered.stderr
    );
    assert_eq!(window_said(&answered), 120, "{}", answered.stderr);

    // And a run whose configuration names no window at all, asked with no flag:
    // the bus's own default is waited, and it is long enough that a reply after
    // a one-second window would have elapsed is still the answer.
    let bare = run_of(&world, "askbare");
    let asked = asking(&world, &bare, &["which bound?"], &[]);
    let surface = question_on(&world, &bare);
    std::thread::sleep(Duration::from_secs(2));
    answer(&world, &bare, &surface, "the bus's own");
    let answered = waited(asked);
    assert_eq!(answered.code, 0, "{}", answered.stderr);
    assert_eq!(answered.answer()["answer"], json!("reply"));
    assert_eq!(
        window_said(&answered),
        onemessagebus::DEFAULT_REPLY_WINDOW.as_secs(),
        "the bus's default was not the window waited: {}",
        answered.stderr
    );
}

/// A wait that elapses is not a ruling: the question stands on the channel,
/// marked abandoned, and the asker exits `1` with the bus's own word for it.
#[test]
fn an_elapsed_wait_answers_timeout_at_exit_one_and_leaves_the_question_standing() {
    let world = World::new("ask-timeout");
    let run = run_of(&world, "asktimeout");

    let asked = asking(&world, &run, &["--timeout", "1", "anybody there?"], &[]);
    let surface = question_on(&world, &run);
    let answered = waited(asked);

    assert_eq!(answered.code, 1, "{}", answered.stderr);
    let elapsed = answered.answer();
    assert_eq!(elapsed["answer"], json!("timeout"), "{elapsed}");
    assert_eq!(elapsed["correlation"], surface["correlation"], "{elapsed}");
    // The question is still the run's to answer, and the verb says how.
    assert!(
        answered
            .stderr
            .contains("the question stands on the channel"),
        "{}",
        answered.stderr
    );
    // The question is still on the channel — the elapsed wait withdrew nothing —
    // and it is marked abandoned, which is what lets a later listener take it
    // back.
    let standing = questions_on(&world, &run);
    let [question] = &standing[..] else {
        panic!("the elapsed question was withdrawn: {standing:?}");
    };
    assert_eq!(question["message"], json!("anybody there?"), "{question}");
    let held = held_on(&world, &run);
    assert_eq!(held["correlation"], surface["correlation"], "{held}");
    assert_eq!(
        held["abandoned"],
        json!(true),
        "the elapsed question was left attended: {held}"
    );

    // What the advice cannot promise on a *settled* run is that the reply will
    // reach anybody, and the reply verb says exactly that rather than queueing
    // one nothing will read — so the elapsed asker's advice is the standing
    // question's, not a promise about this run.
    let correlation = surface["correlation"].as_str().expect("a correlation");
    world
        .run_with_stdin(
            &["reply", &run, "--correlation", correlation],
            &json!({"version": 2, "completion": false, "message": "here now"}).to_string(),
        )
        .exited(REFUSED)
        .err_has("has settled");
}

/// An elapsed question that cannot be marked abandoned is still `timeout` at
/// exit `1` — the wait did elapse — and the advice says the mark failed rather
/// than promising a later listener takes the question back.
#[cfg(unix)]
#[test]
fn an_elapsed_question_that_cannot_be_marked_says_so_rather_than_promising_it_was() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new("ask-unmarked");
    let run = run_of(&world, "askunmarked");
    let asked = asking(&world, &run, &["--timeout", "3", "anybody there?"], &[]);
    question_on(&world, &run);

    // The channel directory made one this asker may not write, once the question
    // is on it and handed over, so the one write left — the abandoned mark — is
    // refused.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] a run root the asker may not write is
    // the host's doing rather than the CLI's, and nothing user-facing produces it; the same
    // placement `channel.rs::a_read_still_answers_from_the_log_when_it_cannot_write_the_
    // projection_back` makes. Everything asserted after it is read off the binary's streams.
    let channel = world.run_file(&run, "channel");
    let writable = std::fs::metadata(&channel)
        .expect("the channel directory")
        .permissions();
    std::fs::set_permissions(&channel, std::fs::Permissions::from_mode(0o555))
        .expect("the directory is made read-only");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let answered = waited(asked);
    std::fs::set_permissions(&channel, writable).expect("the directory is writable again");

    assert_eq!(answered.code, 1, "{}", answered.stderr);
    assert_eq!(answered.answer()["answer"], json!("timeout"));
    assert!(
        answered
            .stderr
            .contains("but could not be marked abandoned (")
            && !answered.stderr.contains(", marked abandoned,"),
        "the advice promised a mark that was not made: {}",
        answered.stderr
    );
}

/// Everything refused before anything is raised, at exit `2`, naming its cause —
/// and with the run's channel carrying nothing at all afterwards.
///
/// One run for the whole table on purpose: what makes each of these a refusal
/// rather than a question is that the channel is untouched, and a single
/// channel asserted empty at the end says that of every one of them at once.
#[test]
fn every_refusal_names_its_cause_at_exit_two_and_raises_nothing_on_the_channel() {
    let world = World::new("ask-refused");
    let run = run_of(&world, "askrefused");
    let blank_file = world.root.join("blank.md");
    std::fs::write(&blank_file, "   \n").expect("a blank question file");

    // No run to ask on. Named, because a worker whose launch did not export it
    // cannot otherwise tell this from a channel that refused it.
    let mut no_run = world.cmd(&["ask", "which bound?"]);
    no_run.env_remove("ONEPIPELINE_RUN_ID");
    world
        .run_on(no_run, "ask with no run")
        .exited(REFUSED)
        .err_has("ONEPIPELINE_RUN_ID is not set");

    // An asker named as nobody. Unset means nobody; set and blank is a caller
    // that meant to name one.
    let blank_asker = waited(asking(
        &world,
        &run,
        &["which bound?"],
        &[("ONEPIPELINE_CHANNEL_ASKER", "   ")],
    ));
    assert_eq!(blank_asker.code, REFUSED, "{}", blank_asker.stderr);
    assert!(
        blank_asker.stderr.contains("ONEPIPELINE_CHANNEL_ASKER"),
        "{}",
        blank_asker.stderr
    );

    // A blank question, in each of the three forms it can arrive in, each
    // refusal naming the form so the caller knows where to look.
    let words = waited(asking(&world, &run, &["   "], &[]));
    assert_eq!(words.code, REFUSED, "{}", words.stderr);
    assert!(
        words.stderr.contains("the argument words"),
        "{}",
        words.stderr
    );

    let from_file = waited(asking(
        &world,
        &run,
        &["--file", &blank_file.to_string_lossy()],
        &[],
    ));
    assert_eq!(from_file.code, REFUSED, "{}", from_file.stderr);
    assert!(
        from_file.stderr.contains("the file named with --file"),
        "{}",
        from_file.stderr
    );

    // A question file that is not there: named, with the path.
    let missing = world.root.join("no-such-question.md");
    let unread = waited(asking(
        &world,
        &run,
        &["--file", &missing.to_string_lossy()],
        &[],
    ));
    assert_eq!(unread.code, REFUSED, "{}", unread.stderr);
    assert!(
        unread.stderr.contains("could not be read")
            && unread.stderr.contains(&*missing.to_string_lossy()),
        "{}",
        unread.stderr
    );

    // A reply window of nothing: no wait at all is not a window.
    let zero = waited(asking(&world, &run, &["--timeout", "0", "which?"], &[]));
    assert_eq!(zero.code, REFUSED, "{}", zero.stderr);
    assert!(zero.stderr.contains("--timeout"), "{}", zero.stderr);

    // Standard input carrying nothing: the form a worker reaches when its
    // heredoc was empty.
    let mut piped = world.cmd(&["ask"]);
    piped.env("ONEPIPELINE_RUN_ID", &run);
    let empty = world.run_with_stdin_on(piped, "\n  \n");
    empty
        .exited(REFUSED)
        .err_has("the question from standard input is blank");

    // Standard input that is not text: named as the form it came in, rather
    // than guessed at.
    let mut bytes = world.cmd(&["ask"]);
    bytes.env("ONEPIPELINE_RUN_ID", &run);
    let mut child = bytes
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts");
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(b"which \xff\xfe bound?")
            .expect("the bytes are written");
    }
    let undecoded = waited(child);
    assert_eq!(undecoded.code, REFUSED, "{}", undecoded.stderr);
    assert!(
        undecoded
            .stderr
            .contains("the question on standard input could not be read"),
        "{}",
        undecoded.stderr
    );

    // A run id that names no run under the runs root.
    let nowhere = waited(asking(&world, "no-such-run", &["which bound?"], &[]));
    assert_eq!(nowhere.code, REFUSED, "{}", nowhere.stderr);
    assert!(nowhere.stdout.is_empty(), "{}", nowhere.stdout);
    assert!(nowhere.stderr.contains("no-such-run"), "{}", nowhere.stderr);

    // A question no frame can carry.
    let mut nul = world.cmd(&["ask"]);
    nul.env("ONEPIPELINE_RUN_ID", &run);
    world
        .run_with_stdin_on(nul, "which\0bound?")
        .exited(REFUSED)
        .err_has("carries a NUL byte");

    // What the question is about, past the bound the bus puts on it, and
    // carrying something the bus will not take.
    let long = "n".repeat(513);
    let over = waited(asking(&world, &run, &["--about", &long, "which?"], &[]));
    assert_eq!(over.code, REFUSED, "{}", over.stderr);
    assert!(over.stderr.contains("--about"), "{}", over.stderr);

    let blank_about = waited(asking(&world, &run, &["--about", "   ", "which?"], &[]));
    assert_eq!(blank_about.code, REFUSED, "{}", blank_about.stderr);
    assert!(
        blank_about.stderr.contains("--about"),
        "{}",
        blank_about.stderr
    );

    let control = waited(asking(
        &world,
        &run,
        &["--about", "bui\u{7}ld", "which?"],
        &[],
    ));
    assert_eq!(control.code, REFUSED, "{}", control.stderr);
    assert!(control.stderr.contains("--about"), "{}", control.stderr);

    // A run whose launch record cannot be read: the policy the question would
    // be raised under is what that record carries, so there is nothing to raise
    // it under.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] no verb truncates a launch record,
    // and none could; what this stands in for is the record a half-written launch or a
    // full disk leaves, which is the state the refusal exists for. Everything asserted
    // after it is read off the compiled binary's own streams.
    let unreadable = run_of(&world, "askunreadable");
    std::fs::write(world.run_file(&unreadable, "launch.json"), "{").expect("a truncated record");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let refused = waited(asking(&world, &unreadable, &["which bound?"], &[]));
    assert_eq!(refused.code, REFUSED, "{}", refused.stderr);
    assert!(
        refused.stderr.contains("launch.json"),
        "the refusal does not name the record it could not read: {}",
        refused.stderr
    );

    // And the whole of it: not one of those reached the channel.
    assert_eq!(
        questions_on(&world, &run),
        Vec::<Value>::new(),
        "a refused question was raised on the channel anyway"
    );
    world
        .run(&["next", &run])
        .exited(0)
        .out_has("\"surface\":null");
}

/// A question the bus itself refuses is answered `refused` at exit `1`, with the
/// bus's reason — never a refusal of the verb's own, because the question was
/// well-formed and it is the channel that declined it.
#[test]
fn a_question_the_bus_refuses_is_answered_refused_at_exit_one() {
    let world = World::new("ask-bus-refused");
    // A real validator on the `surfaces` queue, scripted to decline: the bus
    // declines the frame rather than the verb declining to send it.
    let validator = double("bus-validator").to_string_lossy().into_owned();
    let run = run_under(
        &world,
        "askdeclined",
        &format!(
            "version: 1\ntransport: {{kind: local}}\nvalidators:\n  \
             - {{on: surfaces, kind: command, command: [{validator:?}]}}\n"
        ),
    );
    let reason = "a question this run has no manager to answer";
    world.script("bus-validator.refuse", reason);

    let refused = waited(asking(&world, &run, &["which bound?"], &[]));
    assert_eq!(refused.code, 1, "{}", refused.stderr);
    let answer = refused.answer();
    assert_eq!(answer["answer"], json!("refused"), "{answer}");
    assert!(
        answer["reason"]
            .as_str()
            .is_some_and(|it| it.contains(reason)),
        "the answer does not carry the validator's own words: {answer}"
    );
    assert!(
        refused.stderr.contains("the question was refused"),
        "{}",
        refused.stderr
    );
}

/// The bus's line is the whole of standard output, and the verb's own words go
/// beside it on standard error — so an asker that reads stdout as JSON reads
/// exactly one object, whatever the answer was.
#[test]
fn the_answer_is_the_only_thing_on_standard_output_and_the_advice_is_beside_it() {
    let world = World::new("ask-streams");
    let run = run_of(&world, "askstreams");

    let asked = asking(&world, &run, &["which bound?"], &[]);
    let surface = question_on(&world, &run);
    answer(&world, &run, &surface, "the wider one");
    let answered = waited(asked);

    assert_eq!(answered.code, 0, "{}", answered.stderr);
    // One line, and it parses whole — `answer()` asserts both.
    let answer = answered.answer();
    assert_eq!(answer["answer"], json!("reply"), "{answer}");
    // The correlation is named before the wait, so a caller killed mid-wait
    // still holds the token a manager answers by.
    assert!(
        answered.stderr.contains("correlation: "),
        "the verb never named the correlation: {}",
        answered.stderr
    );
    assert!(
        !answered.stderr.contains("onepipeline: "),
        "a reply carried advice it did not need: {}",
        answered.stderr
    );
}

/// The environment's asker rides the frame when one is named, and no asker field
/// is stamped when the environment names none.
#[test]
fn the_frame_carries_the_asker_the_environment_names_and_none_when_it_names_none() {
    let world = World::new("ask-asker");
    let run = run_of(&world, "askasker");

    let unnamed = asking(&world, &run, &["which bound?"], &[]);
    let surface = question_on(&world, &run);
    assert!(
        surface["asker"].is_null(),
        "an asker nobody named: {surface}"
    );
    answer(&world, &run, &surface, "the wider one");
    assert_eq!(waited(unnamed).code, 0);

    let named = asking(
        &world,
        &run,
        &["which bound?"],
        &[("ONEPIPELINE_CHANNEL_ASKER", "worker-7")],
    );
    let surface = question_on(&world, &run);
    assert_eq!(surface["asker"], json!("worker-7"), "{surface}");
    answer(&world, &run, &surface, "the wider one");
    assert_eq!(waited(named).code, 0);
}
