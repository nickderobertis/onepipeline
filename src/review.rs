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
    BeingReworked {
        rework: String,
    },
    Answered {
        reply_url: String,
        /// The rework whose commits answered it, where the run's record names one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rework: Option<String>,
        /// The short sha of each commit its reply links.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        commits: Vec<String>,
    },
    Declined {
        reply_url: String,
    },
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

pub(crate) fn root<'a>(state: &'a RunState, node: &'a str) -> &'a str {
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

pub(crate) fn derive(
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
                rework: answered_by(state, node, &comment.id.0),
                commits: crate::reply::linked_commits(&reply.body)
                    .into_iter()
                    .map(|sha| sha.chars().take(SHORT_SHA).collect())
                    .collect(),
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
        .get(node)
        .filter(|record| record.comments.iter().any(|named| named.id == comment.id.0))
    {
        return State::BeingReworked {
            rework: provenance.replacement.clone(),
        };
    }
    if let Some(rework) = state.reworks.values().find_map(|record| {
        (record.replacement == node && record.comments.iter().any(|named| named.id == comment.id.0))
            .then_some(record.replacement.clone())
    }) {
        return State::BeingReworked { rework };
    }
    State::Pending
}

/// How many characters of a sha a view shows.
const SHORT_SHA: usize = 7;

/// The rework whose provenance names `comment` in `node`'s lineage: the continued
/// lineage's own, or the newest of the reworks this host applied to it.
fn answered_by(state: &RunState, node: &str, comment: &str) -> Option<String> {
    if let Some(continued) = state
        .continued_reworks
        .get(node)
        .filter(|record| record.comments.iter().any(|named| named.id == comment))
    {
        return Some(continued.replacement.clone());
    }
    let lineage = root(state, node);
    let mut found = None;
    let mut current = lineage;
    let mut seen = BTreeSet::new();
    while seen.insert(current) {
        let Some(record) = state.reworks.get(current) else {
            match state.superseded.get(current) {
                Some(next) => {
                    current = next;
                    continue;
                }
                None => break,
            }
        };
        if record.comments.iter().any(|named| named.id == comment) {
            found = Some(record.replacement.clone());
        }
        current = &record.replacement;
    }
    found
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

/// The pass `onepipeline review`: a readjustment first, whose one batched read per
/// repository and root carries every draft's review feedback, and then discovery from
/// those reads.
pub(crate) fn pass(
    paths: &RunPaths,
    projected: &mut crate::checkpoint::Projected,
    journal: &mut Journal,
) -> Result<()> {
    let detection = crate::readjust::run(
        paths,
        journal,
        projected,
        crate::payload::ReadjustTrigger::Review,
    )?;
    let state: &RunState = projected;
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
    // A repository whose drafts could not even be grouped for a read failed with no
    // draft's read to carry it; every other failure is said per draft below.
    let mut failures: Vec<String> = detection
        .failures
        .iter()
        .filter(|(repo, _)| {
            !detection.reviews.iter().any(|(node, _, read)| {
                read.is_err()
                    && state.graph.get(node).and_then(|node| node.repo.as_deref())
                        == Some(repo.as_str())
            })
        })
        .map(|(repo, detail)| format!("{repo}: {detail}"))
        .collect();
    let mut answerable = Vec::new();
    for (node, url, read) in detection.reviews {
        let node = node.as_str();
        let read = match read {
            Ok(read) => read,
            Err(error) => {
                let said = format!("{node} ({url}): {error}");
                if !failures.contains(&said) {
                    failures.push(said);
                }
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
        answerable.push((node.to_owned(), url, read));
    }
    // The last step: every reply a settled rework owes, drafted and posted out of the
    // reads just made, so the pass returns with nothing it could do left undone.
    answer_owed(paths, state, journal, &channel, &answerable, &mut failures)?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::Sibling {
            tool: "onevcs",
            message: failures.join("\n"),
        })
    }
}

/// Draft and post every reply the pass's reads show owed: one drafting per rework
/// replacement, each reply one POST. A drafting that posts nothing is journalled
/// `review-reply-not-drafted` and surfaced `review-reply-pending`, and is drafted
/// again by the next pass; a post the host refused is the pass's failure.
fn answer_owed(
    paths: &RunPaths,
    state: &RunState,
    journal: &mut Journal,
    channel: &ChannelState,
    reads: &[(String, String, onevcs::ReviewRead)],
    failures: &mut Vec<String>,
) -> Result<()> {
    let launch: crate::ledger::LaunchRecord = crate::ledger::read_json(&paths.launch())?;
    let scope = crate::boundary::of_run(&crate::boundary::plan_now(state)).term_scope();
    for (node, url, read) in reads {
        let owed = match crate::reply::owed(paths, state, node, url, read) {
            Ok(Some(owed)) => owed,
            Ok(None) => continue,
            Err(why) => {
                let named = state
                    .graph
                    .get(node)
                    .map(|_| crate::reply::provenance(state, node))
                    .and_then(|provenance| provenance.map(|rework| rework.comments))
                    .unwrap_or_default();
                not_drafted(paths, journal, channel, node, named, &why)?;
                continue;
            }
        };
        let named = owed.drafting.comment_ids();
        let Some(graph) = launch.review_reply_graph() else {
            not_drafted(
                paths,
                journal,
                channel,
                node,
                named,
                &crate::reply::NotDrafted::unconfigured(),
            )?;
            continue;
        };
        let drafted =
            match crate::reply::draft(paths, graph, node, &owed.drafting, &mut |envelope| {
                if let Err(error) = journal.relay(&envelope) {
                    eprintln!("onepipeline: a reply drafting's event was not journalled: {error}");
                }
            }) {
                Ok(drafted) => drafted,
                Err(why) => {
                    not_drafted(paths, journal, channel, node, named, &why)?;
                    continue;
                }
            };
        for reply in drafted {
            let Some(comment) = owed
                .drafting
                .comments
                .iter()
                .find(|comment| comment.id.0 == reply.comment)
            else {
                continue;
            };
            let key = crate::reply::key_of(&owed.root, comment);
            let posted = match crate::reply::post(
                url,
                &reply.comment,
                &reply.body,
                &key,
                reply.disposition,
                scope.clone(),
            ) {
                Ok(posted) => posted,
                Err(error) => {
                    failures.push(format!(
                        "{node} ({url}): the reply to {} was not posted: {error}",
                        reply.comment
                    ));
                    break;
                }
            };
            failpoint("after-reply-post")?;
            let linked = crate::reply::linked_commits(&reply.body);
            journal.emit(
                PipelineKind::ReviewReplyPosted,
                journal::labels(&paths.run, Some(node)),
                crate::engine::payload_of(&crate::payload::ReviewReplyPosted {
                    node: node.clone(),
                    comment: reply.comment.clone(),
                    disposition: reply.disposition,
                    url: posted.url,
                    commits: owed
                        .drafting
                        .commits()
                        .iter()
                        .filter(|commit| {
                            linked
                                .iter()
                                .any(|sha| commit.sha.starts_with(sha.as_str()))
                        })
                        .map(|commit| commit.sha.clone())
                        .collect(),
                })?,
            )?;
        }
    }
    Ok(())
}

/// Journal a drafting that posted nothing, and surface it once per set of owed
/// revisions: non-blocking, since the next pass drafts it again.
fn not_drafted(
    paths: &RunPaths,
    journal: &mut Journal,
    channel: &ChannelState,
    node: &str,
    comments: Vec<String>,
    why: &crate::reply::NotDrafted,
) -> Result<()> {
    let record = crate::reply::not_drafted(node, comments.clone(), why);
    journal.emit(
        PipelineKind::ReviewReplyNotDrafted,
        journal::labels(&paths.run, Some(node)),
        crate::engine::payload_of(&record)?,
    )?;
    let digest = Sha256::digest(format!("{node}:{}:{:?}", comments.join(","), why.ending));
    let correlation = format!(
        "review-reply-pending:{}",
        digest
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let already = crate::ledger::read_lines(&paths.channel("surfaces.jsonl"))
        .iter()
        .filter_map(|line| serde_json::from_str::<Surface>(line).ok())
        .any(|surface| {
            surface
                .correlation
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                == Some(correlation.as_str())
        });
    if already {
        return Ok(());
    }
    let correlation = correlation
        .parse()
        .map_err(|error| Error::Invalid(format!("review reply correlation: {error}")))?;
    let surface = channel.push(Surface {
        id: 0,
        kind: crate::channel::SurfaceKind::REVIEW_REPLY_PENDING.to_owned(),
        message: format!(
            "Node {node}: the replies to review comments {} were not posted ({}): {}. \
             `onepipeline review` drafts them again.",
            comments.join(", "),
            serde_json::to_value(why.ending)
                .ok()
                .and_then(|word| word.as_str().map(str::to_owned))
                .unwrap_or_default(),
            why.detail
        ),
        source: crate::channel::source::RECONCILER.to_owned(),
        blocking: false,
        queued_at: crate::sys::now_millis(),
        workstream: Some(node.to_owned()),
        abandoned: false,
        asker: None,
        correlation: Some(correlation),
    })?;
    record_surface(paths, journal, &surface)
}

pub(crate) fn lines(comments: &BTreeMap<String, Vec<Comment>>) -> String {
    let mut text = String::new();
    for (node, comments) in comments {
        for comment in comments {
            let state = match &comment.state {
                State::Pending => "pending".to_owned(),
                State::BeingReworked { rework } => format!("being reworked by {rework}"),
                State::Answered {
                    reply_url,
                    rework,
                    commits,
                } => {
                    let by = rework
                        .as_ref()
                        .map(|rework| format!(" by {rework}"))
                        .unwrap_or_default();
                    let linked = if commits.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", commits.join(", "))
                    };
                    format!("answered{by}{linked}: {reply_url}")
                }
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
/// The JSON results view's schema version: **2** since an answered comment names the
/// rework that answered it and the commits its reply links (entry 127), both omitted
/// where empty, so a version-1 reader reads every version-2 document's other fields.
pub(crate) const RESULTS_SCHEMA_VERSION: u32 = 2;

/// The JSON results view; comment lists are absent when empty.
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
