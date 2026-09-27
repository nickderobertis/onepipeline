//! What a landed retry superseded, told to `onevcs` when it lands.
//!
//! `onevcs` chains the sessions cut onto one branch itself, so an earlier attempt
//! on the **same** branch as the retry that landed needs nothing from here. An
//! attempt on a branch of its own is different: to that library it is unpublished
//! work of unknown value, and only this run's journal knows that a retry of it
//! landed. So every landed lineage's earlier attempts on other branches are handed
//! to [`onevcs::record_supersession`] — which is what lets that library retire one
//! that adds nothing and surface one that still differs — and the answer is
//! journalled as `branches-superseded`.
//!
//! A lineage is walked through the journal's retry edges, root first, exactly as
//! `writeback`'s lineages are: [`RunState::superseded`] maps each retried node to
//! its replacement. What landed and where is the fold's: a publication closeout
//! that settled `merged`, a `settle` stating a `landing`, and a relayed
//! `merge-completed` are the three ways a node's landing reaches it.
//!
//! **Nothing here changes a settlement.** The record is idempotent in `onevcs`, a
//! refusal is journalled in the record's `failed` list and the run moves on, and
//! the backfill verb — `onepipeline supersessions` — records what a run that
//! settled before this existed never did, through the same function.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::event::{Envelope, PipelineKind, Source};
use crate::graph::Landing;
use crate::journal::{self, Journal};
use crate::ledger::RunPaths;
use crate::payload::{BranchesSuperseded, SupersededAttempt, UnrecordedAttempt};
use crate::projection::RunState;
use crate::Result;

/// The label key naming the run a supersession was recorded by.
const RUN_LABEL: &str = "run";
/// The label key naming the attempt that was superseded.
const NODE_LABEL: &str = "node";
/// The label key naming the attempt whose landing superseded it.
const BY_LABEL: &str = "superseded_by_node";

/// One earlier attempt of a landed lineage: its node, and the branch it left.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct Attempt {
    /// The attempt's node.
    pub(crate) node: String,
    /// The branch it left.
    pub(crate) branch: String,
}

/// One earlier attempt `onevcs` would not record, and what it answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Unrecorded {
    /// The attempt.
    #[serde(flatten)]
    pub(crate) attempt: Attempt,
    /// What `onevcs` answered.
    pub(crate) error: String,
}

/// A lineage whose head landed, and the earlier attempts on other branches it
/// superseded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Lineage {
    /// The node that landed: the lineage's head.
    pub(crate) node: String,
    /// The branch it landed from.
    pub(crate) branch: String,
    /// The repository its work lands in, as the node names it.
    pub(crate) repo: String,
    /// Where it landed: the commit, or the change request's URL where no commit
    /// is recorded.
    pub(crate) landing: String,
    /// Every earlier attempt, root first, whose branch is not the one that landed.
    /// An attempt on the landed branch is `onevcs`'s own to chain and is not here.
    pub(crate) superseded: Vec<Attempt>,
}

/// What one landed lineage's recording did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Recorded {
    /// The attempts recorded as superseded.
    pub(crate) recorded: Vec<Attempt>,
    /// The attempts `onevcs` refused, and why.
    pub(crate) failed: Vec<Unrecorded>,
}

/// Every landed lineage that has earlier attempts, in head order.
///
/// A lineage whose head's branch or landing the run never recorded is left out:
/// `onevcs` is asked for a supersession by the branch that superseded and where it
/// landed, and there is no answer to give it.
pub(crate) fn lineages(state: &RunState) -> Vec<Lineage> {
    let predecessor: BTreeMap<&str, &str> = state
        .superseded
        .iter()
        .map(|(retried, replacement)| (replacement.as_str(), retried.as_str()))
        .collect();
    let heads: BTreeSet<&String> = state
        .landings
        .keys()
        .chain(state.landing_commits.keys())
        .filter(|node| predecessor.contains_key(node.as_str()))
        .collect();
    heads
        .into_iter()
        .filter(|node| landed(state, node))
        .filter_map(|node| {
            let branch = branch_of(state, node)?;
            let landing = landing_of(state, node)?;
            let repo = state.graph.get(node).and_then(|n| n.repo.clone())?;
            let mut chain: Vec<&str> = Vec::new();
            let mut at = node.as_str();
            while let Some(earlier) = predecessor.get(at) {
                // A journal a person edited can hold a cycle; a lineage is walked
                // once whatever it holds.
                if chain.contains(earlier) || *earlier == node.as_str() {
                    break;
                }
                chain.push(earlier);
                at = earlier;
            }
            chain.reverse();
            let superseded = chain
                .into_iter()
                .filter_map(|attempt| {
                    let left = branch_of(state, attempt)?;
                    (left != branch).then(|| Attempt {
                        node: attempt.to_owned(),
                        branch: left,
                    })
                })
                .collect();
            Some(Lineage {
                node: node.clone(),
                branch,
                repo,
                landing,
                superseded,
            })
        })
        .collect()
}

