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
//! It answers exactly the `gh` invocations `onevcs::GitHub` makes and **refuses
//! everything else**, argument by argument: every flag that library passes is
//! required here, and an invocation carrying one it does not pass is refused. A
//! double is worth what it refuses — one that shrugged off an unknown flag would
//! let this stack reach the real `gh` with an argument it has never taken and
//! keep the suite green. `tests/smoke/` runs the same publication against the
//! real `gh`, which is what holds the shapes below honest.
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

/// What this host knows about one change request.
///
/// Two states and no third: a file holding anything else is a scenario nobody
/// wrote, and reading it leniently would be this program inventing a host
/// behaviour a journey did not ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Opened, and the host has not landed it.
    Open,
    /// The host landed it, at [`MERGE_SHA`].
    Merged,
}

impl Change {
    /// How the state is written down, and the only two spellings read back.
    fn as_str(self) -> &'static str {
        match self {
            Change::Open => "open",
            Change::Merged => "merged",
        }
    }

    fn parse(recorded: &str) -> Option<Self> {
        match recorded.trim() {
            "open" => Some(Change::Open),
            "merged" => Some(Change::Merged),
            _ => None,
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = fake::script_dir();
    fake::record(&dir, "gh", &args);

    match (
        args.first().map(String::as_str),
        args.get(1).map(String::as_str),
    ) {
        (Some("api"), Some("user")) => user(&args),
        (Some("pr"), Some("create")) => create(&args, &dir),
        (Some("pr"), Some("list")) => list(&args),
        (Some("pr"), Some("view")) => view(&args, &dir),
        (Some("pr"), Some("merge")) => merge(&args, &dir),
        (Some("run"), Some("view")) => log(&args),
        (Some(one), Some(two)) => fake::refuse(&format!("unknown gh command '{one} {two}'")),
        (Some(one), None) => fake::refuse(&format!("unknown gh command '{one}'")),
        (None, _) => fake::refuse("gh takes a command"),
    }
}

/// Check one invocation against the exact shape `onevcs` passes.
///
/// `positional` is what must appear before the flags, `flags` every option that
/// must be present with a value, and `bare` every option that must be present
/// without one. Anything left over is an argument this host has never taken, and
/// is refused rather than ignored.
fn shaped(
    args: &[String],
    what: &str,
    positional: usize,
    flags: &[&str],
    bare: &[&str],
) -> Result<(), ExitCode> {
    let mut seen = positional;
    for flag in flags {
        let Some(at) = args.iter().position(|arg| arg == flag) else {
            return Err(fake::refuse(&format!("gh {what} requires {flag}")));
        };
        if args.get(at + 1).is_none() {
            return Err(fake::refuse(&format!("gh {what} needs a value for {flag}")));
        }
        seen += 2;
    }
    for flag in bare {
        if !args.iter().any(|arg| arg == flag) {
            return Err(fake::refuse(&format!("gh {what} requires {flag}")));
        }
        seen += 1;
    }
    if args.len() != seen {
        return Err(fake::refuse(&format!(
            "gh {what} was given arguments it does not take: {args:?}"
        )));
    }
    Ok(())
}

/// `gh api user --jq .login`
fn user(args: &[String]) -> ExitCode {
    if let Err(refusal) = shaped(args, "api user", 2, &["--jq"], &[]) {
        return refusal;
    }
    println!("{WHO}");
    ExitCode::SUCCESS
}

/// `gh run view --repo R --log --job N`
fn log(args: &[String]) -> ExitCode {
    if let Err(refusal) = shaped(args, "run view", 2, &["--repo", "--job"], &["--log"]) {
        return refusal;
    }
    println!("the host's job log");
    ExitCode::SUCCESS
}

/// Where this host's state for one change request lives.
fn state_of(dir: &Path, id: &str) -> PathBuf {
    dir.join("gh").join(fake::segment(id))
}

/// What this host knows about a change request, or nothing if it opened none.
fn recorded(dir: &Path, id: &str) -> Option<Change> {
    Change::parse(&std::fs::read_to_string(state_of(dir, id)).ok()?)
}

/// Write down what this host now knows about a change request.
fn record(dir: &Path, id: &str, state: Change) {
    let path = state_of(dir, id);
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            fake::fail(&format!("cannot create {}: {error}", parent.display()));
        }
    }
    if let Err(error) = std::fs::write(&path, state.as_str()) {
        fake::fail(&format!("cannot write {}: {error}", path.display()));
    }
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
    if let Err(refusal) = shaped(
        args,
        "pr create",
        2,
        &["--repo", "--head", "--base", "--title", "--body"],
        &[],
    ) {
        return refusal;
    }
    let flag = |name: &str| fake::flag(args, name).unwrap_or_default();
    let number = next_number(dir);
    fake::append(
        &dir.join("gh").join("opened.jsonl"),
        &serde_json::json!({
            "repo": flag("--repo"),
            "head": flag("--head"),
            "base": flag("--base"),
            "title": flag("--title"),
            "number": number,
        })
        .to_string(),
    );
    record(dir, &number.to_string(), Change::Open);
    println!("https://github.com/{}/pull/{number}", flag("--repo"));
    ExitCode::SUCCESS
}

/// `gh pr list --repo R --head H --base B --state open --json …`
///
/// Always empty: a journey here publishes a branch once, so every publication
/// opens its own change request rather than adopting one. A host that reported
/// an existing change would make "was one opened?" unanswerable.
fn list(args: &[String]) -> ExitCode {
    if let Err(refusal) = shaped(
        args,
        "pr list",
        2,
        &["--repo", "--head", "--base", "--state", "--json"],
        &[],
    ) {
        return refusal;
    }
    println!("[]");
    ExitCode::SUCCESS
}

/// `gh pr view ID --repo R --json …`
fn view(args: &[String], dir: &Path) -> ExitCode {
    if let Err(refusal) = shaped(args, "pr view", 3, &["--repo", "--json"], &[]) {
        return refusal;
    }
    let id = match fake::required(args, 2, "ID") {
        Ok(id) => id,
        Err(refusal) => return refusal,
    };
    // A number, because that is what `onevcs` takes out of the URL this host
    // printed and hands back here. Anything else names no change request it
    // opened, and answering one would be inventing a host that numbers its
    // changes some other way.
    let Ok(number) = id.parse::<u64>() else {
        eprintln!("{id} is not a pull request number");
        return ExitCode::from(1);
    };
    let Some(state) = recorded(dir, &id) else {
        eprintln!("no pull request found for {id}");
        return ExitCode::from(1);
    };
    let merged = state == Change::Merged;
    println!(
        "{}",
        serde_json::json!({
            "number": number,
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
    // `--auto` is the difference between `change-auto` and `change-direct`, so
    // the shape is checked with and without it rather than by ignoring it.
    let auto = args.iter().any(|arg| arg == "--auto");
    let bare: &[&str] = if auto {
        &["--squash", "--auto"]
    } else {
        &["--squash"]
    };
    if let Err(refusal) = shaped(args, "pr merge", 3, &["--repo"], bare) {
        return refusal;
    }
    let id = match fake::required(args, 2, "ID") {
        Ok(id) => id,
        Err(refusal) => return refusal,
    };
    if recorded(dir, &id).is_none() {
        eprintln!("no pull request found for {id}");
        return ExitCode::from(1);
    }
    if dir.join("gh.merged").exists() {
        record(dir, &id, Change::Merged);
    }
    ExitCode::SUCCESS
}
