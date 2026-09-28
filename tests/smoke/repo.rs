//! The scratch repository, and the `gh` calls that reach it.
//!
//! Its own file rather than a stretch of `main.rs` so that the one thing here an
//! offline journey can prove — the sentence `ensure_repo` writes as the
//! repository's description — is reachable through `#[path]` without the
//! credentialled lifecycle journey coming along: `tests/smoke_repo.rs` includes
//! this file and puts a `gh` stand-in on `PATH`, the way the note binary includes
//! `tests/e2e/harness.rs`. Nothing here is substituted in the smoke itself.

use std::process::Command;

/// Run `gh`, or say what is missing rather than pretending it is not needed.
pub fn gh(args: &[&str]) -> String {
    match gh_try(args) {
        Ok(out) => out,
        Err(refusal) => panic!("{refusal}"),
    }
}

pub fn gh_try(args: &[&str]) -> Result<String, Refusal> {
    let command = args.join(" ");
    let output = match Command::new("gh")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            return Err(Refusal {
                cause: Cause::Unrunnable,
                command,
                status: None,
                stderr: error.to_string(),
            })
        }
    };
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(Refusal {
        cause: classify(output.status.code(), &stderr),
        command,
        status: Some(output.status),
        stderr,
    })
}

/// What a failed `gh` call means, read from the only evidence `gh` gives: its
/// exit status and its stderr. Callers match on this rather than on prose, so
/// an outage is never read as an answer about the repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// `gh` itself could not be started.
    Unrunnable,
    /// GitHub answered that what was asked for does not exist.
    NotFound,
    /// GitHub answered that the repository exists but has no commits yet.
    Empty,
    /// GitHub's rate limit refused the call, primary or secondary.
    RateLimited,
    /// No credential, or one GitHub rejected. `gh` exits 4 for the first.
    Unauthenticated,
    /// The request never reached an answer from GitHub.
    Transport,
    /// Anything else, which is never taken to mean any of the above.
    Unrecognised,
}

/// Classify a failed `gh` call. Outages are checked before answers, so a
/// refusal that happens to mention a status code is never read as one.
pub fn classify(status: Option<i32>, stderr: &str) -> Cause {
    let said = stderr.to_ascii_lowercase();
    let says = |markers: &[&str]| markers.iter().any(|marker| said.contains(marker));
    if says(&["rate limit", "http 429"]) {
        Cause::RateLimited
    } else if status == Some(4)
        || says(&[
            "http 401",
            "bad credentials",
            "requires authentication",
            "gh auth login",
        ])
    {
        Cause::Unauthenticated
    } else if says(&[
        "dial tcp",
        "no such host",
        "connection refused",
        "connection reset",
        "i/o timeout",
        "tls handshake",
        "network is unreachable",
        "context deadline exceeded",
    ]) {
        Cause::Transport
    } else if says(&["could not resolve to a repository", "http 404"]) {
        Cause::NotFound
    } else if says(&["git repository is empty", "http 409"]) {
        Cause::Empty
    } else {
        Cause::Unrecognised
    }
}

/// A `gh` call that failed, carrying `gh`'s own diagnostic unchanged.
#[derive(Debug)]
pub struct Refusal {
    pub cause: Cause,
    command: String,
    /// `None` only for a `gh` that never started.
    status: Option<std::process::ExitStatus>,
    stderr: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reading = match self.cause {
            Cause::Unrunnable => {
                let what = format!("`gh` could not be run: {}", self.stderr);
                return f.write_str(&missing(&what));
            }
            Cause::NotFound => "GitHub answered that it does not exist",
            Cause::Empty => "GitHub answered that the repository has no commits yet",
            Cause::RateLimited => {
                "refused by GitHub's rate limit; wait for the allowance to reset and rerun"
            }
            Cause::Unauthenticated => {
                "refused for authentication; check `gh auth status` or GH_TOKEN"
            }
            Cause::Transport => {
                "a transport failure before GitHub answered; rerun once it is reachable"
            }
            Cause::Unrecognised => "an error this smoke does not recognise",
        };
        let status = self
            .status
            .map(|status| status.to_string())
            .unwrap_or_default();
        write!(
            f,
            "gh {} failed with {status} ({reading}): {}",
            self.command, self.stderr
        )
    }
}

/// What a run with no credential is told, which has to be enough to fix it.
pub fn missing(what: &str) -> String {
    format!(
        "the real-everything smoke needs the GitHub CLI and a credential for it, and {what}. \
         Install `gh` (https://cli.github.com), then either run `gh auth login` or set GH_TOKEN \
         to a token that can push to, open a pull request on, and merge in the scratch \
         repository. In CI it is the RELEASE_PLZ_TOKEN secret, passed as GH_TOKEN — the \
         repository's existing PAT, which the operator chose for this job deliberately rather \
         than provisioning a second one. This journey never skips and never falls back to a \
         fake: a smoke that can pass without talking to GitHub proves nothing."
    )
}

/// What a created repository says about itself on its GitHub page.
///
/// Names no slug, so the same wording stays true after a rename: the rename
/// leaves the old name redirecting, and the probe in [`ensure_repo`] succeeds
/// through it. Kept under GitHub's cap on a description by its wording;
/// `tests/smoke_repo.rs` proves what it says.
pub const DESCRIPTION: &str = "Intentional, and reused by every run of onepipeline's \
    real-everything smoke, which creates it when absent and never deletes it. Deleting or \
    renaming it is no cleanup (a deleted one is recreated, a renamed one keeps its old name \
    redirecting, and the smoke writes on), so read its owner, tests/smoke/main.rs in \
    nickderobertis/onepipeline, first.";

/// The scratch repository, created private if it is not there yet.
///
/// Created rather than required, so a first run on a fresh account works — and
/// **never deleted**, by this journey or any other: what it removes is the
/// branches and pull requests it made.
pub fn ensure_repo(slug: &str) {
    match gh_try(&["repo", "view", slug, "--json", "name"]) {
        Ok(_) => return,
        Err(refusal) if refusal.cause == Cause::NotFound => {}
        Err(refusal) => panic!(
            "could not tell whether the scratch repository {slug} exists, so it was not \
             created: {refusal}"
        ),
    }
    gh(&[
        "repo",
        "create",
        slug,
        "--private",
        "--add-readme",
        "--description",
        DESCRIPTION,
    ]);
    // `--add-readme` commits asynchronously often enough to matter: a clone that
    // wins the race gets a repository with no branch at all, and every later
    // step then fails naming something other than the reason. Only "not there
    // yet" is waited through; any other refusal is the answer.
    for _ in 0..60 {
        match gh_try(&["api", &format!("repos/{slug}/commits/HEAD")]) {
            Ok(_) => return,
            Err(refusal) if matches!(refusal.cause, Cause::NotFound | Cause::Empty) => {}
            Err(refusal) => panic!(
                "the scratch repository {slug} was created, but waiting for its first commit \
                 failed: {refusal}"
            ),
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    panic!("the scratch repository {slug} was created but never got its first commit");
}