/// Whether the run recorded a node's work reaching its base.
fn landed(state: &RunState, node: &str) -> bool {
    state.landings.get(node) == Some(&Landing::Landed) || state.landing_commits.contains_key(node)
}

/// The branch a node's attempt left: its settlement's, else its session's.
fn branch_of(state: &RunState, node: &str) -> Option<String> {
    state
        .branches
        .get(node)
        .filter(|branch| !branch.is_empty())
        .cloned()
        .or_else(|| {
            state
                .sessions
                .get(node)
                .or_else(|| state.abandoned.get(node))
                .map(|session| session.branch().as_str().to_owned())
        })
}

/// Where a node's work landed, as `onevcs` takes a landing: a commit, else a
/// change request's URL.
fn landing_of(state: &RunState, node: &str) -> Option<String> {
    let stated = state.stated_landings.get(node);
    if let Some(crate::edits::StatedLanding::Commit(commit)) = stated {
        return Some(commit.clone());
    }
    state
        .landing_commits
        .get(node)
        .cloned()
        .or_else(|| stated.map(|stated| stated.reference().to_owned()))
        .or_else(|| state.known_change_url(node))
}

/// Every `(node, branch)` a `branches-superseded` record says `onevcs` recorded.
pub(crate) fn recorded_in(events: &[Envelope]) -> BTreeSet<Attempt> {
    events
        .iter()
        .filter(|event| {
            event.source == Source::Pipeline
                && PipelineKind::from_wire(&event.kind) == Some(PipelineKind::BranchesSuperseded)
        })
        .filter_map(|event| {
            serde_json::from_value::<BranchesSuperseded>(serde_json::Value::Object(
                event.payload.clone(),
            ))
            .ok()
        })
        .flat_map(|record| record.superseded)
        .map(|attempt| Attempt {
            node: attempt.node,
            branch: attempt.branch,
        })
        .collect()
}

/// Record each of `attempts` as superseded by `lineage`'s landing, and say what
/// `onevcs` answered for each.
pub(crate) fn record(run: &str, lineage: &Lineage, attempts: &[Attempt]) -> Recorded {
    let mut recorded = Vec::new();
    let mut failed = Vec::new();
    for attempt in attempts {
        let supersession = onevcs::Supersession {
            repo: lineage.repo.clone(),
            branch: attempt.branch.clone(),
            superseded_by: lineage.branch.clone(),
            landing: lineage.landing.clone(),
            labels: BTreeMap::from([
                (RUN_LABEL.to_owned(), run.to_owned()),
                (NODE_LABEL.to_owned(), attempt.node.clone()),
                (BY_LABEL.to_owned(), lineage.node.clone()),
            ]),
        };
        match onevcs::record_supersession(&supersession) {
            Ok(()) => recorded.push(attempt.clone()),
            Err(error) => failed.push(Unrecorded {
                attempt: attempt.clone(),
                error: crate::engine::bounded(&error.to_string()),
            }),
        }
    }
    Recorded { recorded, failed }
}

/// Journal what one lineage's recording did, as `branches-superseded`.
pub(crate) fn journal(
    paths: &RunPaths,
    journal: &mut Journal,
    lineage: &Lineage,
    recorded: &Recorded,
) -> Result<()> {
    let payload = BranchesSuperseded {
        node: lineage.node.clone(),
        landing: lineage.landing.clone(),
        superseded: recorded
            .recorded
            .iter()
            .map(|attempt| SupersededAttempt {
                node: attempt.node.clone(),
                branch: attempt.branch.clone(),
            })
            .collect(),
        failed: recorded
            .failed
            .iter()
            .map(|unrecorded| UnrecordedAttempt {
                node: unrecorded.attempt.node.clone(),
                branch: unrecorded.attempt.branch.clone(),
                error: unrecorded.error.clone(),
            })
            .collect(),
    };
    let serde_json::Value::Object(payload) =
        serde_json::to_value(payload).expect("a payload struct serializes to an object")
    else {
        unreachable!("a payload struct serializes to an object")
    };
    journal.emit(
        PipelineKind::BranchesSuperseded,
        journal::labels(&paths.run, None),
        payload,
    )
}

/// The driver's half: each lineage recorded once, when its head lands.
///
/// Seeded from the journal, so a fresh driver — an `adopt`, a relaunch — neither
/// records a pair its predecessor already did nor journals it twice. A lineage is
/// answered once per driver whether or not `onevcs` accepted every attempt: a
/// refusal is on the record, and `onepipeline supersessions --record` is what asks
/// again.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    recorded: BTreeSet<Attempt>,
    answered: BTreeSet<String>,
}

