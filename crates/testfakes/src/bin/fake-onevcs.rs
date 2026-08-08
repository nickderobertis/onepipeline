//! A real `onevcs` executable, scripted from a directory.
//!
//! It speaks the sibling's command surface — `session open`, `publish`,
//! `session close`, `events` — hands back a real worktree directory, and records
//! every invocation. It stands in for `onevcs`, which is itself interface-only;
//! what a lifecycle test proves is this crate's half of the composition.

use onepipeline_testfakes as fake;
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
        (Some("session"), Some("close")) => ExitCode::SUCCESS,
        (Some("publish"), _) => publish(&args, &dir),
        (Some("events"), _) => events(&args, &dir),
        (Some("session"), Some(other)) => {
            fake::refuse(&format!("unknown onevcs session command '{other}'"))
        }
        (Some(other), _) => fake::refuse(&format!("unknown onevcs command '{other}'")),
        (None, _) => fake::refuse("onevcs takes a command"),
    }
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

/// `onevcs publish TOKEN [--policy P] [--title T]`
fn publish(args: &[String], dir: &std::path::Path) -> ExitCode {
    let token = match fake::required(args, 1, "TOKEN") {
        Ok(token) => token,
        Err(refusal) => return refusal,
    };
    if dir.join("publish.fail").exists() {
        eprintln!("the merge-path gate rejected the branch");
        return ExitCode::from(1);
    }
    let title = fake::flag(args, "--title").unwrap_or_default();
    fake::append(
        &dir.join("published.jsonl"),
        &serde_json::json!({"token": token, "title": title}).to_string(),
    );
    println!(
        "{}",
        serde_json::json!({
            "url": format!("https://example.invalid/changes/{token}"),
            "id": token,
            "outcome": "change-open",
        })
    );
    ExitCode::SUCCESS
}

/// `onevcs events TOKEN` — the session's own stream, for the merge.
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
    println!(
        "{}",
        serde_json::json!({
            "v": 1,
            "ts": fake::now(),
            "stream": format!("fake-onevcs-{token}"),
            "seq": 0,
            "source": "vcs",
            "kind": "verification-finished",
            "labels": {},
            "payload": {"verdict": "passed", "token": token},
        })
    );
    let _ = dir;
    ExitCode::SUCCESS
}
