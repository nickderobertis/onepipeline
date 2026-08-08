//! The `onevcs` seam.
//!
//! Repository identities, sessions, preserved work, and publication stay in that
//! library. A lifecycle node is this crate opening a session there, running its
//! dispatches inside the worktree that session hands back, and publishing
//! through it — never re-deriving a branch name, a merge policy, or a gate.
//!
//! The machine running the dispatch is the one that opens the session, which is
//! what [`WorkspaceSpec::VcsSession`](crate::executor::WorkspaceSpec::VcsSession)
//! means: the clone, worktree, and branch are cut where the work happens.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use onevcs::{MergePolicy, SessionRequest};
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::event::Envelope;

/// The environment variable naming the `onevcs` executable.
pub const BINARY_ENV: &str = "ONEPIPELINE_ONEVCS_BIN";

/// The executable's name when the environment names none.
pub const DEFAULT_BINARY: &str = "onevcs";

/// The executable this process invokes.
pub fn binary() -> String {
    std::env::var(BINARY_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BINARY.to_string())
}

fn sibling(message: impl Into<String>) -> Error {
    Error::Sibling {
        tool: "onevcs",
        message: message.into(),
    }
}

/// The session `onevcs session open` handed back.
///
/// `onevcs::Session` is `Serialize` only, so what the CLI prints is read back
/// into this mirror rather than into the sibling's own type. It carries exactly
/// the four fields that type does, and no field this crate invented.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OpenSession {
    /// The handle the session is published and closed by.
    pub token: String,
    /// The worktree the change is made in.
    pub worktree: PathBuf,
    /// The branch the worktree has checked out.
    pub branch: String,
    /// The base that branch was cut from.
    pub base: String,
}

/// What a publication produced.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Published {
    /// Where a human reads the change, when one was opened.
    #[serde(default)]
    pub url: Option<String>,
    /// The host's identifier for it, when one was opened.
    #[serde(default)]
    pub id: Option<String>,
    /// How it landed.
    #[serde(default)]
    pub outcome: Option<String>,
}

fn run_json<T: serde::de::DeserializeOwned>(command: &mut Command, what: &str) -> Result<T> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| sibling(format!("cannot start `{} {what}`: {e}", binary())))?;
    if !output.status.success() {
        return Err(sibling(format!(
            "{what} exited {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .map_err(|e| sibling(format!("{what} printed something unreadable: {e}")))
}

/// Open a session over a per-run clone and worktree.
pub fn session_open(request: &SessionRequest) -> Result<OpenSession> {
    let mut command = Command::new(binary());
    command.arg("session").arg("open").arg(&request.repo);
    if let Some(branch) = &request.branch {
        command.arg("--branch").arg(branch);
    }
    if let Some(base) = &request.base {
        command.arg("--base").arg(base);
    }
    if let Some(checkout) = &request.execution_checkout {
        command.arg("--execution-checkout").arg(checkout);
    }
    run_json(&mut command, "session open")
}

/// Verify a session's work and publish it under its policy.
pub fn publish(token: &str, policy: Option<MergePolicy>, title: Option<&str>) -> Result<Published> {
    let mut command = Command::new(binary());
    command.arg("publish").arg(token);
    if let Some(policy) = policy {
        command.arg("--policy").arg(policy_arg(policy));
    }
    if let Some(title) = title {
        command.arg("--title").arg(title);
    }
    run_json(&mut command, "publish")
}

/// How a merge policy is spelled on the command line.
pub fn policy_arg(policy: MergePolicy) -> &'static str {
    match policy {
        MergePolicy::LocalDirect => "local-direct",
        MergePolicy::ChangeOpen => "change-open",
        MergePolicy::ChangeAuto => "change-auto",
        MergePolicy::ChangeDirect => "change-direct",
    }
}

/// Release a session's worktree and its occupancy lease.
///
/// Closing is best-effort on the failure path: a node that already failed must
/// not be reported as a different failure because its cleanup also failed.
pub fn session_close(token: &str) -> Result<()> {
    let output = Command::new(binary())
        .arg("session")
        .arg("close")
        .arg(token)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| sibling(format!("cannot start `{} session close`: {e}", binary())))?;
    if output.status.success() {
        return Ok(());
    }
    Err(sibling(format!(
        "session close {token} exited {}: {}",
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// A session's own event stream, for relaying into the merged one.
pub fn events(token: &str) -> Vec<Envelope> {
    let Ok(output) = Command::new(binary())
        .arg("events")
        .arg(token)
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Envelope>(line).ok())
        .collect()
}

/// The session a lifecycle node asks for.
pub fn request_for(node: &crate::plan::Node) -> Option<SessionRequest> {
    Some(SessionRequest {
        repo: node.repo.clone()?,
        // A `resume` names the branch its continuation lives on, and the
        // reconciler has already pinned `branch` to it, so there is one answer
        // here rather than two.
        branch: node.branch.clone(),
        base: node.base_branch.clone(),
        execution_checkout: node.execution_checkout.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Node;

    #[test]
    fn every_merge_policy_has_one_spelling_on_the_command_line() {
        assert_eq!(policy_arg(MergePolicy::LocalDirect), "local-direct");
        assert_eq!(policy_arg(MergePolicy::ChangeOpen), "change-open");
        assert_eq!(policy_arg(MergePolicy::ChangeAuto), "change-auto");
        assert_eq!(policy_arg(MergePolicy::ChangeDirect), "change-direct");
    }

    #[test]
    fn a_lifecycle_node_asks_for_the_session_its_fields_describe() {
        let node = Node {
            id: "service".into(),
            repo: Some("owner/repo".into()),
            branch: Some("feature".into()),
            base_branch: Some("main".into()),
            execution_checkout: Some("primary".into()),
            persona: Some("engineer".into()),
            task: Some("## What\nship".into()),
            ..Node::default()
        };
        let request = request_for(&node).expect("a lifecycle node asks for a session");
        assert_eq!(request.repo, "owner/repo");
        assert_eq!(request.branch.as_deref(), Some("feature"));
        assert_eq!(request.base.as_deref(), Some("main"));
        assert_eq!(request.execution_checkout.as_deref(), Some("primary"));
    }

    #[test]
    fn a_direct_agent_node_asks_for_no_session() {
        let node = Node {
            id: "build".into(),
            persona: Some("engineer".into()),
            task: Some("## What\ndo it".into()),
            ..Node::default()
        };
        assert!(request_for(&node).is_none());
    }

    #[test]
    fn the_binary_comes_from_the_environment_or_falls_back() {
        assert_eq!(
            std::env::var(BINARY_ENV)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| DEFAULT_BINARY.to_string()),
            binary()
        );
    }
}