impl Watch {
    /// The pairs this run's journal already records.
    pub(crate) fn of_run(paths: &RunPaths) -> Self {
        Self {
            recorded: recorded_in(&journal::read(&paths.journal())),
            answered: BTreeSet::new(),
        }
    }

    /// Record every lineage that landed since this driver last looked.
    ///
    /// # Errors
    ///
    /// The reason the run's journal could not be written.
    pub(crate) fn pass(
        &mut self,
        paths: &RunPaths,
        journal: &mut Journal,
        state: &RunState,
    ) -> Result<()> {
        for lineage in lineages(state) {
            if !self.answered.insert(lineage.node.clone()) {
                continue;
            }
            let pending: Vec<Attempt> = lineage
                .superseded
                .iter()
                .filter(|attempt| !self.recorded.contains(*attempt))
                .cloned()
                .collect();
            if pending.is_empty() {
                continue;
            }
            let recorded = record(&paths.run, &lineage, &pending);
            self::journal(paths, journal, &lineage, &recorded)?;
            self.recorded.extend(recorded.recorded);
        }
        Ok(())
    }
}

/// Record what a run's journal says landed, from a process that is not driving
/// it — the reply that settled a node with nothing driving the run.
///
/// # Errors
///
/// The reason the run's journal could not be written.
pub(crate) fn record_landed(paths: &RunPaths, journal: &mut Journal) -> Result<()> {
    let events = journal::read(&paths.journal());
    let state = crate::projection::fold(&events);
    Watch {
        recorded: recorded_in(&events),
        answered: BTreeSet::new(),
    }
    .pass(paths, journal, &state)
}

/// `onepipeline supersessions RUN [--record]`: which earlier attempts of the run's
/// landed retries were on a branch other than the one that landed, and what
/// became of recording them with `onevcs`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Supersessions {
    /// The run.
    pub(crate) run: String,
    /// Whether this call recorded, rather than only answered.
    pub(crate) record: bool,
    /// Every landed lineage with earlier attempts, in head order.
    pub(crate) lineages: Vec<SupersededLineage>,
}

/// One landed lineage of [`Supersessions`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct SupersededLineage {
    /// The head that landed, and every earlier attempt on another branch.
    #[serde(flatten)]
    pub(crate) lineage: Lineage,
    /// The attempts the run's journal did not yet record: what this call
    /// records, or would.
    pub(crate) to_record: Vec<Attempt>,
    /// The attempts this call recorded.
    pub(crate) recorded: Vec<Attempt>,
    /// The attempts `onevcs` refused this call, and why.
    pub(crate) failed: Vec<Unrecorded>,
}

impl Supersessions {
    /// The status the binary exits with: [`EXIT_SUCCESS`](crate::error::EXIT_SUCCESS) when it answered or
    /// recorded, nothing to record included, and `1` when `onevcs` refused a
    /// record — the contract's code for work left unfinished.
    pub(crate) fn exit_code(&self) -> i32 {
        if self
            .lineages
            .iter()
            .any(|lineage| !lineage.failed.is_empty())
        {
            crate::error::EXIT_QUEUED
        } else {
            crate::error::EXIT_SUCCESS
        }
    }
}

/// `onepipeline supersessions RUN [--record]`.
///
/// Reads the run's journal and nothing else, so it answers for a settled run
/// with no driver. With `record`, each pair the journal does not already record
/// is handed to `onevcs::record_supersession` — the function a driver calls when
/// the retry lands — and one `branches-superseded` per lineage that had any is
/// appended to the run's journal, so a second call records nothing new.
///
/// # Errors
///
/// A run whose store cannot be read, or whose journal cannot be written.
pub(crate) fn backfill(paths: &RunPaths, record: bool) -> Result<Supersessions> {
    let view = crate::views::RunView::open(paths)?;
    let already = recorded_in(&view.events);
    let mut journal = record.then(|| Journal::open(paths));
    let mut lineages = Vec::new();
    for lineage in self::lineages(&view.state) {
        let to_record: Vec<Attempt> = lineage
            .superseded
            .iter()
            .filter(|attempt| !already.contains(*attempt))
            .cloned()
            .collect();
        let (recorded, failed) = match &mut journal {
            Some(journal) if !to_record.is_empty() => {
                let done = self::record(&paths.run, &lineage, &to_record);
                self::journal(paths, journal, &lineage, &done)?;
                (done.recorded, done.failed)
            }
            _ => (Vec::new(), Vec::new()),
        };
        lineages.push(SupersededLineage {
            lineage,
            to_record,
            recorded,
            failed,
        });
    }
    Ok(Supersessions {
        run: paths.run.clone(),
        record,
        lineages,
    })
}

