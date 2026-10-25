//! The real-everything smoke's cleanup of what a journey opened on the scratch repository.
//!
//! Its own file, as `repo.rs` is, so the decision it makes — what a failed close or delete
//! means once GitHub is read back — is reachable through `#[path]` without the credentialled
//! journeys coming along: `tests/smoke_repo.rs` includes this file and drives
//! [`Created::clean`] against a `gh` stand-in on `PATH`. Nothing here is substituted in the
//! smoke itself.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::repo::{gh_read, gh_try, Cause};

/// What one invocation of the journey opened on the scratch repository: the pull requests,
/// by number, and the branches every host recorded publishing under the run's prefix.
#[derive(Default)]
pub(crate) struct Tracked {
    pub(crate) pulls: BTreeSet<u64>,
    pub(crate) branches: BTreeSet<String>,
}

/// What the journey created on the scratch repository, and its cleanup.
///
/// [`Created::clean`] is the journey's own last step: it closes every pull request the hosts
/// recorded opening and every open one whose title carries this run's stamp, deletes every
/// branch under the run's prefix, and then reads each one back from GitHub to show it closed
/// or gone — answering every failure it met rather than counting it as done. A close or delete
/// GitHub failed to answer is left to that read-back, which is what says whether it happened,
/// and every read asks again across such a failure before taking it as one. Its `Drop` runs
/// the same sweep, best-effort, for a journey that failed before that step: declared before
/// every world, it drops after them, once each has stopped the drivers it started, which
/// would otherwise go on publishing after it was cleaned. A failure there is printed, since
/// a panic while unwinding would abort the report of the one that got it there.
pub(crate) struct Created {
    pub(crate) slug: String,
    pub(crate) stamp: String,
    /// The prefixes every branch of the run is cut under — here the run's id and a `/`, once
    /// the first launch has minted it; on a board, each task's issue number and a `/`.
    pub(crate) prefixes: Vec<String>,
    pub(crate) cleaned: bool,
}

/// What a cleanup met: each step it took, and every failure.
#[derive(Default)]
pub(crate) struct Cleanup {
    pub(crate) observed: Vec<String>,
    pub(crate) failures: Vec<String>,
}

impl Cleanup {
    fn failed(&mut self, what: impl std::fmt::Display) {
        self.failures.push(what.to_string());
    }
}

impl Created {
    /// Close and delete everything `tracked` names and everything the stamp and the prefixes
    /// find, then read each back.
    pub(crate) fn clean(&mut self, tracked: &Tracked) -> Cleanup {
        self.cleaned = true;
        let mut cleanup = Cleanup::default();
        let mut pulls = tracked.pulls.clone();
        let stamped = self.stamped_open(&mut cleanup);
        let untracked: Vec<u64> = stamped.difference(&tracked.pulls).copied().collect();
        if !tracked.pulls.is_empty() && !untracked.is_empty() {
            cleanup.failed(format!(
                "open pull requests carrying the stamp that no host recorded opening: {untracked:?}"
            ));
        }
        pulls.extend(stamped);
        let mut branches = tracked.branches.clone();
        // Highest first, so a stacked pull request is closed before the one under it.
        for number in pulls.iter().rev() {
            let Some((state, head)) = self.pull(*number, &mut cleanup) else {
                continue;
            };
            branches.insert(head);
            if state == "OPEN" {
                match gh_try(&["pr", "close", &number.to_string(), "--repo", &self.slug]) {
                    Ok(_) => cleanup.observed.push(format!("closed #{number}")),
                    Err(refusal) if refusal.cause.transient() => cleanup.observed.push(format!(
                        "closing #{number}, left to the read-back: {refusal}"
                    )),
                    Err(refusal) => cleanup.failed(format!("closing #{number}: {refusal}")),
                }
            } else {
                cleanup
                    .observed
                    .push(format!("#{number} was already {state}"));
            }
        }
        branches.extend(self.remote_branches(&mut cleanup));
        for branch in &branches {
            if !self
                .prefixes
                .iter()
                .any(|prefix| branch.starts_with(prefix.as_str()))
            {
                cleanup.failed(format!(
                    "{branch} is outside the run's prefixes {:?}, so it is not deleted",
                    self.prefixes
                ));
                continue;
            }
            match self.branch_exists(branch, &mut cleanup) {
                Some(true) => match gh_try(&[
                    "api",
                    "-X",
                    "DELETE",
                    &format!("repos/{}/git/refs/heads/{branch}", self.slug),
                ]) {
                    Ok(_) => cleanup.observed.push(format!("deleted {branch}")),
                    Err(refusal) if refusal.cause.transient() => cleanup.observed.push(format!(
                        "deleting {branch}, left to the read-back: {refusal}"
                    )),
                    Err(refusal) => cleanup.failed(format!("deleting {branch}: {refusal}")),
                },
                Some(false) => cleanup.observed.push(format!("{branch} was already gone")),
                None => {}
            }
        }
        // Read back: every pull request closed, every branch gone, nothing left to find.
        let mut closed = Vec::new();
        for number in &pulls {
            match self.pull(*number, &mut cleanup) {
                Some((state, _)) if state == "CLOSED" => closed.push(*number),
                Some((state, _)) => {
                    cleanup.failed(format!("#{number} reads {state} after the cleanup"));
                }
                None => {}
            }
        }
        let mut gone = Vec::new();
        for branch in &branches {
            match self.branch_exists(branch, &mut cleanup) {
                Some(false) => gone.push(branch.as_str()),
                Some(true) => {
                    cleanup.failed(format!("{branch} is still on the remote after the cleanup"));
                }
                None => {}
            }
        }
        let left = self.stamped_open(&mut cleanup);
        if !left.is_empty() {
            cleanup.failed(format!("still open after the cleanup: {left:?}"));
        }
        let remaining = self.remote_branches(&mut cleanup);
        if !remaining.is_empty() {
            cleanup.failed(format!(
                "branches under the prefix after the cleanup: {remaining:?}"
            ));
        }
        cleanup.observed.push(format!(
            "read back from GitHub: closed {closed:?} of {pulls:?}; gone {gone:?} of {branches:?}"
        ));
        cleanup
    }

