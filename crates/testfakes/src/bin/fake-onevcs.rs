//! A real `onevcs` executable, scripted from a directory.
//!
//! It speaks the sibling's command surface — `session open`, `publish`,
//! `session close`, `events [--follow]` — hands back a real worktree directory,
//! and records every invocation. It stands in for `onevcs`, which is itself
//! interface-only; what a lifecycle test proves is this crate's half of the
//! composition.
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
fn stream_of(dir: &Path, token: &str) -> PathBuf {
    dir.join("streams").join(format!("{token}.jsonl"))
}

/// The marker a closed session leaves, which is what ends a `--follow`.
fn closed_marker(dir: &Path, token: &str) -> PathBuf {
    dir.join("streams").join(format!("{token}.closed"))
}

/// Append one envelope to the session's stream, in the shape the contract fixes.
fn emit(dir: &Path, token: &str, kind: &str, payload: serde_json::Value) {
    let path = stream_of(dir, token);
    let seq = std::fs::read_to_string(&path)
        .map(|text| text.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0);
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
        "session-opened",
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
fn close(args: &[String], dir: &std::path::Path) -> ExitCode {
    let token = match fake::required(args, 2, "TOKEN") {
        Ok(token) => token,
        Err(refusal) => return refusal,
    };
    emit(dir, &token, "session-closed", serde_json::json!({}));
    // Written last: a follower prints everything it has not printed *and then*
    // asks whether the session closed, so the tail is never lost to the marker
    // arriving first.
    fake::append(&closed_marker(dir, &token), "closed");
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
        "lock-wait",
        serde_json::json!({"identity": token}),
    );
    if dir.join("publish.hold").exists() {
        fake::wait_for(&dir.join("publish.go"));
    }
    emit(dir, &token, "lock-acquired", serde_json::json!({}));
    emit(dir, &token, "gate-started", serde_json::json!({}));
    if dir.join("gate.hold").exists() {
        fake::wait_for(&dir.join("gate.go"));
    }
    if dir.join("publish.fail").exists() {
        emit(
            dir,
            &token,
            "gate-verdict",
            serde_json::json!({"verdict": "failed"}),
        );
        eprintln!("the merge-path gate rejected the branch");
        return ExitCode::from(1);
    }
    emit(
        dir,
        &token,
        "gate-verdict",
        serde_json::json!({"verdict": "passed"}),
    );
    emit(
        dir,
        &token,
        "verification-finished",
        serde_json::json!({"verdict": "passed", "token": token}),
    );
    emit(dir, &token, "push", serde_json::json!({"remote": "origin"}));

    let title = fake::flag(args, "--title").unwrap_or_default();
    fake::append(
        &dir.join("published.jsonl"),
        &serde_json::json!({"token": token, "title": title}).to_string(),
    );
    let url = format!("https://example.invalid/changes/{token}");
    emit(
        dir,
        &token,
        "change-opened",
        serde_json::json!({"url": url, "id": token}),
    );
    println!(
        "{}",
        serde_json::json!({
            "url": url,
            "id": token,
            "outcome": "change-open",
        })
    );
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
    let follow = args.iter().any(|arg| arg == "--follow");
    let path = stream_of(dir, &token);
    let mut written = 0usize;
    loop {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
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
