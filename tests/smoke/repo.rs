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

pub fn gh_try(args: &[&str]) -> Result<String, String> {
    let output = Command::new("gh")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| missing(&format!("`gh` could not be run: {error}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(format!(
        "gh {} exited {}: {}",
        args.join(" "),
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
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
/// Names no slug, so the same wording stays true after a rename — the rename
/// leaves the old name redirecting, and the probe in [`ensure_repo`] succeeds
/// through it — and is pasted by hand onto the existing repository. Under
/// GitHub's cap on a description, which `tests/smoke_repo.rs` spells and holds.
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
    if gh_try(&["repo", "view", slug, "--json", "name"]).is_ok() {
        return;
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
    // step then fails naming something other than the reason.
    for _ in 0..60 {
        if gh_try(&["api", &format!("repos/{slug}/commits/HEAD")]).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    panic!("the scratch repository {slug} was created but never got its first commit");
}
