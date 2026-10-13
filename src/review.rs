//! Explicit review discovery. Review state belongs to the host and rework provenance,
//! never to a second collection on the plan store.

use crate::{
    channel::{ChannelState, Surface},
    error::{Error, Result},
    journal::{self, Journal, PipelineKind},
    ledger::RunPaths,
    projection::RunState,
};
use onevcs::{CommentKind, ReviewComment};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

// llmlint: ignore-block[invalid_states_unrepresentable] These serialized view DTOs preserve entry 122’s agreed string-valued identities, URLs and timestamps. Node identities come from the validated graph; host values and marked replies come from onevcs review_comments, the sibling-owned trust boundary. The engine does not execute these output-only values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum State {
    Pending,
    BeingReworked { rework: String },
    Answered { reply_url: String },
    Declined { reply_url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Comment {
    pub(crate) id: onevcs::CommentId,
    pub(crate) author: String,
    pub(crate) url: String,
    pub(crate) updated_at: String,
    #[serde(flatten)]
    pub(crate) state: State,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

pub(crate) fn key(root: &str, comment: &ReviewComment) -> String {
    let bytes = Sha256::digest(format!("{root}:{}:{}", comment.id.0, comment.updated_at));
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn root<'a>(state: &'a RunState, node: &'a str) -> &'a str {
    let mut current = node;
    let mut seen = BTreeSet::new();
    while seen.insert(current) {
        match state
            .superseded
            .iter()
            .find(|(_, replacement)| replacement.as_str() == current)
        {
            Some((parent, _)) => current = parent,
            None => break,
        }
    }
    current
}

fn derive(
    state: &RunState,
    node: &str,
    comment: &ReviewComment,
    comments: &[ReviewComment],
) -> State {
    let lineage = root(state, node);
    let current = key(lineage, comment);
    if let Some(reply) = comments.iter().rev().find(|reply| {
        reply.ours
            && reply
                .reply_marker
                .as_ref()
                .is_some_and(|marker| marker.in_reply_to == comment.id && marker.key == current)
    }) {
        return if reply
            .reply_marker
            .as_ref()
            .and_then(|marker| marker.label.as_deref())
            == Some("declined")
        {
            State::Declined {
                reply_url: reply.url.clone(),
            }
        } else {
            State::Answered {
                reply_url: reply.url.clone(),
            }
        };
    }
    // An older reply answered the old rework. An edit is new feedback even
    // while that rework's provenance remains on the lineage's store item.
    if comments.iter().any(|reply| {
        reply.ours
            && reply
                .reply_marker
                .as_ref()
                .is_some_and(|marker| marker.in_reply_to == comment.id)
    }) {
        return State::Pending;
    }
    if let Some(provenance) = state
        .continued_reworks
        .get(lineage)
        .filter(|record| record.comments.iter().any(|named| named.id == comment.id.0))
    {
        return State::BeingReworked {
            rework: provenance.replacement.clone(),
        };
    }
    if let Some(rework) = state.reworks.iter().find_map(|(parent, record)| {
        (root(state, parent) == lineage
            && record.comments.iter().any(|named| named.id == comment.id.0))
        .then_some(record.replacement.clone())
    }) {
        return State::BeingReworked { rework };
    }
    State::Pending
}

fn drafts(state: &RunState) -> impl Iterator<Item = (&str, String)> {
    state.graph.iter().filter_map(|node| {
        if state.superseded.contains_key(&node.id)
            || state.landings.get(&node.id) != Some(&crate::graph::Landing::Unlanded)
        {
            return None;
        }
        state
            .known_change_url(&node.id)
            .map(|url| (node.id.as_str(), url))
    })
}

pub(crate) fn read(state: &RunState) -> Result<BTreeMap<String, Vec<Comment>>> {
    let mut found = BTreeMap::new();
    for (node, url) in drafts(state) {
        let read = crate::vcs::review_comments(&url)?;
        let comments = read
            .comments
            .iter()
            .filter(|comment| !comment.ours)
            .map(|comment| Comment {
                id: comment.id.clone(),
                author: comment.author.clone(),
                url: comment.url.clone(),
                updated_at: comment.updated_at.clone(),
                state: derive(state, node, comment, &read.comments),
            })
            .collect();
        found.insert(node.to_owned(), comments);
    }
    Ok(found)
}

fn metadata(comment: &ReviewComment) -> crate::payload::ReviewCommentMetadata {
    let (path, line, thread) = match &comment.kind {
        CommentKind::ReviewThread {
            path, line, thread, ..
        } => (Some(path.clone()), *line, Some(thread.clone())),
        _ => (None, None, None),
    };
    crate::payload::ReviewCommentMetadata {
        id: comment.id.clone(),
        kind: comment.kind.clone(),
        author: comment.author.clone(),
        url: comment.url.clone(),
        updated_at: comment.updated_at.clone(),
        path,
        line,
        thread,
        in_reply_to: comment.in_reply_to.clone(),
    }
}

fn location(comment: &ReviewComment) -> String {
    match &comment.kind {
        CommentKind::ReviewThread { path, line, .. } => {
            line.map_or_else(|| path.clone(), |line| format!("{path}:{line}"))
        }
        CommentKind::Review { .. } => "review summary".to_owned(),
        CommentKind::Conversation => "conversation comment".to_owned(),
        _ => "review comment".to_owned(),
    }
}

pub(crate) fn pass(paths: &RunPaths, state: &RunState, journal: &mut Journal) -> Result<()> {
    let channel = ChannelState::new(paths);
    // Include the queue log: a crash after push and before the journal append must
    // not send the same revision a second time.
    let events = journal::read(&paths.journal());
    let mut surfaced: BTreeSet<String> = events
        .iter()
        .filter(|event| {
            event.source == crate::event::Source::Pipeline
                && event.kind.as_str() == "planner-surface-queued"
                && event.payload.get("kind").and_then(Value::as_str) == Some("review-comment")
        })
        .filter_map(|event| {
            event
                .payload
                .get("correlation")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    for surface in crate::ledger::read_lines(&paths.channel("surfaces.jsonl"))
        .iter()
        .filter_map(|line| serde_json::from_str::<Surface>(line).ok())
        .filter(|surface| surface.kind == "review-comment")
    {
        if let Some(correlation) = surface.correlation.as_ref().map(ToString::to_string) {
            if surfaced.insert(correlation) {
                record_surface(paths, journal, &surface)?;
            }
        }
    }
    let mut failures = Vec::new();
    for (node, url) in drafts(state) {
        let read = match crate::vcs::review_comments(&url) {
            Ok(read) => read,
            Err(error) => {
                failures.push(format!("{node} ({url}): {error}"));
                continue;
            }
        };
        let artifact = journal.artifact("review-comment-bodies", &read.comments)?;
        journal.emit_with_artifacts(
            PipelineKind::ReviewCommentsRead,
            journal::labels(&paths.run, Some(node)),
            journal::payload(&[
                ("node", json!(node)),
                ("change_url", json!(read.change_url)),
                ("marker", json!(read.marker)),
                ("unchanged", json!(read.unchanged)),
                ("cost", json!(read.cost)),
                (
                    "comments",
                    json!(read.comments.iter().map(metadata).collect::<Vec<_>>()),
                ),
            ]),
            vec![artifact],
        )?;
        failpoint("after-read")?;
        for comment in read.comments.iter().filter(|comment| !comment.ours) {
            if derive(state, node, comment, &read.comments) != State::Pending {
                continue;
            }
            let correlation = format!("review-comment:{}:{}", comment.id.0, comment.updated_at);
            if surfaced.contains(&correlation) {
                continue;
            }
            let correlation = correlation
                .parse()
                .map_err(|error| Error::Invalid(format!("review correlation: {error}")))?;
            let surface = channel.push(Surface {
                id: 0, kind: "review-comment".to_owned(), message: format!("Node {node}, change request {url}: {} at {}\n{}\n\n{}\n\nAnswer with rework on {node}, naming {} in comments, or decline this comment.", comment.author, location(comment), comment.url, comment.body, comment.id.0), source: crate::channel::source::RECONCILER.to_owned(), blocking: false, queued_at: crate::sys::now_millis(), workstream: Some(node.to_owned()), abandoned: false, asker: None, correlation: Some(correlation),
            })?;
            failpoint("after-surface")?;
            record_surface(paths, journal, &surface)?;
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::Sibling {
            tool: "onevcs",
            message: failures.join("\n"),
        })
    }
}

pub(crate) fn lines(comments: &BTreeMap<String, Vec<Comment>>) -> String {
    let mut text = String::new();
    for (node, comments) in comments {
        for comment in comments {
            let state = match &comment.state {
                State::Pending => "pending".to_owned(),
                State::BeingReworked { rework } => format!("being reworked by {rework}"),
                State::Answered { reply_url } => format!("answered: {reply_url}"),
                State::Declined { reply_url } => format!("declined: {reply_url}"),
            };
            text.push_str(&format!(
                "  {node} review {}: {state} ({})\n",
                comment.id.0, comment.url
            ));
        }
    }
    text
}

pub(crate) fn requested(
    paths: &RunPaths,
    session: &str,
    timeout: std::time::Duration,
) -> Result<crate::verbs::ReviewReceipt> {
    let view = crate::views::RunView::open(paths)?;
    let driven = matches!(
        view.liveness(),
        crate::views::DriverLiveness::Driving | crate::views::DriverLiveness::Parked
    );
    if !driven {
        if !view.launch.owned_by(session) {
            return Err(Error::NotOwned {
                run: paths.run.clone(),
                owner: view.launch.owner_label(session),
            });
        }
        let _lock = crate::ledger::OwnershipLock::acquire(paths, "review")?;
        let state = crate::checkpoint::fold_and_checkpoint(paths);
        return match pass(paths, &state, &mut Journal::open(paths)) {
            Ok(()) => Ok(crate::verbs::ReviewReceipt::Performed { error: None }),
            Err(Error::Sibling { message, .. }) => Ok(crate::verbs::ReviewReceipt::Performed {
                error: Some(message),
            }),
            Err(error) => Err(error),
        };
    }
    failpoint("before-submit")?;
    let channel = ChannelState::new(paths);
    let id = match crate::engine::accept_as(
        paths,
        &channel,
        crate::channel::Author::planner(),
        &[crate::channel::Command::Review {}],
        "review",
    )? {
        crate::engine::Accepted::Queued(id) => id,
        crate::engine::Accepted::NothingIsDriving(_lock) => {
            // The driver may have ended between the liveness read and submission.
            if !view.launch.owned_by(session) {
                return Err(Error::NotOwned {
                    run: paths.run.clone(),
                    owner: view.launch.owner_label(session),
                });
            }
            let state = crate::checkpoint::fold_and_checkpoint(paths);
            return match pass(paths, &state, &mut Journal::open(paths)) {
                Ok(()) => Ok(crate::verbs::ReviewReceipt::Performed { error: None }),
                Err(Error::Sibling { message, .. }) => Ok(crate::verbs::ReviewReceipt::Performed {
                    error: Some(message),
                }),
                Err(error) => Err(error),
            };
        }
    };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(outcome) = channel.outcome_of(id) {
            return Ok(crate::verbs::ReviewReceipt::Performed {
                error: (!outcome.applied)
                    .then(|| outcome.reason.unwrap_or_else(|| "review failed".to_owned())),
            });
        }
        if std::time::Instant::now() >= deadline {
            return Ok(crate::verbs::ReviewReceipt::Requested { request: id });
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Test-only crash seam compiled into the debug binary driven by the journeys.
pub(crate) fn failpoint(stage: &str) -> Result<()> {
    #[cfg(debug_assertions)]
    if let Some(directory) = std::env::var_os("ONEPIPELINE_REVIEW_FAILPOINT") {
        let directory = std::path::PathBuf::from(directory);
        if directory.join(stage).is_file() {
            let reached = directory.join(format!("{stage}.reached"));
            std::fs::write(&reached, "reached").map_err(|source| Error::Ledger {
                path: reached,
                source,
            })?;
            while !directory.join(format!("{stage}.release")).is_file() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = stage;
    Ok(())
}

fn record_surface(paths: &RunPaths, journal: &mut Journal, surface: &Surface) -> Result<()> {
    journal.emit(
        PipelineKind::PlannerSurfaceQueued,
        journal::labels(&paths.run, surface.workstream.as_deref()),
        journal::payload(&[
            ("surface", json!(surface.id)),
            ("kind", json!(surface.kind)),
            ("message", json!(journal::bounded_text(&surface.message))),
            ("source", json!(surface.source)),
            ("blocking", json!(false)),
            ("correlation", json!(surface.correlation)),
        ]),
    )
}

// llmlint: ignore-block[invalid_states_unrepresentable] Entry 122 fixes this output-only JSON document’s run and node ids as strings. They borrow the recorded RunPaths and validated graph identities; these DTOs never accept or execute external input. Changing their serialized shape would change that agreed view contract.
/// The first JSON results view; comment lists are absent when empty.
#[derive(Serialize)]
pub(crate) struct ResultsDocument<'a> {
    pub(crate) schema_version: u32,
    pub(crate) run: &'a str,
    pub(crate) nodes: Vec<ResultNode<'a>>,
}

#[derive(Serialize)]
pub(crate) struct ResultNode<'a> {
    pub(crate) id: &'a str,
    pub(crate) status: crate::graph::NodeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) outcome: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) comments: Vec<Comment>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]
