//! A real `onevcs` executable, scripted from a directory.
//!
//! It speaks the sibling's command surface — `session open`, `publish`,
//! `session close`, `events [--follow]` — hands back a real worktree directory,
//! and records every invocation. `onevcs` is fully implemented — `real_vcs.rs`
//! drives the real binary through a whole publication over a real git origin —
//! so this stands in for it only to *state a scenario directly*: a rejected gate,
//! a held publication, a session that will not open, a host that queued the
//! merge. Every kind it writes comes from [`onevcs::EventKind`], so a shape the
//! sibling does not produce cannot be scripted here.
//!
//! The session's stream is a **file it appends to as the session works**, which
//! is what makes `events --follow` mean anything: a publication writes its gate,
//! its push, and its change request one at a time, and a reader following the
//! stream sees each as it lands rather than all of them once the session closes.

use onepipeline_testfakes as fake;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "onevcs", &args);

    match (
        args.first().map(String::as_str),
        args.get(1).map(String::as_str),
    ) {
        (Some("session"), Some("open")) => open(&args, &dir),
        (Some("session"), Some("close")) => close(&args, &dir),
        (Some("publish"), _) => publish(&args, &dir),
        (Some("events"), _) => events(&args, &dir),
        (Some("session"), Some(other)) => {
            fake::refuse(&format!("unknown onevcs session command '{other}'"))
        }
        (Some(other), _) => fake::refuse(&format!("unknown onevcs command '{other}'")),
        (None, _) => fake::refuse("onevcs takes a command"),
    }
}

/// Where one session's stream is written, so `events` can read it back.
///
/// The token arrives on a command line, so it is written as one path segment
/// rather than interpolated whole: a token carrying a separator would put this
/// double's scratch outside the directory the test gave it.
fn stream_of(dir: &Path, token: &str) -> PathBuf {
    dir.join("streams")
        .join(format!("{}.jsonl", fake::segment(token)))
}

/// The marker a closed session leaves, which is what ends a `--follow`.
fn closed_marker(dir: &Path, token: &str) -> PathBuf {
    dir.join("streams")
        .join(format!("{}.closed", fake::segment(token)))
}

/// Append one envelope to the session's stream, in the shape the contract fixes.
///
/// The kind is [`onevcs::EventKind`] — the sibling's own enum, which is what
/// spells a kind on the wire — rather than a string spelled again here. A kind
/// the sibling does not have cannot be written, which is how this double stays a
/// statement of a scenario the real one could produce rather than an oracle for
/// a shape nobody emits.
fn emit(dir: &Path, token: &str, kind: onevcs::EventKind, payload: serde_json::Value) {
    let path = stream_of(dir, token);
    // The sibling's own numbering: `onevcs::stream::Stream` resumes the series
    // from what the file already holds and increments *before* it writes, so the
    // first record of a stream is `1` and no record is ever `0`. A double that
    // numbered from zero would let a consumer treat "nothing relayed" and "the
    // first record relayed" as one answer and never hear about it.
    let seq = std::fs::read_to_string(&path)
        .map(|text| text.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0)
        + 1;
    fake::append(
        &path,
        &serde_json::json!({
            "v": 1,
            "ts": fake::now(),
            "stream": format!("fake-onevcs-{token}"),
            "seq": seq,
            "source": "vcs",
            "kind": kind,
            // Stamped with what a session knows, which does not include the
            // graph node it is working for: naming one here would make this
            // double a weaker oracle than the sibling, whose envelopes arrive
            // with no node on them at all.
            "labels": {},
            "payload": payload,
        })
        .to_string(),
    );
}

/// `onevcs session open REPO [--branch B] [--base C] [--execution-checkout A]`
fn open(args: &[String], dir: &std::path::Path) -> ExitCode {
    let repo = match fake::required(args, 2, "REPO") {
        Ok(repo) => repo,
        Err(refusal) => return refusal,
    };
    if dir.join("session-open.fail").exists() {
        eprintln!("no registered identity for {repo}");
        return ExitCode::from(2);
    }
    let branch = fake::flag(args, "--branch")
        .unwrap_or_else(|| format!("onepipeline/{}", repo.replace('/', "-")));
    let base = fake::flag(args, "--base").unwrap_or_else(|| "main".into());
    // A real worktree directory: the dispatch runs in it, so it has to exist.
    let token = format!("session-{}", branch.replace('/', "-"));
    let worktree = dir.join("worktrees").join(&token);
    if let Err(error) = std::fs::create_dir_all(&worktree) {
        eprintln!("cannot create the worktree {}: {error}", worktree.display());
        return ExitCode::from(2);
    }
    // The stream exists from the moment the session does, which is what a
    // follower asking for it straight afterwards depends on.
    emit(
        dir,
        &token,
        onevcs::EventKind::SessionOpened,
        serde_json::json!({"branch": branch, "base": base}),
    );

    println!(
        "{}",
        serde_json::json!({
            "token": token,
            "worktree": worktree,
            "branch": branch,
            "base": base,
        })
    );
    ExitCode::SUCCESS
}

