//! Readjustment: keeping a stack of review drafts true while people merge some of
//! them on the host and the rest are still in review.
//!
//! One operation, run first by every trigger — `onepipeline review`, `onepipeline
//! readjust`, applying a `rework` and the republication continuing it, a restack or a
//! placement at dispatch, and a continuing launch or an adoption — and by nothing
//! else: nothing here runs on a timer.
//!
//! - **Detect.** Every draft of the run is read in one batched standings read per
//!   repository and root ([`detect`]), and each answer is journalled once: a merge into
//!   the root `node-landed`, a merge into another node's branch `node-absorbed`, a
//!   draft the host closed under its landed parent's deleted branch reopened
//!   (`change-reopened`), any other close `change-closed`, and a deferred
//!   republication's verdict `node-checks-read`. A repository whose read failed is
//!   journalled `readjust-read-failed`, and its drafts are left as they were.
//! - **Place and bring up to date.** Every draft connected to a node that landed or
//!   was absorbed is owed a readjustment — the fold keeps that set — and the engine
//!   restacks each onto the placement [`crate::stack::placement`] computes over the
//!   fresh state, its parents' pushes first.
//! - **Retarget.** After a moved draft's push its change request is moved to the new
//!   base, with the Stack block rewritten from the description this read returned.

use std::collections::{BTreeMap, BTreeSet};

use crate::channel::Surface;
use crate::error::{Error, Result};
use crate::graph::Landing;
use crate::journal::{self, Journal, PipelineKind};
use crate::ledger::RunPaths;
use crate::payload::{ChecksVerdictWord, LandingDetection, ReadjustTrigger};
use crate::projection::RunState;

/// The surface naming a draft closed without merging.
pub(crate) const CHANGE_CLOSED: &str = "change-closed";
/// The surface naming a draft merged into its parent's branch.
pub(crate) const STACK_MERGED_OUT_OF_ORDER: &str = "stack-merged-out-of-order";
/// The surface naming a draft merged into the root with unlanded ancestors' work.
pub(crate) const STACK_LANDED_OUT_OF_ORDER: &str = "stack-landed-out-of-order";
/// The surface naming a failed or parked draft retargeted with no merge.
pub(crate) const READJUST_PENDING: &str = "readjust-pending";
/// The surface naming a draft a person moved off its computed base.
pub(crate) const CHANGE_BASE_OVERRIDDEN: &str = "change-base-overridden";
/// The blocking surface naming a draft merged into a branch this run does not know.
pub(crate) const CHANGE_MERGED_ELSEWHERE: &str = "change-merged-elsewhere";
/// The surface naming a node whose change request merged while its rework ran.
pub(crate) const MERGED_DURING_REWORK: &str = "merged-during-rework";

/// The outcome a rework replacement settles `failed` under when its node's change
/// request merged while it ran.
pub(crate) const MERGED_DURING_REWORK_OUTCOME: &str = "merged-during-rework";

/// One draft of the run: a lineage head on a repository, with the change request its
/// lineage opened, not recorded landed, absorbed or closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Draft {
    // llmlint: ignore-block[invalid_states_unrepresentable] the node, change request, repo
    // and root are `RunState`'s own strings — a graph-validated id, a URL `known_change_url`
    // already parsed, the plan's `repo` and a branch `vcs::usable` checked — handed straight
    // to `onevcs`'s request types, which take them as these same strings.
    /// The lineage head.
    pub(crate) node: String,
    /// Its lineage's change request.
    pub(crate) url: String,
    /// Its repository, as the plan names it.
    pub(crate) repo: String,
    /// The root its stack stands on.
    pub(crate) root: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// The lineage head `node` belongs to: the newest node every supersession since it
/// names.
pub(crate) fn head_of(state: &RunState, node: &str) -> String {
    let mut at = node.to_owned();
    let mut seen = BTreeSet::new();
    while let Some(next) = state.superseded.get(&at) {
        if !seen.insert(at.clone()) {
            break;
        }
        at = next.clone();
    }
    at
}

/// The change request `node`'s lineage opened: its own, or the newest one a node it
/// superseded recorded.
pub(crate) fn lineage_change(state: &RunState, node: &str) -> Option<String> {
    let mut at = node.to_owned();
    let mut seen = BTreeSet::new();
    loop {
        if let Some(url) = state.known_change_url(&at) {
            return Some(url);
        }
        if !seen.insert(at.clone()) {
            return None;
        }
        at = state
            .superseded
            .iter()
            .find(|(_, by)| **by == at)
            .map(|(predecessor, _)| predecessor.clone())?;
    }
}

