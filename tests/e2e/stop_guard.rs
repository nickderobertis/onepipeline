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

/// Where the guard remembers what it last blocked `session` on.
fn memory(world: &World, session: &str) -> PathBuf {
    world
        .root
        .join("state")
        .join("onepipeline")
        .join("stop-guard")
        .join(hex(&Sha256::digest(session.as_bytes())))
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

    // A continuation over the same report: the manager was told and did nothing,
    // so nothing is said again, and the memory stands.
    let again = ask(&world, &json!({"session": session, "continuation": true}));
    again.exited(0);
    assert_eq!(
        verdict(&again.stdout),
        Some(json!({"verdict": "none"})),
        "{}",
        again.stdout
    );
    assert!(memory(&world, &session).is_file(), "the memory was dropped");

    // The same continuation flag as `--continuation`, decided the same way.
    world
        .run_with_stdin(
            &["stop-guard", "--continuation"],
            &json!({"session": session}).to_string(),
        )
        .exited(0)
        .out_has("\"none\"");

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

    // A memory this guard cannot read, on a continuation: a directory where the
    // file goes.
    //
    // llmlint: ignore-block[tests_mirror_real_usage] nothing writes a directory at a memory's
    // path; it stands in for a record this process cannot read back — a permission, a
    // filesystem refusing — in the one form every platform refuses to read as a file.
    let path = memory(&world, &session);
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
    let page = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/stop-guard.md"),
    )
    .expect("the docs page ships");
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
    // continuation is still silent, and with the memory gone the same payload
    // blocks again in the same shape.
    let codex = world.run_with_stdin(&["stop-guard", "--format", "codex"], &payload(true));
    codex.exited(0);
    assert_eq!(codex.stdout, "");
    std::fs::remove_file(memory(&world, &session)).expect("the memory");
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