    /// The open pull requests on the scratch repository whose title carries the stamp.
    fn stamped_open(&self, cleanup: &mut Cleanup) -> BTreeSet<u64> {
        let listed = match gh_read(&[
            "pr",
            "list",
            "--repo",
            &self.slug,
            "--state",
            "open",
            "--limit",
            "200",
            "--json",
            "number,title",
        ]) {
            Ok(listed) => listed,
            Err(refusal) => {
                cleanup.failed(format!("listing open pull requests: {refusal}"));
                return BTreeSet::new();
            }
        };
        let listed: Vec<Value> = match serde_json::from_str(&listed) {
            Ok(listed) => listed,
            Err(why) => {
                cleanup.failed(format!("reading the open pull requests: {why}: {listed}"));
                return BTreeSet::new();
            }
        };
        // An entry naming no title or number is a listing that cannot say whether it
        // carries the stamp, and is a failure rather than a pull request that does not.
        let mut stamped = BTreeSet::new();
        for pull in &listed {
            match (pull["title"].as_str(), pull["number"].as_u64()) {
                (Some(title), Some(number)) => {
                    if title.contains(&self.stamp) {
                        stamped.insert(number);
                    }
                }
                _ => cleanup.failed(format!(
                    "an open pull request names no title or number: {pull}"
                )),
            }
        }
        stamped
    }

    /// Pull request `number`'s state and head branch, or the failure that kept them.
    fn pull(&self, number: u64, cleanup: &mut Cleanup) -> Option<(String, String)> {
        let read = gh_read(&[
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            &self.slug,
            "--json",
            "state,headRefName",
        ]);
        let read = match read {
            Ok(read) => read,
            Err(refusal) => {
                cleanup.failed(format!("reading #{number}: {refusal}"));
                return None;
            }
        };
        let pull: Value = match serde_json::from_str(&read) {
            Ok(pull) => pull,
            Err(why) => {
                cleanup.failed(format!("reading #{number}: {why}: {read}"));
                return None;
            }
        };
        match (pull["state"].as_str(), pull["headRefName"].as_str()) {
            (Some(state), Some(head)) => Some((state.to_owned(), head.to_owned())),
            _ => {
                cleanup.failed(format!("#{number} names no state or head: {pull}"));
                None
            }
        }
    }

    /// Whether `branch` is on the remote: `None` where GitHub's answer said neither.
    fn branch_exists(&self, branch: &str, cleanup: &mut Cleanup) -> Option<bool> {
        match gh_read(&[
            "api",
            &format!("repos/{}/git/ref/heads/{branch}", self.slug),
        ]) {
            Ok(_) => Some(true),
            Err(refusal) if refusal.cause == Cause::NotFound => Some(false),
            Err(refusal) => {
                cleanup.failed(format!("reading {branch}: {refusal}"));
                None
            }
        }
    }

    /// Every branch on the remote under one of the run's prefixes.
    fn remote_branches(&self, cleanup: &mut Cleanup) -> BTreeSet<String> {
        let mut branches = BTreeSet::new();
        for prefix in &self.prefixes {
            let listed = match gh_read(&[
                "api",
                &format!("repos/{}/git/matching-refs/heads/{prefix}", self.slug),
            ]) {
                Ok(listed) => listed,
                Err(refusal) => {
                    cleanup.failed(format!("listing branches under {prefix}: {refusal}"));
                    continue;
                }
            };
            let refs: Vec<Value> = match serde_json::from_str(&listed) {
                Ok(refs) => refs,
                Err(why) => {
                    cleanup.failed(format!("reading branches under {prefix}: {why}: {listed}"));
                    continue;
                }
            };
            // A reference naming no branch is one this cleanup cannot say is gone, so it
            // fails rather than reading as a branch that is not there.
            for reference in &refs {
                match reference["ref"]
                    .as_str()
                    .and_then(|name| name.strip_prefix("refs/heads/"))
                {
                    Some(branch) => {
                        if branch.starts_with(prefix.as_str()) {
                            branches.insert(branch.to_owned());
                        }
                    }
                    None => cleanup.failed(format!(
                        "a reference under {prefix} names no branch: {reference}"
                    )),
                }
            }
        }
        branches
    }
}

impl Drop for Created {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        let cleanup = self.clean(&Tracked::default());
        for step in &cleanup.observed {
            eprintln!("smoke: cleanup after a failed journey: {step}");
        }
        for failure in &cleanup.failures {
            eprintln!("smoke: cleanup after a failed journey FAILED: {failure}");
        }
    }
}
