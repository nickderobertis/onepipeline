//! A `gh` executable, scripted from a directory.
//!
//! **Not a double for a sibling.** `onepipeline` calls `onevcs` as a library, so
//! nothing in this repository stands in for it any more; what stands in here is
//! *GitHub*, at `onevcs`'s own documented `ONEVCS_GH` override — the seam that
//! library declares precisely so a journey can decide what the host does without
//! a network or a credential. Everything on the repository side stays real: a
//! journey that reaches this has already cloned, committed, gated, and pushed
//! with real git against a real origin on disk.
//!
//! It answers exactly the `gh` invocations `onevcs::GitHub` makes — the
//! authenticated user, opening a change request, listing them, viewing one,
//! merging one, and a job log — and refuses anything else, so a call this
//! library learns to make is a refusal here rather than a silent success.
//!
//! Two endings, because they are the two a host decides: a change request the
//! host **merged**, when `gh.merged` is scripted, and one it is holding
//! otherwise. Which of those a publication settles on is the repository's
//! policy, not this program's.

use onepipeline_testfakes as fake;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The login this host reports as the caller.
///
/// `onevcs` records it on the change request it opens, so a journey asserting
/// who opened one has a name to assert against.
const WHO: &str = "onepipeline-e2e";

/// The commit a change request's checks are reported against.
///
/// A real object id shape — forty hex characters — because it travels into
/// `onevcs::Sha` and out onto the stream a journey reads.
const HEAD_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

/// The commit a merge lands at, when this host merges one.
const MERGE_SHA: &str = "fedcba9876543210fedcba9876543210fedcba98";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "gh", &args);

    match (
        args.first().map(String::as_str),
        args.get(1).map(String::as_str),
    ) {
        (Some("api"), Some("user")) => {
            println!("{WHO}");
            ExitCode::SUCCESS
        }
        (Some("pr"), Some("create")) => create(&args, &dir),
        (Some("pr"), Some("list")) => list(&dir),
        (Some("pr"), Some("view")) => view(&args, &dir),
        (Some("pr"), Some("merge")) => merge(&args, &dir),
        (Some("run"), Some("view")) => {
            println!("the host's job log");
            ExitCode::SUCCESS
        }
        (Some(one), Some(two)) => fake::refuse(&format!("unknown gh command '{one} {two}'")),
        (Some(one), None) => fake::refuse(&format!("unknown gh command '{one}'")),
        (None, _) => fake::refuse("gh takes a command"),
    }
}

/// Where this host's state for one change request lives.
fn state_of(dir: &Path, id: &str) -> PathBuf {
    dir.join("gh").join(fake::segment(id))
}

/// The number this host gives the next change request it opens.
///
/// Counted from what it has already opened, so a journey that opens two reads
/// two different change requests rather than one addressed twice.
fn next_number(dir: &Path) -> u64 {
    let opened = dir.join("gh").join("opened.jsonl");
    std::fs::read_to_string(&opened)
        .map(|text| text.lines().filter(|line| !line.trim().is_empty()).count() as u64)
        .unwrap_or(0)
        + 1
}

/// `gh pr create --repo R --head H --base B --title T --body B`
///
/// Prints the change request's URL, which is the only thing `onevcs` reads back:
/// it takes the host's number out of that URL's last segment, so a URL that does
/// not end in one would be refused there rather than here.
fn create(args: &[String], dir: &Path) -> ExitCode {
    let Some(repo) = fake::flag(args, "--repo") else {
        return fake::refuse("gh pr create requires --repo");
    };
    let Some(head) = fake::flag(args, "--head") else {
        return fake::refuse("gh pr create requires --head");
    };
    let Some(base) = fake::flag(args, "--base") else {
        return fake::refuse("gh pr create requires --base");
    };
    let Some(title) = fake::flag(args, "--title") else {
        return fake::refuse("gh pr create requires --title");
    };
    let number = next_number(dir);
    fake::append(
        &dir.join("gh").join("opened.jsonl"),
        &serde_json::json!({"repo": repo, "head": head, "base": base, "title": title, "number": number})
            .to_string(),
    );
    fake::append(&state_of(dir, &number.to_string()), "open");
    println!("https://github.com/{repo}/pull/{number}");
    ExitCode::SUCCESS
}

/// `gh pr list --repo R --head H --base B --state open --json …`
///
/// Always empty: a journey here publishes a branch once, so every publication
/// opens its own change request rather than adopting one. A host that reported
/// an existing change would make "was one opened?" unanswerable.
fn list(_dir: &Path) -> ExitCode {
    println!("[]");
    ExitCode::SUCCESS
}

/// `gh pr view ID --repo R --json …`
fn view(args: &[String], dir: &Path) -> ExitCode {
    let id = match fake::required(args, 2, "ID") {
        Ok(id) => id,
        Err(refusal) => return refusal,
    };
    let state = std::fs::read_to_string(state_of(dir, &id)).unwrap_or_default();
    if state.trim().is_empty() {
        eprintln!("no pull request found for {id}");
        return ExitCode::from(1);
    }
    let merged = state.contains("merged");
    println!(
        "{}",
        serde_json::json!({
            "number": id.parse::<u64>().unwrap_or_default(),
            "state": if merged { "MERGED" } else { "OPEN" },
            "mergeStateStatus": "CLEAN",
            "headRefOid": HEAD_SHA,
            "mergeCommit": merged.then(|| serde_json::json!({"oid": MERGE_SHA})),
            // No checks reported. The journeys here gate with a command, which
            // is the repository's own bar; a host rollup would be a second one
            // this program decided.
            "statusCheckRollup": [],
        })
    );
    ExitCode::SUCCESS
}

/// `gh pr merge ID --repo R --squash [--auto]`
///
/// Whether the merge *lands* is the scripted part. Without `gh.merged` the host
/// has accepted the request and not landed it, which is what a queued merge is —
/// and `onevcs` reads that back from the next `pr view`, not from this command,
/// so scripting it here rather than in the exit code is what makes the two agree.
fn merge(args: &[String], dir: &Path) -> ExitCode {
    let id = match fake::required(args, 2, "ID") {
        Ok(id) => id,
        Err(refusal) => return refusal,
    };
    let path = state_of(dir, &id);
    if !path.is_file() {
        eprintln!("no pull request found for {id}");
        return ExitCode::from(1);
    }
    if dir.join("gh.merged").exists() {
        fake::append(&path, "merged");
    }
    ExitCode::SUCCESS
}