/// The root a lineage head's work stands on, read back through its predecessors where
/// the head itself has not settled yet.
fn root_of(state: &RunState, node: &str) -> Option<String> {
    let mut at = node.to_owned();
    let mut seen = BTreeSet::new();
    loop {
        if let Some(root) = crate::stack::root_of(state, &at, &mut BTreeSet::new()) {
            return Some(root);
        }
        if !seen.insert(at.clone()) {
            return None;
        }
        at = state
            .superseded
            .iter()
            .find(|(_, by)| **by == at)
            .map(|(predecessor, _)| predecessor.clone())?;
    }
}

/// Every draft of the run, in graph order.
pub(crate) fn drafts(state: &RunState) -> Vec<Draft> {
    state
        .graph
        .iter()
        .filter(|node| !state.superseded.contains_key(&node.id))
        .filter(|node| state.landings.get(&node.id) != Some(&Landing::Landed))
        .filter(|node| !state.absorbed.contains_key(&node.id))
        .filter(|node| !state.closed_changes.contains(&node.id))
        .filter_map(|node| {
            Some(Draft {
                repo: node.repo.clone()?,
                url: lineage_change(state, &node.id)?,
                root: root_of(state, &node.id),
                node: node.id.clone(),
            })
        })
        .collect()
}

/// Every unlanded draft of `node`'s repository connected to it by dependency edges in
/// either direction, through nodes of that repository whatever their state.
///
/// What a landing or an absorption makes owed: a draft above it moves, and a sibling
/// that is a stack parent of a moved draft is brought up to date with it.
pub(crate) fn connected(state: &RunState, node: &str) -> BTreeSet<String> {
    let Some(repo) = state.graph.get(node).and_then(|node| node.repo.clone()) else {
        return BTreeSet::new();
    };
    let same_repo = |id: &str| {
        state
            .graph
            .get(id)
            .and_then(|node| node.repo.as_deref())
            .is_some_and(|other| other == repo)
    };
    let mut seen = BTreeSet::from([node.to_owned()]);
    let mut pending = vec![node.to_owned()];
    while let Some(at) = pending.pop() {
        let mut neighbours: Vec<String> = state
            .graph
            .get(&at)
            .map(|node| node.deps.clone())
            .unwrap_or_default();
        neighbours.extend(
            state
                .graph
                .iter()
                .filter(|other| other.deps.iter().any(|dep| *dep == at))
                .map(|other| other.id.clone()),
        );
        for next in neighbours {
            if same_repo(&next) && seen.insert(next.clone()) {
                pending.push(next);
            }
        }
    }
    seen.remove(node);
    seen.into_iter()
        .filter(|id| !state.superseded.contains_key(id))
        .filter(|id| state.landings.get(id) != Some(&Landing::Landed))
        .filter(|id| !state.absorbed.contains_key(id))
        .filter(|id| lineage_change(state, id).is_some())
        .collect()
}

/// What one readjustment's read found, kept for the rest of the trigger: each draft's
/// standing, the review reads a review pass discovers from, and what failed.
#[derive(Debug, Default)]
pub(crate) struct Detection {
    /// Each draft's standing, by node.
    pub(crate) standings: BTreeMap<String, onevcs::ChangeStanding>,
    /// Each draft's review read, in a review pass.
    pub(crate) reviews: Vec<(String, String, Result<onevcs::ReviewRead>)>,
    /// Each repository whose read failed, and why.
    pub(crate) failures: Vec<(String, String)>,
    /// Each open draft a person moved off its recorded base, and the base it targets.
    pub(crate) overridden: BTreeMap<String, String>,
    /// Each draft whose deferred verdict was read red: the head it is about, and the
    /// checks that failed.
    pub(crate) red: BTreeMap<String, Red>,
}

/// A red verdict a trigger read: the head it is about, and the checks that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Red {
    // llmlint: ignore-block[invalid_states_unrepresentable] the host's own words, read off the
    // standing `onevcs` parsed: the head it reports and the check names it says failed.
    /// The head the checks ran on.
    pub(crate) head: String,
    /// The required checks that failed.
    pub(crate) failed: Vec<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

