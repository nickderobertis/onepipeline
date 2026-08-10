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

// llmlint: ignore-file[invalid_states_unrepresentable] `Published` carries a change
// request's URL, its host-assigned id, and how it landed as the strings the sibling
// recorded them as. Narrowing them here would mean this crate deciding a vocabulary the
// sibling owns — the exact re-declaration src/AGENTS.md forbids — and `onevcs` publishes
// no type for what a publication produced: `publish::Outcome` is behind a private module.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use onevcs::{MergePolicy, Session, SessionRequest};

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

/// What a publication produced.
///
/// Folded from the session's own event stream rather than read off the command's
/// stdout, because **`onevcs publish` prints one line of prose for a person and
/// no machine-readable record at all** — `merged at SHA`, `change request open at
/// URL`, `merge queued for URL`, `nothing to publish: …`. What it writes down
/// *is* structured: the change request it opened, the merge that landed, and the
/// queue it entered are envelopes on the stream this crate already relays. So the
/// outcome is read from those, and never from a sentence whose wording is the
/// sibling's to change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Published {
    /// Where a human reads the change, when one was opened.
    pub url: Option<String>,
    /// The host's identifier for it, when one was opened.
    pub id: Option<String>,
    /// How it landed.
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
pub fn session_open(request: &SessionRequest) -> Result<Session> {
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
///
/// The exit status is the whole verdict — `onevcs publish` says whether the
/// change landed with its code and describes it on stdout in prose — so success
/// is read from the status and the *shape* of what happened from the session's
/// own stream. Reading stdout as JSON is what this used to do, and against the
/// real sibling every publication then failed as unreadable: a change that had
/// already merged, reported as a publication failure.
pub fn publish(token: &str, policy: Option<MergePolicy>, title: Option<&str>) -> Result<Published> {
    let mut command = Command::new(binary());
    command.arg("publish").arg(token);
    if let Some(policy) = policy {
        command.arg("--policy").arg(policy_arg(policy));
    }
    if let Some(title) = title {
        command.arg("--title").arg(title);
    }
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| sibling(format!("cannot start `{} publish`: {e}", binary())))?;
    if !output.status.success() {
        return Err(sibling(format!(
            "publish exited {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(published_from(&events(token)))
}

/// What the session's stream says its publication produced.
///
/// The kinds are read back into [`onevcs::EventKind`] — the sibling's own enum,
/// which is what spells them on the wire — rather than compared against strings
/// this crate restated. A kind renamed there stops matching here at the type
/// level instead of silently never firing. Each carries the fields that command
/// records: `ChangeOpened` names the change request and its URL, `ChangeMerged`
/// and `MergeCompleted` say it reached its base, and `MergeQueued` carrying a
/// `url` is the host holding it. `MergeQueued` *without* one is the identity's
/// own lock queue, which every publication passes through and which says nothing
/// about the outcome — reading it as one would report every local merge as
/// queued.
fn published_from(events: &[Envelope]) -> Published {
    /// How far a publication got, so a later record never reads as less than an
    /// earlier one.
    fn rank(outcome: &str) -> u8 {
        match outcome {
            "merged" => 3,
            "queued" => 2,
            "change-open" => 1,
            _ => 0,
        }
    }
    let text = |envelope: &Envelope, key: &str| {
        envelope
            .payload
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    let mut published = Published::default();
    for envelope in events {
        // Through the sibling's own deserializer: a kind this build does not
        // know — a later `onevcs` emitting one it has learned — is passed over
        // rather than guessed at, which is what the merged stream already does
        // with it.
        let Ok(kind) = serde_json::from_value::<onevcs::EventKind>(serde_json::Value::String(
            envelope.kind.0.clone(),
        )) else {
            continue;
        };
        let reached = match kind {
            onevcs::EventKind::ChangeOpened => {
                published.url = text(envelope, "url").or(published.url.take());
                published.id = text(envelope, "id").or(published.id.take());
                "change-open"
            }
            onevcs::EventKind::ChangeMerged | onevcs::EventKind::MergeCompleted => {
                published.url = text(envelope, "url").or(published.url.take());
                "merged"
            }
            onevcs::EventKind::MergeQueued if text(envelope, "url").is_some() => {
                published.url = text(envelope, "url").or(published.url.take());
                "queued"
            }
            _ => continue,
        };
        if rank(reached) > rank(published.outcome.as_deref().unwrap_or_default()) {
            published.outcome = Some(reached.to_string());
        }
    }
    published
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

/// How long a follow may keep reading after its session was closed.
///
/// `onevcs events --follow` returns on a session it reads as closed, so this
/// only covers the case where the close itself failed and nothing will ever
/// mark it: a node that has already settled must not hang on its own cleanup.
const FOLLOW_GRACE: Duration = Duration::from_secs(5);

/// How often a finishing follow re-asks whether its reader has ended.
const FOLLOW_POLL: Duration = Duration::from_millis(20);

/// A session's own event stream, followed as `onevcs` writes it.
///
/// Read *once at settlement*, a lifecycle node's gate run, push, change
/// request, check polling, and merge are one opaque blocking call: every record
/// appears at once, when it is over — and that stretch is the longest
/// wall-clock segment the node has. `onevcs events TOKEN --follow` is the
/// sibling's own general answer to that, and this is it, used.
///
/// `None` when the follow could not be started at all. That is a publication
/// nobody is watching rather than a publication with no record, so it is said
/// out loud and the caller reads the stream once instead.
pub fn follow(token: &str, sink: Box<dyn Fn(Envelope) + Send>) -> Option<Follower> {
    let started = Command::new(binary())
        .arg("events")
        .arg(token)
        .arg("--follow")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Inherited rather than piped: nothing here would read a pipe, and a
        // full one stops the follow mid-publication. The sibling's own words
        // reach the driver's stderr, which is where its other refusals go.
        .stderr(Stdio::inherit())
        .spawn();
    let mut child = match started {
        Ok(child) => child,
        Err(error) => {
            eprintln!("onepipeline: cannot follow session {token}'s events: {error}");
            return None;
        }
    };
    let Some(stdout) = child.stdout.take() else {
        stop(&mut child);
        eprintln!("onepipeline: cannot read session {token}'s events as they are written");
        return None;
    };

    let relayed = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&relayed);
    let reader = std::thread::Builder::new()
        .name(format!("{}-events", binary()))
        .spawn(move || {
            let mut skipped = 0usize;
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::io::Result::ok)
            {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Envelope>(&line) {
                    Ok(envelope) => {
                        counted.fetch_add(1, Ordering::SeqCst);
                        sink(envelope);
                    }
                    Err(_) => skipped += 1,
                }
            }
            report_skipped("onevcs", skipped);
        });
    match reader {
        Ok(reader) => Some(Follower {
            child,
            reader: Some(reader),
            relayed,
        }),
        Err(error) => {
            stop(&mut child);
            eprintln!("onepipeline: cannot follow session {token}'s events: {error}");
            None
        }
    }
}

/// One session's stream, being followed.
///
/// Dropping one ends the follow. Not every caller reaches a settlement — a node
/// whose next step needs a person holds its session *open* for them, and returns
/// — and a follow left behind there is a process nothing would ever collect,
/// reading a stream nobody is waiting for.
#[derive(Debug)]
pub struct Follower {
    child: Child,
    /// Taken by [`finish`](Follower::finish), so a drop after one has nothing
    /// left to wait on.
    reader: Option<std::thread::JoinHandle<()>>,
    relayed: Arc<AtomicU64>,
}

impl Drop for Follower {
    fn drop(&mut self) {
        stop(&mut self.child);
        if let Some(reader) = self.reader.take() {
            // The pipe is closed by the kill above, so the reader is already on
            // its way out.
            let _ = reader.join();
        }
    }
}

impl Follower {
    /// Stop following, and report whether anything was relayed.
    ///
    /// Called *after* `session close`, which is what ends the follow: `onevcs
    /// events --follow` prints everything it has not printed yet and only then
    /// returns on a closed session, so waiting for it loses nothing.
    ///
    /// `false` is the one answer a caller has to act on: the follow neither
    /// ended cleanly nor relayed a record, so the session's evidence is still
    /// unread and reading it once leaves no gap rather than a duplicate. A
    /// follow that *did* end cleanly having read nothing is a session that
    /// recorded nothing, which is not the same thing.
    pub fn finish(mut self) -> bool {
        let deadline = Instant::now() + FOLLOW_GRACE;
        let ended = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Err(_) => break false,
                Ok(None) if Instant::now() >= deadline => {
                    stop(&mut self.child);
                    break false;
                }
                Ok(None) => std::thread::sleep(FOLLOW_POLL),
            }
        };
        // The reader ends on its own once the pipe closes, which the wait above
        // has already made true.
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        ended || self.relayed.load(Ordering::SeqCst) > 0
    }
}

/// End a follow this process started and will not read.
fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
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
pub fn session_opened_event(session: &Session, labels: &crate::event::Labels) -> Envelope {
    Envelope {
        v: crate::event::ENVELOPE_VERSION,
        ts: crate::sys::now_rfc3339(),
        stream: format!("onevcs-{}", session.token.0),
        seq: 0,
        source: crate::event::Source::Vcs,
        kind: crate::event::EventKind("session-opened".into()),
        labels: labels.clone(),
        payload: crate::journal::payload(&[
            ("token", serde_json::json!(session.token.0)),
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

    fn recorded(kind: &str, payload: serde_json::Value) -> Envelope {
        Envelope {
            v: crate::event::ENVELOPE_VERSION,
            ts: "2026-01-01T00:00:00.000Z".into(),
            stream: "s-1".into(),
            seq: 1,
            source: crate::event::Source::Vcs,
            kind: crate::event::EventKind(kind.into()),
            labels: crate::event::Labels::default(),
            payload: payload.as_object().cloned().unwrap_or_default(),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn a_change_request_the_session_recorded_is_where_a_human_reads_it() {
        let published = published_from(&[recorded(
            "change-opened",
            serde_json::json!({"url": "https://example.invalid/pull/7", "id": "7"}),
        )]);
        assert_eq!(
            published.url.as_deref(),
            Some("https://example.invalid/pull/7")
        );
        assert_eq!(published.id.as_deref(), Some("7"));
        assert_eq!(published.outcome.as_deref(), Some("change-open"));
    }

    #[test]
    fn a_change_that_reached_its_base_outranks_the_request_that_opened_it() {
        let published = published_from(&[
            recorded(
                "change-opened",
                serde_json::json!({"url": "https://example.invalid/pull/7", "id": "7"}),
            ),
            recorded(
                "merge-queued",
                serde_json::json!({"url": "https://example.invalid/pull/7"}),
            ),
            recorded(
                "change-merged",
                serde_json::json!({"url": "https://example.invalid/pull/7", "sha": "abc"}),
            ),
        ]);
        assert_eq!(published.outcome.as_deref(), Some("merged"));
        assert_eq!(published.id.as_deref(), Some("7"));
    }

    #[test]
    fn the_identitys_own_lock_queue_is_not_a_publication_the_host_is_holding() {
        // Every publication passes through it, and it carries no change request.
        // Read as an outcome, a local merge would report itself queued forever.
        let published = published_from(&[
            recorded("merge-queued", serde_json::json!({"identity": "repo"})),
            recorded(
                "merge-completed",
                serde_json::json!({"identity": "repo", "sha": "abc"}),
            ),
        ]);
        assert_eq!(published.outcome.as_deref(), Some("merged"));
        assert_eq!(published.url, None);
    }

    #[test]
    fn a_session_that_recorded_nothing_publishable_claims_no_outcome() {
        assert_eq!(published_from(&[]), Published::default());
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
