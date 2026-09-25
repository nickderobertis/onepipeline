//! `onepipeline stop-guard` — the general stop guard over `unwatched`, and the
//! two harness renderings of its one verdict.
//!
//! What this exists for is the same measured failure `tests/e2e/unwatched.rs`
//! states: the manager forgets to arm the watch and dispatched work sits for
//! hours. A harness's stop hook can refuse to end a turn, so the question is
//! asked by a hook — and a hook is worth nothing unless the guard behind it
//! blocks on evidence alone, blocks **once** per condition, and says so, out
//! loud and without blocking, on every ending that is not evidence. Every
//! journey here drives the compiled binary over its neutral input, and the last
//! drives the Claude Code wiring `docs/stop-guard.md` states over real `Stop`
//! payloads.
//!
//! **The session asked about is always the input's.** Every command below runs
//! in an environment whose `ONEPIPELINE_LAUNCHER_SESSION` names *another*
//! session — the one a dispatched worker inherits from its manager — so an
//! answer about the right session is one the environment could not have given.

// llmlint: ignore-file[e2e_not_mocked] `World` substitutes `oneagentgraph` at its
// subprocess boundary and nothing inside the crate under test, which is driven here as a
// real compiled binary against a real run store; `harness.rs` carries the same suppression
// and the full rationale. Every claim below is read off that binary's own streams.

use std::path::PathBuf;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::harness::{agent, plan_of, World, RUNS_UNWATCHED};

/// The session a worker's environment carries: its manager's, never its own.
const INHERITED: &str = "the-managers-session";

/// The guard's own world: the owner's, seen under a stranger's session — the
/// one a dispatched worker inherits from its manager — with the guard's memory
/// kept under the world.
///
/// Both worlds stay alive for the length of a journey: a world removes its root
/// when it is dropped, so the owner is what holds the root and this is a view
/// of it.
fn guarded(owner: &World) -> World {
    owner.as_session(INHERITED).with_env(
        "XDG_STATE_HOME",
        &owner.root.join("state").display().to_string(),
    )
}

/// A run with a dispatch held open — unsettled and watched by nothing.
fn held(world: &World, name: &str) -> String {
    let path = world.plan(name, &plan_of(name, vec![agent("build", &[])]));
    world.run(&["start", &path, "--detach"]).exited(0);
    world.until("the run to dispatch something", |world| {
        !world.events_of(name, "node-dispatched").is_empty()
    });
    name.to_string()
}

/// Where the guard remembers what it last blocked `session` on, under the
/// state root `guarded` names, spelled by the template `docs/stop-guard.md`
/// documents — so the page's path and the verb's cannot drift apart.
fn memory(world: &World, session: &str) -> PathBuf {
    let page = page();
    let template = page
        .split('`')
        .find(|span| span.starts_with("${XDG_STATE_HOME:-"))
        .expect("the page documents the memory's path");
    let under = template
        .split_once("}/")
        .and_then(|(_, rest)| rest.strip_suffix("/<sha256(session)>"))
        .expect("the template is `${state root}/<dir>/<sha256(session)>`");
    world
        .root
        .join("state")
        .join(under)
        .join(hex(&Sha256::digest(session.as_bytes())))
}

/// Where the guard remembers what `command`, declared as a source, last
/// blocked `session` on: beside the verb's own memory, under the file name the
/// page spells for it.
#[cfg(unix)]
fn source_memory(world: &World, session: &str, command: &str) -> PathBuf {
    let name = page()
        .split("## Declared sources")
        .nth(1)
        .and_then(|section| {
            section
                .split('`')
                .find(|span| span.starts_with("<sha256(session)>."))
        })
        .expect("the page spells a source memory's file name")
        .replace(
            "<sha256(session)>",
            &hex(&Sha256::digest(session.as_bytes())),
        )
        .replace(
            "<sha256(command)>",
            &hex(&Sha256::digest(command.as_bytes())),
        );
    memory(world, session).with_file_name(name)
}

/// Every `--flag` a synopsis or a `--help` names, `--help` itself aside.
pub(crate) fn flags_of(text: &str) -> std::collections::BTreeSet<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|word| word.starts_with("--") && word.len() > 2 && *word != "--help")
        .map(str::to_owned)
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The one object on standard output, or nothing.
fn verdict(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    assert_eq!(
        stdout.lines().count(),
        1,
        "the guard wrote more than one line on standard output: {stdout:?}"
    );
    Some(serde_json::from_str(trimmed).expect("the verdict is one JSON object"))
}

/// `docs/stop-guard.md`, the page the neutral contract is stated on.
fn page() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/stop-guard.md"),
    )
    .expect("the docs page ships")
}

/// The neutral input object the page documents, with `<ID>` naming `session`.
///
/// Read out of the page rather than restated, so the object a reader copies is
/// the one these journeys feed: the page and the verb cannot drift apart.
fn documented_input(session: &str) -> Value {
    let contract = page()
        .split("## The contract")
        .nth(1)
        .expect("the page states the contract")
        .to_owned();
    let block = contract
        .split("```\n")
        .find(|block| block.trim_start().starts_with("{\"session\""))
        .expect("the contract carries the input object");
    serde_json::from_str(&block.replace("<ID>", session)).expect("the input object is JSON")
}

/// The output object the page documents for `verdict`.
fn documented_output(verdict: &str) -> Value {
    let row = page()
        .lines()
        .find(|line| line.starts_with(&format!("| {verdict} |")))
        .unwrap_or_else(|| panic!("the page has no `{verdict}` row"))
        .to_owned();
    let object = row
        .split('`')
        .nth(1)
        .expect("the row carries its object in backticks");
    serde_json::from_str(object).expect("the documented object is JSON")
}

/// Hold `told` to the shape the page documents for its verdict: the same
/// verdict word and the same fields — the values beside it are what the run
/// under test makes them.
fn documented(told: &Value) {
    let word = told["verdict"].as_str().expect("a verdict word");
    let shape = documented_output(word);
    let keys = |object: &Value| -> Vec<String> {
        let mut keys: Vec<String> = object
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    };
    assert_eq!(shape["verdict"], told["verdict"], "{told}");
    assert_eq!(
        keys(&shape),
        keys(told),
        "`{word}` is not the shape docs/stop-guard.md documents: {told}"
    );
}