impl Detection {
    /// Whether any repository's read failed.
    pub(crate) fn failed(&self) -> Option<String> {
        (!self.failures.is_empty()).then(|| {
            self.failures
                .iter()
                .map(|(repo, detail)| format!("{repo}: {detail}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }
}

/// One read's group: the drafts of one repository on one root.
struct Group {
    repo: String,
    root: String,
    drafts: Vec<Draft>,
}

/// Group `drafts` by normalized repository and root, answering each one that cannot be
/// grouped as a failure of its repository.
fn grouped(drafts: Vec<Draft>, failures: &mut Vec<(String, String)>) -> Vec<Group> {
    let mut identities = BTreeMap::<String, std::result::Result<String, String>>::new();
    let mut groups = BTreeMap::<(String, String), Group>::new();
    for draft in drafts {
        let identity = identities
            .entry(draft.repo.clone())
            .or_insert_with(|| {
                crate::vcs::repository_identity(&draft.repo).map_err(|error| error.to_string())
            })
            .clone();
        let identity = match identity {
            Ok(identity) => identity,
            Err(detail) => {
                if !failures.iter().any(|(repo, _)| *repo == draft.repo) {
                    failures.push((draft.repo.clone(), detail));
                }
                continue;
            }
        };
        let Some(root) = draft.root.clone() else {
            failures.push((
                draft.repo.clone(),
                format!(
                    "draft {} has no recorded root to read its standing against",
                    draft.node
                ),
            ));
            continue;
        };
        groups
            .entry((identity.clone(), root.clone()))
            .or_insert_with(|| Group {
                repo: identity,
                root,
                drafts: Vec::new(),
            })
            .drafts
            .push(draft);
    }
    groups.into_values().collect()
}

/// Read every draft's standing — and, in a review pass, its review feedback — and
/// journal what each answer says, once per change.
///
/// `review` reads through `review_standings`, whose review reads the pass then
/// discovers from; every other trigger reads `change_standings`. A repository whose
/// read failed is journalled `readjust-read-failed` and contributes nothing.
pub(crate) fn detect(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &RunState,
    trigger: ReadjustTrigger,
    review: bool,
) -> Result<Detection> {
    let mut detection = Detection::default();
    let drafts = drafts(state);
    if drafts.is_empty() {
        return Ok(detection);
    }
    let mut failures = Vec::new();
    let groups = grouped(drafts, &mut failures);
    let mut read: Vec<(Draft, onevcs::ChangeStanding)> = Vec::new();
    for group in groups {
        let urls = group
            .drafts
            .iter()
            .map(|draft| {
                draft
                    .url
                    .parse::<onevcs::Url>()
                    .map_err(|error| format!("change URL {}: {error}", draft.url))
            })
            .collect::<std::result::Result<Vec<_>, String>>();
        let answer = match urls {
            Err(detail) => Err(detail),
            Ok(changes) if review => {
                crate::vcs::review_standings(&onevcs::ReviewStandingsRequest {
                    repo: group.repo.clone(),
                    root: group.root.clone(),
                    changes: changes
                        .into_iter()
                        .map(|change| onevcs::ReviewTarget {
                            change,
                            since: None,
                        })
                        .collect(),
                })
                .map(|answer| (answer.standings, answer.reads))
                .map_err(|error| error.to_string())
            }
            Ok(changes) => crate::vcs::change_standings(&onevcs::ChangeStandingsRequest {
                repo: group.repo.clone(),
                root: group.root.clone(),
                changes,
            })
            .map(|answer| (answer.standings, Vec::new()))
            .map_err(|error| error.to_string()),
        };
        match answer {
            Ok((standings, reads)) => {
                let mut reads = reads.into_iter();
                for (draft, standing) in group.drafts.into_iter().zip(standings) {
                    if let Some(review) = reads.next() {
                        detection
                            .reviews
                            .push((draft.node.clone(), draft.url.clone(), Ok(review)));
                    }
                    read.push((draft, standing));
                }
            }
            Err(detail) => {
                if review {
                    for draft in &group.drafts {
                        detection.reviews.push((
                            draft.node.clone(),
                            draft.url.clone(),
                            Err(Error::Sibling {
                                tool: "onevcs",
                                message: detail.clone(),
                            }),
                        ));
                    }
                }
                let repo = group
                    .drafts
                    .first()
                    .map_or_else(|| group.repo.clone(), |draft| draft.repo.clone());
                failures.push((repo, detail));
            }
        }
    }
    for (repo, detail) in &failures {
        journal.emit(
            PipelineKind::ReadjustReadFailed,
            journal::labels(&paths.run, None),
            crate::engine::payload_of(&crate::payload::ReadjustReadFailed {
                repo: repo.clone(),
                detail: crate::engine::bounded(&crate::views::one_line(detail)),
            })?,
        )?;
    }
    detection.failures = failures;
    take_up(paths, journal, state, trigger, &read, &mut detection)?;
    detection.standings = read
        .into_iter()
        .map(|(draft, standing)| (draft.node, standing))
        .collect();
    Ok(detection)
}

/// Journal what each standing says, in graph order so a lower landing is on the record
/// before anything above it is read.
fn take_up(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &RunState,
    trigger: ReadjustTrigger,
    read: &[(Draft, onevcs::ChangeStanding)],
    detection: &mut Detection,
) -> Result<()> {
    let mut landed_now: BTreeSet<String> = BTreeSet::new();
    let mut moved_now: BTreeSet<String> = BTreeSet::new();
    let by_branch: BTreeMap<&str, &str> = read
        .iter()
        .map(|(draft, standing)| (standing.head_branch.as_str(), draft.node.as_str()))
        .collect();
    for (draft, standing) in read {
        let node = draft.node.as_str();
        match &standing.state {
            onevcs::ChangeState::Merged {
                commit,
                landing: true,
                ..
            } => {
                landed(
                    paths,
                    journal,
                    state,
                    trigger,
                    draft,
                    &commit.0,
                    &landed_now,
                )?;
                landed_now.insert(node.to_owned());
                moved_now.insert(node.to_owned());
            }
            onevcs::ChangeState::Merged { into, commit, .. } => {
                let absorber = by_branch
                    .get(into.as_str())
                    .map(|node| (*node).to_owned())
                    .or_else(|| branch_owner(state, into));
                match absorber.filter(|absorber| absorber != node) {
                    Some(into_node) => {
                        absorbed(paths, journal, draft, &into_node, &commit.0)?;
                        moved_now.insert(node.to_owned());
                    }
                    None => merged_elsewhere(paths, journal, draft, into, &commit.0)?,
                }
            }
            onevcs::ChangeState::Closed { base_deleted } => {
                let parent = read.iter().find(|(_, parent)| {
                    parent.head_branch == standing.base
                        && matches!(parent.state, onevcs::ChangeState::Merged { .. })
                });
                match parent.filter(|_| *base_deleted) {
                    Some((_, parent)) => {
                        if let Err(error) = reopen(paths, journal, draft, parent) {
                            detection
                                .failures
                                .push((draft.repo.clone(), error.to_string()));
                        }
                    }
                    None => closed(paths, journal, draft)?,
                }
            }
            _ => {
                verdict(paths, journal, state, draft, standing, detection)?;
            }
        }
    }
    // A person's own move of a draft no landing moved is moved back, and said.
    for (draft, standing) in read {
        let node = draft.node.as_str();
        if !matches!(standing.state, onevcs::ChangeState::Open) {
            continue;
        }
        let Some(recorded) = state.bases.get(node) else {
            continue;
        };
        let owed = state.readjust_owed.contains_key(node)
            || moved_now
                .iter()
                .any(|moved| connected(state, moved).contains(node));
        if *recorded != standing.base && !owed {
            detection
                .overridden
                .insert(node.to_owned(), standing.base.clone());
        }
    }
    Ok(())
}

/// The lineage head whose recorded branch is `branch`.
fn branch_owner(state: &RunState, branch: &str) -> Option<String> {
    state
        .branches
        .iter()
        .find(|(node, held)| *held == branch && !state.superseded.contains_key(*node))
        .map(|(node, _)| node.clone())
}

/// Every unlanded draft of `node`'s repository it depends on, directly or not.
fn unlanded_ancestors(state: &RunState, node: &str, landed_now: &BTreeSet<String>) -> Vec<String> {
    let repo = state.graph.get(node).and_then(|node| node.repo.clone());
    let mut found = BTreeSet::new();
    let mut pending: Vec<String> = state
        .graph
        .get(node)
        .map(|node| node.deps.clone())
        .unwrap_or_default();
    while let Some(next) = pending.pop() {
        let Some(dep) = state.graph.get(&next) else {
            continue;
        };
        if dep.repo != repo || !found.insert(next.clone()) {
            continue;
        }
        pending.extend(dep.deps.iter().cloned());
    }
    state
        .graph
        .iter()
        .map(|node| node.id.clone())
        .filter(|id| found.contains(id))
        .filter(|id| !state.superseded.contains_key(id))
        .filter(|id| state.landings.get(id) != Some(&Landing::Landed) && !landed_now.contains(id))
        .filter(|id| !state.absorbed.contains_key(id))
        .filter(|id| lineage_change(state, id).is_some())
        .collect()
}

fn landed(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &RunState,
    trigger: ReadjustTrigger,
    draft: &Draft,
    commit: &str,
    landed_now: &BTreeSet<String>,
) -> Result<()> {
    let node = draft.node.as_str();
    emit(
        paths,
        journal,
        PipelineKind::NodeLanded,
        node,
        &crate::payload::NodeLanded {
            node: node.to_owned(),
            change_url: draft.url.clone(),
            commit: commit.to_owned(),
            detected_by: LandingDetection::Readjust,
            trigger,
            via: None,
        },
    )?;
    // What was absorbed into it lands with it.
    for (absorbed, into) in &state.absorbed {
        if into != node || state.landings.get(absorbed) == Some(&Landing::Landed) {
            continue;
        }
        let Some(url) = lineage_change(state, absorbed) else {
            continue;
        };
        emit(
            paths,
            journal,
            PipelineKind::NodeLanded,
            absorbed,
            &crate::payload::NodeLanded {
                node: absorbed.clone(),
                change_url: url,
                commit: commit.to_owned(),
                detected_by: LandingDetection::AbsorberLanded,
                trigger,
                via: Some(node.to_owned()),
            },
        )?;
    }
    let carried = unlanded_ancestors(state, node, landed_now);
    if !carried.is_empty() {
        emit(
            paths,
            journal,
            PipelineKind::StackLandedOutOfOrder,
            node,
            &crate::payload::StackLandedOutOfOrder {
                node: node.to_owned(),
                carried: carried.clone(),
            },
        )?;
        notify(
            paths,
            journal,
            STACK_LANDED_OUT_OF_ORDER,
            node,
            format!(
                "node '{node}' ({}) was merged into its root while {} were still unlanded, so \
                 their work reached the root through it. Nothing was closed or reverted; each \
                 is brought up to date onto its placement.",
                draft.url,
                quoted(&carried)
            ),
        )?;
    }
    Ok(())
}

fn absorbed(
    paths: &RunPaths,
    journal: &mut Journal,
    draft: &Draft,
    into: &str,
    commit: &str,
) -> Result<()> {
    let node = draft.node.as_str();
    emit(
        paths,
        journal,
        PipelineKind::NodeAbsorbed,
        node,
        &crate::payload::NodeAbsorbed {
            node: node.to_owned(),
            into: into.to_owned(),
            change_url: draft.url.clone(),
            commit: commit.to_owned(),
        },
    )?;
    notify(
        paths,
        journal,
        STACK_MERGED_OUT_OF_ORDER,
        node,
        format!(
            "node '{node}' ({}) was merged into the branch of '{into}' rather than its root. Its \
             work now sits in '{into}''s draft and reaches the root only with it; the drafts \
             above it are placed on '{into}''s branch.",
            draft.url
        ),
    )
}

/// A draft merged into a branch neither the root nor any node of this run: a blocking
/// question, raised once, and nothing journalled about the node.
fn merged_elsewhere(
    paths: &RunPaths,
    journal: &mut Journal,
    draft: &Draft,
    into: &str,
    commit: &str,
) -> Result<()> {
    let correlation = elsewhere_correlation(&draft.url, commit);
    let channel = crate::channel::ChannelState::new(paths);
    if channel.raised_under(&correlation).is_some() {
        return Ok(());
    }
    let node = draft.node.as_str();
    crate::engine::raise(
        paths,
        journal,
        Surface {
            id: 0,
            kind: CHANGE_MERGED_ELSEWHERE.to_owned(),
            message: format!(
                "node '{node}''s change request {} was merged into `{into}` as {commit}, which is \
                 neither the run's root nor the branch of any node of this run. Nothing was \
                 recorded about it, and neither it nor anything stacked on it is readjusted until \
                 you answer this: say how the run goes on with '{node}'.",
                draft.url
            ),
            source: crate::channel::source::RECONCILER.to_owned(),
            blocking: true,
            queued_at: crate::sys::now_millis(),
            abandoned: false,
            asker: None,
            workstream: Some(node.to_owned()),
            correlation: Some(correlation),
        },
    )
}

fn elsewhere_correlation(url: &str, commit: &str) -> onemessagebus::Correlation {
    format!("merged-elsewhere:{url}:{commit}")
        .parse()
        .unwrap_or_else(|why| unreachable!("a merged-elsewhere correlation: {why}"))
}

/// The nodes held by an unanswered merged-elsewhere question, and every node depending
/// on them.
pub(crate) fn held_elsewhere(paths: &RunPaths, state: &RunState) -> BTreeSet<String> {
    let channel = crate::channel::ChannelState::new(paths);
    let answered = channel.answered();
    let held: Vec<String> = channel
        .every_surface()
        .into_iter()
        .filter(|surface| surface.kind == CHANGE_MERGED_ELSEWHERE)
        .filter(|surface| {
            surface
                .correlation
                .as_ref()
                .is_some_and(|correlation| !answered.contains(correlation))
        })
        .filter_map(|surface| surface.workstream)
        .collect();
    let mut all: BTreeSet<String> = held.iter().cloned().collect();
    for node in held {
        all.extend(descendants(&state.graph, &node));
    }
    all
}

/// Every node depending on `id`, directly or not.
pub(crate) fn descendants(graph: &crate::graph::Graph, id: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut pending = graph.dependents_of(id);
    while let Some(next) = pending.pop() {
        if found.insert(next.clone()) {
            pending.extend(graph.dependents_of(&next));
        }
    }
    found
}

fn reopen(
    paths: &RunPaths,
    journal: &mut Journal,
    draft: &Draft,
    parent: &onevcs::ChangeStanding,
) -> Result<()> {
    let change = draft
        .url
        .parse::<onevcs::Url>()
        .map_err(|error| Error::Invalid(format!("change URL {}: {error}", draft.url)))?;
    let reopened = crate::vcs::reopen_change(&onevcs::ReopenRequest {
        repo: draft.repo.clone(),
        change,
        restore_base_at: parent.head.clone(),
    })?;
    emit(
        paths,
        journal,
        PipelineKind::ChangeReopened,
        &draft.node,
        &crate::payload::ChangeReopened {
            node: draft.node.clone(),
            change_url: draft.url.clone(),
            restored_base: reopened.restored_base,
        },
    )
}

fn closed(paths: &RunPaths, journal: &mut Journal, draft: &Draft) -> Result<()> {
    let node = draft.node.as_str();
    emit(
        paths,
        journal,
        PipelineKind::ChangeClosed,
        node,
        &crate::payload::ChangeClosed {
            node: node.to_owned(),
            change_url: draft.url.clone(),
        },
    )?;
    notify(
        paths,
        journal,
        CHANGE_CLOSED,
        node,
        format!(
            "node '{node}''s change request {} was closed without being merged. Nothing was \
             moved: its branch stays where it is, and the drafts stacked on it stay on it.",
            draft.url
        ),
    )
}

/// A deferred republication's verdict, read off the same standing: for a draft whose
/// recorded head the host reports and whose verdict the record still owes, and for a
/// settled review draft whose head on the host moved past the one this host recorded —
/// the cold host's case, whose store holds neither a clean restack's head nor its unread
/// verdict — once per head.
fn verdict(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &RunState,
    draft: &Draft,
    standing: &onevcs::ChangeStanding,
    detection: &mut Detection,
) -> Result<()> {
    let node = draft.node.as_str();
    let outcome = state.outcomes.get(node).map(String::as_str);
    let recorded = state.heads.get(node);
    let owed =
        outcome == Some(crate::vcs::REVIEW_DRAFTED_UNCHECKED) && recorded == Some(&standing.head.0);
    let moved_past = matches!(
        outcome,
        Some(crate::vcs::REVIEW_DRAFTED | crate::vcs::REVIEW_DRAFTED_UNCHECKED)
    ) && recorded != Some(&standing.head.0)
        && !state.restacking.contains_key(node)
        && state
            .recorded
            .get(node)
            .map(|recorded| recorded.clone().status())
            == Some(crate::graph::NodeStatus::Done)
        && state.checks_read.get(node) != Some(&standing.head.0);
    if !owed && !moved_past {
        return Ok(());
    }
    let (word, failed) = match &standing.checks {
        onevcs::ChecksVerdict::Passed => (ChecksVerdictWord::Passed, None),
        onevcs::ChecksVerdict::PassedWithSkipped { .. } => {
            (ChecksVerdictWord::PassedWithSkipped, None)
        }
        onevcs::ChecksVerdict::NoneRequired => (ChecksVerdictWord::NoneRequired, None),
        onevcs::ChecksVerdict::Failed { failed } => {
            (ChecksVerdictWord::Failed, Some(failed.clone()))
        }
        _ => (ChecksVerdictWord::Pending, None),
    };
    emit(
        paths,
        journal,
        PipelineKind::NodeChecksRead,
        node,
        &crate::payload::NodeChecksRead {
            node: node.to_owned(),
            head: standing.head.0.clone(),
            verdict: word,
        },
    )?;
    if let Some(failed) = failed {
        detection.red.insert(
            node.to_owned(),
            Red {
                head: standing.head.0.clone(),
                failed,
            },
        );
    }
    Ok(())
}

fn emit<T: serde::Serialize>(
    paths: &RunPaths,
    journal: &mut Journal,
    kind: PipelineKind,
    node: &str,
    payload: &T,
) -> Result<()> {
    journal.emit(
        kind,
        journal::labels(&paths.run, Some(node)),
        crate::engine::payload_of(payload)?,
    )
}

/// Raise a non-blocking report to the planner about `node`.
pub(crate) fn notify(
    paths: &RunPaths,
    journal: &mut Journal,
    kind: &str,
    node: &str,
    message: String,
) -> Result<()> {
    crate::engine::raise(
        paths,
        journal,
        Surface {
            id: 0,
            kind: kind.to_owned(),
            message,
            source: crate::channel::source::RECONCILER.to_owned(),
            blocking: false,
            queued_at: crate::sys::now_millis(),
            abandoned: false,
            asker: None,
            workstream: Some(node.to_owned()),
            correlation: None,
        },
    )
}

fn quoted(nodes: &[String]) -> String {
    nodes
        .iter()
        .map(|node| format!("'{node}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The Stack block a draft placed on its root carries: there is nothing under it.
pub(crate) fn root_block(root: &str) -> String {
    format!(
        "{}\n## Stack\n\nDrafted against `{root}`, the root: every change it was stacked on has \
         landed.\n{}",
        crate::stack::BLOCK_OPEN,
        crate::stack::BLOCK_CLOSE
    )
}

/// What one driver's readjustments remember between the read and the restacks it owes.
#[derive(Debug, Default)]
pub(crate) struct Readjuster {
    /// A trigger whose readjustment the next pass runs.
    pub(crate) due: Option<ReadjustTrigger>,
    /// Whether this pass already read, so every placement in it reuses that read.
    pub(crate) read_this_pass: bool,
    /// Each draft's description as the latest read returned it.
    pub(crate) bodies: BTreeMap<String, String>,
    /// Each draft a person moved off its computed base, and the base it targets.
    pub(crate) overridden: BTreeMap<String, String>,
    /// Each draft whose deferred verdict was read red, and what it is about.
    pub(crate) red: BTreeMap<String, Red>,
    /// Each readjustment in flight: where it moves its draft from and to, and why.
    pub(crate) plans: BTreeMap<String, Plan>,
    /// Each draft whose readjustment this driver started since its last read, which
    /// is not started again until a trigger reads again.
    pub(crate) tried: BTreeSet<String>,
    /// Each node a placement read was made for, so a node held across passes is not
    /// read for again.
    pub(crate) placed: BTreeSet<String>,
}

/// One readjustment in flight.
#[derive(Debug, Clone)]
pub(crate) struct Plan {
    /// Where its draft was placed.
    pub(crate) from: crate::payload::Placement,
    /// Where it is placed now.
    pub(crate) to: crate::payload::Placement,
    /// The landed or absorbed nodes that moved it.
    pub(crate) cause: Vec<String>,
}

impl Readjuster {
    /// Keep what `detection` read for the rest of this trigger.
    pub(crate) fn absorb(&mut self, detection: &Detection) {
        self.read_this_pass = true;
        self.tried.clear();
        for (node, standing) in &detection.standings {
            self.bodies.insert(node.clone(), standing.body.clone());
        }
        self.overridden.extend(detection.overridden.clone());
        self.red.extend(detection.red.clone());
    }
}

/// Run one trigger's readjustment read over the run's folded state, keep what it read,
/// and fold what it journalled.
pub(crate) fn run(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &mut crate::checkpoint::Projected,
    trigger: ReadjustTrigger,
    review: bool,
) -> Result<Detection> {
    let detection = detect(paths, journal, state, trigger, review)?;
    state.readjust.absorb(&detection);
    state.refresh(paths);
    Ok(detection)
}

/// Whether the record owes a readjustment something a driver has to do: a draft to
/// restack or retarget, or a red verdict to repair.
pub(crate) fn owes_work(state: &crate::checkpoint::Projected) -> bool {
    state
        .readjust_owed
        .keys()
        .chain(state.readjust.overridden.keys())
        .chain(state.readjust.red.keys())
        .any(|node| !state.superseded.contains_key(node))
}

/// End an explicit trigger on a run nothing drives: where its readjustment owes a
/// restack, a retarget or a repair, drive the run under the lock already held until
/// those settle — with what the trigger read, so it is not read again — and answer the
/// drafts whose verdict the record still owes.
pub(crate) fn finish_undriven(
    paths: &RunPaths,
    lock: crate::ledger::OwnershipLock,
    mut state: crate::checkpoint::Projected,
) -> Result<Vec<String>> {
    if !owes_work(&state) {
        drop(lock);
        return Ok(outstanding(&state));
    }
    let readjuster = std::mem::take(&mut state.readjust);
    drop(state);
    crate::engine::drive_after_readjust(paths, lock, readjuster)?;
    Ok(outstanding(&crate::checkpoint::fold_and_checkpoint(paths)))
}

/// Where a readjusted draft's change request is moved after its push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Retarget {
    // llmlint: ignore-block[invalid_states_unrepresentable] each is the `String`
    // `onevcs::RetargetRequest` takes it as, read off the run's own record — the plan's
    // `repo`, a change request `known_change_url` parsed, a branch the placement named —
    // and held by `retarget_change` to the repository, a URL and git's grammar before
    // anything is written.
    /// The repository, as the plan names it.
    pub(crate) repo: String,
    /// The change request.
    pub(crate) change: String,
    /// The base it is moved onto.
    pub(crate) base: String,
    /// The Stack block it carries there.
    pub(crate) block: String,
    /// Its description as the readjustment's read returned it, where that read is
    /// still in hand.
    pub(crate) body: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// Whether the base it leaves is one a reopen restored, which the move deletes.
    pub(crate) remove_restored_base: bool,
}

/// Move a readjusted draft's change request onto its new base, writing its description
/// with only the Stack block rewritten, in one write.
///
/// A description no read of this trigger returned is read first, in a standings read of
/// that one change request: the block is spliced into the text the host holds now, so
/// nothing a person wrote outside its markers moves.
pub(crate) fn retarget(retarget: &Retarget) -> Result<crate::payload::RetargetWord> {
    let change = retarget
        .change
        .parse::<onevcs::Url>()
        .map_err(|error| Error::Invalid(format!("change URL {}: {error}", retarget.change)))?;
    let body = match &retarget.body {
        Some(body) => body.clone(),
        None => crate::vcs::change_standings(&onevcs::ChangeStandingsRequest {
            repo: retarget.repo.clone(),
            root: retarget.base.clone(),
            changes: vec![change.clone()],
        })?
        .standings
        .into_iter()
        .next()
        .map(|standing| standing.body)
        .unwrap_or_default(),
    };
    let moved = crate::vcs::retarget_change(&onevcs::RetargetRequest {
        repo: retarget.repo.clone(),
        change,
        base: retarget.base.clone(),
        body: Some(crate::stack::splice(&body, &retarget.block)),
        remove_restored_base: retarget.remove_restored_base,
    })?;
    Ok(match moved.outcome {
        onevcs::RetargetOutcome::Retargeted => crate::payload::RetargetWord::Retargeted,
        _ => crate::payload::RetargetWord::Unchanged,
    })
}

/// The drafts whose verdict the record still owes, for the views and the verbs.
pub(crate) fn outstanding(state: &RunState) -> Vec<String> {
    state
        .graph
        .iter()
        .filter(|node| !state.superseded.contains_key(&node.id))
        .filter(|node| state.landings.get(&node.id) != Some(&Landing::Landed))
        .filter(|node| {
            state.outcomes.get(&node.id).map(String::as_str)
                == Some(crate::vcs::REVIEW_DRAFTED_UNCHECKED)
        })
        .map(|node| node.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Node;

    fn node(id: &str, deps: &[&str], repo: &str) -> Node {
        Node {
            id: id.into(),
            repo: Some(repo.into()),
            deps: deps.iter().map(|dep| (*dep).to_owned()).collect(),
            ..Node::default()
        }
    }

    fn state(nodes: Vec<Node>) -> RunState {
        let mut state = RunState::default();
        for node in nodes {
            state.change_urls.insert(
                node.id.clone(),
                format!(
                    "https://github.com/o/r/pull/{}",
                    state.change_urls.len() + 1
                ),
            );
            state.landings.insert(node.id.clone(), Landing::Unlanded);
            state.graph.insert(node);
        }
        state
    }

    /// A landing owes every draft of its repository reachable from it, through landed
    /// nodes too, and none of another repository's.
    #[test]
    fn a_landing_owes_its_connected_drafts_of_its_own_repository() {
        let mut state = state(vec![
            node("a", &[], "service"),
            node("b", &["a"], "service"),
            node("c", &["b"], "service"),
            node("x", &[], "service"),
            node("y", &["b", "x"], "service"),
            node("z", &["a"], "other"),
        ]);
        state.landings.insert("b".into(), Landing::Landed);
        let owed = connected(&state, "a");
        assert_eq!(
            owed,
            ["c", "x", "y"].iter().map(|id| (*id).to_owned()).collect()
        );
    }
}