/// `onevcs session close TOKEN`
///
/// The marker comes **first**, because that is the sibling's own order:
/// `app::session_close` calls `workspace::close`, which flips the record to
/// `Closed`, and only then opens the stream and emits `session-closed`. A
/// follower prints what the file holds and *then* asks whether the session
/// closed, so the record can land after the follow has already returned — a
/// window this double used to close by writing the record first, which made the
/// tail unlosable here and losable against every real session.
fn close(args: &[String], dir: &std::path::Path) -> ExitCode {
    let token = match fake::required(args, 2, "TOKEN") {
        Ok(token) => token,
        Err(refusal) => return refusal,
    };
    fake::append(&closed_marker(dir, &token), "closed");
    // The real window is microseconds wide, which is a flake rather than a
    // scenario. Widening it on request is how a journey *states* the case —
    // "the sibling wrote its last record after the follow ended" — and depends
    // on it rather than on how the host happened to schedule two processes.
    if dir.join("close.slow-record").exists() {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    emit(
        dir,
        &token,
        onevcs::EventKind::SessionClosed,
        serde_json::json!({}),
    );
    ExitCode::SUCCESS
}

/// `onevcs publish TOKEN [--policy P] [--title T]`
///
/// The longest wall-clock stretch of a lifecycle node, and the one that used to
/// be invisible until it was over. It records each phase as it reaches it.
fn publish(args: &[String], dir: &std::path::Path) -> ExitCode {
    let token = match fake::required(args, 1, "TOKEN") {
        Ok(token) => token,
        Err(refusal) => return refusal,
    };
    // The identity's lock comes before its gate, and a publication waits on
    // both. Each is held separately where a test asks, so the two stretches can
    // be measured apart from each other and from the agent's.
    emit(
        dir,
        &token,
        onevcs::EventKind::LockWait,
        serde_json::json!({"identity": token}),
    );
    if dir.join("publish.hold").exists() {
        fake::wait_for(&dir.join("publish.go"));
    }
    emit(
        dir,
        &token,
        onevcs::EventKind::LockAcquired,
        serde_json::json!({}),
    );
    emit(
        dir,
        &token,
        onevcs::EventKind::GateStarted,
        serde_json::json!({}),
    );
    if dir.join("gate.hold").exists() {
        fake::wait_for(&dir.join("gate.go"));
    }
    if dir.join("publish.fail").exists() {
        emit(
            dir,
            &token,
            onevcs::EventKind::GateVerdict,
            serde_json::json!({"verdict": "failed"}),
        );
        eprintln!("the merge-path gate rejected the branch");
        return ExitCode::from(1);
    }
    emit(
        dir,
        &token,
        onevcs::EventKind::GateVerdict,
        serde_json::json!({"verdict": "passed"}),
    );
    // No `verification-finished` between the verdict and the push: this double
    // used to write one, and `onevcs::EventKind` has no such variant because the
    // real command has never emitted it. Taking the kinds from the sibling's enum
    // is what found it.
    emit(
        dir,
        &token,
        onevcs::EventKind::Push,
        serde_json::json!({"remote": "origin"}),
    );

    let title = fake::flag(args, "--title").unwrap_or_default();
    fake::append(
        &dir.join("published.jsonl"),
        &serde_json::json!({"token": token, "title": title}).to_string(),
    );
    let url = format!("https://example.invalid/changes/{token}");
    emit(
        dir,
        &token,
        onevcs::EventKind::ChangeOpened,
        serde_json::json!({"url": url, "id": token}),
    );

    // What the host did with the change request it was handed. Each ending is the
    // sibling's own: the records it writes and, on stdout, the one line of prose
    // it prints for a person — `Outcome::describe`. A double that printed a JSON
    // record here would be an oracle for a shape the sibling has never produced,
    // which is how this crate came to parse the publication's stdout as JSON and
    // fail every real publication as unreadable.
    if dir.join("publish.merged").exists() {
        let sha = format!("{token}-sha");
        emit(
            dir,
            &token,
            onevcs::EventKind::ChangeMerged,
            serde_json::json!({"url": url, "sha": sha}),
        );
        println!("merged at {sha}");
        return ExitCode::SUCCESS;
    }
    if dir.join("publish.queued").exists() {
        emit(
            dir,
            &token,
            onevcs::EventKind::MergeQueued,
            serde_json::json!({"identity": token, "queue_position": 0, "url": url}),
        );
        println!("merge queued for {url}");
        return ExitCode::SUCCESS;
    }
    println!("change request open at {url}");
    ExitCode::SUCCESS
}

/// `onevcs events TOKEN [--follow]` — the session's own stream, for the merge.
///
/// `--follow` keeps reading until the session closes, printing everything it
/// has not printed yet *before* it checks — the sibling's own ordering, which is
/// what stops a follower losing the tail of a session that closed under it.
fn events(args: &[String], dir: &std::path::Path) -> ExitCode {
    let token = match fake::required(args, 1, "TOKEN") {
        Ok(token) => token,
        Err(refusal) => return refusal,
    };
    if dir.join("events.fail").exists() {
        eprintln!("no such session {token}");
        return ExitCode::from(2);
    }
    if dir.join("events.unreadable").exists() {
        println!("{{\"from\":\"a newer onevcs\"}}");
        return ExitCode::SUCCESS;
    }
    // The real CLI parses this command with clap, so it refuses an argument it
    // does not know. A double that shrugged one off would let this crate reach
    // the sibling with a flag it has never had and keep the suite green.
    if let Some(unknown) = args.iter().skip(2).find(|arg| *arg != "--follow") {
        return fake::refuse(&format!("onevcs events does not take '{unknown}'"));
    }
    let follow = args.iter().any(|arg| arg == "--follow");
    let path = stream_of(dir, &token);
    // The real CLI refuses a token whose stream it cannot find, and a double
    // that read a missing file as an empty one would let a caller ask for a
    // session that never existed and hear nothing about it.
    if !path.is_file() {
        eprintln!("no event stream for {token:?}");
        return ExitCode::from(2);
    }
    let mut written = 0usize;
    loop {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("cannot read the stream for {token:?}: {error}");
                return ExitCode::from(2);
            }
        };
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        for line in lines.iter().skip(written) {
            println!("{line}");
        }
        written = lines.len();
        if !follow || closed_marker(dir, &token).exists() {
            return ExitCode::SUCCESS;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
