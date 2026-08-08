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

// llmlint: ignore-file[invalid_states_unrepresentable] `OpenSession` and `Published`
// mirror, field for field, what the `onevcs` CLI prints. A token, a branch name, a change
// id, and an outcome are strings *on that wire*, and narrowing them here would mean this
// crate deciding a vocabulary the sibling owns — the exact re-declaration src/AGENTS.md
// forbids. `onevcs::SessionToken` and `onevcs::MergeOutcome` are `Serialize`-only today,
// so they cannot be read back into; when they gain `Deserialize` these two mirrors go
// away entirely and the sibling's types are used directly.

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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
    let read = Command::new(binary())
        .arg("events")
        .arg(token)
        .stdin(Stdio::null())
        .output();
    let output = match read {
        Ok(output) if output.status.success() => output,
        // A session whose stream cannot be read leaves the node's own
        // settlement intact — the evidence is missing, not the result — but a
        // silent gap in the merged store is what makes a later reader think
        // nothing happened, so it is said out loud.
        Ok(output) => {
            eprintln!(
                "onepipeline: cannot read session {token}'s events: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return Vec::new();
        }
        Err(error) => {
            eprintln!("onepipeline: cannot read session {token}'s events: {error}");
            return Vec::new();
        }
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let envelopes: Vec<Envelope> = lines
        .iter()
        .filter_map(|line| serde_json::from_str::<Envelope>(line).ok())
        .collect();
    report_skipped("onevcs", lines.len() - envelopes.len());
    envelopes
}

/// Say when a sibling's stream carried lines this build could not read.
///
/// Skipping them is right — a sibling emitting a kind this build does not know
/// must not stop the ones it does — but skipping them *quietly* turns a schema
/// mismatch into a run that merely looks uneventful.
pub fn report_skipped(tool: &str, skipped: usize) {
    if skipped > 0 {
        eprintln!("onepipeline: skipped {skipped} {tool} line(s) this build cannot read");
    }
}

/// The envelope that records a session opening, for the merged stream.
///
/// It carries `Source::Vcs` because `onevcs` is what opened the session: the
/// merge is an interleaving of three streams, and a lifecycle node's branch
/// belongs to that one.
pub fn session_opened_event(session: &OpenSession, labels: &crate::event::Labels) -> Envelope {
    Envelope {
        v: crate::event::ENVELOPE_VERSION,
        ts: crate::sys::now_rfc3339(),
        stream: format!("onevcs-{}", session.token),
        seq: 0,
        source: crate::event::Source::Vcs,
        kind: crate::event::EventKind("session-opened".into()),
        labels: labels.clone(),
        payload: crate::journal::payload(&[
            ("token", serde_json::json!(session.token)),
            ("branch", serde_json::json!(session.branch)),
            ("base", serde_json::json!(session.base)),
            ("worktree", serde_json::json!(session.worktree)),
        ]),
        artifacts: Vec::new(),
    }
}

/// The envelope that records a publication, for the merged stream.
pub fn published_event(
    published: &Published,
    branch: &str,
    labels: &crate::event::Labels,
) -> Envelope {
    Envelope {
        v: crate::event::ENVELOPE_VERSION,
        ts: crate::sys::now_rfc3339(),
        stream: format!("onevcs-{branch}"),
        seq: 1,
        source: crate::event::Source::Vcs,
        kind: crate::event::EventKind("published".into()),
        labels: labels.clone(),
        payload: crate::journal::payload(&[
            ("branch", serde_json::json!(branch)),
            ("url", serde_json::json!(published.url)),
            ("id", serde_json::json!(published.id)),
            ("outcome", serde_json::json!(published.outcome)),
        ]),
        artifacts: Vec::new(),
    }
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