/// The synopsis the page states is the verb's own: the same flags `--help`
/// lists, and the same format names.
#[test]
fn the_documented_synopsis_is_the_verbs_own() {
    let world = World::new("stop-guard-synopsis");
    let page = page();
    let synopsis = page
        .lines()
        .find(|line| line.starts_with("onepipeline stop-guard "))
        .expect("the page states the synopsis");
    let help = world.run(&["stop-guard", "--help"]);
    help.exited(0);
    assert_eq!(
        flags_of(synopsis),
        flags_of(&help.stdout),
        "the page's synopsis and the verb's flags differ:\n{synopsis}\n{}",
        help.stdout
    );
    // And the synopsis entry 85 of the register proposes is that same one.
    let register = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
    )
    .expect("the register ships");
    let proposed = register
        .split("add `onepipeline stop-guard")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .expect("entry 85 proposes the synopsis");
    assert_eq!(
        flags_of(proposed),
        flags_of(&help.stdout),
        "entry 85's synopsis and the verb's flags differ:\n{proposed}\n{}",
        help.stdout
    );
    for format in ["neutral", "claude-code", "codex"] {
        assert!(
            proposed.contains(format),
            "entry 85 does not propose `--format {format}`"
        );
    }
    // The source timeout's default is the one the page states, and a timeout
    // no clock could hold, or none at all, is refused at the flag.
    let default = page
        .split("(default `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .expect("the page states the source timeout's default");
    let offered = help
        .stdout
        .split("--source-timeout <SECONDS>")
        .nth(1)
        .and_then(|rest| rest.split("[default: ").nth(1))
        .and_then(|rest| rest.split(']').next())
        .expect("`--help` states the source timeout's default");
    assert_eq!(
        default, offered,
        "the page and `--help` differ on the default"
    );
    let most: u64 = page
        .split("at most `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .and_then(|most| most.parse().ok())
        .expect("the page states the longest source timeout");
    for accepted in ["1".to_owned(), most.to_string()] {
        world
            .run(&[
                "stop-guard",
                "--session",
                "s",
                "--source-timeout",
                &accepted,
            ])
            .exited(0);
    }
    for refused in ["0".to_owned(), (most + 1).to_string()] {
        world
            .run(&["stop-guard", "--session", "s", "--source-timeout", &refused])
            .exited(2)
            .err_has("--source-timeout");
    }
    for format in ["neutral", "claude-code", "codex"] {
        assert!(
            page.contains(&format!("`--format {format}`")) && help.stdout.contains(format),
            "`--format {format}` is not both documented and offered"
        );
    }
}

/// Feed one neutral input object to the guard.
fn ask(world: &World, input: &Value) -> crate::harness::Run {
    world.run_with_stdin(&["stop-guard"], &input.to_string())
}

/// The whole journey of one condition: blocked once, silent on an unchanged
/// continuation, blocked again on a changed one, silent with the memory
/// removed once nothing is unwatched.
#[test]
fn a_run_nothing_watches_blocks_once_per_condition_and_the_memory_says_which() {
    let owner = World::new("stop-guard-block");
    let world = guarded(&owner);
    owner.script("build.wait", "hold");
    let run = held(&owner, "guardblock");
    let session = owner.session.clone();
    let report = owner
        .run(&["unwatched", "--session", &session])
        .exited(RUNS_UNWATCHED)
        .stdout
        .clone();

    // Blocked, with the verb's own lines as the reason, and the memory written
    // with a digest of exactly that report.
    let first = ask(&world, &json!({"session": session}));
    first.exited(0);
    let told = verdict(&first.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("block"), "{told}");
    assert_eq!(told["reason"], json!(report), "{told}");
    documented(&told);
    assert!(told["reason"]
        .as_str()
        .is_some_and(|reason| reason.contains(&run)));
    let remembered = std::fs::read_to_string(memory(&world, &session)).expect("the memory");
    assert_eq!(
        remembered.trim(),
        hex(&Sha256::digest(report.as_bytes())),
        "the memory is not a digest of the report"
    );
    assert!(
        first.stderr.is_empty(),
        "a block wrote to standard error: {}",
        first.stderr
    );

    // A continuation over the same report, asked with the very object the docs
    // page documents: the manager was told and did nothing, so nothing is said
    // again, and the memory stands.
    let continuing = documented_input(&session);
    assert_eq!(continuing["continuation"], json!(true), "{continuing}");
    let again = ask(&world, &continuing);
    again.exited(0);
    assert_eq!(
        verdict(&again.stdout),
        Some(json!({"verdict": "none"})),
        "{}",
        again.stdout
    );
    documented(&verdict(&again.stdout).expect("a verdict"));
    assert!(memory(&world, &session).is_file(), "the memory was dropped");

    // The same continuation flag as `--continuation`, decided the same way.
    world
        .run_with_stdin(
            &["stop-guard", "--continuation"],
            &json!({"session": session}).to_string(),
        )
        .exited(0)
        .out_has("\"none\"");

    // And the whole input as flags, nothing on standard input: a first stop
    // blocks on the same report, and its continuation is silent.
    let flagged = world.run(&["stop-guard", "--session", &session]);
    flagged.exited(0);
    assert_eq!(
        verdict(&flagged.stdout),
        Some(json!({"verdict": "block", "reason": report})),
        "{}",
        flagged.stdout
    );
    world
        .run(&["stop-guard", "--session", &session, "--continuation"])
        .exited(0)
        .out_has("\"none\"");

    // A relative `XDG_STATE_HOME` is ignored rather than resolved against
    // wherever the hook happened to run: the memory lands under the home
    // directory's state root instead.
    let home = owner.root.join("home");
    let mut relative = world.cmd(&["stop-guard", "--session", &session]);
    relative
        .env("XDG_STATE_HOME", "relative-state")
        .env("HOME", &home)
        .current_dir(&owner.root);
    let relative = world.run_on(relative, "stop-guard under a relative XDG_STATE_HOME");
    relative.exited(0);
    assert_eq!(
        verdict(&relative.stdout).expect("a verdict")["verdict"],
        json!("block")
    );
    assert!(
        home.join(".local/state/onepipeline/stop-guard")
            .join(hex(&Sha256::digest(session.as_bytes())))
            .is_file(),
        "the memory is not under the home directory's state root"
    );
    assert!(
        !owner.root.join("relative-state").exists(),
        "a relative XDG_STATE_HOME was resolved against the working directory"
    );

    // The condition moved — a second run of the session is unwatched — so the
    // report changed and a continuation blocks again. A memory holding only
    // "blocked before" would have stayed silent here.
    let second = held(&owner, "guardsecond");
    let moved = ask(&world, &json!({"session": session, "continuation": true}));
    moved.exited(0);
    let told = verdict(&moved.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("block"), "{told}");
    let reason = told["reason"].as_str().expect("a reason");
    assert!(
        reason.contains(&run) && reason.contains(&second),
        "{reason}"
    );
    assert_ne!(
        std::fs::read_to_string(memory(&world, &session))
            .expect("the memory")
            .trim(),
        remembered.trim(),
        "the memory did not move with the report"
    );

    // Watched, both of them: nothing to report, silence, and the memory removed.
    let mut watches: Vec<std::process::Child> = [run.as_str(), second.as_str()]
        .iter()
        .map(|run| {
            owner
                .cmd(&["watch", run, "--timeout", "none"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("the watch starts")
        })
        .collect();
    // Waited on by the records the watches write rather than by asking a
    // process: a watch records itself under the run's `watchers/` directory
    // before it waits, and that file is what makes the run watched.
    for run in [&run, &second] {
        owner.until("the watch to record itself", |owner| {
            std::fs::read_dir(owner.run_file(run, "watchers"))
                .map(|entries| entries.count() >= 1)
                .unwrap_or(false)
        });
    }
    let quiet = ask(&world, &json!({"session": session}));
    quiet.exited(0);
    assert_eq!(verdict(&quiet.stdout), Some(json!({"verdict": "none"})));
    assert!(
        !memory(&world, &session).exists(),
        "a session with nothing to report kept its memory"
    );
    assert!(quiet.stderr.is_empty(), "{}", quiet.stderr);

    for watch in &mut watches {
        watch.kill().expect("the watch takes the signal");
        watch.wait().expect("the watch ends");
    }
    owner.release("build.go");
}

/// Input the guard cannot read, and a session that names nobody, are `none`:
/// a hook handed something it does not understand knows nothing about whether a
/// run is watched, and the environment's session is never the answer.
#[test]
fn unreadable_input_and_a_blank_session_are_silent_and_never_the_environments() {
    let owner = World::new("stop-guard-silent");
    let world = guarded(&owner);
    owner.script("build.wait", "hold");
    // The environment's own session owns an unwatched run, so an answer out of
    // it would be a block.
    let _ = held(&world, "guardinherited");
    world
        .run(&["unwatched", "--session", INHERITED])
        .exited(RUNS_UNWATCHED);

    for input in [
        "not json at all".to_string(),
        "".to_string(),
        json!({"session": ""}).to_string(),
        json!({"session": "   "}).to_string(),
        json!({"continuation": true}).to_string(),
        json!({"session_id": INHERITED}).to_string(),
        json!([INHERITED]).to_string(),
    ] {
        let asked = world.run_with_stdin(&["stop-guard"], &input);
        asked.exited(0);
        assert_eq!(
            verdict(&asked.stdout),
            Some(json!({"verdict": "none"})),
            "input {input:?} was not answered `none`: {}",
            asked.stdout
        );
        assert!(asked.stderr.is_empty(), "{input:?}: {}", asked.stderr);
    }
    // Bytes that are not text at all — a payload the harness garbled — are the
    // same nothing: the read itself fails, and that says nothing about a run.
    let mut child = world
        .cmd(&["stop-guard"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the binary starts");
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(b"{\"session\": \"\xff\xfe\"}")
            .expect("the bytes are written");
    }
    let garbled = child.wait_with_output().expect("the guard ends");
    assert_eq!(garbled.status.code(), Some(0), "{garbled:?}");
    assert_eq!(
        verdict(&String::from_utf8_lossy(&garbled.stdout)),
        Some(json!({"verdict": "none"})),
        "{garbled:?}"
    );
    assert!(garbled.stderr.is_empty(), "{garbled:?}");

    // And `--session ""` is that same nothing rather than a fall-through.
    let blank = world.run(&["stop-guard", "--session", ""]);
    blank.exited(0);
    assert_eq!(verdict(&blank.stdout), Some(json!({"verdict": "none"})));
    owner.release("build.go");
}

/// Everything that is not evidence is one `warn` and never a block: a runs
/// root the question cannot be asked over, a memory that cannot be read on a
/// continuation, and one that cannot be written before a block.
#[test]
fn a_question_that_cannot_be_asked_and_a_memory_that_cannot_be_kept_warn_and_never_block() {
    let owner = World::new("stop-guard-warn");
    let world = guarded(&owner);
    owner.script("build.wait", "hold");
    let run = held(&owner, "guardwarn");
    let session = owner.session.clone();

    // The runs root is a file: the question `unwatched` refuses, in place of an
    // answer.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] no verb makes a runs root out of a
    // file, and none could: what this stands in for is an operator or a harness pointing
    // `ONEPIPELINE_RUNS_DIR` at something that is not a directory of runs. Everything
    // asserted after it is read off the compiled binary's own streams.
    let unreadable = world.root.join("runs-that-are-a-file");
    std::fs::write(&unreadable, "not a directory of runs").expect("something in the way");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let mut refused = world.cmd(&["stop-guard"]);
    refused.env("ONEPIPELINE_RUNS_DIR", &unreadable);
    let refused = world.run_with_stdin_on(refused, &json!({"session": session}).to_string());
    refused.exited(0);
    let told = verdict(&refused.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("warn"), "{told}");
    documented(&told);
    let message = told["message"].as_str().expect("a message");
    assert!(
        message.contains(&format!("onepipeline unwatched --session {session}")),
        "the warning does not name the command to ask by hand: {message}"
    );
    assert!(
        message.contains("could not be answered"),
        "the warning does not say what could not be answered: {message}"
    );
    assert!(
        refused.stderr.is_empty(),
        "a warning wrote to standard error: {}",
        refused.stderr
    );

    // A memory holding something other than a digest this guard wrote, on a
    // continuation: not compared against, because it vouches for nothing.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] nothing user-facing writes a memory
    // but the guard; this stands in for a record damaged by something else on the host.
    let path = memory(&world, &session);
    std::fs::create_dir_all(path.parent().expect("the memory's directory"))
        .expect("the memory's directory");
    std::fs::write(&path, "not a digest\n").expect("a damaged memory");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let damaged = ask(&world, &json!({"session": session, "continuation": true}));
    damaged.exited(0);
    let told = verdict(&damaged.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("warn"), "{told}");
    assert!(
        told["message"].as_str().is_some_and(|message| {
            message.contains("could not read what it last blocked on")
                && message.contains("not a digest this guard wrote")
        }),
        "{told}"
    );
    std::fs::remove_file(&path).expect("the damaged memory");

    // A memory this guard cannot read, on a continuation: a directory where the
    // file goes.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] nothing writes a directory at a memory's
    // path; it stands in for a record this process cannot read back — a permission, a
    // filesystem refusing — in the one form every platform refuses to read as a file.
    std::fs::create_dir_all(&path).expect("something unreadable at the memory's path");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let unread = ask(&world, &json!({"session": session, "continuation": true}));
    unread.exited(0);
    let told = verdict(&unread.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("warn"), "{told}");
    let message = told["message"].as_str().expect("a message");
    assert!(
        message.contains(&run) && message.contains("could not read what it last blocked on"),
        "{message}"
    );
    assert!(message.contains(&format!("onepipeline unwatched --session {session}")));
    assert!(message.contains("onepipeline watch"), "{message}");

    // And one it cannot write, before a block: the same directory is what the
    // write meets on a first stop.
    let unwritten = ask(&world, &json!({"session": session}));
    unwritten.exited(0);
    let told = verdict(&unwritten.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("warn"), "{told}");
    let message = told["message"].as_str().expect("a message");
    assert!(
        message.contains(&run) && message.contains("could not record what it would block on"),
        "{message}"
    );
    std::fs::remove_dir_all(&path).expect("the directory in the way");

    // And one it cannot remove, for a session with nothing unwatched: silence
    // there would leave a memory a later continuation is let through on.
    let idle = "a-session-owning-nothing";
    // llmlint: ignore-block[tests_mirror_real_usage] the same directory-in-the-way the
    // two cases above place, filled so it cannot be removed as a file either; nothing
    // user-facing leaves one, and it stands in for a memory the host refuses to delete.
    let stuck = memory(&world, idle);
    std::fs::create_dir_all(stuck.join("held")).expect("something unremovable at the path");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let unremoved = ask(&world, &json!({"session": idle}));
    unremoved.exited(0);
    let told = verdict(&unremoved.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("warn"), "{told}");
    documented(&told);
    let message = told["message"].as_str().expect("a message");
    assert!(
        message.contains("could not remove what it last blocked on")
            && message.contains(&format!("onepipeline unwatched --session {idle}")),
        "{message}"
    );
    std::fs::remove_dir_all(&stuck).expect("the directory in the way");
    let cleared = ask(&world, &json!({"session": idle}));
    cleared.exited(0);
    assert_eq!(verdict(&cleared.stdout), Some(json!({"verdict": "none"})));

    // And no state root to keep a memory under at all — no `XDG_STATE_HOME`
    // and no home directory — is the same `warn` before a block.
    let mut rootless = world.cmd(&["stop-guard", "--session", &session]);
    rootless
        .env_remove("XDG_STATE_HOME")
        .env_remove("HOME")
        .env_remove("USERPROFILE");
    let rootless = world.run_on(rootless, "stop-guard with no state root");
    rootless.exited(0);
    let told = verdict(&rootless.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("warn"), "{told}");
    assert!(
        told["message"].as_str().is_some_and(|message| {
            message.contains("could not record what it would block on")
                && message.contains("XDG_STATE_HOME")
        }),
        "{told}"
    );

    // With the way clear, the same stop blocks — so the warnings above were
    // about the memory and nothing else.
    let blocked = ask(&world, &json!({"session": session}));
    blocked.exited(0);
    assert_eq!(
        verdict(&blocked.stdout).expect("a verdict")["verdict"],
        json!("block")
    );
    owner.release("build.go");
}

/// The guard reads no run's merged event store: a run whose store cannot be
/// read is still answered about, from its watch state.
#[test]
fn a_run_whose_event_store_cannot_be_read_is_still_decided_from_its_watch_state() {
    let owner = World::new("stop-guard-nostore");
    let world = guarded(&owner);
    owner.script("build.wait", "hold");
    let run = held(&owner, "guardnostore");
    let session = owner.session.clone();
    let paths = onepipeline::views::RunPaths::under(&world.runs, &run);

    // The store is unreadable and the document beside it is current about what
    // is there — the state `tests/e2e/unwatched.rs` decides the verb over.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] the same substitution
    // `unwatched.rs::store_unreadable` makes, for the same reason: a directory at the
    // store's path is the one form of unreadable every platform agrees on, and the
    // document beside it carries this build's own stamp for what the path now holds.
    std::fs::remove_file(paths.journal()).expect("the run's merged store");
    std::fs::create_dir(paths.journal()).expect("something unreadable at the store's path");
    let about = std::fs::metadata(paths.journal()).expect("what is at the store's path");
    let modified = about
        .modified()
        .expect("a modification time")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("an instant past the epoch")
        .as_millis();
    let mut summary: Value =
        serde_json::from_str(&std::fs::read_to_string(paths.summary()).expect("the document"))
            .expect("a summary document");
    summary["journal_len"] = json!(about.len());
    summary["journal_mtime_ms"] = json!(u64::try_from(modified).expect("a millisecond count"));
    std::fs::write(paths.summary(), summary.to_string()).expect("the document");
    // llmlint: ignore-end[tests_mirror_real_usage]
    assert!(
        std::fs::read(paths.journal()).is_err(),
        "the store can still be read, so nothing below is a claim"
    );

    let asked = ask(&world, &json!({"session": session}));
    asked.exited(0);
    let told = verdict(&asked.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("block"), "{told}");
    assert!(told["reason"]
        .as_str()
        .is_some_and(|reason| reason.contains(&run)));

    // What `unwatched` could not resolve about a run it still reports — here a
    // file where the run's watch records go — the guard writes on standard error
    // exactly as `unwatched` does, and nothing else there.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] the substitution
    // `unwatched.rs` makes for the same state: no verb puts a file where the watch records
    // go, and what it stands in for is a runs root the guard may not read a directory of,
    // a permission this crate cannot set portably. What follows is read off the binary.
    std::fs::write(world.run_file(&run, "watchers"), "not a directory")
        .expect("something where the records go");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let plain = owner.run(&["unwatched", "--session", &session]);
    plain
        .exited(RUNS_UNWATCHED)
        .err_has(&run)
        .err_has("watcher directory");
    let guarded = ask(&world, &json!({"session": session}));
    guarded.exited(0);
    assert_eq!(
        verdict(&guarded.stdout).expect("a verdict")["verdict"],
        json!("block"),
        "{}",
        guarded.stdout
    );
    assert_eq!(
        guarded.stderr, plain.stderr,
        "the guard's standard error is not what `unwatched` writes for the same question"
    );
    owner.release("build.go");
}

/// The Claude Code wiring `docs/stop-guard.md` states, driven over real `Stop`
/// payloads: the payload's own field names in, Claude Code's decision shape
/// out, and silence where the verdict is `none`. Codex's rendering is the same
/// shape, and is driven beside it.
#[test]
fn the_documented_claude_code_wiring_reads_real_stop_payloads_and_answers_its_decision_shape() {
    let owner = World::new("stop-guard-claude");
    let world = guarded(&owner);
    owner.script("build.wait", "hold");
    let _run = held(&owner, "guardclaude");
    let session = owner.session.clone();
    let report = owner
        .run(&["unwatched", "--session", &session])
        .exited(RUNS_UNWATCHED)
        .stdout
        .clone();

    // The command exactly as the docs page wires it, read out of the page so the
    // wiring driven here is the one a reader copies.
    let page = page();
    let settings: Value = serde_json::from_str(
        page.split("## Claude Code")
            .nth(1)
            .expect("the page has a Claude Code section")
            .split("```json")
            .nth(1)
            .expect("the section carries the settings.json entry")
            .split("```")
            .next()
            .expect("the block is fenced"),
    )
    .expect("the settings entry is JSON");
    let command = settings["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .expect("the entry names a command");
    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0]["type"],
        json!("command")
    );
    assert!(
        settings["hooks"]["Stop"][0]["hooks"][0]["timeout"].is_u64(),
        "the entry names no timeout"
    );
    let words: Vec<&str> = command.split_whitespace().collect();
    assert_eq!(words[0], "onepipeline", "{command}");
    let args = &words[1..];

    // A real Stop payload, as Claude Code writes one.
    let payload = |active: bool| {
        json!({
            "session_id": session,
            "transcript_path": "/home/someone/.claude/projects/x/abc.jsonl",
            "cwd": world.root.display().to_string(),
            "permission_mode": "default",
            "hook_event_name": "Stop",
            "stop_hook_active": active,
            "last_assistant_message": "Done for now.",
            "background_tasks": [],
            "session_crons": []
        })
        .to_string()
    };
    let blocked = world.run_with_stdin(args, &payload(false));
    blocked.exited(0);
    assert_eq!(
        verdict(&blocked.stdout),
        Some(json!({"decision": "block", "reason": report})),
        "{}",
        blocked.stdout
    );
    assert!(blocked.stderr.is_empty(), "{}", blocked.stderr);

    // The continuation: Claude Code says so with `stop_hook_active`, and the
    // guard says nothing at all — the harness's "proceed" is silence.
    let again = world.run_with_stdin(args, &payload(true));
    again.exited(0);
    assert_eq!(again.stdout, "", "{}", again.stdout);

    // And Codex's rendering is the same presentation of the same verdict: the
    // continuation is still silent, and a stop that continues nothing — which
    // consults no memory — blocks in the same shape.
    let codex = world.run_with_stdin(&["stop-guard", "--format", "codex"], &payload(true));
    codex.exited(0);
    assert_eq!(codex.stdout, "");
    let codex = world.run_with_stdin(&["stop-guard", "--format", "codex"], &payload(false));
    codex.exited(0);
    assert_eq!(
        verdict(&codex.stdout),
        Some(json!({"decision": "block", "reason": report})),
        "{}",
        codex.stdout
    );

    // A warning is Claude Code's `systemMessage`, with no decision beside it.
    let unreadable = world.root.join("runs-that-are-a-file");
    std::fs::write(&unreadable, "not a directory of runs").expect("something in the way");
    let mut refused = world.cmd(args);
    refused.env("ONEPIPELINE_RUNS_DIR", &unreadable);
    let refused = world.run_with_stdin_on(refused, &payload(false));
    refused.exited(0);
    let told = verdict(&refused.stdout).expect("a verdict");
    assert!(told.get("decision").is_none(), "{told}");
    assert!(
        told["systemMessage"]
            .as_str()
            .is_some_and(|message| message.contains("onepipeline unwatched --session")),
        "{told}"
    );
    owner.release("build.go");
}

/// A declared source as a host writes one: a script that records the bytes it
/// was handed and the session its environment names, and answers whatever the
/// journey last put in its answer file.
///
/// A POSIX shell script, because `--source` is run by the platform shell and
/// `cmd` would not run one — which is why the journeys that use it are Unix's.
#[cfg(unix)]
struct Source {
    command: String,
    answer: PathBuf,
    asked: PathBuf,
    environment: PathBuf,
}

#[cfg(unix)]
impl Source {
    fn new(world: &World, name: &str) -> Self {
        let dir = world.root.join("sources");
        std::fs::create_dir_all(&dir).expect("the sources directory");
        let path = dir.join(name);
        let answer = dir.join(format!("{name}.answer"));
        let asked = dir.join(format!("{name}.asked"));
        let environment = dir.join(format!("{name}.environment"));
        onepipeline_testfakes::executable(
            &path,
            format!(
                "#!/bin/sh\ncat >> '{}'\nprintf '%s\\n' \"$ONEPIPELINE_LAUNCHER_SESSION\" >> '{}'\ncat '{}'\n",
                asked.display(),
                environment.display(),
                answer.display()
            ),
        );
        let source = Self {
            command: path.display().to_string(),
            answer,
            asked,
            environment,
        };
        source.answers(&json!({"verdict": "none"}));
        source
    }

    fn answers(&self, verdict: &Value) {
        std::fs::write(&self.answer, format!("{verdict}\n")).expect("the answer is written");
    }

    fn inputs(&self) -> Vec<String> {
        std::fs::read_to_string(&self.asked)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn environments(&self) -> Vec<String> {
        std::fs::read_to_string(&self.environment)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn declaring<'a>(sources: &'a [&'a str], rest: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["stop-guard"];
    for source in sources {
        args.extend(["--source", source]);
    }
    args.extend(rest);
    args
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] Measured at 4.4s on a
// host at load 32 over 20 cores, beside the journeys in this module that hold a real run
// for 19-24s each on the same run. It holds a run because what it proves is a source's
// verdict combined with the verb's own `unwatched` block, which only a real unwatched run
// makes; the code it guards is `src/stopguard.rs`, under the crate's own edge, so a
// narrower project would drop it out of `nx affected` for the changes it exists to catch.
/// Declared sources are asked about this stop's session with the verb's own
/// input bytes, and every answer is combined into one verdict with the verb's
/// own: the strongest wins and no reason is dropped. The continuation rule
/// holds per source — an unchanged continuation ends the turn, and one where a
/// single source moved refuses on that source alone.
#[cfg(unix)]
#[test]
fn declared_sources_combine_with_the_verbs_own_verdict_and_continue_per_source() {
    let owner = World::new("stop-guard-sources");
    let world = guarded(&owner);
    owner.script("build.wait", "hold");
    let run = held(&owner, "guardsources");
    let session = owner.session.clone();
    let report = owner
        .run(&["unwatched", "--session", &session])
        .exited(RUNS_UNWATCHED)
        .stdout
        .clone();
    let refusing = Source::new(&world, "unpublished");
    let second = Source::new(&world, "unpushed");
    let warning = Source::new(&world, "advisory");
    let quiet = Source::new(&world, "quiet");
    refusing
        .answers(&json!({"verdict": "block", "reason": "branch b-1 preserved, never published"}));
    second
        .answers(&json!({"verdict": "block", "reason": "branch b-2 preserved, never published\n"}));
    warning.answers(&json!({"verdict": "warn", "message": "the registry is stale"}));
    let sources = [
        refusing.command.as_str(),
        second.command.as_str(),
        warning.command.as_str(),
        quiet.command.as_str(),
        refusing.command.as_str(),
    ];

    let first = ask_with(&world, &sources, &json!({"session": session}));
    first.exited(0);
    let told = verdict(&first.stdout).expect("a verdict");
    documented(&told);
    let reason = told["reason"].as_str().expect("a block carries a reason");
    assert!(
        reason.starts_with(&report),
        "the verb's own report leads: {reason}"
    );
    for said in [
        "branch b-1 preserved, never published",
        "branch b-2 preserved, never published",
        "the registry is stale",
        &refusing.command,
        &second.command,
        &warning.command,
    ] {
        assert!(reason.contains(said), "{said:?} is missing from {reason}");
    }
    assert!(!reason.contains(&quiet.command), "{reason}");
    // Every block in the order declared, and the warning after them all.
    let at = |said: &str| reason.find(said).expect("said");
    assert!(
        at(&report) < at("branch b-1")
            && at("branch b-1") < at("branch b-2")
            && at("branch b-2") < at("the registry is stale"),
        "not in the order declared: {reason}"
    );
    assert_eq!(
        reason.matches("branch b-1").count(),
        1,
        "a source declared twice was asked twice: {reason}"
    );
    assert!(reason.contains(&run), "{reason}");
    assert!(first.stderr.is_empty(), "{}", first.stderr);

    // Each source was handed the verb's own neutral input — the very shape the
    // page documents — naming the input's session, never the environment's.
    let template = page()
        .split("## Declared sources")
        .nth(1)
        .and_then(|section| {
            section
                .split('`')
                .find(|span| span.starts_with("{\"session\""))
        })
        .expect("the page spells the input a source is handed")
        .to_owned();
    let bytes = template
        .replace("<ID>", &session)
        .replace("<bool>", "false");
    for source in [&refusing, &second, &warning, &quiet] {
        assert_eq!(source.inputs(), vec![bytes.clone()]);
        let handed: Value = serde_json::from_str(&bytes).expect("the input is JSON");
        let mut documented = documented_input(&session);
        documented["continuation"] = json!(false);
        assert_eq!(handed, documented);
        assert_eq!(source.environments(), vec![session.clone()]);
    }
    // And by hand, those same bytes drive the same answer out of the source.
    let by_hand = std::process::Command::new(&refusing.command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .expect("stdin is piped")
                .write_all(refusing.inputs()[0].as_bytes())?;
            child.wait_with_output()
        })
        .expect("the source runs by hand");
    assert_eq!(
        serde_json::from_slice::<Value>(&by_hand.stdout).expect("its answer"),
        json!({"verdict": "block", "reason": "branch b-1 preserved, never published"})
    );

    // The continuation over every unchanged report ends the turn, the warning
    // included in nothing because a warning was never a refusal to continue.
    warning.answers(&json!({"verdict": "none"}));
    let ended = ask_with(
        &world,
        &sources,
        &json!({"session": session, "continuation": true}),
    );
    ended.exited(0);
    assert_eq!(verdict(&ended.stdout), Some(json!({"verdict": "none"})));
    assert!(refusing.inputs()[1].contains("\"continuation\":true"));

    // One source's condition moves: the continuation refuses on it alone, and
    // neither the verb's unchanged report nor the other source's is repeated.
    refusing
        .answers(&json!({"verdict": "block", "reason": "branch b-3 preserved, never published"}));
    let moved = ask_with(
        &world,
        &sources,
        &json!({"session": session, "continuation": true}),
    );
    moved.exited(0);
    let reason = verdict(&moved.stdout).expect("a verdict")["reason"]
        .as_str()
        .expect("a reason")
        .to_owned();
    assert!(reason.contains("b-3"), "{reason}");
    assert!(
        !reason.contains("b-2") && !reason.contains(&run),
        "{reason}"
    );

    // Its condition clears: nothing to refuse on, and what it last blocked
    // on is forgotten — so should that same condition return, even a
    // continuation refuses on it again.
    let idle = json!({"session": "a-session-owning-nothing"});
    let alone = [refusing.command.as_str()];
    ask_with(&world, &alone, &idle).exited(0);
    let remembered = source_memory(&world, "a-session-owning-nothing", &refusing.command);
    assert!(remembered.is_file(), "the block was not remembered");
    refusing.answers(&json!({"verdict": "none"}));
    let cleared = ask_with(&world, &alone, &idle);
    cleared.exited(0);
    assert_eq!(verdict(&cleared.stdout), Some(json!({"verdict": "none"})));
    assert!(!remembered.exists(), "a cleared source kept its memory");
    refusing
        .answers(&json!({"verdict": "block", "reason": "branch b-3 preserved, never published"}));
    let returned = ask_with(
        &world,
        &alone,
        &json!({"session": "a-session-owning-nothing", "continuation": true}),
    );
    returned.exited(0);
    assert_eq!(
        verdict(&returned.stdout).expect("a verdict")["verdict"],
        json!("block"),
        "{}",
        returned.stdout
    );
    // A warning in its place clears it just the same.
    assert!(
        remembered.is_file(),
        "the returned block was not remembered"
    );
    refusing.answers(&json!({"verdict": "warn", "message": "the origin is slow"}));
    ask_with(&world, &alone, &idle).exited(0);
    assert!(!remembered.exists(), "a source now warning kept its block");

    // Warnings alone are one warning carrying each, under every rendering.
    refusing.answers(&json!({"verdict": "none"}));
    second.answers(&json!({"verdict": "warn", "message": "the origin is unreachable"}));
    warning.answers(&json!({"verdict": "warn", "message": "the registry is stale"}));
    let only = [warning.command.as_str(), second.command.as_str()];
    let warned = ask_with(&world, &only, &idle);
    warned.exited(0);
    let told = verdict(&warned.stdout).expect("a verdict");
    documented(&told);
    assert!(
        told["message"]
            .as_str()
            .is_some_and(|message| message.contains("the registry is stale")
                && message.contains("the origin is unreachable")),
        "{told}"
    );
    let hooked = world.run_with_stdin(
        &declaring(&only, &["--format", "claude-code"]),
        &json!({"session_id": "a-session-owning-nothing", "stop_hook_active": false}).to_string(),
    );
    hooked.exited(0);
    let told = verdict(&hooked.stdout).expect("a verdict");
    assert!(told.get("decision").is_none(), "{told}");
    assert!(told["systemMessage"]
        .as_str()
        .is_some_and(|message| message.contains("the registry is stale")));
    owner.release("build.go");
}

// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

#[cfg(unix)]
fn ask_with(world: &World, sources: &[&str], input: &Value) -> crate::harness::Run {
    world.run_with_stdin(&declaring(sources, &[]), &input.to_string())
}

/// Under both hook renderings a declared source is asked about the payload's
/// session — never the one the hook's environment names — and its block is the
/// harness's refusal, after which the continuation the harness marks ends the
/// turn.
#[cfg(unix)]
#[test]
fn a_hook_rendering_asks_a_source_about_the_payloads_session_and_its_continuation_ends_the_turn() {
    let owner = World::new("stop-guard-source-hook");
    let world = guarded(&owner);
    let source = Source::new(&world, "unfinished");
    source.answers(&json!({"verdict": "block", "reason": "branch b-1 preserved, never published"}));
    let session = "the-workers-own-session";
    let payload = |active: bool| {
        json!({
            "session_id": session,
            "transcript_path": "/home/someone/.claude/projects/x/abc.jsonl",
            "cwd": world.root.display().to_string(),
            "hook_event_name": "Stop",
            "stop_hook_active": active,
        })
        .to_string()
    };
    // A second refusing source, so each rendering's refusal is a combined one
    // and carries every block rather than the first.
    let other = Source::new(&world, "unpushed");
    other.answers(&json!({"verdict": "block", "reason": "stash s-1 never pushed"}));
    let sources = [source.command.as_str(), other.command.as_str()];
    for (format, turn) in [("claude-code", 0), ("codex", 1)] {
        // Each rendering's first stop blocks; a fresh reason per rendering
        // keeps the one before it from being this one's continuation.
        source.answers(&json!({
            "verdict": "block",
            "reason": format!("branch b-{turn} preserved, never published"),
        }));
        let blocked =
            world.run_with_stdin(&declaring(&sources, &["--format", format]), &payload(false));
        blocked.exited(0);
        let told = verdict(&blocked.stdout).expect("a verdict");
        assert_eq!(told["decision"], json!("block"), "{format}: {told}");
        assert!(
            told["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains(&format!("b-{turn}"))
                    && reason.contains(&source.command)
                    && reason.contains("stash s-1 never pushed")
                    && reason.contains(&other.command)),
            "{format}: {told}"
        );
        let ended =
            world.run_with_stdin(&declaring(&sources, &["--format", format]), &payload(true));
        ended.exited(0);
        assert_eq!(
            ended.stdout, "",
            "{format}: the continuation did not end the turn"
        );
    }
    for input in source.inputs() {
        let handed: Value = serde_json::from_str(&input).expect("the input is JSON");
        assert_eq!(handed["session"], json!(session), "{handed}");
        assert!(!input.contains(INHERITED), "{input}");
    }
    assert_eq!(source.inputs().len(), 4);
    assert_eq!(other.inputs(), source.inputs());
    assert!(source.environments().iter().all(|named| named == session));
}

// llmlint: ignore-block[expensive_tests_stay_behind_their_own_edge] Measured at 14.9s on a
// host at load 32 over 20 cores, beside the journeys in this module that hold a real run
// for 19-24s each on the same run; its cost is two one-second deadlines and short-lived
// scripts. What it guards is `src/stopguard.rs` and the clap surface in `src/cli.rs`, both
// under the crate's own edge, so a narrower project would drop it out of `nx affected` for
// the changes it exists to catch — the ground `tests/e2e/session_reuse.rs` carries too.
/// A source that cannot be consulted is reported in the verdict — naming the
/// source and what went wrong — and refuses the stop rather than letting it
/// pass; the continuation after that refusal ends the turn, so a broken source
/// cannot hold one in a loop — unless the failure moved, which refuses again.
#[cfg(unix)]
#[test]
fn a_source_that_cannot_be_consulted_refuses_the_stop_naming_itself_and_never_passes() {
    let owner = World::new("stop-guard-source-broken");
    let world = guarded(&owner);
    let session = "a-session-owning-nothing";
    let answering = |name: &str, body: &str| {
        let path = world.root.join("sources").join(name);
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("the directory");
        onepipeline_testfakes::executable(&path, format!("#!/bin/sh\ncat > /dev/null\n{body}\n"));
        path.display().to_string()
    };
    let left_behind = world.root.join("left-behind.pid");
    let missing = world
        .root
        .join("sources/nothing-here")
        .display()
        .to_string();
    let cases: Vec<(String, &str)> = vec![
        (missing, "could not find"),
        (
            answering(
                "fails",
                "echo '{\"verdict\":\"none\"}'; echo 'what went wrong' >&2; exit 3",
            ),
            "exited with status 3",
        ),
        (answering("silent", "true"), "wrote nothing"),
        // A command line rather than a script, so the shell running it is
        // what the signal ends.
        ("kill -9 $$".to_owned(), "ended by a signal"),
        (
            answering("slow", "exec sleep 30"),
            "did not answer within 1 second(s)",
        ),
        (answering("prose", "echo 'all good'"), "not one JSON object"),
        (
            answering("unknown-word", "echo '{\"verdict\":\"maybe\"}'"),
            "unknown variant `maybe`",
        ),
        (
            answering("reasonless", "echo '{\"verdict\":\"block\"}'"),
            "missing field `reason`",
        ),
        (
            answering(
                "blank-reason",
                "echo '{\"verdict\":\"block\",\"reason\":\"  \"}'",
            ),
            "`reason` is blank",
        ),
        (
            answering(
                "blank-message",
                "echo '{\"verdict\":\"warn\",\"message\":\"\"}'",
            ),
            "`message` is blank",
        ),
        (
            answering("crossed", "echo '{\"verdict\":\"warn\",\"reason\":\"x\"}'"),
            "unknown field `reason`",
        ),
        (
            answering("extra", "echo '{\"verdict\":\"none\",\"why\":\"x\"}'"),
            "unknown field `why`",
        ),
        (
            answering(
                "twice",
                "echo '{\"verdict\":\"none\"}'; echo '{\"verdict\":\"none\"}'",
            ),
            "not one JSON object",
        ),
        (
            answering("array", "echo '[{\"verdict\":\"none\"}]'"),
            "not one JSON object",
        ),
        (
            answering(
                "repeated-key",
                "echo '{\"verdict\":\"none\",\"verdict\":\"block\",\"reason\":\"x\"}'",
            ),
            "duplicate field `verdict`",
        ),
        (
            answering("binary", "printf '\\377\\n'"),
            "bytes that are not text",
        ),
        (
            answering(
                "flood",
                "head -c 1100000 /dev/zero | tr '\\0' ' '; echo '{\"verdict\":\"none\"}'",
            ),
            "more than 1048576 bytes",
        ),
        // It exits at once, and what it left behind holds its standard output
        // past the deadline.
        (
            answering(
                "left-behind",
                &format!(
                    "sleep 300 &\necho $! > '{}'\necho '{{\"verdict\":\"none\"}}'",
                    left_behind.display()
                ),
            ),
            "did not answer within 1 second(s)",
        ),
    ];
    for (command, what) in &cases {
        let sources = [command.as_str()];
        let started = std::time::Instant::now();
        let refused = world.run_with_stdin(
            &declaring(&sources, &["--source-timeout", "1"]),
            &json!({"session": session}).to_string(),
        );
        refused.exited(0);
        // The slow source sleeps for 30 seconds, so a stop answered sooner is
        // one that ended it rather than waited it out.
        assert!(
            !command.ends_with("/slow") || started.elapsed() < std::time::Duration::from_secs(30),
            "{command}: the stop was held {:?}",
            started.elapsed()
        );
        let told = verdict(&refused.stdout).expect("a verdict");
        assert_eq!(told["verdict"], json!("block"), "{command}: {told}");
        documented(&told);
        let reason = told["reason"].as_str().expect("a reason");
        assert!(
            reason.contains(command.as_str())
                && reason.contains("could not be consulted")
                && reason.contains(what),
            "{command}: the refusal does not name the source and {what:?}: {reason}"
        );
        assert!(
            reason.contains(&json!({"session": session, "continuation": false}).to_string()),
            "{command}: the refusal does not say what to hand it by hand: {reason}"
        );
        assert!(refused.stderr.is_empty(), "{command}: {}", refused.stderr);
        if command.ends_with("/left-behind") {
            let pid = std::fs::read_to_string(&left_behind).expect("the pid it left behind");
            owner.until("what the source left behind to be ended", |_| {
                !std::process::Command::new("kill")
                    .args(["-0", pid.trim()])
                    .status()
                    .expect("kill runs")
                    .success()
            });
        }

        // Were the refusal's report to move between a stop and its
        // continuation, this would refuse again, and a broken source would
        // hold the turn until the harness gave up on it.
        let ended = world.run_with_stdin(
            &declaring(&sources, &["--source-timeout", "1"]),
            &json!({"session": session, "continuation": true}).to_string(),
        );
        ended.exited(0);
        assert_eq!(
            verdict(&ended.stdout),
            Some(json!({"verdict": "none"})),
            "{command}: {}",
            ended.stdout
        );
    }

    // A failure that *moves* is a new condition: a continuation over a
    // different failure refuses again on it, and only an unchanged one ends
    // the turn.
    let failing = world.root.join("sources/failing.how");
    let moving = answering("moving", &format!(". '{}'", failing.display()));
    let stop = |how: &str, continuation: bool| {
        std::fs::write(&failing, format!("{how}\n")).expect("how it fails is written");
        let run = world.run_with_stdin(
            &declaring(&[moving.as_str()], &[]),
            &json!({"session": session, "continuation": continuation}).to_string(),
        );
        run.exited(0);
        verdict(&run.stdout).expect("a verdict")
    };
    let refused_on = |told: &Value, what: &str| {
        assert_eq!(told["verdict"], json!("block"), "{what}: {told}");
        assert!(
            told["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("could not be consulted")
                    && reason.contains(what)
                    && reason.contains(&moving)),
            "{what}: {told}"
        );
    };
    refused_on(&stop("exit 3", false), "exited with status 3");
    assert_eq!(stop("exit 3", true), json!({"verdict": "none"}));
    refused_on(&stop("exit 4", true), "exited with status 4");
    refused_on(&stop("echo 'all good'", true), "not one JSON object");
    assert_eq!(stop("echo 'all good'", true), json!({"verdict": "none"}));

    let unstartable = answering("unstartable", "echo '{\"verdict\":\"none\"}'");
    let mut shell_less = world.cmd(&declaring(&[unstartable.as_str()], &[]));
    shell_less.env("PATH", world.empty_path());
    let shell_less = world.run_with_stdin_on(shell_less, &json!({"session": session}).to_string());
    shell_less.exited(0);
    let told = verdict(&shell_less.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("block"), "{told}");
    assert!(
        told["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("could not be consulted")
                && reason.contains("it could not be started")
                && reason.contains(&unstartable)),
        "{told}"
    );

    // And a source whose memory cannot be kept stands aside in a warning that
    // still names what it would refuse on, exactly as the verb's own does.
    let refusing = answering(
        "refusing",
        "echo '{\"verdict\":\"block\",\"reason\":\"branch b-1 preserved\"}'",
    );
    let path = source_memory(&world, session, &refusing);
    // llmlint: ignore-block[tests_mirror_real_usage] nothing writes a directory at a
    // memory's path; the same stand-in the verb's own memory journey above uses for a
    // record this process cannot write.
    std::fs::create_dir_all(path.join("held")).expect("something in the way");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let unread = world.run_with_stdin(
        &declaring(&[refusing.as_str()], &[]),
        &json!({"session": session, "continuation": true}).to_string(),
    );
    unread.exited(0);
    let told = verdict(&unread.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("warn"), "{told}");
    assert!(
        told["message"].as_str().is_some_and(|message| message
            .contains("could not read what it last blocked on")
            && message.contains("branch b-1 preserved")
            && message.contains(&refusing)),
        "{told}"
    );
    let aside = world.run_with_stdin(
        &declaring(&[refusing.as_str()], &[]),
        &json!({"session": session}).to_string(),
    );
    aside.exited(0);
    let told = verdict(&aside.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("warn"), "{told}");
    assert!(
        told["message"].as_str().is_some_and(|message| message
            .contains("could not record what it would block on")
            && message.contains("branch b-1 preserved")),
        "{told}"
    );
    // One that answers nothing to refuse over that same unremovable memory
    // says so rather than letting a later continuation through in silence.
    let quiet = answering("quiet-refusing", "echo '{\"verdict\":\"none\"}'");
    let stuck = source_memory(&world, session, &quiet);
    // llmlint: ignore-block[tests_mirror_real_usage] the same directory-in-the-way.
    std::fs::create_dir_all(stuck.join("held")).expect("something in the way");
    // llmlint: ignore-end[tests_mirror_real_usage]
    let unremoved = world.run_with_stdin(
        &declaring(&[quiet.as_str()], &[]),
        &json!({"session": session}).to_string(),
    );
    unremoved.exited(0);
    let told = verdict(&unremoved.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("warn"), "{told}");
    assert!(
        told["message"]
            .as_str()
            .is_some_and(|message| message.contains("could not remove what it last blocked on")),
        "{told}"
    );
}
// llmlint: ignore-end[expensive_tests_stay_behind_their_own_edge]

/// Declared sources are consulted together, so the stop is bounded by the
/// slowest of them rather than by their sum: each of these two answers only
/// once it has seen the other start, which consulting them one after the other
/// could never satisfy.
#[cfg(unix)]
#[test]
fn declared_sources_are_consulted_together() {
    let owner = World::new("stop-guard-source-together");
    let world = guarded(&owner);
    let dir = world.root.join("sources");
    std::fs::create_dir_all(&dir).expect("the sources directory");
    let meeting = |mine: &str, theirs: &str| {
        let path = dir.join(mine);
        onepipeline_testfakes::executable(
            &path,
            format!(
                "#!/bin/sh\ncat > /dev/null\ntouch '{}'\nuntil [ -f '{}' ]; do sleep 0.1; done\necho '{{\"verdict\":\"none\"}}'\n",
                dir.join(format!("{mine}.started")).display(),
                dir.join(format!("{theirs}.started")).display()
            ),
        );
        path.display().to_string()
    };
    let first = meeting("first", "second");
    let second = meeting("second", "first");
    let met = world.run_with_stdin(
        &declaring(
            &[first.as_str(), second.as_str()],
            &["--source-timeout", "20"],
        ),
        &json!({"session": "a-session-owning-nothing"}).to_string(),
    );
    met.exited(0);
    assert_eq!(
        verdict(&met.stdout),
        Some(json!({"verdict": "none"})),
        "{}",
        met.stdout
    );
}

/// A source that closes its standard input before its input was delivered in
/// full answered without knowing whose stop this is, so its `none` is not
/// taken: the stop is refused naming the failed delivery, and the continuation
/// after that refusal ends the turn.
///
/// The session is longer than a pipe buffer (64 KiB on Linux), so the input
/// cannot sit whole in the pipe ahead of the source's closing it: the write is
/// refused every time rather than racing the source's start. It stays under
/// the 128 KiB one environment string may hold, because the source's
/// environment names the session too.
#[cfg(unix)]
#[test]
fn a_source_not_delivered_its_whole_input_refuses_the_stop_and_its_continuation_ends_the_turn() {
    let owner = World::new("stop-guard-source-undelivered");
    let world = guarded(&owner);
    let path = world.root.join("sources/closes-its-input");
    std::fs::create_dir_all(path.parent().expect("a directory")).expect("the directory");
    onepipeline_testfakes::executable(
        &path,
        "#!/bin/sh\nexec 0<&-\necho '{\"verdict\":\"none\"}'\n",
    );
    let source = path.display().to_string();
    let session = format!("s-{}", "x".repeat(100_000));
    let sources = [source.as_str()];

    // Input the pipe took is delivered whether or not it was read: this one
    // reads a single byte — which a write this size lands whole behind — and
    // leaves the rest, and its answer is taken.
    let partly = world.root.join("sources/reads-one-byte");
    onepipeline_testfakes::executable(
        &partly,
        "#!/bin/sh\nhead -c 1 > /dev/null\necho '{\"verdict\":\"block\",\"reason\":\"host-wide freeze\"}'\n",
    );
    let partly = partly.display().to_string();
    let taken = world.run_with_stdin(
        &declaring(&[partly.as_str()], &[]),
        &json!({"session": "a-short-session"}).to_string(),
    );
    taken.exited(0);
    let told = verdict(&taken.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("block"), "{told}");
    assert!(
        told["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("host-wide freeze")
                && !reason.contains("could not be consulted")),
        "{told}"
    );

    let refused = world.run_with_stdin(
        &declaring(&sources, &[]),
        &json!({"session": session}).to_string(),
    );
    refused.exited(0);
    let told = verdict(&refused.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("block"), "the `none` was taken");
    documented(&told);
    let reason = told["reason"].as_str().expect("a reason");
    let head: String = reason.chars().take(400).collect();
    assert!(
        reason.contains(&source)
            && reason.contains("could not be consulted")
            && reason.contains("its input could not be delivered to it in full"),
        "the refusal does not name the source and the failed delivery: {head}"
    );
    assert!(refused.stderr.is_empty(), "{}", refused.stderr);

    let ended = world.run_with_stdin(
        &declaring(&sources, &[]),
        &json!({"session": session, "continuation": true}).to_string(),
    );
    ended.exited(0);
    assert_eq!(
        verdict(&ended.stdout),
        Some(json!({"verdict": "none"})),
        "the continuation did not end the turn"
    );

    // One that stays alive and never reads an input larger than the pipe holds
    // cannot hold the stop on that full pipe: it is ended at the deadline.
    let stalled = world.root.join("sources/never-reads");
    onepipeline_testfakes::executable(
        &stalled,
        "#!/bin/sh
exec sleep 30
",
    );
    let stalled = stalled.display().to_string();
    let started = std::time::Instant::now();
    let late = world.run_with_stdin(
        &declaring(&[stalled.as_str()], &["--source-timeout", "1"]),
        &json!({"session": session}).to_string(),
    );
    late.exited(0);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the stop was held {:?}",
        started.elapsed()
    );
    let told = verdict(&late.stdout).expect("a verdict");
    assert_eq!(told["verdict"], json!("block"), "the stall was taken");
    let reason = told["reason"].as_str().expect("a reason");
    assert!(
        reason.contains(&stalled) && reason.contains("did not answer within 1 second(s)"),
        "the refusal does not name the source and its deadline: {}",
        reason.chars().take(400).collect::<String>()
    );
}