/// The lines `onepipeline supersessions RUN` prints.
pub(crate) fn render(answered: &Supersessions) -> String {
    if answered.lineages.is_empty() {
        return format!(
            "run {}: no retry that landed has an earlier attempt, so there is nothing to \
             record\n",
            answered.run
        );
    }
    let mut out = String::new();
    for lineage in &answered.lineages {
        let head = &lineage.lineage;
        out.push_str(&format!(
            "{} landed {} at {}\n",
            head.node, head.branch, head.landing
        ));
        if head.superseded.is_empty() {
            out.push_str(
                "  every earlier attempt was on the branch that landed, which onevcs chains \
                 itself\n",
            );
        }
        for attempt in &head.superseded {
            let state = if let Some(failed) = lineage
                .failed
                .iter()
                .find(|failed| &failed.attempt == attempt)
            {
                format!("not recorded: {}", failed.error)
            } else if lineage.recorded.contains(attempt) {
                "recorded".to_owned()
            } else if lineage.to_record.contains(attempt) {
                "to record: run again with --record".to_owned()
            } else {
                "already recorded".to_owned()
            };
            out.push_str(&format!(
                "  superseded {} on {}: {state}\n",
                attempt.node, attempt.branch
            ));
        }
    }
    let failed: Vec<String> = answered
        .lineages
        .iter()
        .flat_map(|lineage| &lineage.failed)
        .map(|failed| failed.attempt.branch.clone())
        .collect();
    if !failed.is_empty() {
        out.push_str(&format!(
            "onevcs refused to record {}; run `onepipeline supersessions {} --record` again \
             once that is fixed\n",
            failed.join(", "),
            answered.run
        ));
    }
    out
}

/// The one JSON object `onepipeline supersessions RUN --json` prints.
pub(crate) fn render_json(answered: &Supersessions) -> String {
    serde_json::to_string(answered).expect("the answer serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> RunState {
        crate::projection::fold(&[])
    }

    fn retried(state: &mut RunState, from: &str, to: &str) {
        state.superseded.insert(from.to_owned(), to.to_owned());
    }

    fn lifecycle(state: &mut RunState, id: &str) {
        let node: crate::plan::Node = serde_json::from_value(serde_json::json!({
            "id": id, "repo": "service", "task": "## What\nShip."
        }))
        .expect("a node parses");
        state.graph.insert(node);
    }

    /// The walk is root first through every retry edge, drops the attempts on the
    /// landed branch, and takes a commit before a change request's URL.
    #[test]
    fn a_landed_lineage_names_every_earlier_attempt_on_another_branch_root_first() {
        let mut state = state();
        lifecycle(&mut state, "r3");
        retried(&mut state, "r1", "r2");
        retried(&mut state, "r2", "r3");
        for (node, branch) in [("r1", "a"), ("r2", "c"), ("r3", "c")] {
            state.branches.insert(node.to_owned(), branch.to_owned());
        }
        state.landings.insert("r3".to_owned(), Landing::Landed);
        state
            .change_urls
            .insert("r3".to_owned(), "https://github.com/o/s/pull/7".to_owned());
        let found = lineages(&state);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].landing, "https://github.com/o/s/pull/7");
        assert_eq!(
            found[0].superseded,
            vec![Attempt {
                node: "r1".to_owned(),
                branch: "a".to_owned()
            }]
        );

        state
            .landing_commits
            .insert("r3".to_owned(), "f".repeat(40));
        assert_eq!(lineages(&state)[0].landing, "f".repeat(40));
    }

    /// A head that did not land, or whose branch nothing recorded, is no lineage
    /// `onevcs` can be told about; a node nothing retried is not a lineage at all.
    #[test]
    fn an_unlanded_head_an_unrecorded_branch_and_an_unretried_node_are_not_lineages() {
        let mut state = state();
        lifecycle(&mut state, "b");
        lifecycle(&mut state, "alone");
        retried(&mut state, "a", "b");
        state.branches.insert("a".to_owned(), "x".to_owned());
        state.landings.insert("b".to_owned(), Landing::Unlanded);
        state
            .landing_commits
            .insert("alone".to_owned(), "e".repeat(40));
        state.branches.insert("alone".to_owned(), "y".to_owned());
        assert!(lineages(&state).is_empty());

        state.landings.insert("b".to_owned(), Landing::Landed);
        state.landing_commits.insert("b".to_owned(), "e".repeat(40));
        assert!(lineages(&state).is_empty(), "b's own branch is unrecorded");
    }
}
